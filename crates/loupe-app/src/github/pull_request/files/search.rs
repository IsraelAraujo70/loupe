//! Find in the diff: the lines of the loaded patches that contain a text,
//! in the order the diff shows them, as the browser's find does on GitHub's
//! Files changed.

use loupe_github::Side;
use loupe_github::diff::{Hunk, LineKind};

/// How many lines before a match the diff scrolls to, so the match lands
/// under the pinned file header instead of behind it.
pub const LEAD: usize = 3;

/// A line of the diff that holds the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// The file's place among the files searched.
    pub file: usize,
    pub path: String,
    /// The line as the diff numbers it: a removed line on the left with its
    /// old number, any other on the right with its new number.
    pub line: u32,
    pub side: Side,
    /// The left number of an unchanged line, which a split diff also shows.
    pub old_line: Option<u32>,
    /// The line to scroll to: a few lines above the match in its file, or
    /// `None` near the top of the file, where its header is the place.
    pub lead: Option<(u32, Side)>,
}

/// The text searched for, folded once for a case-insensitive search.
enum Needle {
    Ascii(Vec<u8>),
    Unicode(String),
}

impl Needle {
    fn new(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        Some(if query.is_ascii() {
            Self::Ascii(query.to_ascii_lowercase().into_bytes())
        } else {
            Self::Unicode(query.to_lowercase())
        })
    }

    fn found_in(&self, line: &str) -> bool {
        match self {
            // ASCII letters fold without allocating, and only ASCII bytes
            // can equal them, so a multibyte character never matches half.
            Self::Ascii(needle) => line
                .as_bytes()
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle)),
            Self::Unicode(needle) => line.to_lowercase().contains(needle.as_str()),
        }
    }
}

/// The lines of `files` (each a path and its parsed hunks, in the diff's
/// order) that contain `query`, ignoring case, in the order the diff shows
/// them. Each line counts once, however many times it holds the text.
pub fn find<'a>(files: impl IntoIterator<Item = (&'a str, &'a [Hunk])>, query: &str) -> Vec<Match> {
    let Some(needle) = Needle::new(query) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (file, (path, hunks)) in files.into_iter().enumerate() {
        let lines: Vec<_> = hunks.iter().flat_map(|hunk| &hunk.lines).collect();
        for (ix, line) in lines.iter().enumerate() {
            if !needle.found_in(&line.text) {
                continue;
            }
            let (number, side) = line.anchor();
            found.push(Match {
                file,
                path: path.to_owned(),
                line: number,
                side,
                old_line: (line.kind == LineKind::Context)
                    .then_some(line.old_line)
                    .flatten(),
                lead: ix.checked_sub(LEAD).map(|lead| lines[lead].anchor()),
            });
        }
    }
    found
}

/// Where to start in `matches` when the text changes: the first match in
/// the file at the top of the diff or after it, else the first one.
pub fn first_from(matches: &[Match], top_file: Option<usize>) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let top = top_file.unwrap_or(0);
    Some(
        matches
            .iter()
            .position(|found| found.file >= top)
            .unwrap_or(0),
    )
}

/// The match `delta` steps from `current`, going round at either end.
pub fn step(len: usize, current: Option<usize>, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current else {
        return Some(if delta < 0 { len - 1 } else { 0 });
    };
    Some((current as isize + delta).rem_euclid(len as isize) as usize)
}

#[cfg(test)]
mod tests {
    use loupe_github::Side;
    use loupe_github::diff::parse_patch;

    use super::{Match, find, first_from, step};

    #[test]
    fn finds_lines_in_the_diffs_order_ignoring_case() {
        let first = parse_patch(
            "@@ -1,2 +1,2 @@\n fn Render() {}\n-let old = render();\n+let new = RENDER();\n",
        )
        .expect("patch");
        let second = parse_patch("@@ -10 +10,2 @@\n keep\n+render here\n").expect("patch");
        let found = find(
            [("a.rs", first.as_slice()), ("b.rs", second.as_slice())],
            "render",
        );
        let at: Vec<_> = found
            .iter()
            .map(|found| (found.file, found.line, found.side, found.old_line))
            .collect();
        assert_eq!(
            at,
            [
                (0, 1, Side::Right, Some(1)),
                (0, 2, Side::Left, None),
                (0, 2, Side::Right, None),
                (1, 11, Side::Right, None),
            ]
        );
    }

    #[test]
    fn a_line_counts_once_and_an_empty_text_finds_nothing() {
        let hunks = parse_patch("@@ -1 +1 @@\n-abc abc\n+x\n").expect("patch");
        assert_eq!(find([("a.rs", hunks.as_slice())], "abc").len(), 1);
        assert!(find([("a.rs", hunks.as_slice())], "").is_empty());
    }

    #[test]
    fn non_ascii_text_folds_case_too() {
        let hunks = parse_patch("@@ -1 +1 @@\n-x\n+Ação concluída\n").expect("patch");
        assert_eq!(find([("a.rs", hunks.as_slice())], "AÇÃO").len(), 1);
        assert_eq!(find([("a.rs", hunks.as_slice())], "ÍDA").len(), 1);
    }

    #[test]
    fn the_diff_scrolls_to_a_few_lines_above_the_match() {
        let hunks = parse_patch("@@ -1,5 +1,5 @@\n a\n b\n c\n-d\n+target\n e\n").expect("patch");
        let found = find([("a.rs", hunks.as_slice())], "target");
        assert_eq!(
            found[0].lead,
            Some((2, Side::Right)),
            "three lines above, on b"
        );
        let near_top = find([("a.rs", hunks.as_slice())], "b");
        assert_eq!(
            near_top[0].lead, None,
            "near the top the header is the place"
        );
    }

    #[test]
    fn starts_at_the_file_on_screen_and_steps_round() {
        let at = |file| Match {
            file,
            path: String::new(),
            line: 1,
            side: Side::Right,
            old_line: None,
            lead: None,
        };
        let matches = [at(0), at(2), at(5)];
        assert_eq!(first_from(&matches, Some(1)), Some(1));
        assert_eq!(
            first_from(&matches, Some(9)),
            Some(0),
            "past the last goes round"
        );
        assert_eq!(first_from(&[], Some(0)), None);
        assert_eq!(step(3, Some(2), 1), Some(0));
        assert_eq!(step(3, Some(0), -1), Some(2));
        assert_eq!(step(3, None, -1), Some(2));
        assert_eq!(step(0, None, 1), None);
    }
}
