//! Pure unified-diff model: GitHub patch text to hunks and numbered lines,
//! side-by-side pairing, and the `(line, side)` anchors GitHub's review API
//! takes for any rendered line.

use crate::{ChangedFile, FileStatus, LineAnchor, Side};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the base file; `None` for added lines.
    pub old_line: Option<u32>,
    /// Line number in the head file; `None` for removed lines.
    pub new_line: Option<u32>,
    /// The line without its `+`, `-`, or ` ` prefix.
    pub text: String,
    /// Followed by `\ No newline at end of file`.
    pub no_newline_at_eof: bool,
}

impl DiffLine {
    /// The anchor GitHub uses for this line: removed lines are on the left
    /// with their old number, added and context lines on the right with
    /// their new number.
    pub fn anchor(&self) -> (u32, Side) {
        match (self.kind, self.old_line, self.new_line) {
            (LineKind::Removed, Some(old), _) => (old, Side::Left),
            (_, _, Some(new)) => (new, Side::Right),
            (_, Some(old), None) => (old, Side::Left),
            (_, None, None) => (0, Side::Right),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The whole `@@ -a,b +c,d @@ section` line.
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchError {
    /// One-based line of the patch text.
    pub line: usize,
    pub message: String,
}

/// A hunk being filled, with the numbers its next lines get and how many
/// lines of each side its header still promises.
struct OpenHunk {
    hunk: Hunk,
    next_old: u32,
    next_new: u32,
    old_left: u32,
    new_left: u32,
}

impl OpenHunk {
    fn new(hunk: Hunk) -> Self {
        Self {
            next_old: hunk.old_start,
            next_new: hunk.new_start,
            old_left: hunk.old_count,
            new_left: hunk.new_count,
            hunk,
        }
    }

    /// Takes one body line, or says the hunk has no room for it.
    fn push(&mut self, kind: LineKind, text: &str) -> Result<(), String> {
        let needs_old = kind != LineKind::Added;
        let needs_new = kind != LineKind::Removed;
        if (needs_old && self.old_left == 0) || (needs_new && self.new_left == 0) {
            return Err("hunk has more lines than its header says".to_owned());
        }
        let old_line = needs_old.then_some(self.next_old);
        let new_line = needs_new.then_some(self.next_new);
        if needs_old {
            self.next_old += 1;
            self.old_left -= 1;
        }
        if needs_new {
            self.next_new += 1;
            self.new_left -= 1;
        }
        self.hunk.lines.push(DiffLine {
            kind,
            old_line,
            new_line,
            text: text.to_owned(),
            no_newline_at_eof: false,
        });
        Ok(())
    }

    fn expects_more(&self) -> bool {
        self.old_left > 0 || self.new_left > 0
    }
}

/// Parses `@@ -a[,b] +c[,d] @@[ section]`; a missing count means 1.
fn parse_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Some(Hunk {
        header: line.to_owned(),
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    })
}

fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

fn close_hunk(open: OpenHunk, line: usize, hunks: &mut Vec<Hunk>) -> Result<(), PatchError> {
    if open.expects_more() {
        return Err(PatchError {
            line,
            message: format!(
                "hunk has fewer lines than its header says (missing {} old, {} new)",
                open.old_left, open.new_left
            ),
        });
    }
    hunks.push(open.hunk);
    Ok(())
}

/// Parses a GitHub `patch` (hunks only, no file headers).
///
/// A hunk whose body disagrees with its header counts is an error: for too
/// many lines at the extra line, for too few at the next header or the line
/// after the last one.
pub fn parse_patch(patch: &str) -> Result<Vec<Hunk>, PatchError> {
    let mut hunks = Vec::new();
    let mut open: Option<OpenHunk> = None;
    let mut total = 0;

    for (index, raw) in patch.lines().enumerate() {
        let number = index + 1;
        total = number;
        let text = raw.strip_suffix('\r').unwrap_or(raw);
        let fail = |message: &str| PatchError {
            line: number,
            message: message.to_owned(),
        };

        if text.starts_with("@@") {
            if let Some(previous) = open.take() {
                close_hunk(previous, number, &mut hunks)?;
            }
            let hunk = parse_header(text).ok_or_else(|| fail("malformed hunk header"))?;
            open = Some(OpenHunk::new(hunk));
            continue;
        }

        let Some(current) = open.as_mut() else {
            if text.is_empty() {
                continue;
            }
            return Err(fail("text before the first hunk header"));
        };

        let (kind, body) = match text.chars().next() {
            Some(' ') => (LineKind::Context, &text[1..]),
            None => (LineKind::Context, ""),
            Some('+') => (LineKind::Added, &text[1..]),
            Some('-') => (LineKind::Removed, &text[1..]),
            Some('\\') => {
                let last = current
                    .hunk
                    .lines
                    .last_mut()
                    .ok_or_else(|| fail("\"No newline\" marker with no line before it"))?;
                last.no_newline_at_eof = true;
                continue;
            }
            Some(_) => return Err(fail("line does not start with ' ', '+', '-', or '\\'")),
        };
        current.push(kind, body).map_err(|message| fail(&message))?;
    }

    if let Some(last) = open {
        close_hunk(last, total + 1, &mut hunks)?;
    }
    Ok(hunks)
}

/// Finds the line GitHub would anchor at `(line, side)`, as
/// `(hunk index, line index)`. On the left that is a context or removed line
/// with that old number; on the right a context or added line with that new
/// number.
pub fn find_line(hunks: &[Hunk], line: u32, side: Side) -> Option<(usize, usize)> {
    hunks.iter().enumerate().find_map(|(h, hunk)| {
        let position = hunk.lines.iter().position(|l| match side {
            Side::Left => l.old_line == Some(line),
            Side::Right => l.new_line == Some(line),
        })?;
        Some((h, position))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// The line is not part of the patch, so GitHub cannot anchor to it.
    NotInDiff { line: u32, side: Side },
    /// GitHub only accepts a multi-line comment within one hunk.
    SpansHunks,
}

/// Builds the anchor for a comment from `first` to `last` (either order, any
/// sides). Equal ends give a single-line anchor.
///
/// Two different `(line, side)` pairs that name the same rendered line (a
/// context line seen from each side) also give a single-line anchor, on
/// `last`'s side.
pub fn anchor_for_range(
    hunks: &[Hunk],
    first: (u32, Side),
    last: (u32, Side),
) -> Result<LineAnchor, AnchorError> {
    let locate = |(line, side): (u32, Side)| {
        find_line(hunks, line, side).ok_or(AnchorError::NotInDiff { line, side })
    };
    let (first_hunk, first_index) = locate(first)?;
    let (last_hunk, last_index) = locate(last)?;
    if first_hunk != last_hunk {
        return Err(AnchorError::SpansHunks);
    }
    let (start, end) = if first_index <= last_index {
        (first, last)
    } else {
        (last, first)
    };
    Ok(LineAnchor {
        line: end.0,
        side: end.1,
        start: (first_index != last_index).then_some(start),
    })
}

/// Quotes a path the way Git does for `core.quotePath`: wrapped in double
/// quotes with C escapes and octal bytes, whenever it holds a control or
/// non-ASCII byte, a quote, a backslash, or a space.
fn c_quote(path: &str) -> String {
    let plain = path
        .bytes()
        .all(|b| (0x21..0x7f).contains(&b) && b != b'"' && b != b'\\');
    if plain {
        return path.to_owned();
    }
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for b in path.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            b'\r' => out.push_str("\\r"),
            b' ' => out.push(' '),
            b if !(0x20..0x7f).contains(&b) => out.push_str(&format!("\\{b:03o}")),
            b => out.push(char::from(b)),
        }
    }
    out.push('"');
    out
}

/// A complete Git patch for one file (`diff --git`, rename and new/deleted
/// headers, `---`/`+++`, then the hunks), as diff viewers parse it. `None`
/// when GitHub sent no patch.
pub fn git_patch(file: &ChangedFile) -> Option<String> {
    let patch = file.patch.as_deref()?.trim_end_matches('\n');
    let new = file.path.as_str();
    let old = match file.status {
        FileStatus::Renamed | FileStatus::Copied => file.previous_path.as_deref().unwrap_or(new),
        _ => new,
    };
    let a = |path: &str| c_quote(&format!("a/{path}"));
    let b = |path: &str| c_quote(&format!("b/{path}"));

    let mut out = format!("diff --git {} {}\n", a(old), b(new));
    match file.status {
        FileStatus::Added => {
            out.push_str(&format!(
                "new file mode 100644\n--- /dev/null\n+++ {}\n",
                b(new)
            ));
        }
        FileStatus::Removed => {
            out.push_str(&format!(
                "deleted file mode 100644\n--- {}\n+++ /dev/null\n",
                a(new)
            ));
        }
        FileStatus::Renamed | FileStatus::Copied => {
            let verb = if file.status == FileStatus::Renamed {
                "rename"
            } else {
                "copy"
            };
            out.push_str(&format!(
                "{verb} from {}\n{verb} to {}\n",
                c_quote(old),
                c_quote(new)
            ));
            if !patch.is_empty() {
                out.push_str(&format!("--- {}\n+++ {}\n", a(old), b(new)));
            }
        }
        FileStatus::Modified | FileStatus::Changed | FileStatus::Unchanged => {
            out.push_str(&format!("--- {}\n+++ {}\n", a(new), b(new)));
        }
    }
    if !patch.is_empty() {
        out.push_str(patch);
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        AnchorError, DiffLine, Hunk, LineKind, anchor_for_range, c_quote, find_line, git_patch,
        parse_patch,
    };
    use crate::{ChangedFile, FileStatus, LineAnchor, Side, ViewedState};

    fn patch(lines: &[&str]) -> String {
        lines.join("\n")
    }

    fn hunks(lines: &[&str]) -> Vec<Hunk> {
        parse_patch(&patch(lines)).expect("fixture patch parses")
    }

    fn numbers(hunk: &Hunk) -> Vec<(LineKind, Option<u32>, Option<u32>)> {
        hunk.lines
            .iter()
            .map(|l| (l.kind, l.old_line, l.new_line))
            .collect()
    }

    fn file(
        status: FileStatus,
        path: &str,
        previous: Option<&str>,
        body: Option<&str>,
    ) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            previous_path: previous.map(str::to_owned),
            status,
            additions: 0,
            deletions: 0,
            patch: body.map(str::to_owned),
            viewed: ViewedState::Unviewed,
        }
    }

    /// Two hunks:
    /// 0: ` a`(1,1) `-b`(2,-) `+B`(-,2) ` c`(3,3)
    /// 1: ` x`(10,10) `-y`(11,-) `+Y`(-,11) `+Z`(-,12)
    fn anchor_fixture() -> Vec<Hunk> {
        hunks(&[
            "@@ -1,3 +1,3 @@",
            " a",
            "-b",
            "+B",
            " c",
            "@@ -10,2 +10,3 @@",
            " x",
            "-y",
            "+Y",
            "+Z",
        ])
    }

    #[test]
    fn additions_only_new_file() {
        let parsed = hunks(&["@@ -0,0 +1,3 @@", "+a", "+b", "+c"]);
        assert_eq!(parsed.len(), 1);
        let hunk = &parsed[0];
        assert_eq!(
            (
                hunk.old_start,
                hunk.old_count,
                hunk.new_start,
                hunk.new_count
            ),
            (0, 0, 1, 3)
        );
        assert_eq!(
            numbers(hunk),
            vec![
                (LineKind::Added, None, Some(1)),
                (LineKind::Added, None, Some(2)),
                (LineKind::Added, None, Some(3)),
            ]
        );
        assert_eq!(hunk.lines[1].text, "b");
    }

    #[test]
    fn deletions_only() {
        let parsed = hunks(&["@@ -1,2 +0,0 @@", "-a", "-b"]);
        assert_eq!(
            numbers(&parsed[0]),
            vec![
                (LineKind::Removed, Some(1), None),
                (LineKind::Removed, Some(2), None),
            ]
        );
    }

    #[test]
    fn context_lines_and_section_header() {
        let parsed = hunks(&[
            "@@ -3,3 +3,3 @@ fn parse()",
            " keep",
            "-old",
            "+new",
            " tail",
        ]);
        let hunk = &parsed[0];
        assert_eq!(hunk.header, "@@ -3,3 +3,3 @@ fn parse()");
        assert_eq!(
            numbers(hunk),
            vec![
                (LineKind::Context, Some(3), Some(3)),
                (LineKind::Removed, Some(4), None),
                (LineKind::Added, None, Some(4)),
                (LineKind::Context, Some(5), Some(5)),
            ]
        );
        assert_eq!(hunk.lines[0].text, "keep");
    }

    #[test]
    fn no_newline_marker_on_both_sides() {
        let parsed = hunks(&[
            "@@ -1 +1 @@",
            "-a",
            "\\ No newline at end of file",
            "+b",
            "\\ No newline at end of file",
        ]);
        let lines = &parsed[0].lines;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].no_newline_at_eof);
        assert!(lines[1].no_newline_at_eof);
    }

    #[test]
    fn no_newline_marker_only_marks_previous_line() {
        let parsed = hunks(&[
            "@@ -1,2 +1,2 @@",
            "-a",
            "\\ No newline at end of file",
            "+a",
            " z",
        ]);
        let flags: Vec<bool> = parsed[0]
            .lines
            .iter()
            .map(|l| l.no_newline_at_eof)
            .collect();
        assert_eq!(flags, vec![true, false, false]);
    }

    #[test]
    fn multiple_hunks_are_numbered_independently() {
        let parsed = hunks(&[
            "@@ -1,2 +1,3 @@",
            " a",
            "+b",
            " c",
            "@@ -20,2 +30,1 @@ later",
            "-x",
            " y",
        ]);
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            numbers(&parsed[0]),
            vec![
                (LineKind::Context, Some(1), Some(1)),
                (LineKind::Added, None, Some(2)),
                (LineKind::Context, Some(2), Some(3)),
            ]
        );
        assert_eq!(
            numbers(&parsed[1]),
            vec![
                (LineKind::Removed, Some(20), None),
                (LineKind::Context, Some(21), Some(30)),
            ]
        );
    }

    #[test]
    fn header_without_counts_means_one() {
        let parsed = hunks(&["@@ -1 +1 @@", "-a", "+b"]);
        let hunk = &parsed[0];
        assert_eq!(
            (
                hunk.old_start,
                hunk.old_count,
                hunk.new_start,
                hunk.new_count
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(hunk.lines.len(), 2);
    }

    #[test]
    fn crlf_line_ends_are_stripped() {
        let parsed = parse_patch("@@ -1,2 +1,2 @@\r\n a\r\n-b\r\n+c\r\n").expect("parses");
        let texts: Vec<&str> = parsed[0].lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn trailing_newline_is_optional() {
        let with = parse_patch("@@ -1 +1 @@\n-a\n+b\n").expect("parses");
        let without = parse_patch("@@ -1 +1 @@\n-a\n+b").expect("parses");
        assert_eq!(with, without);
    }

    #[test]
    fn empty_context_line_that_lost_its_space() {
        let parsed = hunks(&["@@ -1,3 +1,3 @@", " a", "", " c"]);
        let lines = &parsed[0].lines;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].kind, LineKind::Context);
        assert_eq!(lines[1].text, "");
        assert_eq!(lines[1].old_line, Some(2));
        assert_eq!(lines[1].new_line, Some(2));
    }

    #[test]
    fn empty_patch_has_no_hunks() {
        assert_eq!(parse_patch("").expect("parses"), Vec::new());
    }

    #[test]
    fn too_many_lines_is_an_error_at_the_extra_line() {
        let err = parse_patch("@@ -1 +1 @@\n-a\n+b\n+c").expect_err("count mismatch");
        assert_eq!(err.line, 4);
    }

    #[test]
    fn blank_line_after_a_complete_hunk_is_an_error() {
        let err = parse_patch("@@ -1 +1 @@\n-a\n+b\n\n\n").expect_err("count mismatch");
        assert_eq!(err.line, 4);
    }

    #[test]
    fn too_few_lines_is_an_error_at_the_next_header() {
        let err =
            parse_patch("@@ -1,2 +1,2 @@\n a\n@@ -9 +9 @@\n-x\n+y").expect_err("count mismatch");
        assert_eq!(err.line, 3);
    }

    #[test]
    fn too_few_lines_at_end_is_an_error_after_the_last_line() {
        let err = parse_patch("@@ -1,3 +1,3 @@\n a\n b").expect_err("count mismatch");
        assert_eq!(err.line, 4);
    }

    #[test]
    fn text_before_first_hunk_is_an_error() {
        let err = parse_patch("\ndiff --git a/x b/x\n@@ -1 +1 @@\n-a\n+b").expect_err("junk");
        assert_eq!(err.line, 2);
    }

    #[test]
    fn malformed_header_and_stray_lines_are_errors() {
        assert_eq!(parse_patch("@@ nope @@\n").expect_err("header").line, 1);
        assert_eq!(
            parse_patch("@@ -1 +1 @@\n?a\n").expect_err("prefix").line,
            2
        );
        assert_eq!(
            parse_patch("@@ -1 +1 @@\n\\ No newline at end of file\n-a\n+b")
                .expect_err("marker first")
                .line,
            2
        );
    }

    #[test]
    fn diff_line_anchor() {
        let parsed = hunks(&["@@ -4,2 +4,2 @@", " c", "-r", "+a"]);
        let anchors: Vec<(u32, Side)> = parsed[0].lines.iter().map(DiffLine::anchor).collect();
        assert_eq!(
            anchors,
            vec![(4, Side::Right), (5, Side::Left), (5, Side::Right)]
        );
    }

    #[test]
    fn find_line_on_both_sides() {
        let h = anchor_fixture();
        assert_eq!(find_line(&h, 2, Side::Left), Some((0, 1)));
        assert_eq!(find_line(&h, 2, Side::Right), Some((0, 2)));
        assert_eq!(find_line(&h, 3, Side::Left), Some((0, 3)));
        assert_eq!(find_line(&h, 3, Side::Right), Some((0, 3)));
        assert_eq!(find_line(&h, 10, Side::Left), Some((1, 0)));
        assert_eq!(find_line(&h, 12, Side::Right), Some((1, 3)));
        assert_eq!(find_line(&h, 12, Side::Left), None);
        assert_eq!(find_line(&h, 50, Side::Right), None);
    }

    #[test]
    fn anchor_single_line() {
        let h = anchor_fixture();
        assert_eq!(
            anchor_for_range(&h, (2, Side::Right), (2, Side::Right)),
            Ok(LineAnchor {
                line: 2,
                side: Side::Right,
                start: None
            })
        );
    }

    #[test]
    fn anchor_left_side_range() {
        let h = anchor_fixture();
        assert_eq!(
            anchor_for_range(&h, (2, Side::Left), (3, Side::Left)),
            Ok(LineAnchor {
                line: 3,
                side: Side::Left,
                start: Some((2, Side::Left)),
            })
        );
    }

    #[test]
    fn anchor_range_in_reverse_order() {
        let h = anchor_fixture();
        assert_eq!(
            anchor_for_range(&h, (12, Side::Right), (10, Side::Right)),
            Ok(LineAnchor {
                line: 12,
                side: Side::Right,
                start: Some((10, Side::Right)),
            })
        );
    }

    #[test]
    fn anchor_from_left_line_to_right_line() {
        let h = anchor_fixture();
        assert_eq!(
            anchor_for_range(&h, (2, Side::Left), (2, Side::Right)),
            Ok(LineAnchor {
                line: 2,
                side: Side::Right,
                start: Some((2, Side::Left)),
            })
        );
    }

    #[test]
    fn anchor_same_context_line_from_both_sides_is_single() {
        let h = hunks(&["@@ -5,1 +8,1 @@", " k"]);
        assert_eq!(
            anchor_for_range(&h, (5, Side::Left), (8, Side::Right)),
            Ok(LineAnchor {
                line: 8,
                side: Side::Right,
                start: None
            })
        );
    }

    #[test]
    fn anchor_not_in_diff() {
        let h = anchor_fixture();
        assert_eq!(
            anchor_for_range(&h, (2, Side::Left), (99, Side::Right)),
            Err(AnchorError::NotInDiff {
                line: 99,
                side: Side::Right
            })
        );
        assert_eq!(
            anchor_for_range(&h, (99, Side::Left), (2, Side::Left)),
            Err(AnchorError::NotInDiff {
                line: 99,
                side: Side::Left
            })
        );
    }

    #[test]
    fn anchor_spanning_hunks() {
        let h = anchor_fixture();
        assert_eq!(
            anchor_for_range(&h, (1, Side::Right), (10, Side::Right)),
            Err(AnchorError::SpansHunks)
        );
    }

    #[test]
    fn git_patch_is_none_without_a_patch() {
        assert_eq!(
            git_patch(&file(FileStatus::Modified, "a.bin", None, None)),
            None
        );
    }

    #[test]
    fn git_patch_for_added_file() {
        let f = file(
            FileStatus::Added,
            "src/new.rs",
            None,
            Some("@@ -0,0 +1 @@\n+hi"),
        );
        assert_eq!(
            git_patch(&f).as_deref(),
            Some(
                "diff --git a/src/new.rs b/src/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1 @@\n+hi\n"
            )
        );
    }

    #[test]
    fn git_patch_for_removed_file() {
        let f = file(
            FileStatus::Removed,
            "old.rs",
            None,
            Some("@@ -1 +0,0 @@\n-bye\n"),
        );
        assert_eq!(
            git_patch(&f).as_deref(),
            Some(
                "diff --git a/old.rs b/old.rs\ndeleted file mode 100644\n--- a/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n"
            )
        );
    }

    #[test]
    fn git_patch_for_modified_file_ends_with_exactly_one_newline() {
        let f = file(
            FileStatus::Modified,
            "lib.rs",
            None,
            Some("@@ -1 +1 @@\n-a\n+b\n\n\n"),
        );
        assert_eq!(
            git_patch(&f).as_deref(),
            Some("diff --git a/lib.rs b/lib.rs\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1 @@\n-a\n+b\n")
        );
    }

    #[test]
    fn git_patch_for_rename_with_changes() {
        let f = file(
            FileStatus::Renamed,
            "new/name.rs",
            Some("old/name.rs"),
            Some("@@ -1 +1 @@\n-a\n+b"),
        );
        assert_eq!(
            git_patch(&f).as_deref(),
            Some(
                "diff --git a/old/name.rs b/new/name.rs\nrename from old/name.rs\nrename to new/name.rs\n--- a/old/name.rs\n+++ b/new/name.rs\n@@ -1 +1 @@\n-a\n+b\n"
            )
        );
    }

    #[test]
    fn git_patch_for_pure_rename_and_copy() {
        let renamed = file(FileStatus::Renamed, "b.rs", Some("a.rs"), Some(""));
        assert_eq!(
            git_patch(&renamed).as_deref(),
            Some("diff --git a/a.rs b/b.rs\nrename from a.rs\nrename to b.rs\n")
        );
        let copied = file(FileStatus::Copied, "b.rs", Some("a.rs"), Some(""));
        assert_eq!(
            git_patch(&copied).as_deref(),
            Some("diff --git a/a.rs b/b.rs\ncopy from a.rs\ncopy to b.rs\n")
        );
    }

    #[test]
    fn git_patch_quotes_paths_with_spaces() {
        let f = file(
            FileStatus::Modified,
            "my file.rs",
            None,
            Some("@@ -1 +1 @@\n-a\n+b"),
        );
        assert_eq!(
            git_patch(&f).as_deref(),
            Some(
                "diff --git \"a/my file.rs\" \"b/my file.rs\"\n--- \"a/my file.rs\"\n+++ \"b/my file.rs\"\n@@ -1 +1 @@\n-a\n+b\n"
            )
        );
        let renamed = file(FileStatus::Renamed, "b c.rs", Some("a b.rs"), Some(""));
        assert_eq!(
            git_patch(&renamed).as_deref(),
            Some(
                "diff --git \"a/a b.rs\" \"b/b c.rs\"\nrename from \"a b.rs\"\nrename to \"b c.rs\"\n"
            )
        );
    }

    #[test]
    fn c_quote_escapes_like_git() {
        assert_eq!(c_quote("plain/path.rs"), "plain/path.rs");
        assert_eq!(c_quote("a\tb"), "\"a\\tb\"");
        assert_eq!(c_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(c_quote("back\\slash"), "\"back\\\\slash\"");
        assert_eq!(c_quote("é.rs"), "\"\\303\\251.rs\"");
        assert_eq!(c_quote("a\u{1}b"), "\"a\\001b\"");
    }
}
