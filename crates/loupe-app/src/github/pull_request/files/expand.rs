//! The lines between a file's hunks that its patch leaves out, revealed on
//! demand from the file at the commit the diff shows, as the Expand arrows
//! of GitHub's Files changed do. Pure, so it is unit tested.

use std::ops::Range;

use loupe_github::diff::{DiffLine, Hunk, LineKind};
use loupe_github::{FileStatus, Side};

/// How many lines one Expand up or Expand down reveals, as on GitHub.
pub const STEP: u32 = 20;

/// Whether a file of this status has lines its patch can leave out: an
/// added or deleted file's patch already holds every line.
pub fn expandable(status: FileStatus) -> bool {
    matches!(
        status,
        FileStatus::Modified | FileStatus::Changed | FileStatus::Renamed | FileStatus::Copied
    )
}

/// A file's text at the commit the diff shows, in lines as Git counts them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    lines: Vec<String>,
    /// Whether the last line ends with a newline.
    final_newline: bool,
}

impl Source {
    pub fn new(text: &str) -> Self {
        let lines = text
            .split_inclusive('\n')
            .map(|line| {
                let line = line.strip_suffix('\n').unwrap_or(line);
                line.strip_suffix('\r').unwrap_or(line).to_owned()
            })
            .collect();
        Self {
            lines,
            final_newline: text.is_empty() || text.ends_with('\n'),
        }
    }

    pub fn line_count(&self) -> u32 {
        self.lines.len() as u32
    }
}

/// Which part of a gap a control reveals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The lines just below the hunk over the gap.
    Down,
    /// The lines just above the hunk under the gap.
    Up,
    /// The whole gap.
    All,
}

/// A run of lines the diff does not show, numbered in the file the diff
/// shows (the new side).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    /// The first hidden line.
    pub start: u32,
    /// The line after the last hidden one; `None` at the end of a file not
    /// read yet, whose length is unknown.
    pub end: Option<u32>,
    /// The last line shown above the gap, where its controls go; `None` at
    /// the top of the file.
    pub above: Option<(u32, Side)>,
    /// Whether a hunk follows the gap, so it is not the end of the file.
    pub before_hunk: bool,
    /// What turns a line number of the gap into the old side's: unchanged
    /// lines sit at the same place on both sides, moved by the lines added
    /// and removed above them.
    shift: i64,
}

impl Gap {
    /// How many lines the gap hides, when known.
    pub fn hidden(&self) -> Option<u32> {
        self.end.map(|end| end - self.start)
    }

    /// The controls the gap offers, in the order they are drawn: one for
    /// the whole gap when it is short, else Down under a hunk, Up over one,
    /// and the whole gap once its length is known.
    pub fn directions(&self) -> Vec<Direction> {
        if self.hidden().is_some_and(|hidden| hidden <= STEP) {
            return vec![Direction::All];
        }
        let mut directions = Vec::new();
        if self.above.is_some() {
            directions.push(Direction::Down);
        }
        if self.before_hunk {
            directions.push(Direction::Up);
        }
        if self.end.is_some() {
            directions.push(Direction::All);
        }
        directions
    }

    /// The lines `direction` reveals. Past the end of a file not read yet,
    /// Down asks for a whole step and gets what the file has.
    pub fn reveal(&self, direction: Direction) -> Range<u32> {
        let end = self.end.unwrap_or(self.start + STEP);
        match direction {
            Direction::Down => self.start..end.min(self.start + STEP),
            Direction::Up => end.saturating_sub(STEP).max(self.start)..end,
            Direction::All => self.start..end,
        }
    }
}

/// The lines a hunk covers on one side, `begin..end`. A hunk with no lines on
/// a side names the line before where it sits, as Git writes it.
fn span(start: u32, count: u32) -> (u32, u32) {
    let begin = if count == 0 { start + 1 } else { start };
    (begin, begin + count)
}

fn old_span(hunk: &Hunk) -> (u32, u32) {
    span(hunk.old_start, hunk.old_count)
}

fn new_span(hunk: &Hunk) -> (u32, u32) {
    span(hunk.new_start, hunk.new_count)
}

enum Part {
    Gap(Gap),
    Hunk(usize),
}

/// The file as `hunks` show it: each hunk with the gaps around it, in file
/// order.
fn parts(hunks: &[Hunk], total: Option<u32>) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut next = 1;
    let mut shift = 0;
    let mut above = None;
    for (ix, hunk) in hunks.iter().enumerate() {
        let (begin, end) = new_span(hunk);
        if begin > next {
            parts.push(Part::Gap(Gap {
                start: next,
                end: Some(begin),
                above,
                before_hunk: true,
                shift,
            }));
        }
        parts.push(Part::Hunk(ix));
        next = next.max(end);
        shift = i64::from(old_span(hunk).1) - i64::from(end);
        above = hunk.lines.last().map(DiffLine::anchor).or(above);
    }
    // The patch marks the last line of a file without a final newline.
    let reaches_end = hunks
        .last()
        .and_then(|hunk| hunk.lines.iter().rev().find(|line| line.new_line.is_some()))
        .is_some_and(|line| line.no_newline_at_eof);
    let end = match total {
        Some(total) => (next <= total).then_some(Some(total + 1)),
        None => (!hunks.is_empty() && !reaches_end).then_some(None),
    };
    if let Some(end) = end {
        parts.push(Part::Gap(Gap {
            start: next,
            end,
            above,
            before_hunk: false,
            shift,
        }));
    }
    parts
}

/// The runs of lines `hunks` leave out of a file of `total` lines, in file
/// order. With `total` unknown (the file not read yet), whether lines follow
/// the last hunk is unknown unless the patch reaches the end of the file.
pub fn gaps(hunks: &[Hunk], total: Option<u32>) -> Vec<Gap> {
    parts(hunks, total)
        .into_iter()
        .filter_map(|part| match part {
            Part::Gap(gap) => Some(gap),
            Part::Hunk(_) => None,
        })
        .collect()
}

/// `hunks` with the new-side lines of `revealed` read from `source` merged
/// in as unchanged lines: a gap revealed whole joins the hunks around it,
/// and a hunk keeps the scope its header names. A hunk nothing joined is
/// kept as it was.
pub fn expanded(hunks: &[Hunk], source: &Source, revealed: &[Range<u32>]) -> Vec<Hunk> {
    let total = source.line_count();
    // Each line shown, with the hunk of the patch it comes from.
    let mut lines: Vec<(DiffLine, Option<usize>)> = Vec::new();
    for part in parts(hunks, Some(total)) {
        match part {
            Part::Hunk(ix) => {
                lines.extend(hunks[ix].lines.iter().map(|line| (line.clone(), Some(ix))))
            }
            Part::Gap(gap) => {
                let end = gap.end.unwrap_or(total + 1);
                lines.extend(
                    (gap.start..end)
                        .filter(|line| revealed.iter().any(|range| range.contains(line)))
                        .map(|line| {
                            let line = DiffLine {
                                kind: LineKind::Context,
                                old_line: Some((i64::from(line) + gap.shift) as u32),
                                new_line: Some(line),
                                text: source.lines[line as usize - 1].clone(),
                                no_newline_at_eof: line == total && !source.final_newline,
                            };
                            (line, None)
                        }),
                );
            }
        }
    }

    let mut shown = Vec::new();
    let mut current: Vec<(DiffLine, Option<usize>)> = Vec::new();
    // The old and new numbers the next line of the current hunk must have.
    let mut next = (0, 0);
    let mut previous_origin = None;
    for (line, origin) in lines {
        let opens_hunk = origin.is_some() && origin != previous_origin;
        let continues = match origin {
            _ if current.is_empty() => false,
            Some(ix) if opens_hunk => next == (old_span(&hunks[ix]).0, new_span(&hunks[ix]).0),
            _ => {
                line.old_line.is_none_or(|old| old == next.0)
                    && line.new_line.is_none_or(|new| new == next.1)
            }
        };
        if !continues && !current.is_empty() {
            shown.push(close(std::mem::take(&mut current), hunks));
        }
        if current.is_empty() {
            next = match origin {
                Some(ix) if opens_hunk => (old_span(&hunks[ix]).0, new_span(&hunks[ix]).0),
                _ => (line.old_line.unwrap_or(0), line.new_line.unwrap_or(0)),
            };
        }
        if line.old_line.is_some() {
            next.0 += 1;
        }
        if line.new_line.is_some() {
            next.1 += 1;
        }
        previous_origin = origin;
        current.push((line, origin));
    }
    if !current.is_empty() {
        shown.push(close(current, hunks));
    }
    shown
}

/// The hunk of `lines`, with a header that counts them.
fn close(lines: Vec<(DiffLine, Option<usize>)>, hunks: &[Hunk]) -> Hunk {
    let first_origin = lines.iter().find_map(|(_, origin)| *origin);
    if let Some(ix) = first_origin
        && lines.len() == hunks[ix].lines.len()
        && lines.iter().all(|(_, origin)| *origin == Some(ix))
    {
        return hunks[ix].clone();
    }
    let lines: Vec<DiffLine> = lines.into_iter().map(|(line, _)| line).collect();
    let count = |side: fn(&DiffLine) -> Option<u32>| lines.iter().filter_map(side).count() as u32;
    // A side with no line keeps the number the patch's header gave it.
    let start = |side: fn(&DiffLine) -> Option<u32>, header: fn(&Hunk) -> u32| {
        lines
            .iter()
            .find_map(side)
            .or_else(|| first_origin.map(|ix| header(&hunks[ix])))
            .unwrap_or(0)
    };
    let old_line: fn(&DiffLine) -> Option<u32> = |line| line.old_line;
    let new_line: fn(&DiffLine) -> Option<u32> = |line| line.new_line;
    let (old_count, new_count) = (count(old_line), count(new_line));
    let old_start = start(old_line, |hunk| hunk.old_start);
    let new_start = start(new_line, |hunk| hunk.new_start);
    let scope = first_origin.map_or("", |ix| scope(&hunks[ix].header));
    Hunk {
        header: format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@{scope}"),
        old_start,
        old_count,
        new_start,
        new_count,
        lines,
    }
}

/// What follows the closing `@@` of a hunk header: the scope Git names.
fn scope(header: &str) -> &str {
    header
        .strip_prefix("@@")
        .and_then(|rest| rest.find("@@").map(|ix| &rest[ix + 2..]))
        .unwrap_or("")
}

/// `hunks` written back as a patch body, the shape GitHub sends.
pub fn patch_text(hunks: &[Hunk]) -> String {
    let mut out = String::new();
    for hunk in hunks {
        out.push_str(&hunk.header);
        out.push('\n');
        for line in &hunk.lines {
            out.push(match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            });
            out.push_str(&line.text);
            out.push('\n');
            if line.no_newline_at_eof {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    out.pop();
    out
}

/// The line `lead` lines above new-side line `line` among the lines `hunks`
/// show, to scroll to so `line` lands under the pinned file header; `None`
/// near the top of the file, where the header is the place.
pub fn lead(hunks: &[Hunk], line: u32, lead: usize) -> Option<(u32, Side)> {
    let lines: Vec<&DiffLine> = hunks.iter().flat_map(|hunk| &hunk.lines).collect();
    let ix = lines
        .iter()
        .position(|shown| shown.new_line == Some(line))?;
    ix.checked_sub(lead).map(|ix| lines[ix].anchor())
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::diff::DiffFile;
    use loupe_github::Side;
    use loupe_github::diff::{Hunk, LineKind, parse_patch};

    use super::{Direction, Gap, STEP, Source, expanded, gaps, lead, patch_text};

    /// A file of 101 lines whose line 10 changed and that gained a line
    /// after line 60, as GitHub sends it.
    const PATCH: &str = "@@ -7,7 +7,7 @@ fn first()\n line 7\n line 8\n line 9\n-line 10\n+LINE 10\n line 11\n line 12\n line 13\n@@ -57,7 +57,8 @@ fn second()\n line 57\n line 58\n line 59\n-line 60\n+LINE 60\n+line 60b\n line 61\n line 62\n line 63";

    fn source() -> Source {
        let text: String = (1..=100)
            .flat_map(|line| match line {
                10 => vec!["LINE 10".to_owned()],
                60 => vec!["LINE 60".to_owned(), "line 60b".to_owned()],
                line => vec![format!("line {line}")],
            })
            .map(|line| line + "\n")
            .collect();
        Source::new(&text)
    }

    fn hunks() -> Vec<Hunk> {
        parse_patch(PATCH).expect("parses")
    }

    #[test]
    fn gaps_lie_above_between_and_after_the_hunks() {
        let found = gaps(&hunks(), Some(source().line_count()));
        let spans: Vec<_> = found.iter().map(|gap| (gap.start, gap.end)).collect();
        assert_eq!(spans, [(1, Some(7)), (14, Some(57)), (65, Some(102))]);
        assert_eq!(found[0].above, None, "the first sits under the file header");
        assert_eq!(found[1].above, Some((13, Side::Right)));
        assert_eq!(found[2].above, Some((64, Side::Right)));
        assert!(found[1].before_hunk && !found[2].before_hunk);

        let unread = gaps(&hunks(), None);
        assert_eq!(
            unread.last().map(|gap| (gap.start, gap.end)),
            Some((65, None)),
            "lines may follow the last hunk of a file not read yet"
        );
        let at_end = parse_patch(
            "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n\\ No newline at end of file",
        )
        .expect("parses");
        assert!(gaps(&at_end, None).is_empty(), "the patch reaches the end");
    }

    #[test]
    fn each_gap_offers_the_controls_github_does() {
        let gap = |start: u32, end: Option<u32>, above: bool, before_hunk| Gap {
            start,
            end,
            above: above.then_some((start - 1, Side::Right)),
            before_hunk,
            shift: 0,
        };
        use Direction::{All, Down, Up};
        assert_eq!(gap(14, Some(57), true, true).directions(), [Down, Up, All]);
        assert_eq!(gap(1, Some(40), false, true).directions(), [Up, All]);
        assert_eq!(gap(65, Some(102), true, false).directions(), [Down, All]);
        assert_eq!(gap(65, None, true, false).directions(), [Down]);
        assert_eq!(
            gap(14, Some(30), true, true).directions(),
            [All],
            "a short gap opens whole"
        );

        let middle = gap(14, Some(57), true, true);
        assert_eq!(middle.reveal(Down), 14..14 + STEP);
        assert_eq!(middle.reveal(Up), 57 - STEP..57);
        assert_eq!(middle.reveal(All), 14..57);
        assert_eq!(gap(65, None, true, false).reveal(Down), 65..65 + STEP);
    }

    #[test]
    fn revealed_lines_join_the_hunk_beside_them_with_both_numbers() {
        let (hunks, source) = (hunks(), source());
        let shown = expanded(&hunks, &source, &[14..34, 37..57]);
        assert_eq!(shown.len(), 2, "lines 34 to 36 still part the hunks");
        assert_eq!(shown[0].header, "@@ -7,27 +7,27 @@ fn first()");
        assert_eq!(shown[1].header, "@@ -37,27 +37,28 @@ fn second()");
        let joined = &shown[0].lines[8];
        assert_eq!(
            (
                joined.kind,
                joined.old_line,
                joined.new_line,
                joined.text.as_str()
            ),
            (LineKind::Context, Some(14), Some(14), "line 14")
        );

        // Past the added line the old side is one line behind.
        let after = expanded(&hunks, &source, std::slice::from_ref(&(65..70)));
        assert_eq!(after[0], hunks[0], "a hunk nothing joined stays as it was");
        let first_after = &after[1].lines[after[1].lines.len() - 5];
        assert_eq!(
            (
                first_after.old_line,
                first_after.new_line,
                first_after.text.as_str()
            ),
            (Some(64), Some(65), "line 64")
        );

        let whole = expanded(&hunks, &source, &[1..7, 14..57, 65..102]);
        assert_eq!(
            whole.len(),
            1,
            "a gap revealed whole joins the hunks around it"
        );
        assert_eq!((whole[0].old_count, whole[0].new_count), (100, 101));
        assert!(gaps(&whole, Some(source.line_count())).is_empty());

        assert_eq!(
            expanded(&hunks, &source, &[]),
            hunks,
            "nothing revealed changes nothing"
        );
    }

    #[test]
    fn the_last_line_keeps_its_missing_newline() {
        let hunks = parse_patch("@@ -1,2 +1,2 @@\n-a\n+A\n b").expect("parses");
        let shown = expanded(
            &hunks,
            &Source::new("A\nb\nc\nd"),
            std::slice::from_ref(&(3..5)),
        );
        assert_eq!(shown[0].header, "@@ -1,4 +1,4 @@");
        let last = shown[0].lines.last().expect("line");
        assert_eq!((last.text.as_str(), last.no_newline_at_eof), ("d", true));
        assert_eq!(parse_patch(&patch_text(&shown)).expect("parses"), shown);
    }

    #[test]
    fn the_written_patch_reads_back_in_the_viewer() {
        let shown = expanded(&hunks(), &source(), &[1..7, 14..20]);
        let text = patch_text(&shown);
        assert_eq!(parse_patch(&text).expect("parses"), shown);
        assert_eq!(shown[0].lines[0].text, "line 1");
        let patch = format!("diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n{text}\n");
        let doc = DiffFile::parse(&patch)
            .expect("the viewer reads it")
            .remove(0);
        assert_eq!((doc.additions(), doc.deletions()), (3, 2));
    }

    #[test]
    fn the_lead_line_sits_a_few_lines_above() {
        let shown = expanded(&hunks(), &source(), std::slice::from_ref(&(14..34)));
        assert_eq!(lead(&shown, 14, 3), Some((11, Side::Right)));
        assert_eq!(
            lead(&shown, 8, 3),
            None,
            "near the top the header is the place"
        );
    }
}
