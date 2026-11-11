//! Suggested changes in review comments (```suggestion blocks): splitting a
//! body into markdown and suggestions, the lines a suggestion replaces, and
//! committing one to the pull request's head branch.

use serde::Deserialize;
use serde_json::json;

use crate::{Error, GitHub, RepoRef, Result};

/// One part of a review comment body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyPart {
    Markdown(String),
    /// The lines a ```suggestion block proposes, without the fences.
    Suggestion(String),
}

/// Splits `body` around its ```suggestion blocks. A block left open runs to
/// the end of the body, as GitHub renders it.
pub fn split_suggestions(body: &str) -> Vec<BodyPart> {
    let mut parts = Vec::new();
    let mut markdown = String::new();
    let mut suggestion: Option<String> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']).trim();
        match &mut suggestion {
            None if trimmed == "```suggestion" => {
                if !markdown.trim().is_empty() {
                    parts.push(BodyPart::Markdown(std::mem::take(&mut markdown)));
                }
                markdown.clear();
                suggestion = Some(String::new());
            }
            None => markdown.push_str(line),
            Some(_) if trimmed == "```" => {
                let lines = suggestion.take().unwrap_or_default();
                parts.push(BodyPart::Suggestion(trim_one_newline(lines)));
            }
            Some(lines) => lines.push_str(line),
        }
    }
    if let Some(lines) = suggestion {
        parts.push(BodyPart::Suggestion(trim_one_newline(lines)));
    }
    if !markdown.trim().is_empty() {
        parts.push(BodyPart::Markdown(markdown));
    }
    parts
}

fn trim_one_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

/// The last `count` lines of the new side in `diff_hunk` (the hunk GitHub
/// stores with a comment ends at the commented line): what a suggestion on
/// those lines replaces.
pub fn suggested_lines_original(diff_hunk: &str, count: usize) -> Vec<String> {
    let new_side: Vec<&str> = diff_hunk
        .lines()
        .filter(|line| !line.starts_with("@@") && !line.starts_with('-') && !line.starts_with('\\'))
        .collect();
    new_side[new_side.len().saturating_sub(count)..]
        .iter()
        .map(|line| line.get(1..).unwrap_or_default().to_owned())
        .collect()
}

/// `content` with lines `start..=end` (1-based) replaced by `replacement`,
/// keeping the file's line ending and its trailing newline; `None` when the
/// lines are not in the file.
pub fn replace_lines(content: &str, start: u32, end: u32, replacement: &str) -> Option<String> {
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing = content.ends_with('\n');
    let mut lines: Vec<&str> = content.lines().collect();
    let (start, end) = (start as usize, end as usize);
    if start == 0 || start > end || end > lines.len() {
        return None;
    }
    let replacement: Vec<&str> = if replacement.is_empty() {
        Vec::new()
    } else {
        replacement.lines().collect()
    };
    lines.splice(start - 1..end, replacement);
    let mut out = lines.join(newline);
    if trailing && !out.is_empty() {
        out.push_str(newline);
    }
    Some(out)
}

const FILE_AT_COMMIT: &str = "query FileAtCommit($owner: String!, $name: String!, \
    $expression: String!) { repository(owner: $owner, name: $name) { \
    object(expression: $expression) { ... on Blob { text isBinary } } } }";

const COMMIT_FILE: &str = "mutation CommitFile($branch: CommittableBranch!, \
    $message: CommitMessage!, $expectedHeadOid: GitObjectID!, $fileChanges: FileChanges!) { \
    createCommitOnBranch(input: {branch: $branch, message: $message, \
    expectedHeadOid: $expectedHeadOid, fileChanges: $fileChanges}) { commit { oid } } }";

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[
    ("file_at_commit", FILE_AT_COMMIT),
    ("commit_file", COMMIT_FILE),
];

/// A suggestion to commit: lines `start..=end` of `path` on the new side of
/// the pull request, at head commit `head_oid` of `branch` in `repo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestionCommit {
    pub repo: RepoRef,
    pub branch: String,
    pub head_oid: String,
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub replacement: String,
    pub message: String,
}

impl GitHub {
    /// Commits a suggestion to the head branch, as GitHub's "Commit
    /// suggestion" does. Fails without writing when the branch moved past
    /// `head_oid`, so the lines are always the ones the reviewer saw.
    pub fn commit_suggestion(&self, change: &SuggestionCommit) -> Result<()> {
        #[derive(Deserialize)]
        struct Data {
            repository: Option<Repository>,
        }
        #[derive(Deserialize)]
        struct Repository {
            object: Option<Blob>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Blob {
            text: Option<String>,
            is_binary: Option<bool>,
        }
        let data: Data = self.graphql(
            FILE_AT_COMMIT,
            json!({
                "owner": change.repo.owner,
                "name": change.repo.name,
                "expression": format!("{}:{}", change.head_oid, change.path),
            }),
        )?;
        let blob = data
            .repository
            .and_then(|repository| repository.object)
            .ok_or_else(|| Error::Decode(format!("{} not found at the head", change.path)))?;
        let text = match blob {
            Blob {
                text: Some(text),
                is_binary: Some(false) | None,
            } => text,
            _ => return Err(Error::Decode(format!("{} is not a text file", change.path))),
        };
        let updated = replace_lines(&text, change.start, change.end, &change.replacement)
            .ok_or_else(|| Error::Decode("the suggested lines are not in the file".into()))?;
        let _: serde::de::IgnoredAny = self.graphql(
            COMMIT_FILE,
            json!({
                "branch": {
                    "repositoryNameWithOwner": format!("{}/{}", change.repo.owner, change.repo.name),
                    "branchName": change.branch,
                },
                "message": { "headline": change.message },
                "expectedHeadOid": change.head_oid,
                "fileChanges": {
                    "additions": [{ "path": change.path, "contents": base64(updated.as_bytes()) }]
                },
            }),
        )?;
        Ok(())
    }
}

/// Standard base64 with padding, which `createCommitOnBranch` takes for file
/// contents.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{BodyPart, base64, replace_lines, split_suggestions, suggested_lines_original};

    #[test]
    fn splits_markdown_and_suggestions() {
        let body = "Use checked math:\n```suggestion\n    a.checked_div(b)\n```\nThanks";
        assert_eq!(
            split_suggestions(body),
            [
                BodyPart::Markdown("Use checked math:\n".into()),
                BodyPart::Suggestion("    a.checked_div(b)".into()),
                BodyPart::Markdown("Thanks".into()),
            ]
        );
        assert_eq!(
            split_suggestions("plain"),
            [BodyPart::Markdown("plain".into())]
        );
        assert_eq!(
            split_suggestions("```suggestion\n```"),
            [BodyPart::Suggestion(String::new())],
            "an empty suggestion deletes the lines"
        );
    }

    #[test]
    fn original_lines_come_from_the_new_side_of_the_hunk() {
        let hunk = "@@ -1,3 +1,3 @@\n fn a() {\n-    old()\n+    new()\n }";
        assert_eq!(suggested_lines_original(hunk, 2), ["    new()", "}"]);
    }

    #[test]
    fn replaces_lines_keeping_line_endings() {
        assert_eq!(
            replace_lines("a\nb\nc\n", 2, 2, "B1\nB2").as_deref(),
            Some("a\nB1\nB2\nc\n")
        );
        assert_eq!(
            replace_lines("a\r\nb\r\n", 1, 2, "x").as_deref(),
            Some("x\r\n")
        );
        assert_eq!(replace_lines("a\nb", 2, 2, "").as_deref(), Some("a"));
        assert_eq!(replace_lines("a\n", 2, 2, "x"), None, "line out of range");
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("ação\n".as_bytes()), "YcOnw6NvCg==");
    }
}
