//! Splits a comment's Markdown around HTML `<details>` blocks, so each one
//! renders as a fold that starts closed, as on GitHub. The Markdown renderer
//! sees `<details>`, its content, and `</details>` as unrelated blocks and
//! would show everything open.

use std::collections::VecDeque;

/// A run of a comment body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Segment {
    Markdown(String),
    /// A `<details>` block: the `<summary>` text, whether the source asked
    /// for it open (`<details open>`), and what it hides.
    Details {
        summary: String,
        open: bool,
        body: Vec<Segment>,
    },
}

/// Splits `body` into Markdown runs and `<details>` blocks, nested blocks
/// included. Tags inside fenced code stay code. An unclosed block runs to the
/// end of the comment, as a browser would render it.
pub(super) fn split(body: &str) -> Vec<Segment> {
    let mut lines: VecDeque<String> = body.lines().map(str::to_owned).collect();
    parse(&mut lines, false)
}

const CLOSE: &str = "</details>";

fn parse(lines: &mut VecDeque<String>, nested: bool) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut text = String::new();
    let mut fence: Option<Fence> = None;
    while let Some(line) = lines.pop_front() {
        if let Some(open) = fence {
            if open.closed_by(&line) {
                fence = None;
            }
            push_line(&mut text, &line);
            continue;
        }
        if let Some(open) = Fence::opened_by(&line) {
            fence = Some(open);
            push_line(&mut text, &line);
            continue;
        }
        let trimmed = line.trim_start();
        // An opening tag first: its own `</details>` may share the line.
        if let Some((open, rest)) = details_open(trimmed) {
            flush(&mut segments, &mut text);
            let summary = take_summary(rest, lines);
            let body = parse(lines, true);
            segments.push(Segment::Details {
                summary,
                open,
                body,
            });
            continue;
        }
        if nested && let Some(end) = trimmed.to_ascii_lowercase().find(CLOSE) {
            let (before, after) = (&trimmed[..end], &trimmed[end + CLOSE.len()..]);
            if !after.trim().is_empty() {
                lines.push_front(after.to_owned());
            }
            if !before.trim().is_empty() {
                push_line(&mut text, before);
            }
            flush(&mut segments, &mut text);
            return segments;
        }
        push_line(&mut text, &line);
    }
    flush(&mut segments, &mut text);
    segments
}

fn push_line(text: &mut String, line: &str) {
    text.push_str(line);
    text.push('\n');
}

fn flush(segments: &mut Vec<Segment>, text: &mut String) {
    if !text.trim().is_empty() {
        segments.push(Segment::Markdown(std::mem::take(text)));
    }
    text.clear();
}

/// A fenced code block opener: its character and length.
#[derive(Clone, Copy)]
struct Fence {
    marker: char,
    len: usize,
}

impl Fence {
    fn opened_by(line: &str) -> Option<Fence> {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let len = trimmed.chars().take_while(|c| *c == marker).count();
        (len >= 3).then_some(Fence { marker, len })
    }

    fn closed_by(self, line: &str) -> bool {
        let trimmed = line.trim();
        trimmed.chars().take_while(|c| *c == self.marker).count() >= self.len
            && trimmed.chars().all(|c| c == self.marker)
    }
}

/// `<details>` or `<details open>` at the start of `line`: whether it opens
/// expanded, and the rest of the line.
fn details_open(line: &str) -> Option<(bool, &str)> {
    let lower = line.get(..8)?.to_ascii_lowercase();
    if lower != "<details" {
        return None;
    }
    let after = &line[8..];
    if !(after.starts_with('>') || after.starts_with(char::is_whitespace)) {
        return None;
    }
    let end = after.find('>')?;
    let attributes = after[..end].to_ascii_lowercase();
    let open = attributes
        .split(|c: char| c.is_whitespace() || c == '=')
        .any(|word| word == "open");
    Some((open, &after[end + 1..]))
}

/// Takes the `<summary>` that follows a `<details>` tag, on the same line or
/// the next ones, and leaves whatever comes after it to be parsed. GitHub
/// shows "Details" when there is none.
fn take_summary(rest: &str, lines: &mut VecDeque<String>) -> String {
    let mut source = rest.trim().to_owned();
    if source.is_empty() {
        while lines.front().is_some_and(|line| line.trim().is_empty()) {
            lines.pop_front();
        }
        let starts_summary = lines.front().is_some_and(|line| {
            line.trim_start()
                .to_ascii_lowercase()
                .starts_with("<summary")
        });
        if !starts_summary {
            return "Details".into();
        }
        source = lines.pop_front().unwrap_or_default().trim().to_owned();
    } else if !source.to_ascii_lowercase().starts_with("<summary") {
        lines.push_front(source);
        return "Details".into();
    }
    // The summary may span lines until `</summary>`.
    while !source.to_ascii_lowercase().contains("</summary>") {
        match lines.pop_front() {
            Some(line) => {
                source.push(' ');
                source.push_str(line.trim());
            }
            None => break,
        }
    }
    let lower = source.to_ascii_lowercase();
    let (inside, after) = match lower.find("</summary>") {
        Some(end) => (&source[..end], &source[end + "</summary>".len()..]),
        None => (source.as_str(), ""),
    };
    if !after.trim().is_empty() {
        lines.push_front(after.trim().to_owned());
    }
    let summary = strip_tags(inside);
    if summary.is_empty() {
        "Details".into()
    } else {
        summary
    }
}

/// Text with HTML tags removed and whitespace collapsed.
fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line that stands for a folded comment: its first line with text,
/// without Markdown markers or HTML tags.
pub(super) fn preview(body: &str) -> String {
    let mut fence: Option<Fence> = None;
    for line in body.lines() {
        if let Some(open) = fence {
            if open.closed_by(line) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = Fence::opened_by(line) {
            fence = Some(open);
            continue;
        }
        let text = strip_tags(line);
        let text = text
            .trim_start_matches(|c: char| {
                matches!(c, '#' | '>' | '-' | '*' | '+' | '|') || c.is_whitespace()
            })
            .replace(['*', '`'], "");
        let text = text.trim();
        let is_rule = !text.is_empty()
            && text
                .chars()
                .all(|c| matches!(c, '-' | '=' | '|' | ':' | ' '));
        if !text.is_empty() && !is_rule {
            return text.to_owned();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::{Segment, preview, split};

    fn markdown(text: &str) -> Segment {
        Segment::Markdown(text.into())
    }

    #[test]
    fn a_details_block_becomes_a_closed_fold_with_its_summary() {
        let body = "## Plan\n\n<details><summary>Change Result (Click me)</summary>\n\n```hcl\nresource \"a\" {}\n```\n\n</details>\n\nDone.";
        assert_eq!(
            split(body),
            vec![
                markdown("## Plan\n\n"),
                Segment::Details {
                    summary: "Change Result (Click me)".into(),
                    open: false,
                    body: vec![markdown("\n```hcl\nresource \"a\" {}\n```\n\n")],
                },
                markdown("\nDone.\n"),
            ]
        );
    }

    #[test]
    fn open_attribute_nesting_and_multiline_summaries() {
        let body = "<details open>\n<summary>\n<b>Outer</b>\n</summary>\ntext\n<details><summary>Inner</summary>deep</details>\n</details>";
        assert_eq!(
            split(body),
            vec![Segment::Details {
                summary: "Outer".into(),
                open: true,
                body: vec![
                    markdown("text\n"),
                    Segment::Details {
                        summary: "Inner".into(),
                        open: false,
                        body: vec![markdown("deep\n")],
                    },
                ],
            }]
        );
    }

    #[test]
    fn tags_inside_code_and_lookalikes_stay_text() {
        let body = "```html\n<details><summary>x</summary>\n```\n<detailsx>no</detailsx>";
        assert_eq!(split(body), vec![markdown(&format!("{body}\n"))]);
    }

    #[test]
    fn a_missing_summary_reads_details_and_an_unclosed_block_runs_to_the_end() {
        assert_eq!(
            split("<details>\nhidden"),
            vec![Segment::Details {
                summary: "Details".into(),
                open: false,
                body: vec![markdown("hidden\n")],
            }]
        );
    }

    #[test]
    fn previews_skip_markup_code_and_rules() {
        assert_eq!(
            preview("\n```\ncode\n```\n---\n## 📋 **Terragrunt** Validation"),
            "📋 Terragrunt Validation"
        );
        assert_eq!(
            preview("<details><summary>Change Result</summary>"),
            "Change Result"
        );
        assert_eq!(
            preview("Set `TRUST_PROXY_HOPS` to **1**"),
            "Set TRUST_PROXY_HOPS to 1"
        );
        assert_eq!(preview("  \n"), "");
    }
}
