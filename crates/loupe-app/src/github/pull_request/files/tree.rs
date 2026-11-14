//! The folder tree of the files tab, flattened into the rows the list draws,
//! plus the pure rules of a Zed-style project panel: keyboard moves, pinned
//! ("sticky") folders, and the indent guide that holds the selection.
//!
//! Folders come before files at each level, both sorted by name ignoring
//! case, and a chain of folders with a single child folder and no files
//! shows as one compact row (`src/lib/http`).

use std::collections::{BTreeMap, HashSet};

use loupe_github::{ChangedFile, FileStatus};

/// One visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    /// The full path: a folder's key for its open or closed state, or the
    /// file's path.
    pub path: String,
    /// The row of the folder this row sits in.
    pub parent: Option<usize>,
    pub kind: RowKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    Dir(DirRow),
    /// `index` points into the file list the rows were built from.
    File {
        index: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirRow {
    /// What the row shows: the compacted path below the parent folder.
    pub label: String,
    /// Every file under the folder that passed the filter.
    pub files: Vec<usize>,
    pub additions: u32,
    pub deletions: u32,
    pub expanded: bool,
}

impl TreeRow {
    pub fn dir(&self) -> Option<&DirRow> {
        match &self.kind {
            RowKind::Dir(dir) => Some(dir),
            RowKind::File { .. } => None,
        }
    }

    fn is_expanded_dir(&self) -> bool {
        self.dir().is_some_and(|dir| dir.expanded)
    }
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: Vec<(String, usize)>,
}

impl Dir {
    fn insert(&mut self, path: &str, index: usize) {
        match path.split_once('/') {
            Some((head, rest)) => self
                .dirs
                .entry(head.to_owned())
                .or_default()
                .insert(rest, index),
            None => self.files.push((path.to_owned(), index)),
        }
    }

    fn collect_files(&self, out: &mut Vec<usize>) {
        for (_, dir) in sorted_dirs(self) {
            dir.collect_files(out);
        }
        out.extend(sorted_files(self).into_iter().map(|(_, index)| index));
    }
}

fn sort_key(name: &str) -> (String, &str) {
    (name.to_lowercase(), name)
}

fn sorted_dirs(dir: &Dir) -> Vec<(&String, &Dir)> {
    let mut dirs: Vec<_> = dir.dirs.iter().collect();
    dirs.sort_by(|a, b| sort_key(a.0).cmp(&sort_key(b.0)));
    dirs
}

fn sorted_files(dir: &Dir) -> Vec<(&str, usize)> {
    let mut files: Vec<_> = dir
        .files
        .iter()
        .map(|(name, index)| (name.as_str(), *index))
        .collect();
    files.sort_by(|a, b| sort_key(a.0).cmp(&sort_key(b.0)));
    files
}

fn build(files: &[ChangedFile], filter: &str) -> Dir {
    let filter = filter.trim().to_lowercase();
    let mut root = Dir::default();
    for (index, file) in files.iter().enumerate() {
        // A case-insensitive substring of the full path, so `http/` or
        // `client.rs` both narrow the tree.
        if filter.is_empty() || file.path.to_lowercase().contains(&filter) {
            root.insert(&file.path, index);
        }
    }
    root
}

/// The rows to draw for `files`. A folder in `collapsed` hides its children,
/// except while a filter is set: then every folder opens so each match shows.
pub fn tree_rows(files: &[ChangedFile], filter: &str, collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut emitter = Emitter {
        files,
        collapsed,
        force_open: !filter.trim().is_empty(),
        rows: Vec::new(),
    };
    emitter.emit(&build(files, filter), "", 0, None);
    emitter.rows
}

struct Emitter<'a> {
    files: &'a [ChangedFile],
    collapsed: &'a HashSet<String>,
    force_open: bool,
    rows: Vec<TreeRow>,
}

impl Emitter<'_> {
    fn emit(&mut self, dir: &Dir, parent_path: &str, depth: usize, parent: Option<usize>) {
        for (name, mut child) in sorted_dirs(dir) {
            let mut label = name.clone();
            while child.files.is_empty() && child.dirs.len() == 1 {
                let (next_name, next) = child.dirs.iter().next().expect("one child folder");
                label = format!("{label}/{next_name}");
                child = next;
            }
            let path = if parent_path.is_empty() {
                label.clone()
            } else {
                format!("{parent_path}/{label}")
            };
            let mut indices = Vec::new();
            child.collect_files(&mut indices);
            let (additions, deletions) = indices.iter().fold((0, 0), |(a, d), &ix| {
                (a + self.files[ix].additions, d + self.files[ix].deletions)
            });
            let expanded = self.force_open || !self.collapsed.contains(&path);
            let row = self.rows.len();
            self.rows.push(TreeRow {
                depth,
                path: path.clone(),
                parent,
                kind: RowKind::Dir(DirRow {
                    label,
                    files: indices,
                    additions,
                    deletions,
                    expanded,
                }),
            });
            if expanded {
                self.emit(child, &path, depth + 1, Some(row));
            }
        }
        for (_, index) in sorted_files(dir) {
            self.rows.push(TreeRow {
                depth,
                path: self.files[index].path.clone(),
                parent,
                kind: RowKind::File { index },
            });
        }
    }
}

/// Reorders `files` into the tree's order, so the diff below reads in the
/// same order as the list beside it.
pub fn into_tree_order(files: Vec<ChangedFile>) -> Vec<ChangedFile> {
    let mut order = Vec::with_capacity(files.len());
    build(&files, "").collect_files(&mut order);
    let mut slots: Vec<Option<ChangedFile>> = files.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect()
}

/// What a key press does to the tree, decided before any state changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Select(usize),
    /// Open the folder at this row.
    Expand(usize),
    /// Close the folder at this row and select it.
    Collapse(usize),
}

pub fn move_down(rows: &[TreeRow], selected: Option<usize>) -> Option<Move> {
    match selected {
        None if !rows.is_empty() => Some(Move::Select(0)),
        Some(ix) if ix + 1 < rows.len() => Some(Move::Select(ix + 1)),
        _ => None,
    }
}

pub fn move_up(rows: &[TreeRow], selected: Option<usize>) -> Option<Move> {
    match selected {
        None if !rows.is_empty() => Some(Move::Select(rows.len() - 1)),
        Some(ix) if ix > 0 => Some(Move::Select(ix - 1)),
        _ => None,
    }
}

/// → opens a closed folder; on an open one it steps to the first child.
pub fn move_right(rows: &[TreeRow], selected: usize) -> Option<Move> {
    let dir = rows.get(selected)?.dir()?;
    if !dir.expanded {
        return Some(Move::Expand(selected));
    }
    rows.get(selected + 1)
        .filter(|child| child.parent == Some(selected))
        .map(|_| Move::Select(selected + 1))
}

/// ← closes the nearest open folder at or above the selection: the folder
/// itself, or the folder that holds a file or a closed folder.
pub fn move_left(rows: &[TreeRow], selected: usize) -> Option<Move> {
    let mut ix = selected;
    loop {
        let row = rows.get(ix)?;
        if row.is_expanded_dir() {
            return Some(Move::Collapse(ix));
        }
        ix = row.parent?;
    }
}

/// The folders pinned at the top of the list while it scrolls: the folders
/// that hold the row the pinned stack would cover. `drifting` means the
/// last pinned folder ends right below, so it slides up as its last row
/// leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sticky {
    pub rows: Vec<usize>,
    /// The row the stack is computed from.
    pub anchor: usize,
    pub drifting: bool,
}

/// The pinned folders when `top` is the first row in view.
pub fn sticky_folders(rows: &[TreeRow], top: usize) -> Option<Sticky> {
    // A stack of k pinned folders covers rows top..top + k, so the stack is
    // complete at the first row whose folders fit above it.
    for offset in 0..rows.len().saturating_sub(top) {
        let anchor = top + offset;
        let depth = rows[anchor].depth;
        let next_closes_folder = rows
            .get(anchor + 1)
            .is_some_and(|next| next.depth + 1 == depth);
        let (found, drifting) = if depth < offset {
            (true, false)
        } else if next_closes_folder && (depth == offset || depth == offset + 1) {
            (true, depth == offset + 1)
        } else {
            (false, false)
        };
        if found {
            let pinned = ancestors(rows, anchor);
            // Folders already in their own place would only draw over
            // themselves.
            let in_place = pinned
                .iter()
                .enumerate()
                .all(|(position, &row)| row == top + position);
            return (!pinned.is_empty() && !in_place).then_some(Sticky {
                rows: pinned,
                anchor,
                drifting,
            });
        }
    }
    None
}

/// The folders holding `ix`, outermost first.
pub fn ancestors(rows: &[TreeRow], ix: usize) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut current = rows.get(ix).and_then(|row| row.parent);
    while let Some(parent) = current {
        chain.push(parent);
        current = rows[parent].parent;
    }
    chain.reverse();
    chain
}

/// The open folder whose indent guide is highlighted: the selected folder
/// when open, else the folder holding the selection.
pub fn active_folder(rows: &[TreeRow], selected: usize) -> Option<usize> {
    let mut ix = selected;
    loop {
        let row = rows.get(ix)?;
        if row.is_expanded_dir() {
            return Some(ix);
        }
        ix = row.parent?;
    }
}

/// How many of a folder's files are viewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewedMark {
    None,
    Some,
    All,
}

pub fn viewed_mark(viewed: impl IntoIterator<Item = bool>) -> ViewedMark {
    let (mut seen, mut any, mut all) = (false, false, true);
    for viewed in viewed {
        seen = true;
        any |= viewed;
        all &= viewed;
    }
    match (seen && all, any) {
        (true, _) => ViewedMark::All,
        (false, true) => ViewedMark::Some,
        (false, false) => ViewedMark::None,
    }
}

/// The git color of a row's name, as Zed tints its project panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitTint {
    Created,
    Modified,
    Deleted,
    Renamed,
}

pub fn file_tint(status: FileStatus) -> GitTint {
    match status {
        FileStatus::Added | FileStatus::Copied => GitTint::Created,
        FileStatus::Removed => GitTint::Deleted,
        FileStatus::Renamed => GitTint::Renamed,
        FileStatus::Modified | FileStatus::Changed | FileStatus::Unchanged => GitTint::Modified,
    }
}

/// A folder is green when every file under it is new, red when every one is
/// gone, and yellow otherwise.
pub fn folder_tint(statuses: impl IntoIterator<Item = FileStatus>) -> GitTint {
    let mut tints = statuses.into_iter().map(file_tint);
    let Some(first) = tints.next() else {
        return GitTint::Modified;
    };
    match first {
        GitTint::Created | GitTint::Deleted if tints.all(|tint| tint == first) => first,
        _ => GitTint::Modified,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use loupe_github::{ChangedFile, FileStatus, ViewedState};

    use super::{
        GitTint, Move, RowKind, Sticky, TreeRow, ViewedMark, active_folder, folder_tint,
        into_tree_order, move_down, move_left, move_right, move_up, sticky_folders, tree_rows,
        viewed_mark,
    };

    fn file(path: &str, additions: u32, deletions: u32) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            previous_path: None,
            status: FileStatus::Modified,
            additions,
            deletions,
            patch: None,
            viewed: ViewedState::Unviewed,
        }
    }

    /// Renders rows as indented text: `label/ [files] +a -d` for folders
    /// (`>` when closed) and the file path for files.
    fn outline(files: &[ChangedFile], rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth);
                match &row.kind {
                    RowKind::Dir(dir) => format!(
                        "{indent}{}{}/ [{}] +{} -{}",
                        if dir.expanded { "" } else { "> " },
                        dir.label,
                        dir.files.len(),
                        dir.additions,
                        dir.deletions
                    ),
                    RowKind::File { index } => format!("{indent}{}", files[*index].path),
                }
            })
            .collect()
    }

    fn sample() -> Vec<ChangedFile> {
        vec![
            file("README.md", 1, 0),
            file("src/lib/http/client.rs", 10, 2),
            file("src/lib/http/retry.rs", 3, 1),
            file("src/main.rs", 4, 4),
            file("Cargo.toml", 1, 1),
            file("docs/guide/intro.md", 2, 0),
            file("src/App.rs", 1, 0),
        ]
    }

    fn find(rows: &[TreeRow], path: &str) -> usize {
        rows.iter()
            .position(|row| row.path == path)
            .expect("row exists")
    }

    #[test]
    fn folders_nest_before_files_and_single_child_chains_compact() {
        let files = sample();
        let rows = tree_rows(&files, "", &HashSet::new());
        assert_eq!(
            outline(&files, &rows),
            [
                "docs/guide/ [1] +2 -0",
                "  docs/guide/intro.md",
                "src/ [4] +18 -7",
                "  lib/http/ [2] +13 -3",
                "    src/lib/http/client.rs",
                "    src/lib/http/retry.rs",
                "  src/App.rs",
                "  src/main.rs",
                "Cargo.toml",
                "README.md",
            ]
        );
        assert_eq!(rows[3].path, "src/lib/http");
        assert_eq!(rows[4].parent, Some(3));
        assert_eq!(rows[3].parent, Some(2));
        assert_eq!(rows[8].parent, None);
    }

    #[test]
    fn a_collapsed_folder_hides_its_children_but_keeps_its_totals() {
        let files = sample();
        let collapsed = HashSet::from(["src".to_owned()]);
        let rows = tree_rows(&files, "", &collapsed);
        assert_eq!(
            outline(&files, &rows),
            [
                "docs/guide/ [1] +2 -0",
                "  docs/guide/intro.md",
                "> src/ [4] +18 -7",
                "Cargo.toml",
                "README.md",
            ]
        );
    }

    #[test]
    fn the_filter_keeps_matching_paths_and_opens_their_folders() {
        let files = sample();
        let collapsed = HashSet::from(["src".to_owned(), "src/lib/http".to_owned()]);
        let rows = tree_rows(&files, "  HTTP/R ", &collapsed);
        assert_eq!(
            outline(&files, &rows),
            ["src/lib/http/ [1] +3 -1", "  src/lib/http/retry.rs"]
        );
        assert!(tree_rows(&files, "nothing-matches", &collapsed).is_empty());
    }

    #[test]
    fn tree_order_matches_the_rows_without_a_filter() {
        let ordered: Vec<String> = into_tree_order(sample())
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(
            ordered,
            [
                "docs/guide/intro.md",
                "src/lib/http/client.rs",
                "src/lib/http/retry.rs",
                "src/App.rs",
                "src/main.rs",
                "Cargo.toml",
                "README.md",
            ]
        );
    }

    #[test]
    fn arrows_move_open_and_close_like_zed() {
        let files = sample();
        let rows = tree_rows(&files, "", &HashSet::new());
        let src = find(&rows, "src");
        let http = find(&rows, "src/lib/http");
        let client = find(&rows, "src/lib/http/client.rs");
        assert_eq!(move_down(&rows, None), Some(Move::Select(0)));
        assert_eq!(move_up(&rows, None), Some(Move::Select(rows.len() - 1)));
        assert_eq!(move_down(&rows, Some(rows.len() - 1)), None);
        assert_eq!(move_up(&rows, Some(0)), None);
        // → on an open folder steps into it; on a file it does nothing.
        assert_eq!(move_right(&rows, src), Some(Move::Select(src + 1)));
        assert_eq!(move_right(&rows, client), None);
        // ← on a file closes the folder holding it; on an open folder,
        // closes that folder.
        assert_eq!(move_left(&rows, client), Some(Move::Collapse(http)));
        assert_eq!(move_left(&rows, src), Some(Move::Collapse(src)));
        assert_eq!(move_left(&rows, find(&rows, "README.md")), None);

        let collapsed = HashSet::from(["src/lib/http".to_owned()]);
        let rows = tree_rows(&files, "", &collapsed);
        let http = find(&rows, "src/lib/http");
        // → opens a closed folder; ← on a closed folder closes its parent.
        assert_eq!(move_right(&rows, http), Some(Move::Expand(http)));
        assert_eq!(
            move_left(&rows, http),
            Some(Move::Collapse(find(&rows, "src")))
        );
    }

    #[test]
    fn folders_pin_while_their_rows_scroll_past() {
        let files = sample();
        let rows = tree_rows(&files, "", &HashSet::new());
        let src = find(&rows, "src");
        let http = find(&rows, "src/lib/http");
        // The top of the list pins nothing: `docs/guide` is still in view.
        assert_eq!(sticky_folders(&rows, 0), None);
        // With `docs/guide` off the top, it pins and drifts: its only file
        // is the last row before `src`.
        assert_eq!(
            sticky_folders(&rows, 1),
            Some(Sticky {
                rows: vec![0],
                anchor: 1,
                drifting: true,
            })
        );
        // With `src` scrolled off, it and `lib/http` pin above `retry.rs`.
        assert_eq!(
            sticky_folders(&rows, src + 1),
            Some(Sticky {
                rows: vec![src, http],
                anchor: find(&rows, "src/lib/http/retry.rs"),
                drifting: false,
            })
        );
        // With `client.rs` at the top, `lib/http` is about to end: it
        // drifts up under `src` as `retry.rs` leaves.
        assert_eq!(
            sticky_folders(&rows, http + 1),
            Some(Sticky {
                rows: vec![src, http],
                anchor: find(&rows, "src/lib/http/retry.rs"),
                drifting: true,
            })
        );
        // Past `src`, the root-level files pin nothing.
        assert_eq!(sticky_folders(&rows, find(&rows, "Cargo.toml")), None);
    }

    #[test]
    fn the_active_guide_belongs_to_the_open_folder_holding_the_selection() {
        let files = sample();
        let rows = tree_rows(&files, "", &HashSet::new());
        let http = find(&rows, "src/lib/http");
        assert_eq!(
            active_folder(&rows, find(&rows, "src/lib/http/retry.rs")),
            Some(http)
        );
        assert_eq!(active_folder(&rows, http), Some(http));
        assert_eq!(active_folder(&rows, find(&rows, "README.md")), None);
    }

    #[test]
    fn folder_marks_and_tints_summarize_their_files() {
        assert_eq!(viewed_mark([true, true]), ViewedMark::All);
        assert_eq!(viewed_mark([true, false]), ViewedMark::Some);
        assert_eq!(viewed_mark([false, false]), ViewedMark::None);
        assert_eq!(viewed_mark([]), ViewedMark::None);
        assert_eq!(
            folder_tint([FileStatus::Added, FileStatus::Added]),
            GitTint::Created
        );
        assert_eq!(folder_tint([FileStatus::Removed]), GitTint::Deleted);
        assert_eq!(
            folder_tint([FileStatus::Added, FileStatus::Removed]),
            GitTint::Modified
        );
        assert_eq!(folder_tint([FileStatus::Renamed]), GitTint::Modified);
    }
}
