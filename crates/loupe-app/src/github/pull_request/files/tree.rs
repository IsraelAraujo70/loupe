//! The folder tree of the files tab, flattened into the rows the list draws,
//! plus the pure rules of a Zed-style project panel: keyboard moves, pinned
//! ("sticky") folders, and the indent guide that holds the selection.
//!
//! The tree reads in review order: all the code, then all the tests, then
//! all the docs, then the low-signal files (`signal::Signal::Low`). The
//! first kind the pull request has fills the tree at the top; each kind
//! after it leaves its folders for a group of its own ([`TESTS`], [`DOCS`],
//! [`LOW_SIGNAL`]) with a tree of its own inside, so a test never reads
//! between two files of code. Within a tree folders come before files, both
//! sorted by name ignoring case. A chain of folders with a single child
//! folder and no files shows as one compact row (`src/lib/http`).

use std::collections::{BTreeMap, HashMap, HashSet};

use loupe_github::{ChangedFile, FileStatus};

use super::signal::Signal;

/// The low-signal group: its row's label and the key the tree opens and
/// closes it by. The folders inside a group are keyed under it
/// (`Low signal/data`), so closing one never closes the folder of the same
/// name among the code.
pub const LOW_SIGNAL: &str = "Low signal";
/// The group of the tests, when the pull request has code.
pub const TESTS: &str = "Tests";
/// The group of the docs, when the pull request has code or tests.
pub const DOCS: &str = "Docs";
/// The groups in the order they follow the tree at the top.
pub const GROUPS: [&str; 3] = [TESTS, DOCS, LOW_SIGNAL];

/// The group each file sits in, from the kinds `signals` holds in file
/// order: `None` for the tree at the top, which holds the first kind the
/// pull request has.
pub fn file_groups(signals: &[Signal]) -> Vec<Option<&'static str>> {
    let lead = signals
        .iter()
        .filter(|signal| !signal.is_low())
        .map(|signal| signal.rank())
        .min();
    signals
        .iter()
        .map(|signal| match signal {
            Signal::Low(_) => Some(LOW_SIGNAL),
            _ if Some(signal.rank()) == lead => None,
            Signal::Code => None,
            Signal::Test => Some(TESTS),
            Signal::Docs => Some(DOCS),
        })
        .collect()
}

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

struct Dir {
    dirs: BTreeMap<String, Dir>,
    /// Each file's name, index, and rank (`Signal::rank`).
    files: Vec<(String, usize, u8)>,
    /// The first rank of a file under the folder, which the folder sorts by.
    rank: u8,
}

impl Default for Dir {
    fn default() -> Self {
        Dir {
            dirs: BTreeMap::new(),
            files: Vec::new(),
            rank: u8::MAX,
        }
    }
}

impl Dir {
    fn insert(&mut self, path: &str, index: usize, rank: u8) {
        self.rank = self.rank.min(rank);
        match path.split_once('/') {
            Some((head, rest)) => self
                .dirs
                .entry(head.to_owned())
                .or_default()
                .insert(rest, index, rank),
            None => self.files.push((path.to_owned(), index, rank)),
        }
    }

    fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.files.is_empty()
    }

    fn collect_files(&self, out: &mut Vec<usize>) {
        for entry in entries(self) {
            match entry {
                Entry::Dir(_, dir) => dir.collect_files(out),
                Entry::File(index) => out.push(index),
            }
        }
    }
}

enum Entry<'a> {
    Dir(&'a String, &'a Dir),
    File(usize),
}

fn sort_key(name: &str) -> (String, &str) {
    (name.to_lowercase(), name)
}

/// A folder's folders and files in reading order: by rank, then folders
/// before files, then by name ignoring case.
fn entries(dir: &Dir) -> Vec<Entry<'_>> {
    let mut entries: Vec<(u8, bool, (String, &str), Entry)> = dir
        .dirs
        .iter()
        .map(|(name, child)| (child.rank, false, sort_key(name), Entry::Dir(name, child)))
        .chain(
            dir.files
                .iter()
                .map(|(name, index, rank)| (*rank, true, sort_key(name), Entry::File(*index))),
        )
        .collect();
    entries.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    entries.into_iter().map(|(.., entry)| entry).collect()
}

/// Whether the files filter keeps `path`: a case-insensitive substring of
/// the full path, so `http/` or `client.rs` both narrow the files.
pub fn filter_keeps(path: &str, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty() || path.to_lowercase().contains(&filter)
}

/// The tree at the top, and each group with files and its tree, in reading
/// order. `signals` holds each file's kind, in file order.
fn build(
    files: &[ChangedFile],
    signals: &[Signal],
    filter: &str,
) -> (Dir, Vec<(&'static str, Dir)>) {
    let mut main = Dir::default();
    let mut groups: Vec<(&'static str, Dir)> =
        GROUPS.iter().map(|name| (*name, Dir::default())).collect();
    let file_groups = file_groups(signals);
    for (index, file) in files.iter().enumerate() {
        if !filter_keeps(&file.path, filter) {
            continue;
        }
        let signal = signals.get(index).copied().unwrap_or(Signal::Code);
        let tree = match file_groups.get(index).copied().flatten() {
            Some(name) => match groups.iter_mut().find(|(group, _)| *group == name) {
                Some((_, tree)) => tree,
                None => &mut main,
            },
            None => &mut main,
        };
        tree.insert(&file.path, index, signal.rank());
    }
    groups.retain(|(_, tree)| !tree.is_empty());
    (main, groups)
}

/// The rows to draw for `files`, whose kinds `signals` holds in file order:
/// the tree at the top, then each group. A folder in
/// `collapsed` hides its children, except while a filter is set: then every
/// folder opens so each match shows.
pub fn tree_rows(
    files: &[ChangedFile],
    signals: &[Signal],
    filter: &str,
    collapsed: &HashSet<String>,
) -> Vec<TreeRow> {
    let mut emitter = Emitter {
        files,
        collapsed,
        force_open: !filter.trim().is_empty(),
        rows: Vec::new(),
    };
    let (main, groups) = build(files, signals, filter);
    emitter.emit(&main, "", 0, None);
    for (name, tree) in &groups {
        let row = emitter.push_dir(tree, (*name).to_owned(), (*name).to_owned(), 0, None);
        if let Some(row) = row {
            emitter.emit(tree, name, 1, Some(row));
        }
    }
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
        for entry in entries(dir) {
            match entry {
                Entry::Dir(name, mut child) => {
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
                    if let Some(row) = self.push_dir(child, path.clone(), label, depth, parent) {
                        self.emit(child, &path, depth + 1, Some(row));
                    }
                }
                Entry::File(index) => self.rows.push(TreeRow {
                    depth,
                    path: self.files[index].path.clone(),
                    parent,
                    kind: RowKind::File { index },
                }),
            }
        }
    }

    /// Adds the row of folder `dir`, and returns it when it is open.
    fn push_dir(
        &mut self,
        dir: &Dir,
        path: String,
        label: String,
        depth: usize,
        parent: Option<usize>,
    ) -> Option<usize> {
        let mut indices = Vec::new();
        dir.collect_files(&mut indices);
        let (additions, deletions) = indices.iter().fold((0, 0), |(a, d), &ix| {
            (a + self.files[ix].additions, d + self.files[ix].deletions)
        });
        let expanded = self.force_open || !self.collapsed.contains(&path);
        let row = self.rows.len();
        self.rows.push(TreeRow {
            depth,
            path,
            parent,
            kind: RowKind::Dir(DirRow {
                label,
                files: indices,
                additions,
                deletions,
                expanded,
            }),
        });
        expanded.then_some(row)
    }
}

/// Above this many files a tree opens as an overview of its folders.
pub const LARGE_TREE: usize = 200;
/// How many rows the overview opens to: about one screen of the tree.
const OVERVIEW_ROWS: usize = 40;

/// The folders a tree of `files` opens with closed. Up to [`LARGE_TREE`]
/// files it opens whole. A larger tree opens from the top a level at a time
/// while the level fits in [`OVERVIEW_ROWS`]; a level that does not still
/// opens the folder holding most of the files (`crates/` in a Rust
/// monorepo), so the first screen lists the folders that split the work,
/// each with its count, instead of the first few hundred files.
///
/// The low-signal group opens closed whenever there is code beside it, so
/// the tree starts on what needs reading.
pub fn initial_collapsed(files: &[ChangedFile], signals: &[Signal]) -> HashSet<String> {
    let mut collapsed = overview_collapsed(files, signals);
    let low = signals.iter().filter(|signal| signal.is_low()).count();
    if low > 0 && low < files.len() {
        collapsed.insert(LOW_SIGNAL.to_owned());
    }
    collapsed
}

fn overview_collapsed(files: &[ChangedFile], signals: &[Signal]) -> HashSet<String> {
    if files.len() <= LARGE_TREE {
        return HashSet::new();
    }
    let rows = tree_rows(files, signals, "", &HashSet::new());
    let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (ix, row) in rows.iter().enumerate() {
        children.entry(row.parent).or_default().push(ix);
    }
    let count = |folder: usize| children.get(&Some(folder)).map_or(0, Vec::len);
    let folders_in = |rows_in: &[usize]| -> Vec<usize> {
        rows_in
            .iter()
            .copied()
            .filter(|&ix| rows[ix].dir().is_some())
            .collect()
    };
    let top = children.get(&None).cloned().unwrap_or_default();
    let mut shown = top.len();
    let mut frontier = folders_in(&top);
    let mut open = HashSet::new();
    while !frontier.is_empty() {
        let level: usize = frontier.iter().map(|&folder| count(folder)).sum();
        let opening = if shown + level <= OVERVIEW_ROWS {
            frontier.clone()
        } else {
            let lead = frontier.iter().copied().find(|&folder| {
                rows[folder]
                    .dir()
                    .is_some_and(|dir| dir.files.len() * 5 >= files.len() * 4)
            });
            match lead {
                Some(lead) => vec![lead],
                None => break,
            }
        };
        frontier = Vec::new();
        for folder in opening {
            shown += count(folder);
            open.insert(folder);
            frontier.extend(folders_in(
                children.get(&Some(folder)).map_or(&[], Vec::as_slice),
            ));
        }
    }
    rows.iter()
        .enumerate()
        .filter(|(ix, row)| row.dir().is_some() && !open.contains(ix))
        .map(|(_, row)| row.path.clone())
        .collect()
}

/// Reorders `files` and their kinds into the tree's order (all the code,
/// then the tests, then the docs, then the low-signal files), so the diff
/// below reads in the same order as the list beside it.
pub fn into_tree_order(
    files: Vec<ChangedFile>,
    signals: Vec<Signal>,
) -> (Vec<ChangedFile>, Vec<Signal>) {
    let mut order = Vec::with_capacity(files.len());
    let (main, groups) = build(&files, &signals, "");
    main.collect_files(&mut order);
    for (_, tree) in &groups {
        tree.collect_files(&mut order);
    }
    let mut slots: Vec<Option<(ChangedFile, Signal)>> =
        files.into_iter().zip(signals).map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| slots.get_mut(index).and_then(Option::take))
        .unzip()
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

/// The row that stands for `path`: its own, or while a closed folder hides
/// it, the innermost folder on screen that holds it, so the tree still
/// shows where the diff is. `group` is the group the file sits in, `None`
/// for the tree at the top.
pub fn shown_row(rows: &[TreeRow], path: &str, group: Option<&str>) -> Option<usize> {
    if let Some(ix) = rows.iter().position(|row| row.path == path) {
        return Some(ix);
    }
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.dir().is_some() && holds(&row.path, path, group))
        .max_by_key(|(_, row)| row.depth)
        .map(|(ix, _)| ix)
}

/// Whether the folder keyed `folder` holds the file at `path`, which sits
/// in `group`. A group holds its files, and a folder inside it
/// (`Low signal/data`) the files of the group in the folder it names.
pub fn holds(folder: &str, path: &str, group: Option<&str>) -> bool {
    if group == Some(folder) {
        return true;
    }
    let inside = GROUPS.iter().find_map(|name| {
        let inner = folder.strip_prefix(name)?.strip_prefix('/')?;
        Some((*name, inner))
    });
    let real = match inside {
        Some((name, inner)) if group == Some(name) => inner,
        None if group.is_none() => folder,
        _ => return false,
    };
    path.strip_prefix(real)
        .is_some_and(|rest| rest.starts_with('/'))
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

/// Where each file's name stops sharing its start with a sibling file's,
/// at the start of a word (after `.`, `-`, `_`, or a space) and never past
/// the last word before the extension, so a name cut short keeps the part
/// that tells it apart: of `collaborator.service.ts` and
/// `collaborator.repository.ts`, the part after `collaborator.`. The part
/// kept is the end of no other sibling's name from one of its words on, so
/// `atelier_cave_dark.rs` keeps `cave_dark.rs` beside `atelier_cave_light.rs`
/// and `atelier_dune_dark.rs`. 0 for a folder and for a name whose start is
/// its own.
pub fn distinct_starts(rows: &[TreeRow]) -> Vec<usize> {
    let mut starts = vec![0; rows.len()];
    let mut siblings: HashMap<Option<usize>, Vec<(&str, usize)>> = HashMap::new();
    for (ix, row) in rows.iter().enumerate() {
        if let RowKind::File { .. } = row.kind {
            let name = row.path.rsplit('/').next().unwrap_or(&row.path);
            siblings.entry(row.parent).or_default().push((name, ix));
        }
    }
    for mut names in siblings.into_values() {
        // How many names end in each of their own word-led ends.
        let mut ends: HashMap<&str, usize> = HashMap::new();
        for &(name, _) in &names {
            for start in word_starts(name) {
                *ends.entry(&name[start..]).or_default() += 1;
            }
        }
        // In sorted order a name shares the most with a neighbor.
        names.sort_unstable();
        for (position, &(name, ix)) in names.iter().enumerate() {
            let shared = [position.checked_sub(1), Some(position + 1)]
                .into_iter()
                .flatten()
                .filter_map(|other| names.get(other))
                .map(|&(other, _)| common_prefix(name, other))
                .max()
                .unwrap_or(0);
            let stem = name.rfind('.').filter(|&dot| dot > 0).unwrap_or(name.len());
            let latest = shared.min(stem);
            starts[ix] = word_starts(name)
                .filter(|&start| start <= latest && ends.get(&name[start..]) == Some(&1))
                .last()
                .unwrap_or(0);
        }
    }
    starts
}

/// Where the words of a file name start, first to last.
fn word_starts(name: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(name.match_indices(SEPARATORS).map(|(ix, _)| ix + 1))
}

/// What separates the words of a file name.
const SEPARATORS: [char; 4] = ['.', '-', '_', ' '];

/// The length in bytes of the start `a` and `b` share.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((ix, _), _)| ix)
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

    use super::super::signal::{Attributes, Signal, classify_all};
    use super::{
        DOCS, GitTint, LARGE_TREE, LOW_SIGNAL, Move, RowKind, Sticky, TESTS, TreeRow, ViewedMark,
        active_folder, distinct_starts, folder_tint, holds, initial_collapsed, into_tree_order,
        move_down, move_left, move_right, move_up, shown_row, sticky_folders, tree_rows,
        viewed_mark,
    };

    /// Every file as code, so a tree sorts by name alone.
    fn code(files: &[ChangedFile]) -> Vec<Signal> {
        vec![Signal::Code; files.len()]
    }

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
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
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
    fn a_folder_whose_only_child_is_a_folder_shares_its_row() {
        // `ai2/src` holds `prompts/` beside files, so it is not a chain;
        // `call/src` is, all the way down to the folder with the files.
        let files = vec![
            file("crates/ai2/Cargo.toml", 1, 0),
            file("crates/ai2/src/ai2.rs", 1, 0),
            file("crates/ai2/src/prompts/base.rs", 1, 0),
            file("crates/call/src/room/mod.rs", 1, 0),
        ];
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
        assert_eq!(
            outline(&files, &rows),
            [
                "crates/ [4] +4 -0",
                "  ai2/ [3] +3 -0",
                "    src/ [2] +2 -0",
                "      prompts/ [1] +1 -0",
                "        crates/ai2/src/prompts/base.rs",
                "      crates/ai2/src/ai2.rs",
                "    crates/ai2/Cargo.toml",
                "  call/src/room/ [1] +1 -0",
                "    crates/call/src/room/mod.rs",
            ]
        );
    }

    /// A pull request shaped like zed#3174: nearly every file under
    /// `crates/`, split over many crates, beside a few root files.
    fn monorepo(crates: usize, files_per_crate: usize) -> Vec<ChangedFile> {
        let mut files = vec![
            file("Cargo.lock", 1, 1),
            file("Cargo.toml", 1, 0),
            file("script/bump.py", 1, 0),
        ];
        for krate in 0..crates {
            for ix in 0..files_per_crate {
                files.push(file(&format!("crates/c{krate:02}/src/f{ix}.rs"), 2, 0));
            }
        }
        files
    }

    #[test]
    fn a_small_tree_opens_whole() {
        let files = monorepo(10, 10);
        assert!(files.len() <= LARGE_TREE);
        assert!(initial_collapsed(&files, &code(&files)).is_empty());
    }

    #[test]
    fn a_large_tree_opens_as_an_overview_of_its_folders() {
        let files = monorepo(36, 10);
        assert!(files.len() > LARGE_TREE);
        let collapsed = initial_collapsed(&files, &code(&files));
        let rows = tree_rows(&files, &code(&files), "", &collapsed);
        let shown = outline(&files, &rows);
        // The top level and its crates do not fit a screen together, but
        // `crates/` holds most of the files, so it opens on its crates,
        // each closed with its count; the rest of the top stays closed.
        assert_eq!(shown[0], "crates/ [360] +720 -0");
        assert_eq!(shown[1], "  > c00/src/ [10] +20 -0");
        assert_eq!(shown[36], "  > c35/src/ [10] +20 -0");
        assert_eq!(
            &shown[37..],
            ["> script/ [1] +1 -0", "Cargo.lock", "Cargo.toml"]
        );
        assert!(rows.len() <= 40, "the overview fits a screen");
        assert!(collapsed.contains("script"));
        assert!(!collapsed.contains("crates"));
    }

    #[test]
    fn an_overview_opens_every_level_that_fits_a_screen() {
        // Three folders of 70 files: the top level fits, its files do not.
        let files: Vec<ChangedFile> = ["api", "web", "docs"]
            .iter()
            .flat_map(|dir| (0..70).map(move |ix| file(&format!("{dir}/f{ix}.rs"), 1, 0)))
            .collect();
        let rows = tree_rows(
            &files,
            &code(&files),
            "",
            &initial_collapsed(&files, &code(&files)),
        );
        assert_eq!(
            outline(&files, &rows),
            [
                "> api/ [70] +70 -0",
                "> docs/ [70] +70 -0",
                "> web/ [70] +70 -0"
            ]
        );
        // Ten folders of two subfolders each open down to the subfolders.
        let files: Vec<ChangedFile> = (0..10)
            .flat_map(|top| {
                (0..2).flat_map(move |sub| {
                    (0..11).map(move |ix| file(&format!("t{top}/s{sub}/f{ix}.rs"), 1, 0))
                })
            })
            .collect();
        let rows = tree_rows(
            &files,
            &code(&files),
            "",
            &initial_collapsed(&files, &code(&files)),
        );
        assert_eq!(rows.len(), 30);
        assert_eq!(outline(&files, &rows)[1], "  > s0/ [11] +11 -0");
    }

    #[test]
    fn a_collapsed_folder_hides_its_children_but_keeps_its_totals() {
        let files = sample();
        let collapsed = HashSet::from(["src".to_owned()]);
        let rows = tree_rows(&files, &code(&files), "", &collapsed);
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
        let rows = tree_rows(&files, &code(&files), "  HTTP/R ", &collapsed);
        assert_eq!(
            outline(&files, &rows),
            ["src/lib/http/ [1] +3 -1", "  src/lib/http/retry.rs"]
        );
        assert!(tree_rows(&files, &code(&files), "nothing-matches", &collapsed).is_empty());
    }

    #[test]
    fn tree_order_matches_the_rows_without_a_filter() {
        let files = sample();
        let signals = code(&files);
        let ordered: Vec<String> = into_tree_order(files, signals)
            .0
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
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
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
        let rows = tree_rows(&files, &code(&files), "", &collapsed);
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
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
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
    fn a_hidden_file_shows_as_the_innermost_folder_on_screen() {
        let files = sample();
        let client = "src/lib/http/client.rs";
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
        assert_eq!(shown_row(&rows, client, None), Some(find(&rows, client)));
        let rows = tree_rows(
            &files,
            &code(&files),
            "",
            &HashSet::from(["src/lib/http".to_owned()]),
        );
        assert_eq!(
            shown_row(&rows, client, None),
            Some(find(&rows, "src/lib/http"))
        );
        let collapsed = HashSet::from(["src".to_owned(), "src/lib/http".to_owned()]);
        let rows = tree_rows(&files, &code(&files), "", &collapsed);
        assert_eq!(shown_row(&rows, client, None), Some(find(&rows, "src")));
        // `src` is not a folder of `srcs/a.rs`.
        assert_eq!(shown_row(&rows, "srcs/a.rs", None), None);
    }

    #[test]
    fn the_active_guide_belongs_to_the_open_folder_holding_the_selection() {
        let files = sample();
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
        let http = find(&rows, "src/lib/http");
        assert_eq!(
            active_folder(&rows, find(&rows, "src/lib/http/retry.rs")),
            Some(http)
        );
        assert_eq!(active_folder(&rows, http), Some(http));
        assert_eq!(active_folder(&rows, find(&rows, "README.md")), None);
    }

    #[test]
    fn a_name_keeps_from_where_it_parts_from_its_siblings() {
        let files = vec![
            file("api/collaborator/collaborator.controller.ts", 1, 0),
            file("api/collaborator/collaborator.repository.ts", 1, 0),
            file("api/collaborator/collaborator.repository.spec.ts", 1, 0),
            file("api/collaborator/foo-audit.interceptor.spec.ts", 1, 0),
            file("api/collaborator/bar-audit.interceptor.spec.ts", 1, 0),
            file("api/collaborator/index.ts", 1, 0),
            file("api/account/account.ts", 1, 0),
            file("api/account/accounts.module.ts", 1, 0),
            file("web/collaborator.controller.ts", 1, 0),
            file("themes/atelier_cave_dark.rs", 1, 0),
            file("themes/atelier_cave_light.rs", 1, 0),
            file("themes/atelier_dune_dark.rs", 1, 0),
        ];
        let rows = tree_rows(&files, &code(&files), "", &HashSet::new());
        let starts = distinct_starts(&rows);
        let start = |path: &str| {
            let ix = find(&rows, path);
            let name = path.rsplit('/').next().unwrap();
            name[starts[ix]..].to_owned()
        };
        assert_eq!(
            start("api/collaborator/collaborator.controller.ts"),
            "controller.ts"
        );
        assert_eq!(
            start("api/collaborator/collaborator.repository.ts"),
            "repository.ts"
        );
        assert_eq!(
            start("api/collaborator/collaborator.repository.spec.ts"),
            "repository.spec.ts"
        );
        // Siblings that part at their first letter keep their whole start.
        assert_eq!(
            start("api/collaborator/foo-audit.interceptor.spec.ts"),
            "foo-audit.interceptor.spec.ts"
        );
        assert_eq!(start("api/collaborator/index.ts"), "index.ts");
        // A shared start that ends inside a word keeps the word.
        assert_eq!(
            start("api/account/accounts.module.ts"),
            "accounts.module.ts"
        );
        // A file alone in its folder, and folders, keep everything.
        assert_eq!(
            start("web/collaborator.controller.ts"),
            "collaborator.controller.ts"
        );
        // What is kept ends no other sibling's name.
        assert_eq!(start("themes/atelier_cave_dark.rs"), "cave_dark.rs");
        assert_eq!(start("themes/atelier_cave_light.rs"), "light.rs");
        assert_eq!(start("themes/atelier_dune_dark.rs"), "dune_dark.rs");
        assert_eq!(starts[find(&rows, "api")], 0);
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

    /// The kinds of a pull request like the review fixture's, read as the
    /// files tab reads them.
    fn fixture() -> (Vec<ChangedFile>, Vec<Signal>) {
        let files = vec![
            file("assets/badge.png", 0, 0),
            file("assets/icon.png", 0, 0),
            file("docs/guide/quickstart.md", 5, 4),
            file("README.md", 2, 1),
            file("scripts/seed.sh", 0, 0),
            file("src/generated/units_table.rs", 7, 7),
            file("src/units.rs", 5, 5),
            file("web/src/components/IngredientList.test.tsx", 10, 0),
            file("web/src/components/IngredientList.tsx", 20, 0),
            file("web/pnpm-lock.yaml", 40, 12),
            file("Cargo.toml", 1, 1),
        ];
        let attributes = Attributes::parse("src/generated/** linguist-generated\n");
        let signals = classify_all(&files, &attributes);
        into_tree_order(files, signals)
    }

    #[test]
    fn all_the_code_reads_first_then_the_tests_the_docs_and_low_signal() {
        let (files, signals) = fixture();
        let rows = tree_rows(&files, &signals, "", &HashSet::new());
        assert_eq!(
            outline(&files, &rows),
            [
                "src/ [1] +5 -5",
                "  src/units.rs",
                "web/src/components/ [1] +20 -0",
                "  web/src/components/IngredientList.tsx",
                "Cargo.toml",
                "Tests/ [1] +10 -0",
                "  web/src/components/ [1] +10 -0",
                "    web/src/components/IngredientList.test.tsx",
                "Docs/ [2] +7 -5",
                "  docs/guide/ [1] +5 -4",
                "    docs/guide/quickstart.md",
                "  README.md",
                "Low signal/ [5] +47 -19",
                "  assets/ [2] +0 -0",
                "    assets/badge.png",
                "    assets/icon.png",
                "  scripts/ [1] +0 -0",
                "    scripts/seed.sh",
                "  src/generated/ [1] +7 -7",
                "    src/generated/units_table.rs",
                "  web/ [1] +40 -12",
                "    web/pnpm-lock.yaml",
            ]
        );
        // The diff reads in the same order.
        let tree_files: Vec<&str> = rows
            .iter()
            .filter_map(|row| match row.kind {
                RowKind::File { index } => Some(files[index].path.as_str()),
                RowKind::Dir(_) => None,
            })
            .collect();
        let diff_files: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(tree_files, diff_files);
        assert!(signals[..6].iter().all(|signal| !signal.is_low()));
        assert!(signals[6..].iter().all(|signal| signal.is_low()));
        // Without code the tests fill the tree at the top, and the docs
        // still follow them in a group.
        let files = vec![file("docs/testing.md", 3, 0), file("tests/scale.rs", 4, 0)];
        let signals = classify_all(&files, &Attributes::default());
        let (files, signals) = into_tree_order(files, signals);
        let rows = tree_rows(&files, &signals, "", &HashSet::new());
        assert_eq!(
            outline(&files, &rows),
            [
                "tests/ [1] +4 -0",
                "  tests/scale.rs",
                "Docs/ [1] +3 -0",
                "  docs/ [1] +3 -0",
                "    docs/testing.md",
            ]
        );
    }

    #[test]
    fn the_low_signal_group_opens_closed_beside_code_and_keeps_its_own_folders() {
        let (files, signals) = fixture();
        let collapsed = initial_collapsed(&files, &signals);
        assert_eq!(collapsed, HashSet::from([LOW_SIGNAL.to_owned()]));
        let rows = tree_rows(&files, &signals, "", &collapsed);
        assert_eq!(
            outline(&files, &rows).last().map(String::as_str),
            Some("> Low signal/ [5] +47 -19")
        );
        let group = find(&rows, LOW_SIGNAL);
        // A low-signal file hidden in the group shows as the group, and a
        // code file never does.
        let low = Some(LOW_SIGNAL);
        assert_eq!(shown_row(&rows, "assets/badge.png", low), Some(group));
        assert_eq!(shown_row(&rows, "web/pnpm-lock.yaml", low), Some(group));
        // Closing `Low signal/web` closes the lockfile's folder only.
        let closed = HashSet::from(["Low signal/web".to_owned()]);
        let rows = tree_rows(&files, &signals, "", &closed);
        assert!(
            rows.iter()
                .any(|row| row.path == "web/src/components/IngredientList.tsx")
        );
        assert!(!rows.iter().any(|row| row.path == "web/pnpm-lock.yaml"));
        assert_eq!(
            shown_row(&rows, "web/pnpm-lock.yaml", low),
            Some(find(&rows, "Low signal/web"))
        );
        // A pull request of low-signal files alone opens the group.
        let lock = vec![file("Cargo.lock", 3, 3)];
        let lock_signals = classify_all(&lock, &Attributes::default());
        assert!(initial_collapsed(&lock, &lock_signals).is_empty());
    }

    #[test]
    fn a_folder_holds_the_files_of_its_own_section() {
        let low = Some(LOW_SIGNAL);
        assert!(holds(LOW_SIGNAL, "assets/badge.png", low));
        assert!(!holds(LOW_SIGNAL, "src/units.rs", None));
        assert!(holds("Low signal/assets", "assets/badge.png", low));
        assert!(!holds("Low signal/assets", "assets/badge.png", None));
        assert!(holds("web", "web/src/a.ts", None));
        assert!(!holds("web", "web/pnpm-lock.yaml", low));
        assert!(!holds("Low signal/web", "website/a.csv", low));
        assert!(holds(TESTS, "web/a.test.ts", Some(TESTS)));
        assert!(holds("Tests/web", "web/a.test.ts", Some(TESTS)));
        assert!(!holds("Tests/web", "web/a.ts", None));
        assert!(!holds("web", "web/a.test.ts", Some(TESTS)));
        assert!(!holds(DOCS, "web/a.test.ts", Some(TESTS)));
        // A real folder whose name starts like a group is an ordinary one.
        assert!(holds("Low signals", "Low signals/a.rs", None));
        assert!(holds("Tests", "Tests/Scale.swift", None));
    }

    #[test]
    fn the_filter_keeps_the_group_while_one_of_its_files_matches() {
        let (files, signals) = fixture();
        let collapsed = HashSet::from([LOW_SIGNAL.to_owned()]);
        let rows = tree_rows(&files, &signals, "png", &collapsed);
        assert_eq!(
            outline(&files, &rows),
            [
                "Low signal/ [2] +0 -0",
                "  assets/ [2] +0 -0",
                "    assets/badge.png",
                "    assets/icon.png",
            ]
        );
        let rows = tree_rows(&files, &signals, "units.rs", &collapsed);
        assert!(!rows.iter().any(|row| row.path == LOW_SIGNAL));
    }
}
