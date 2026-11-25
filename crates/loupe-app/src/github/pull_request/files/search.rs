//! Find in the diff: every place the loaded patches hold a text, in the
//! order the diff shows them, as the browser's find does on GitHub's Files
//! changed.

use std::ops::Range;

use loupe_github::Side;
use loupe_github::diff::{Hunk, LineKind};

use super::expand::scope;

/// How many lines before a match the diff scrolls to, so the match lands
/// under the pinned file header instead of behind it.
pub const LEAD: usize = 3;

/// One place of the diff that holds the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// The file's place among the files searched.
    pub file: usize,
    pub path: String,
    /// The line as the diff numbers it: a removed line on the left with its
    /// old number, any other on the right with its new number. For a match
    /// in a hunk's header, the hunk's first line.
    pub line: u32,
    pub side: Side,
    /// The left number of an unchanged line, which a split diff also shows.
    pub old_line: Option<u32>,
    /// Where the text is, in bytes of the line or of the hunk's header.
    pub range: Range<usize>,
    /// The text is in the scope Git names in the hunk's `@@` header, which
    /// the diff draws above `line`.
    pub in_header: bool,
    /// The line to scroll to: a few lines above the match in its file, or
    /// `None` near the top of the file, where its header is the place.
    pub lead: Option<(u32, Side)>,
}

/// The text searched for, folded once for a case-insensitive search.
pub enum Needle {
    Ascii(Vec<u8>),
    Unicode(Vec<char>),
}

impl Needle {
    pub fn new(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        Some(if query.is_ascii() {
            Self::Ascii(query.to_ascii_lowercase().into_bytes())
        } else {
            Self::Unicode(query.chars().flat_map(char::to_lowercase).collect())
        })
    }

    /// The byte ranges of `text` that hold the needle, ignoring case, from
    /// left to right without overlapping.
    pub fn find_in(&self, text: &str) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        match self {
            // ASCII letters fold without allocating, and only ASCII bytes
            // can equal them, so a multibyte character never matches half.
            Self::Ascii(needle) => {
                let bytes = text.as_bytes();
                let mut at = 0;
                while at + needle.len() <= bytes.len() {
                    if bytes[at..at + needle.len()].eq_ignore_ascii_case(needle) {
                        found.push(at..at + needle.len());
                        at += needle.len();
                    } else {
                        at += 1;
                    }
                }
            }
            Self::Unicode(needle) => {
                let mut from = 0;
                for (start, _) in text.char_indices() {
                    if start < from {
                        continue;
                    }
                    if let Some(len) = folded_prefix(&text[start..], needle) {
                        found.push(start..start + len);
                        from = start + len;
                    }
                }
            }
        }
        found
    }
}

/// The length in bytes of the start of `text` that folds to `needle`, if
/// one does, whole characters only.
fn folded_prefix(text: &str, needle: &[char]) -> Option<usize> {
    let mut rest = needle;
    for (ix, ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            let (first, tail) = rest.split_first()?;
            if *first != lower {
                return None;
            }
            rest = tail;
        }
        if rest.is_empty() {
            return Some(ix + ch.len_utf8());
        }
    }
    None
}

/// Every place in `files` (each a path and its parsed hunks, in the diff's
/// order) that holds `query`, ignoring case, in the order the diff shows
/// them: each occurrence in a line, and in the scope of a hunk's header.
pub fn find<'a>(files: impl IntoIterator<Item = (&'a str, &'a [Hunk])>, query: &str) -> Vec<Match> {
    let Some(needle) = Needle::new(query) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (file, (path, hunks)) in files.into_iter().enumerate() {
        let lines: Vec<_> = hunks.iter().flat_map(|hunk| &hunk.lines).collect();
        let lead = |ix: usize| ix.checked_sub(LEAD).map(|lead| lines[lead].anchor());
        let mut ix = 0;
        for hunk in hunks {
            if let Some(first) = hunk.lines.first() {
                let (line, side) = first.anchor();
                let offset = hunk.header.len() - scope(&hunk.header).len();
                for range in needle.find_in(scope(&hunk.header)) {
                    found.push(Match {
                        file,
                        path: path.to_owned(),
                        line,
                        side,
                        old_line: None,
                        range: offset + range.start..offset + range.end,
                        in_header: true,
                        lead: lead(ix),
                    });
                }
            }
            for line in &hunk.lines {
                let (number, side) = line.anchor();
                for range in needle.find_in(&line.text) {
                    found.push(Match {
                        file,
                        path: path.to_owned(),
                        line: number,
                        side,
                        old_line: (line.kind == LineKind::Context)
                            .then_some(line.old_line)
                            .flatten(),
                        range,
                        in_header: false,
                        lead: lead(ix),
                    });
                }
                ix += 1;
            }
        }
    }
    found
}

/// The byte ranges that hold the needle in one side of a hunk as the
/// diff's highlighter gets it: its lines, each ending in a newline but
/// maybe the last. A match never runs from one line into the next.
pub fn marks(needle: &Needle, text: &str) -> Vec<Range<usize>> {
    let mut marks = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        marks.extend(
            needle
                .find_in(content)
                .into_iter()
                .map(|range| start + range.start..start + range.end),
        );
        start += line.len();
    }
    marks
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

    use super::{Match, Needle, find, first_from, marks, step};

    #[test]
    fn finds_each_occurrence_in_the_diffs_order_ignoring_case() {
        let first = parse_patch(
            "@@ -1,2 +1,2 @@\n fn Render() {}\n-let old = render();\n+let new = RENDER(render);\n",
        )
        .expect("patch");
        let second = parse_patch("@@ -10 +10,2 @@\n keep\n+render here\n").expect("patch");
        let found = find(
            [("a.rs", first.as_slice()), ("b.rs", second.as_slice())],
            "render",
        );
        let at: Vec<_> = found
            .iter()
            .map(|found| {
                (
                    found.file,
                    found.line,
                    found.side,
                    found.old_line,
                    found.range.clone(),
                )
            })
            .collect();
        assert_eq!(
            at,
            [
                (0, 1, Side::Right, Some(1), 3..9),
                (0, 2, Side::Left, None, 10..16),
                (0, 2, Side::Right, None, 10..16),
                (0, 2, Side::Right, None, 17..23),
                (1, 11, Side::Right, None, 0..6),
            ]
        );
    }

    #[test]
    fn occurrences_do_not_overlap_and_an_empty_text_finds_nothing() {
        let hunks = parse_patch("@@ -1 +1 @@\n-abc abc\n+aaaa\n").expect("patch");
        assert_eq!(find([("a.rs", hunks.as_slice())], "abc").len(), 2);
        let ranges: Vec<_> = find([("a.rs", hunks.as_slice())], "aa")
            .into_iter()
            .map(|found| found.range)
            .collect();
        assert_eq!(ranges, [0..2, 2..4]);
        assert!(find([("a.rs", hunks.as_slice())], "").is_empty());
    }

    #[test]
    fn non_ascii_text_folds_case_too() {
        let hunks = parse_patch("@@ -1 +1 @@\n-x\n+Ação concluída, ação\n").expect("patch");
        let ranges: Vec<_> = find([("a.rs", hunks.as_slice())], "AÇÃO")
            .into_iter()
            .map(|found| found.range)
            .collect();
        assert_eq!(ranges, [0..6, 19..25], "byte ranges of whole characters");
        assert_eq!(find([("a.rs", hunks.as_slice())], "ÍDA").len(), 1);
    }

    #[test]
    fn the_scope_of_a_hunk_header_is_searched_but_not_its_numbers() {
        let hunks = parse_patch(
            "@@ -1,2 +1,2 @@ fn first()\n a\n-b\n+c\n@@ -10,2 +10,2 @@ Prism is an app\n x\n-y\n+z\n",
        )
        .expect("patch");
        let found = find([("a.rs", hunks.as_slice())], "prism");
        assert_eq!(found.len(), 1);
        assert!(found[0].in_header);
        assert_eq!((found[0].line, found[0].side), (10, Side::Right));
        assert_eq!(&hunks[1].header[found[0].range.clone()], "Prism");
        assert!(find([("a.rs", hunks.as_slice())], "10").is_empty());
    }

    #[test]
    fn marks_fall_on_each_line_of_a_hunk_side() {
        let needle = Needle::new("ab").expect("needle");
        assert_eq!(marks(&needle, "xab ab\r\nab\nza"), [1..3, 4..6, 8..10]);
        assert!(marks(&needle, "a\nb").is_empty(), "no match across lines");
        assert!(Needle::new("").is_none());
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
            range: 0..1,
            in_header: false,
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
