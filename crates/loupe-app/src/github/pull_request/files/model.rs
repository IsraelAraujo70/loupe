//! Pure view-model logic of the files tab: side mapping, thread placement,
//! composer labels, viewed bookkeeping. No GPUI state, so it is unit tested.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::diff::{DiffFile, DiffMode, DiffSide};
use loupe_github::diff::{AnchorError, Hunk};
use loupe_github::{ChangedFile, FileStatus, LineAnchor, NodeId, ReviewThread, Side, ViewedState};

use crate::github::highlight::language_for_path;
use crate::store::DiffLayout;

pub fn diff_mode(layout: DiffLayout) -> DiffMode {
    match layout {
        DiffLayout::Unified => DiffMode::Unified,
        DiffLayout::Split => DiffMode::Split,
    }
}

pub fn to_diff_side(side: Side) -> DiffSide {
    match side {
        Side::Left => DiffSide::Original,
        Side::Right => DiffSide::Modified,
    }
}

pub fn to_side(side: DiffSide) -> Side {
    match side {
        DiffSide::Original => Side::Left,
        DiffSide::Modified => Side::Right,
    }
}

/// How many files the viewer has marked as viewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewedCounts {
    pub viewed: usize,
    pub total: usize,
}

impl ViewedCounts {
    pub fn label(self) -> String {
        format!("{} / {} viewed", self.viewed, self.total)
    }
}

/// A file counts as viewed only in `Viewed`; a later push turns it `Dismissed`.
pub fn is_viewed(state: ViewedState) -> bool {
    state == ViewedState::Viewed
}

pub fn viewed_counts(files: &[ChangedFile]) -> ViewedCounts {
    ViewedCounts {
        viewed: files.iter().filter(|file| is_viewed(file.viewed)).count(),
        total: files.len(),
    }
}

/// Marks `path` viewed or unviewed and returns the state it had before, or
/// `None` when the file is unknown.
pub fn set_viewed(files: &mut [ChangedFile], path: &str, viewed: bool) -> Option<ViewedState> {
    let file = files.iter_mut().find(|file| file.path == path)?;
    let previous = file.viewed;
    file.viewed = if viewed {
        ViewedState::Viewed
    } else {
        ViewedState::Unviewed
    };
    Some(previous)
}

pub fn restore_viewed(files: &mut [ChangedFile], path: &str, state: ViewedState) {
    if let Some(file) = files.iter_mut().find(|file| file.path == path) {
        file.viewed = state;
    }
}

/// Splits a path into its directory (with the trailing slash) and file name.
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(ix) => path.split_at(ix + 1),
        None => ("", path),
    }
}

pub fn status_letter(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "A",
        FileStatus::Removed => "D",
        FileStatus::Renamed => "R",
        FileStatus::Copied => "C",
        FileStatus::Modified | FileStatus::Changed => "M",
        FileStatus::Unchanged => "·",
    }
}

/// Where a review thread is drawn in the diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Beneath a source line.
    Line {
        path: String,
        side: DiffSide,
        line: usize,
    },
    /// Beneath the file header, for threads that no longer map to a line.
    File { path: String, label: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedThread {
    pub thread_id: NodeId,
    pub placement: Placement,
}

pub fn outdated_label(thread: &ReviewThread) -> String {
    match thread.original_line.or(thread.line) {
        Some(line) => format!("Outdated comment on line {line}"),
        None => "Outdated comment".to_owned(),
    }
}

/// Places every thread whose file is part of the diff. A thread with a line
/// that the patch does not contain falls back to the file header, so it never
/// disappears.
pub fn place_threads(
    threads: &[ReviewThread],
    has_file: impl Fn(&str) -> bool,
    has_line: impl Fn(&str, Side, u32) -> bool,
) -> Vec<PlacedThread> {
    threads
        .iter()
        .filter(|thread| has_file(&thread.path))
        .map(|thread| {
            let placement = match thread.line {
                Some(line) if has_line(&thread.path, thread.side, line) => Placement::Line {
                    path: thread.path.clone(),
                    side: to_diff_side(thread.side),
                    line: line as usize,
                },
                Some(line) => Placement::File {
                    path: thread.path.clone(),
                    label: format!("Comment on line {line} (outside the visible diff)"),
                },
                None => Placement::File {
                    path: thread.path.clone(),
                    label: outdated_label(thread),
                },
            };
            PlacedThread {
                thread_id: thread.id.clone(),
                placement,
            }
        })
        .collect()
}

fn line_ref(line: u32, side: Side) -> String {
    let prefix = match side {
        Side::Left => 'L',
        Side::Right => 'R',
    };
    format!("{prefix}{line}")
}

/// The composer header: "Comment on line R40", or "Comment on lines
/// L12–L15 (left)" for a range on one side.
pub fn composer_label(anchor: &LineAnchor) -> String {
    let end = line_ref(anchor.line, anchor.side);
    match anchor.start {
        Some((start, start_side)) if (start, start_side) != (anchor.line, anchor.side) => {
            let start_ref = line_ref(start, start_side);
            if start_side == anchor.side {
                let side = match anchor.side {
                    Side::Left => "left",
                    Side::Right => "right",
                };
                format!("Comment on lines {start_ref}–{end} ({side})")
            } else {
                format!("Comment on lines {start_ref}–{end}")
            }
        }
        _ => format!("Comment on line {end}"),
    }
}

pub fn anchor_error_message(error: &AnchorError) -> String {
    match error {
        AnchorError::SpansHunks => "GitHub only accepts comments within one hunk.".to_owned(),
        AnchorError::NotInDiff { line, side } => format!(
            "Line {} is not part of the diff, so GitHub cannot attach a comment to it.",
            line_ref(*line, *side)
        ),
    }
}

/// A Git patch GitHub has no hunks for (binary or too large), so the viewer
/// still lists the file in the diff and the tab can annotate it.
pub fn placeholder_patch(path: &str) -> String {
    format!("diff --git a/{path} b/{path}\nBinary files a/{path} and b/{path} differ\n")
}

/// Why a file has no diff in the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// GitHub sent no patch: a binary file or one too large to diff.
    NoPatch,
    /// GitHub sent a patch the viewer could not read.
    Unreadable,
}

impl Missing {
    pub fn note(self) -> &'static str {
        match self {
            Missing::NoPatch => {
                "Diff not shown: GitHub has no text diff for this file (binary or too large)."
            }
            Missing::Unreadable => "Diff not shown: the patch could not be read.",
        }
    }
}

/// The changed files with everything the tab derives from their patches.
pub struct LoadedFiles {
    pub files: Vec<ChangedFile>,
    /// One document per file, in file order (a path the viewer cannot even
    /// stand a placeholder in is left out and only listed).
    pub docs: Vec<DiffFile>,
    /// Parsed hunks per path, used to anchor new comments.
    pub hunks: HashMap<String, Arc<Vec<Hunk>>>,
    pub missing: HashMap<String, Missing>,
}

/// Builds the viewer documents for `files`. `patch_for` turns a file into a
/// full Git patch; `parse_hunks` reads its hunks. A file that fails either
/// step keeps a placeholder document, so one bad patch never hides the rest.
pub fn build_files(
    files: Vec<ChangedFile>,
    patch_for: impl Fn(&ChangedFile) -> Option<String>,
    parse_hunks: impl Fn(&str) -> Option<Vec<Hunk>>,
) -> LoadedFiles {
    let mut docs = Vec::with_capacity(files.len());
    let mut hunks = HashMap::new();
    let mut missing = HashMap::new();
    for file in &files {
        let parsed = match patch_for(file) {
            None => Err(Missing::NoPatch),
            Some(patch) => DiffFile::parse(&patch)
                .ok()
                .and_then(|parsed| parsed.into_iter().next())
                .map(|doc| doc.with_language(language_for_path(&file.path)))
                .ok_or(Missing::Unreadable),
        };
        let doc = parsed.or_else(|reason| {
            missing.insert(file.path.clone(), reason);
            DiffFile::parse(&placeholder_patch(&file.path))
                .ok()
                .and_then(|parsed| parsed.into_iter().next())
                .ok_or(reason)
        });
        docs.extend(doc.ok());
        if !missing.contains_key(&file.path)
            && let Some(parsed) = file.patch.as_deref().and_then(&parse_hunks)
        {
            hunks.insert(file.path.clone(), Arc::new(parsed));
        }
    }
    LoadedFiles {
        files,
        docs,
        hunks,
        missing,
    }
}

pub fn resolved_summary(thread: &ReviewThread) -> String {
    match &thread.resolved_by {
        Some(actor) => format!("Resolved by {} · click to expand", actor.login),
        None => "Resolved · click to expand".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Missing, Placement, anchor_error_message, build_files, composer_label, diff_mode,
        is_viewed, outdated_label, place_threads, placeholder_patch, resolved_summary,
        restore_viewed, set_viewed, split_path, status_letter, to_diff_side, to_side,
        viewed_counts,
    };
    use crate::store::DiffLayout;
    use gpui_kit::component::diff::{DiffFile, DiffMode, DiffSide};
    use loupe_github::diff::AnchorError;
    use loupe_github::{
        Actor, ChangedFile, FileStatus, LineAnchor, ReviewThread, Side, ViewedState,
    };

    fn file(path: &str, viewed: ViewedState) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            previous_path: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            patch: None,
            viewed,
        }
    }

    fn thread(id: &str, path: &str, line: Option<u32>, side: Side) -> ReviewThread {
        ReviewThread {
            id: id.to_owned(),
            path: path.to_owned(),
            line,
            start_line: None,
            original_line: Some(7),
            original_start_line: None,
            side,
            start_side: None,
            is_outdated: line.is_none(),
            is_resolved: false,
            resolved_by: None,
            viewer_can_reply: true,
            viewer_can_resolve: true,
            viewer_can_unresolve: false,
            comments: Vec::new(),
        }
    }

    #[test]
    fn sides_map_between_github_and_the_diff() {
        assert_eq!(to_diff_side(Side::Left), DiffSide::Original);
        assert_eq!(to_diff_side(Side::Right), DiffSide::Modified);
        assert_eq!(to_side(DiffSide::Original), Side::Left);
        assert_eq!(to_side(DiffSide::Modified), Side::Right);
    }

    #[test]
    fn only_viewed_files_count_as_viewed() {
        let files = [
            file("a", ViewedState::Viewed),
            file("b", ViewedState::Unviewed),
            file("c", ViewedState::Dismissed),
        ];
        let counts = viewed_counts(&files);
        assert_eq!((counts.viewed, counts.total), (1, 3));
        assert_eq!(counts.label(), "1 / 3 viewed");
        assert!(!is_viewed(ViewedState::Dismissed));
    }

    #[test]
    fn toggling_viewed_returns_the_previous_state_for_revert() {
        let mut files = [file("a", ViewedState::Dismissed)];
        assert_eq!(
            set_viewed(&mut files, "a", true),
            Some(ViewedState::Dismissed)
        );
        assert_eq!(files[0].viewed, ViewedState::Viewed);
        restore_viewed(&mut files, "a", ViewedState::Dismissed);
        assert_eq!(files[0].viewed, ViewedState::Dismissed);
        assert_eq!(set_viewed(&mut files, "missing", true), None);
    }

    #[test]
    fn paths_split_into_muted_directory_and_bright_name() {
        assert_eq!(split_path("src/a/b.rs"), ("src/a/", "b.rs"));
        assert_eq!(split_path("README.md"), ("", "README.md"));
        assert_eq!(status_letter(FileStatus::Removed), "D");
        assert_eq!(status_letter(FileStatus::Changed), "M");
    }

    #[test]
    fn threads_land_on_their_line_or_fall_back_to_the_file() {
        let threads = [
            thread("t1", "src/a.rs", Some(10), Side::Right),
            thread("t2", "src/a.rs", Some(3), Side::Left),
            thread("t3", "src/a.rs", None, Side::Right),
            thread("t4", "gone.rs", Some(1), Side::Right),
        ];
        let placed = place_threads(
            &threads,
            |path| path == "src/a.rs",
            |_, side, line| side == Side::Right && line == 10,
        );
        let by_id = |id: &str| {
            placed
                .iter()
                .find(|placed| placed.thread_id == id)
                .map(|placed| placed.placement.clone())
        };
        assert_eq!(
            by_id("t1"),
            Some(Placement::Line {
                path: "src/a.rs".into(),
                side: DiffSide::Modified,
                line: 10
            })
        );
        assert_eq!(
            by_id("t2"),
            Some(Placement::File {
                path: "src/a.rs".into(),
                label: "Comment on line 3 (outside the visible diff)".into()
            })
        );
        assert_eq!(
            by_id("t3"),
            Some(Placement::File {
                path: "src/a.rs".into(),
                label: "Outdated comment on line 7".into()
            })
        );
        assert_eq!(by_id("t4"), None);
    }

    #[test]
    fn outdated_label_falls_back_without_a_line() {
        let mut outdated = thread("t", "a", None, Side::Right);
        assert_eq!(outdated_label(&outdated), "Outdated comment on line 7");
        outdated.original_line = None;
        assert_eq!(outdated_label(&outdated), "Outdated comment");
    }

    #[test]
    fn composer_label_names_the_line_range_and_side() {
        let single = LineAnchor {
            line: 40,
            side: Side::Right,
            start: None,
        };
        assert_eq!(composer_label(&single), "Comment on line R40");
        let same_point = LineAnchor {
            line: 40,
            side: Side::Right,
            start: Some((40, Side::Right)),
        };
        assert_eq!(composer_label(&same_point), "Comment on line R40");
        let range = LineAnchor {
            line: 15,
            side: Side::Left,
            start: Some((12, Side::Left)),
        };
        assert_eq!(composer_label(&range), "Comment on lines L12–L15 (left)");
        let cross = LineAnchor {
            line: 15,
            side: Side::Right,
            start: Some((12, Side::Left)),
        };
        assert_eq!(composer_label(&cross), "Comment on lines L12–R15");
    }

    #[test]
    fn anchor_errors_read_as_sentences() {
        assert_eq!(
            anchor_error_message(&AnchorError::SpansHunks),
            "GitHub only accepts comments within one hunk."
        );
        assert!(
            anchor_error_message(&AnchorError::NotInDiff {
                line: 9,
                side: Side::Left
            })
            .contains("L9")
        );
    }

    #[test]
    fn resolved_summary_names_who_resolved() {
        let mut resolved = thread("t", "a", Some(1), Side::Right);
        resolved.is_resolved = true;
        assert_eq!(resolved_summary(&resolved), "Resolved · click to expand");
        resolved.resolved_by = Some(Actor {
            login: "ana".into(),
            avatar_url: String::new(),
        });
        assert_eq!(
            resolved_summary(&resolved),
            "Resolved by ana · click to expand"
        );
    }

    #[test]
    fn layout_maps_to_the_diff_mode() {
        assert_eq!(diff_mode(DiffLayout::Unified), DiffMode::Unified);
        assert_eq!(diff_mode(DiffLayout::Split), DiffMode::Split);
    }

    #[test]
    fn a_bad_patch_keeps_a_placeholder_and_never_hides_other_files() {
        const GOOD: &str =
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let mut files = vec![
            file("a.txt", ViewedState::Unviewed),
            file("logo.png", ViewedState::Unviewed),
            file("broken.txt", ViewedState::Unviewed),
        ];
        files[0].patch = Some("@@ -1 +1 @@\n-old\n+new\n".into());
        files[2].patch = Some("not a patch".into());
        let loaded = build_files(
            files,
            |file| match file.path.as_str() {
                "a.txt" => Some(GOOD.to_owned()),
                "broken.txt" => Some("@@ nonsense".to_owned()),
                _ => None,
            },
            |_| Some(Vec::new()),
        );
        let paths: Vec<&str> = loaded.docs.iter().map(|doc| doc.path().as_str()).collect();
        assert_eq!(paths, ["a.txt", "logo.png", "broken.txt"]);
        assert_eq!(loaded.missing.get("logo.png"), Some(&Missing::NoPatch));
        assert_eq!(loaded.missing.get("broken.txt"), Some(&Missing::Unreadable));
        assert!(!loaded.missing.contains_key("a.txt"));
        assert!(loaded.hunks.contains_key("a.txt"));
        assert!(!loaded.hunks.contains_key("broken.txt"));
    }

    #[test]
    fn placeholder_patch_parses_as_one_binary_file_at_the_path() {
        for path in ["logo.png", "assets/dir with space/logo.png"] {
            let parsed = DiffFile::parse(&placeholder_patch(path));
            let files = parsed.expect("placeholder patch parses");
            assert_eq!(files.len(), 1, "{path}");
            assert_eq!(files[0].path().as_str(), path);
            assert!(files[0].is_binary());
        }
    }
}
