//! The files tab of the pull request view: a folder tree of the changed
//! files beside a virtualized diff, with existing review threads and new inline comments drawn as diff
//! annotations.

mod composer;
mod header;
mod model;
mod tree;
mod tree_view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffFile, DiffFileStatus, DiffHoverHighlight, DiffLinePosition,
    DiffLineRange, DiffState,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::diff::{Hunk, anchor_for_range, find_line, git_patch, parse_patch};
use loupe_github::{ChangedFile, NodeId, PullRequestDetail};

use self::composer::{ComposerEvent, ComposerTarget, ComposerView};
use self::model::{
    LoadedFiles, Missing, PlacedThread, Placement, anchor_error_message, build_files, diff_mode,
    is_viewed, place_threads, restore_viewed, set_viewed, split_path, status_letter, to_diff_side,
    to_side, viewed_counts,
};
use self::tree::{
    GitTint, Move, RowKind, TreeRow, ViewedMark, active_folder, ancestors, file_tint, folder_tint,
    into_tree_order, move_down, move_left, move_right, move_up, tree_rows, viewed_mark,
};
use self::tree_view::{RowView, StickyFolders};
use super::tab::{PrTabContext, PrTabEvent};
use super::thread::{HeadRef, ThreadEvent, ThreadView};

pub(super) use self::model::resolved_summary;
use crate::github::common::{self, LoadError, Loadable};
use crate::github::widgets;
use crate::store::{DiffLayout, FileTreePosition, Settings, update_settings};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::{assets, ui};

/// The files tree's starting width and the range its edge drags within.
const FILE_LIST_WIDTH: f32 = 340.;
/// A diff line longer than this runs past the panel at its default width.
const LONG_LINE: usize = 100;
/// What the toolbar's note about long lines explains.
const LONG_LINES_HINT: &str = "These files have lines longer than the view. Scroll sideways with Shift + wheel or ← →, or choose Wrap.";
const FILE_LIST_MIN: f32 = 220.;
const FILE_LIST_MAX: f32 = 640.;
/// How wide the diff's trailing edge fades while long lines run past it.
const FADE_WIDTH: f32 = 32.;

struct ThreadSlot {
    view: Entity<ThreadView>,
    _events: Subscription,
}

struct ComposerSlot {
    view: Entity<ComposerView>,
    _events: Subscription,
}

pub struct FilesTab {
    context: PrTabContext,
    load: Loadable<()>,
    _load: Option<Task<()>>,
    files: Vec<ChangedFile>,
    /// Folders the viewer closed, by full path.
    collapsed: HashSet<String>,
    /// The selected tree row, by path, so it survives filtering and reloads.
    selected: Option<String>,
    tree_focus: FocusHandle,
    tree_scroll: UniformListScrollHandle,
    filter: Entity<InputState>,
    _filter_changes: Subscription,
    hunks: HashMap<String, Arc<Vec<Hunk>>>,
    missing: HashMap<String, Missing>,
    diff: Option<Entity<DiffState>>,
    layout: DiffLayout,
    viewed_error: Option<String>,
    viewed_tasks: HashMap<String, Task<()>>,
    placed: Vec<PlacedThread>,
    threads: HashMap<NodeId, ThreadSlot>,
    threads_dirty: bool,
    composer: Option<ComposerSlot>,
}

impl FilesTab {
    pub fn new(context: PrTabContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let filter_changes = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut tab = Self {
            context,
            load: Loadable::Loading,
            _load: None,
            files: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            tree_focus: cx.focus_handle(),
            tree_scroll: UniformListScrollHandle::new(),
            filter,
            _filter_changes: filter_changes,
            hunks: HashMap::new(),
            missing: HashMap::new(),
            diff: None,
            layout: Settings::get(cx).diff_layout,
            viewed_error: None,
            viewed_tasks: HashMap::new(),
            placed: Vec::new(),
            threads: HashMap::new(),
            threads_dirty: false,
            composer: None,
        };
        tab.load_files(cx);
        tab
    }

    /// Takes the reloaded pull request, keeping drafts and scroll position.
    /// A new head commit reloads the files.
    pub fn set_detail(
        &mut self,
        detail: Arc<PullRequestDetail>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let head_changed = detail.head_oid != self.context.detail.head_oid
            || detail.base_oid != self.context.detail.base_oid
            || detail.base_ref != self.context.detail.base_ref;
        self.context.detail = detail;
        if head_changed && let Some(slot) = &self.composer {
            slot.view.update(cx, |view, cx| view.invalidate(cx));
        }
        if head_changed {
            self.load = Loadable::Loading;
            self.files.clear();
            self.hunks.clear();
            self.diff = None;
        }
        if head_changed || matches!(self.load, Loadable::Failed(_)) {
            self.load_files(cx);
        }
        self.sync_threads(window, cx);
        cx.notify();
    }

    fn load_files(&mut self, cx: &mut Context<Self>) {
        let key = crate::cache::github_key(
            cx,
            &format!(
                "files/{}/{}/{}/{}",
                self.context.pr.url(),
                self.context.detail.head_oid,
                self.context.detail.base_ref,
                self.context.detail.base_oid
            ),
        );
        if let Some(files) = key
            .as_deref()
            .and_then(crate::cache::get::<Vec<ChangedFile>>)
        {
            self.finish_load(
                Ok(build_files(into_tree_order(files), git_patch, |patch| {
                    parse_patch(patch).ok()
                })),
                cx,
            );
        }
        let reused = self.load.value().is_some();
        self.load.start_refresh();
        let expected_head = self.context.detail.head_oid.clone();
        let expected_base = self.context.detail.base_oid.clone();
        let expected_base_ref = self.context.detail.base_ref.clone();
        let revision = key.as_deref().map_or(0, crate::cache::revision);
        let write_key = key.clone();
        let pr = self.context.pr.clone();
        self._load = Some(common::run(
            cx,
            move |gh| {
                let files = match key
                    .as_deref()
                    .and_then(crate::cache::get::<Vec<ChangedFile>>)
                {
                    Some(mut files) => {
                        eprintln!("review-files source=cache pr={}", pr.url());
                        gh.refresh_viewed_states(&pr, &mut files)?;
                        files
                    }
                    None => {
                        eprintln!("review-files source=network pr={}", pr.url());
                        let files = gh.pull_request_files(&pr)?;
                        let current = gh.pull_request(&pr)?;
                        if current.head_oid != expected_head
                            || current.base_oid != expected_base
                            || current.base_ref != expected_base_ref
                        {
                            return Err(loupe_github::Error::Decode(
                                "Head changed while loading files; refresh to retry.".into(),
                            ));
                        }
                        files
                    }
                };
                Ok(build_files(into_tree_order(files), git_patch, |patch| {
                    parse_patch(patch).ok()
                }))
            },
            move |this, result, cx| {
                if write_key
                    .as_deref()
                    .is_some_and(|key| crate::cache::revision(key) != revision)
                {
                    this.load_files(cx);
                    return;
                }
                if let (Some(key), Ok(loaded)) = (&write_key, &result) {
                    crate::cache::put(key, &loaded.files);
                }
                if reused {
                    match result {
                        Ok(loaded) => {
                            this.files = loaded.files;
                            this.load.finish(Ok(()));
                            this.collapse_viewed(cx);
                        }
                        Err(error) => this.load.finish(Err(error)),
                    }
                } else {
                    this.finish_load(result, cx);
                }
            },
        ));
    }

    fn finish_load(&mut self, result: Result<LoadedFiles, LoadError>, cx: &mut Context<Self>) {
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(error) => {
                self.load.finish(Err(error));
                return;
            }
        };
        self.load.finish(Ok(()));
        self.files = loaded.files;
        self.hunks = loaded.hunks;
        self.missing = loaded.missing;
        let mode = diff_mode(self.layout);
        match &self.diff {
            Some(state) => state.update(cx, |state, cx| state.set_files(loaded.docs, cx)),
            None => {
                self.diff = Some(cx.new(|cx| DiffState::new(loaded.docs, cx).with_mode(mode)));
            }
        }
        self.collapse_viewed(cx);
        if let Some(slot) = &self.composer
            && !self
                .files
                .iter()
                .any(|file| file.path == slot.view.read(cx).target().path)
        {
            self.composer = None;
        }
        self.threads_dirty = true;
    }

    fn collapse_viewed(&self, cx: &mut Context<Self>) {
        let Some(state) = &self.diff else { return };
        state.update(cx, |state, cx| {
            for file in &self.files {
                state.set_file_collapsed(&file.path, is_viewed(file.viewed), cx);
            }
        });
    }

    /// Places the pull request's threads in the diff and keeps one entity per
    /// thread, so drafts and expanded state survive a reload.
    fn sync_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.threads_dirty = false;
        if self.load.value().is_none() {
            return;
        }
        let paths: HashSet<&str> = self.files.iter().map(|file| file.path.as_str()).collect();
        let hunks = &self.hunks;
        let placed = place_threads(
            &self.context.detail.threads,
            |path| paths.contains(path),
            |path, side, line| {
                hunks
                    .get(path)
                    .is_some_and(|hunks| find_line(hunks, line, side).is_some())
            },
        );
        let keep: HashSet<&str> = placed
            .iter()
            .map(|placed| placed.thread_id.as_str())
            .collect();
        self.threads.retain(|id, _| keep.contains(id.as_str()));
        let head = HeadRef::of(&self.context.detail);
        for placed_thread in &placed {
            let Some(thread) = self
                .context
                .detail
                .threads
                .iter()
                .find(|thread| thread.id == placed_thread.thread_id)
            else {
                continue;
            };
            let label = match &placed_thread.placement {
                Placement::File { label, .. } => Some(label.clone()),
                Placement::Line { .. } => None,
            };
            match self.threads.get(&placed_thread.thread_id) {
                Some(slot) => slot.view.update(cx, |view, cx| {
                    view.update(thread.clone(), label, head.clone(), cx)
                }),
                None => {
                    let view = cx
                        .new(|cx| ThreadView::new(thread.clone(), label, head.clone(), window, cx));
                    let events = cx.subscribe(&view, |_, _, event: &ThreadEvent, cx| match event {
                        ThreadEvent::Wrote => cx.emit(PrTabEvent::Reload),
                    });
                    self.threads.insert(
                        placed_thread.thread_id.clone(),
                        ThreadSlot {
                            view,
                            _events: events,
                        },
                    );
                }
            }
        }
        self.placed = placed;
    }

    fn set_layout(&mut self, layout: DiffLayout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        update_settings(cx, |settings| settings.diff_layout = layout);
        if let Some(state) = &self.diff {
            state.update(cx, |state, cx| state.set_mode(diff_mode(layout), cx));
        }
        cx.notify();
    }

    fn scroll_to(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(state) = &self.diff {
            state.update(cx, |state, cx| state.scroll_to_file(path, cx));
        }
    }

    /// Marks a file viewed or unviewed at once, collapses or expands it, and
    /// reverts with an inline error when GitHub refuses. One write per file
    /// at a time: dropping a task does not stop its request, so a second
    /// toggle could reach GitHub first and leave it opposite to the screen.
    fn toggle_viewed(&mut self, path: &str, viewed: bool, cx: &mut Context<Self>) {
        if self.viewed_tasks.contains_key(path) {
            return;
        }
        let Some(previous) = set_viewed(&mut self.files, path, viewed) else {
            return;
        };
        // A pre-mutation viewed read must not undo the confirmed mutation.
        self._load = None;
        self.load.finish(Ok(()));
        let cache_key = crate::cache::github_key(
            cx,
            &format!(
                "files/{}/{}/{}/{}",
                self.context.pr.url(),
                self.context.detail.head_oid,
                self.context.detail.base_ref,
                self.context.detail.base_oid
            ),
        );
        if let Some(key) = &cache_key {
            crate::cache::invalidate_reads(key);
        }
        self.viewed_error = None;
        self.set_collapsed(path, viewed, cx);
        let pull_request_id = self.context.detail.id.clone();
        let expected_head = self.context.detail.head_oid.clone();
        let expected_base = self.context.detail.base_oid.clone();
        let request_path = path.to_owned();
        let owned_path = path.to_owned();
        let task = common::run(
            cx,
            move |gh| gh.set_file_viewed(&pull_request_id, &request_path, viewed),
            move |this, result, cx| {
                this.viewed_tasks.remove(&owned_path);
                if this.context.detail.head_oid != expected_head
                    || this.context.detail.base_oid != expected_base
                {
                    return;
                }
                let confirmed = result.is_ok();
                if let Err(error) = result {
                    restore_viewed(&mut this.files, &owned_path, previous);
                    this.set_collapsed(&owned_path, is_viewed(previous), cx);
                    this.viewed_error = Some(format!(
                        "Could not update the viewed mark of {owned_path}: {}",
                        error.detail()
                    ));
                }
                if confirmed && let Some(key) = &cache_key {
                    crate::cache::update::<Vec<ChangedFile>>(key, |files| {
                        set_viewed(files, &owned_path, viewed);
                    });
                }
            },
        );
        self.viewed_tasks.insert(path.to_owned(), task);
        cx.notify();
    }

    /// Marks every file under a folder viewed or unviewed, one write per file
    /// that is not in that state yet.
    fn set_folder_viewed(&mut self, paths: &[String], viewed: bool, cx: &mut Context<Self>) {
        for path in paths {
            let current = self
                .files
                .iter()
                .any(|file| &file.path == path && is_viewed(file.viewed));
            if current != viewed {
                self.toggle_viewed(path, viewed, cx);
            }
        }
    }

    fn toggle_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_owned());
        }
        cx.notify();
    }

    fn set_collapsed(&self, path: &str, collapsed: bool, cx: &mut Context<Self>) {
        if let Some(state) = &self.diff {
            state.update(cx, |state, cx| {
                state.set_file_collapsed(path, collapsed, cx)
            });
        }
    }

    /// Opens the composer under the last line of `range`, or an inline error
    /// when GitHub cannot take a comment there.
    fn open_composer(
        &mut self,
        range: &DiffLineRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = range.path().to_string();
        let first = (range.start() as u32, to_side(range.side()));
        let last = (range.end() as u32, to_side(range.end_side()));
        let anchor = match self.hunks.get(&path) {
            Some(hunks) => {
                anchor_for_range(hunks, first, last).map_err(|error| anchor_error_message(&error))
            }
            None => {
                Err("This file's diff could not be read, so it cannot take a comment.".to_owned())
            }
        };
        let (line, side) = match &anchor {
            Ok(anchor) => (anchor.line, anchor.side),
            Err(_) => last,
        };
        let target = ComposerTarget {
            path,
            line,
            side,
            anchor,
        };
        match &self.composer {
            Some(slot) => slot
                .view
                .update(cx, |view, cx| view.retarget(target, window, cx)),
            None => {
                let pull_request_id = self.context.detail.id.clone();
                let view = cx.new(|cx| ComposerView::new(pull_request_id, target, window, cx));
                let events = cx.subscribe(&view, |this, _, event: &ComposerEvent, cx| {
                    this.composer = None;
                    if let Some(state) = &this.diff {
                        state.update(cx, |state, cx| state.set_selected_lines(None, cx));
                    }
                    if matches!(event, ComposerEvent::Submitted) {
                        cx.emit(PrTabEvent::Reload);
                    }
                    cx.notify();
                });
                self.composer = Some(ComposerSlot {
                    view,
                    _events: events,
                });
            }
        }
        cx.notify();
    }

    /// The files with a line that runs past the panel while lines scroll.
    fn long_files(&self) -> impl Iterator<Item = &ChangedFile> {
        self.files.iter().filter(|file| {
            file.patch
                .as_deref()
                .is_some_and(|patch| patch.lines().any(|line| line.chars().count() > LONG_LINE))
        })
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let wrap = Settings::get(cx).diff_wrap;
        let counts = viewed_counts(&self.files);
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let percent = if counts.total == 0 {
            0.
        } else {
            counts.viewed as f32 * 100. / counts.total as f32
        };
        let (additions, deletions) = self.files.iter().fold((0, 0), |(a, d), file| {
            (a + file.additions, d + file.deletions)
        });
        let this = cx.entity().downgrade();
        let long = if wrap { 0 } else { self.long_files().count() };
        let tree_right = Settings::get(cx).file_tree_position == FileTreePosition::Right;
        let mut parts = vec![
            h_flex()
                .flex_none()
                .w(dp(FILE_LIST_WIDTH - 32.))
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(counts.label()),
                )
                .child(
                    div().flex_1().child(
                        Progress::new("files-viewed")
                            .value(percent)
                            .accessibility_label("Files viewed"),
                    ),
                )
                .into_any_element(),
            widgets::diff_stat(additions, deletions).into_any_element(),
        ];
        if self.load.is_busy() {
            parts.push(Spinner::new().xsmall().color(muted).into_any_element());
        }
        parts.push(div().flex_1().into_any_element());
        // In words, beside the control that answers it: lines go on
        // past the edge. The edge's fade alone is dark on dark and says
        // nothing on a line with no tint, and the component shows its
        // own horizontal bar only while it scrolls.
        if long > 0 {
            parts.push(
                div()
                    .id("long-lines-note")
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .tooltip(|window, cx| Tooltip::new(LONG_LINES_HINT).build(window, cx))
                    .child(match long {
                        1 => "Long lines in 1 file".to_owned(),
                        long => format!("Long lines in {long} files"),
                    })
                    .into_any_element(),
            );
        }
        // Both choices about how the diff is laid out use the one
        // segmented control, side by side.
        parts.push(
            ui::segmented(
                "diff-wrap",
                "Long lines",
                [
                    (Some(IconName::MoveHorizontal), "Scroll"),
                    (Some(IconName::TextWrap), "Wrap"),
                ],
                usize::from(wrap),
                |index, _, cx| {
                    update_settings(cx, |settings| settings.diff_wrap = index == 1);
                },
            )
            .into_any_element(),
        );
        parts.push(
            ui::segmented(
                "diff-layout",
                "Diff layout",
                [
                    (Some(IconName::Rows2), "Unified"),
                    (Some(IconName::Columns2), "Split"),
                ],
                usize::from(self.layout == DiffLayout::Split),
                move |index, _, cx| {
                    let layout = if index == 1 {
                        DiffLayout::Split
                    } else {
                        DiffLayout::Unified
                    };
                    this.update(cx, |this, cx| this.set_layout(layout, cx)).ok();
                },
            )
            .into_any_element(),
        );
        // The bar mirrors with the tree: the viewed count stays over the
        // tree's filter, and the diff's controls over the diff.
        if tree_right {
            parts.reverse();
        }
        h_flex()
            .flex_none()
            .h(scale::toolbar_height())
            .px(scale::content_inset())
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(border)
            .children(parts)
            .into_any_element()
    }

    fn current_rows(&self, cx: &App) -> Vec<TreeRow> {
        tree_rows(&self.files, &self.filter.read(cx).value(), &self.collapsed)
    }

    /// A click selects the row and focuses the tree. A file also scrolls the
    /// diff to it and a folder opens or closes, while a pinned folder
    /// scrolls the list back to its own row, as in Zed.
    fn click_tree_row(
        &mut self,
        path: &str,
        sticky: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tree_focus.focus(window, cx);
        self.selected = Some(path.to_owned());
        let rows = self.current_rows(cx);
        if let Some(ix) = rows.iter().position(|row| row.path == path) {
            if sticky {
                // Below the folders that stay pinned above it.
                self.tree_scroll.scroll_to_item_strict_with_offset(
                    ix,
                    ScrollStrategy::Top,
                    rows[ix].depth,
                );
            } else {
                self.open_row(&rows[ix], cx);
            }
        }
        cx.notify();
    }

    fn open_row(&mut self, row: &TreeRow, cx: &mut Context<Self>) {
        match row.kind {
            RowKind::Dir(_) => self.toggle_folder(&row.path, cx),
            RowKind::File { .. } => self.scroll_to(&row.path, cx),
        }
    }

    /// A file's box flips it; a folder's box marks all its files viewed, or
    /// none once all of them are.
    fn toggle_row_viewed(&mut self, path: &str, cx: &mut Context<Self>) {
        let rows = self.current_rows(cx);
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        match &row.kind {
            RowKind::File { index } => {
                let viewed = is_viewed(self.files[*index].viewed);
                self.toggle_viewed(path, !viewed, cx);
            }
            RowKind::Dir(dir) => {
                let mark =
                    viewed_mark(dir.files.iter().map(|&ix| is_viewed(self.files[ix].viewed)));
                let paths: Vec<String> = dir
                    .files
                    .iter()
                    .map(|&ix| self.files[ix].path.clone())
                    .collect();
                self.set_folder_viewed(&paths, mark != ViewedMark::All, cx);
            }
        }
    }

    /// Zed's project panel keys: ↑↓ move, → opens or steps in, ← closes or
    /// steps out, Enter and Space open the row.
    fn tree_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let rows = self.current_rows(cx);
        let selected = self
            .selected
            .as_deref()
            .and_then(|path| rows.iter().position(|row| row.path == path));
        let step = match event.keystroke.key.as_str() {
            "down" => move_down(&rows, selected),
            "up" => move_up(&rows, selected),
            "right" => selected.and_then(|ix| move_right(&rows, ix)),
            "left" => selected.and_then(|ix| move_left(&rows, ix)),
            "enter" | "space" => {
                if let Some(ix) = selected {
                    self.open_row(&rows[ix], cx);
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        match step {
            Some(Move::Select(ix)) => {
                self.selected = Some(rows[ix].path.clone());
                self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            Some(Move::Expand(ix)) => {
                self.collapsed.remove(&rows[ix].path);
            }
            Some(Move::Collapse(ix)) => {
                self.collapsed.insert(rows[ix].path.clone());
                self.selected = Some(rows[ix].path.clone());
                self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            None => {}
        }
        cx.notify();
    }

    fn row_views(&self, rows: &[TreeRow], focused: bool, cx: &App) -> Vec<RowView> {
        let selected = self
            .selected
            .as_deref()
            .and_then(|path| rows.iter().position(|row| row.path == path));
        let active = selected.and_then(|ix| active_folder(rows, ix));
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        rows.iter()
            .enumerate()
            .map(|(ix, row)| {
                let guides = ancestors(rows, ix)
                    .into_iter()
                    .map(|folder| Some(folder) == active)
                    .collect();
                let path = SharedString::from(row.path.clone());
                let is_selected = selected == Some(ix);
                match &row.kind {
                    RowKind::Dir(dir) => {
                        let files = dir.files.iter().map(|&ix| &self.files[ix]);
                        let viewed = viewed_mark(files.clone().map(|file| is_viewed(file.viewed)));
                        // A folder that is all new or all deleted says so on
                        // its icon; any other folder's icon stays muted.
                        let icon_color = match folder_tint(files.map(|file| file.status)) {
                            GitTint::Created => tokens::open(),
                            GitTint::Deleted => tokens::closed(),
                            GitTint::Modified | GitTint::Renamed => muted,
                        };
                        let count = dir.files.len();
                        let noun = if count == 1 { "file" } else { "files" };
                        RowView {
                            depth: row.depth,
                            folder: Some(dir.expanded),
                            label: dir.label.clone().into(),
                            // Names stay neutral; the status letter carries
                            // the color.
                            label_color: if viewed == ViewedMark::All {
                                muted
                            } else {
                                foreground
                            },
                            icon: if dir.expanded {
                                assets::FOLDER_OPEN
                            } else {
                                assets::FOLDER
                            },
                            icon_color,
                            additions: dir.additions,
                            deletions: dir.deletions,
                            status: None,
                            viewed,
                            selected: is_selected,
                            focused,
                            guides,
                            tooltip: format!(
                                "{} · {count} {noun} · +{} −{}",
                                row.path, dir.additions, dir.deletions
                            )
                            .into(),
                            accessible_name: format!(
                                "Folder {}, {count} {noun}, +{} −{}",
                                row.path, dir.additions, dir.deletions
                            )
                            .into(),
                            path,
                        }
                    }
                    RowKind::File { index } => {
                        let file = &self.files[*index];
                        let viewed = is_viewed(file.viewed);
                        let tint = tint_color(file_tint(file.status));
                        let missing = self.missing.contains_key(&file.path);
                        RowView {
                            depth: row.depth,
                            folder: None,
                            label: split_path(&file.path).1.to_owned().into(),
                            label_color: if viewed { muted } else { foreground },
                            icon: assets::file_icon(&file.path),
                            icon_color: muted,
                            additions: file.additions,
                            deletions: file.deletions,
                            status: Some((status_letter(file.status), tint)),
                            viewed: if viewed {
                                ViewedMark::All
                            } else {
                                ViewedMark::None
                            },
                            selected: is_selected,
                            focused,
                            guides,
                            tooltip: format!(
                                "{} · +{} −{}{}",
                                file.path,
                                file.additions,
                                file.deletions,
                                if missing { " · diff not shown" } else { "" }
                            )
                            .into(),
                            accessible_name: format!(
                                "File {}, +{} −{}",
                                file.path, file.additions, file.deletions
                            )
                            .into(),
                            path,
                        }
                    }
                }
            })
            .collect()
    }

    fn file_list(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rows = Rc::new(self.current_rows(cx));
        let focused = self.tree_focus.is_focused(window);
        let views = Rc::new(self.row_views(&rows, focused, cx));
        let tab = cx.entity().downgrade();
        let body = if rows.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_size(scale::meta())
                .text_color(cx.theme().muted_foreground)
                .child("No files match the filter.")
                .into_any_element()
        } else {
            let list_views = views.clone();
            let list_tab = tab.clone();
            uniform_list("files-tree-list", views.len(), move |range, _, cx| {
                range
                    .map(|ix| tree_view::row(&list_views[ix], ix, false, &list_tab, cx))
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.tree_scroll)
            .with_decoration(StickyFolders { rows, views, tab })
            .size_full()
            .into_any_element()
        };
        // The tree's border thickens the split's hairline where the diff
        // starts. On the right the diff ends in a line of its own.
        let tree_right = Settings::get(cx).file_tree_position == FileTreePosition::Right;
        v_flex()
            .debug_selector(|| "files-tree".into())
            .size_full()
            .when(!tree_right, |tree| tree.border_r_1())
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                // On the toolbar's 16 px edge, under "N / M viewed".
                div().flex_none().px_4().pt_2().pb_1().child(
                    Input::new(&self.filter)
                        .small()
                        .h(rems(1.75))
                        .text_size(scale::body())
                        .prefix(Icon::new(IconName::Search).with_size(dp(14.)))
                        .cleanable(true)
                        .aria_label("Filter files"),
                ),
            )
            .child(
                div()
                    .id("files-tree")
                    .role(Role::Tree)
                    .aria_label("Changed files")
                    .track_focus(&self.tree_focus)
                    .on_key_down(cx.listener(Self::tree_key))
                    .flex_1()
                    .min_h_0()
                    .child(body),
            )
            .into_any_element()
    }

    fn diff_view(&self, state: &Entity<DiffState>, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let mut annotations = Vec::new();
        let mut views: Vec<(ElementId, AnyView)> = Vec::new();
        let mut notes: Vec<(ElementId, SharedString)> = Vec::new();
        for placed in &self.placed {
            let Some(slot) = self.threads.get(&placed.thread_id) else {
                continue;
            };
            let id = ElementId::Name(SharedString::from(placed.thread_id.clone()));
            annotations.push(match &placed.placement {
                Placement::Line { path, side, line } => DiffAnnotation::line(
                    id.clone(),
                    DiffLinePosition::new(path.clone(), *side, *line),
                ),
                Placement::File { path, .. } => DiffAnnotation::file(id.clone(), path.clone()),
            });
            views.push((id, slot.view.clone().into()));
        }
        if let Some(slot) = &self.composer {
            let target = slot.view.read(cx).target();
            let id = ElementId::Name("composer".into());
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(
                    target.path.clone(),
                    to_diff_side(target.side),
                    target.line as usize,
                ),
            ));
            views.push((id, slot.view.clone().into()));
        }
        for (path, reason) in &self.missing {
            let id = ElementId::Name(SharedString::from(format!("no-diff:{path}")));
            annotations.push(DiffAnnotation::file(id.clone(), path.clone()));
            notes.push((id, reason.note().into()));
        }

        // Each file's size, for its header, as GitHub counts it.
        let sizes: Rc<HashMap<String, (u32, u32)>> = Rc::new(
            self.files
                .iter()
                .map(|file| (file.path.clone(), (file.additions, file.deletions)))
                .collect(),
        );
        let wrap = Settings::get(cx).diff_wrap;
        let viewed: Rc<HashSet<String>> = Rc::new(
            self.files
                .iter()
                .filter(|file| is_viewed(file.viewed))
                .map(|file| file.path.clone())
                .collect(),
        );
        let theme = cx.theme();
        let (muted, border, surface, radius) = (
            theme.muted_foreground,
            theme.border,
            theme.group_box,
            theme.radius_lg,
        );
        let add_weak = weak.clone();
        // While lines scroll, whether some line runs past the panel: the
        // toolbar says so in words and the panel's edge fades. The
        // component draws its own horizontal bar only while it scrolls and
        // keeps the scroll offset to itself (`DiffState::horizontal_scroll`
        // is crate-private), so a bar of Loupe's own could not be true.
        let runs_on = !wrap && self.long_files().next().is_some();
        let header = {
            let (viewed, sizes, weak) = (viewed.clone(), sizes.clone(), weak.clone());
            let fade = if runs_on { dp(FADE_WIDTH) } else { px(0.) };
            move |file: &DiffFile, _: &mut Window, _: &mut App| {
                let path = file.path().clone();
                let title = match (file.original_path(), file.modified_path()) {
                    (Some(old), Some(new)) if old != new => format!("{old} → {new}").into(),
                    _ => path.clone(),
                };
                let (additions, deletions) = sizes.get(path.as_ref()).copied().unwrap_or_default();
                let status = match file.status() {
                    DiffFileStatus::Added => Some("Added file"),
                    DiffFileStatus::Deleted => Some("Deleted file"),
                    DiffFileStatus::Renamed => Some("Renamed"),
                    DiffFileStatus::Copied => Some("Copied"),
                    _ => None,
                };
                let weak = weak.clone();
                let toggle_path = path.to_string();
                header::file_header(header::HeaderContent {
                    viewed: viewed.contains(path.as_ref()),
                    title,
                    path,
                    additions,
                    deletions,
                    status,
                    fade,
                    on_viewed: Rc::new(move |checked, _, cx| {
                        weak.update(cx, |this, cx| this.toggle_viewed(&toggle_path, checked, cx))
                            .ok();
                    }),
                })
            }
        };
        div()
            .debug_selector(|| "files-diff".into())
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                // Long lines scroll sideways by default, so a line keeps its
                // indentation (the component has no hanging indent for a
                // wrapped line); the toolbar's Wrap option wraps them. Threads and the composer anchor under a
                // line in a card at most 760 px wide from the leading edge,
                // so their buttons stay in reach however wide the file is.
                Diff::new(state)
                    .size_full()
                    .soft_wrap(wrap)
                    .hover_highlight(DiffHoverHighlight::Both)
                    .annotations(annotations)
                    .render_annotation(move |annotation, _, _| {
                        // Threads and the composer sit in a card under their
                        // line, inset from the gutter like a review comment.
                        let card = div()
                            .mx_3()
                            .my_2()
                            .p(scale::card_padding())
                            .max_w(dp(760.))
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .bg(surface);
                        if let Some((_, view)) = views.iter().find(|(id, _)| id == annotation.id())
                        {
                            return card.child(view.clone()).into_any_element();
                        }
                        let note = notes
                            .iter()
                            .find(|(id, _)| id == annotation.id())
                            .map(|(_, note)| note.clone());
                        h_flex()
                            .mx_3()
                            .my_2()
                            .gap_2()
                            .items_center()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child(Icon::new(IconName::FileX).xsmall())
                            .children(note)
                            .into_any_element()
                    })
                    // The path, the size, and Viewed in the part of the
                    // header on screen, whether lines wrap or scroll.
                    .render_header(header)
                    .on_add_annotation(move |range, window, cx| {
                        let range = range.clone();
                        add_weak
                            .update(cx, |_, cx| {
                                cx.defer_in(window, move |this, window, cx| {
                                    this.open_composer(&range, window, cx)
                                });
                            })
                            .ok();
                    }),
            )
            .when(runs_on, |panel| {
                let shade = tokens::background();
                panel.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(dp(FADE_WIDTH))
                        .bg(linear_gradient(
                            90.,
                            linear_color_stop(shade.opacity(0.), 0.),
                            linear_color_stop(shade, 1.),
                        )),
                )
            })
            .into_any_element()
    }

    fn loaded_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.diff.clone() else {
            return div().into_any_element();
        };
        v_flex()
            .size_full()
            .child(self.toolbar(cx))
            .children(self.viewed_error.clone().map(|message| {
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(Alert::error("viewed-error", message).flex_1())
                    .child(
                        ui::icon_button("dismiss-viewed-error", IconName::Close, "Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.viewed_error = None;
                                cx.notify();
                            })),
                    )
            }))
            .child(
                // The tree's edge drags, so a deep path can be given the
                // room to show whole.
                div().flex_1().min_h_0().w_full().child({
                    let tree = resizable_panel()
                        .size(dp(FILE_LIST_WIDTH))
                        .size_range(dp(FILE_LIST_MIN)..dp(FILE_LIST_MAX))
                        .child(self.file_list(window, cx));
                    let diff = resizable_panel().child(self.diff_view(&state, cx));
                    // One id per side: the group keeps its panels' widths by
                    // index, and the tree's width must not land on the diff.
                    match Settings::get(cx).file_tree_position {
                        FileTreePosition::Left => {
                            h_resizable("files-split").child(tree).child(diff)
                        }
                        FileTreePosition::Right => {
                            h_resizable("files-split-right").child(diff).child(tree)
                        }
                    }
                }),
            )
            .into_any_element()
    }
}

fn tint_color(tint: GitTint) -> Hsla {
    match tint {
        GitTint::Created => tokens::open(),
        GitTint::Modified => tokens::pending(),
        GitTint::Deleted => tokens::closed(),
        GitTint::Renamed => tokens::link(),
    }
}

fn skeleton(cx: &App) -> impl IntoElement {
    const WIDTHS: [f32; 5] = [0.9, 0.65, 0.8, 0.5, 0.75];
    let tree_right = Settings::get(cx).file_tree_position == FileTreePosition::Right;
    let tree = v_flex()
        .flex_none()
        .w(dp(FILE_LIST_WIDTH))
        .h_full()
        .p_3()
        .gap_3()
        .map(|tree| {
            if tree_right {
                tree.border_l_1()
            } else {
                tree.border_r_1()
            }
        })
        .border_color(cx.theme().border)
        .children((0..10).map(|ix| {
            h_flex()
                .gap_2()
                .child(Skeleton::new().size_4().rounded_sm())
                .child(Skeleton::new().h_3().w(relative(WIDTHS[ix % WIDTHS.len()])))
        }));
    let diff = v_flex().flex_1().p_4().gap_2().children((0..16).map(|ix| {
        Skeleton::new()
            .secondary()
            .h_3()
            .w(relative(WIDTHS[(ix + 2) % WIDTHS.len()]))
    }));
    if tree_right {
        h_flex().size_full().child(diff).child(tree)
    } else {
        h_flex().size_full().child(tree).child(diff)
    }
}

impl EventEmitter<PrTabEvent> for FilesTab {}

impl Render for FilesTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.threads_dirty {
            self.sync_threads(window, cx);
        }
        let body = match &self.load {
            Loadable::Loading => skeleton(cx).into_any_element(),
            Loadable::Failed(error) => {
                common::load_error(error, cx.listener(|this, _, _, cx| this.load_files(cx)))
                    .into_any_element()
            }
            Loadable::Loaded(()) | Loadable::Refreshing(()) if self.files.is_empty() => {
                ui::empty_state(
                    IconName::FileDiff,
                    "No changed files",
                    "This pull request does not change any files.",
                )
                .into_any_element()
            }
            Loadable::Loaded(()) | Loadable::Refreshing(()) => self.loaded_view(window, cx),
        };
        div().size_full().child(body)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{DateTime, Utc};
    use gpui_kit::{
        AppContext as _, Bounds, Entity, InputEvent as _, Modifiers, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollEvent, TestAppContext,
        VisualTestContext, WindowBounds, WindowOptions, point, px, size,
    };
    use loupe_github::diff::{git_patch, parse_patch};
    use loupe_github::{
        ChangedFile, Checks, FileStatus, MergeMethod, MergeStateStatus, Mergeable, PrRef, PrState,
        PullRequestDetail, RepoRef, ViewedState,
    };

    use super::model::build_files;
    use super::{FILE_LIST_MAX, FILE_LIST_MIN, FILE_LIST_WIDTH, FilesTab, PrTabContext};
    use crate::services::{Accounts, Services};
    use crate::store::{FileTreePosition, Settings, update_settings};

    const WINDOW_WIDTH: f32 = 1280.;

    fn detail() -> PullRequestDetail {
        let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap();
        PullRequestDetail {
            id: "PR_1".into(),
            pr: PrRef::new(RepoRef::new("acme", "app"), 12),
            url: "https://github.com/acme/app/pull/12".into(),
            title: "Add thing".into(),
            body: String::new(),
            state: PrState::Open,
            is_draft: false,
            mergeable: Mergeable::Mergeable,
            merge_state: MergeStateStatus::Clean,
            merge_methods: vec![MergeMethod::Squash],
            base_ref: "main".into(),
            base_oid: "base123".into(),
            head_ref: "feature".into(),
            head_repo: None,
            head_oid: "abc".into(),
            author: None,
            created_at: epoch,
            updated_at: epoch,
            body_edits: Vec::new(),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            review_decision: None,
            reviewers: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            commits: Vec::new(),
            checks: Checks {
                state: None,
                runs: Vec::new(),
            },
            timeline: Vec::new(),
            threads: Vec::new(),
            pending_review: None,
            viewer_login: "bruno".into(),
            viewer_can_update: true,
            stack: None,
        }
    }

    /// The files tab alone in a window, with one changed file loaded.
    fn open_files_tab(cx: &mut TestAppContext) -> (VisualTestContext, Entity<FilesTab>) {
        open_files_tab_with(
            cx,
            ChangedFile {
                path: "src/main.rs".into(),
                previous_path: None,
                status: FileStatus::Modified,
                additions: 1,
                deletions: 1,
                patch: Some("@@ -1 +1 @@\n-old\n+new".into()),
                viewed: ViewedState::Unviewed,
            },
        )
    }

    /// The files tab alone in a window, with `file` loaded.
    fn open_files_tab_with(
        cx: &mut TestAppContext,
        file: ChangedFile,
    ) -> (VisualTestContext, Entity<FilesTab>) {
        let (handle, tab) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(WINDOW_WIDTH), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let context = PrTabContext {
                        pr: PrRef::new(RepoRef::new("acme", "app"), 12),
                        detail: Arc::new(detail()),
                    };
                    cx.new(|cx| FilesTab::new(context, window, cx))
                },
            )
            .expect("open the window")
        });
        let mut cx = VisualTestContext::from_window(handle, cx);
        // With no GitHub client the first load fails; then the files arrive.
        cx.run_until_parked();
        tab.update(&mut cx, |tab, cx| {
            tab.finish_load(
                Ok(build_files(vec![file], git_patch, |patch| {
                    parse_patch(patch).ok()
                })),
                cx,
            );
            cx.notify();
        });
        (cx, tab)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    /// Where the tree and the diff were painted, once the split settled.
    fn panes(cx: &mut VisualTestContext) -> (Bounds<Pixels>, Bounds<Pixels>) {
        for _ in 0..3 {
            draw(cx);
            cx.run_until_parked();
        }
        (
            cx.debug_bounds("files-tree")
                .expect("the tree is on screen"),
            cx.debug_bounds("files-diff")
                .expect("the diff is on screen"),
        )
    }

    fn dispatch(cx: &mut VisualTestContext, event: gpui_kit::PlatformInput) {
        cx.update(|window, cx| {
            window.dispatch_event(event, cx);
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    /// Presses on the line between the tree and the diff at `x`, moves `by`
    /// to the right in small steps, and releases there.
    fn drag_divider(cx: &mut VisualTestContext, x: Pixels, by: f32) {
        let start = point(x, px(400.));
        let none = Modifiers::none();
        dispatch(
            cx,
            MouseMoveEvent {
                position: start,
                pressed_button: None,
                modifiers: none,
            }
            .to_platform_input(),
        );
        dispatch(
            cx,
            MouseDownEvent {
                position: start,
                button: MouseButton::Left,
                modifiers: none,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
        );
        let mut end = start;
        for step in 1..=10 {
            end = point(start.x + px(by * step as f32 / 10.), start.y);
            dispatch(
                cx,
                MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: none,
                }
                .to_platform_input(),
            );
        }
        dispatch(
            cx,
            MouseUpEvent {
                position: end,
                button: MouseButton::Left,
                modifiers: none,
                click_count: 1,
            }
            .to_platform_input(),
        );
    }

    fn width(bounds: Bounds<Pixels>) -> f32 {
        f32::from(bounds.size.width).round()
    }

    #[gpui_kit::test]
    fn the_tree_sits_on_the_chosen_side_with_the_same_width_and_drag(cx: &mut TestAppContext) {
        let (mut cx, _tab) = open_files_tab(cx);
        let cx = &mut cx;

        let (tree, diff) = panes(cx);
        assert_eq!(f32::from(tree.left()), 0., "the tree starts on the left");
        assert_eq!(width(tree), FILE_LIST_WIDTH);
        assert!(diff.left() >= tree.right(), "the diff comes after the tree");
        assert_eq!(f32::from(diff.right()).round(), WINDOW_WIDTH);

        drag_divider(cx, tree.right(), 60.);
        let (tree, _) = panes(cx);
        assert_eq!(width(tree), FILE_LIST_WIDTH + 60., "the edge drags");

        cx.update(|_, cx| {
            update_settings(cx, |settings| {
                settings.file_tree_position = FileTreePosition::Right
            })
        });
        let (tree, diff) = panes(cx);
        assert_eq!(f32::from(diff.left()), 0., "the diff starts on the left");
        assert!(tree.left() >= diff.right(), "the tree comes after the diff");
        assert_eq!(f32::from(tree.right()).round(), WINDOW_WIDTH);
        assert_eq!(width(tree), FILE_LIST_WIDTH, "the same starting width");

        // The same drag, mirrored: pulling the edge away from the tree's
        // side widens the tree, within the same limits.
        drag_divider(cx, tree.left(), -60.);
        let (tree, diff) = panes(cx);
        assert_eq!(width(tree), FILE_LIST_WIDTH + 60.);
        assert_eq!(f32::from(tree.right()).round(), WINDOW_WIDTH);
        assert!(tree.left() >= diff.right());

        drag_divider(cx, tree.left(), 600.);
        let (tree, _) = panes(cx);
        assert_eq!(width(tree), FILE_LIST_MIN, "no narrower than the minimum");

        drag_divider(cx, tree.left(), -900.);
        let (tree, diff) = panes(cx);
        assert_eq!(width(tree), FILE_LIST_MAX, "no wider than the maximum");
        assert_eq!(f32::from(tree.right()).round(), WINDOW_WIDTH);
        assert!(
            (f32::from(diff.right()) - f32::from(tree.left())).abs() <= 1.,
            "the diff fills what the tree leaves"
        );
    }

    /// A path far wider than the diff beside the tree at its opening width.
    const LONG_PATH: &str = "apps/web-api/src/modules/billing/application/service/reminders/scheduling/notifications/templates/billing.service.payment-reminder.spec.ts";

    fn click(cx: &mut VisualTestContext, position: Point<Pixels>) {
        let none = Modifiers::none();
        dispatch(
            cx,
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: none,
            }
            .to_platform_input(),
        );
        dispatch(
            cx,
            MouseDownEvent {
                position,
                button: MouseButton::Left,
                modifiers: none,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
        );
        dispatch(
            cx,
            MouseUpEvent {
                position,
                button: MouseButton::Left,
                modifiers: none,
                click_count: 1,
            }
            .to_platform_input(),
        );
    }

    /// The path and Viewed in the header, once the diff settled.
    fn header(cx: &mut VisualTestContext) -> (Bounds<Pixels>, Bounds<Pixels>, Bounds<Pixels>) {
        let (_, diff) = panes(cx);
        (
            diff,
            cx.debug_bounds("header-path").expect("the path is drawn"),
            cx.debug_bounds("header-viewed").expect("Viewed is drawn"),
        )
    }

    fn assert_viewed_on_screen(cx: &mut VisualTestContext, case: &str) -> Bounds<Pixels> {
        let (diff, path, viewed) = header(cx);
        assert!(
            viewed.left() >= diff.left() && viewed.right() <= diff.right(),
            "{case}: Viewed {viewed:?} is outside the diff {diff:?}"
        );
        // Clear of the edge's fade and of the vertical scrollbar.
        assert!(
            diff.right() - viewed.right() >= px(12.),
            "{case}: Viewed {viewed:?} is under the diff's edge {diff:?}"
        );
        assert!(
            path.left() >= diff.left() && path.right() <= viewed.left(),
            "{case}: the path {path:?} runs into Viewed {viewed:?}"
        );
        viewed
    }

    #[gpui_kit::test]
    fn viewed_stays_on_screen_beside_a_long_path(cx: &mut TestAppContext) {
        let line = "x".repeat(400);
        let (mut cx, tab) = open_files_tab_with(
            cx,
            ChangedFile {
                path: LONG_PATH.into(),
                previous_path: None,
                status: FileStatus::Modified,
                additions: 1,
                deletions: 1,
                patch: Some(format!("@@ -1 +1 @@\n-old\n+{line}")),
                viewed: ViewedState::Unviewed,
            },
        );
        let cx = &mut cx;

        // Lines scroll sideways: the header is as wide as the long line.
        let before = assert_viewed_on_screen(cx, "lines scroll");
        let (diff, _, _) = header(cx);
        dispatch(
            cx,
            ScrollEvent {
                position: point(diff.left() + px(200.), diff.top() + px(120.)),
                delta: ScrollDelta::Pixels(point(px(-300.), px(0.))),
                ..ScrollEvent::default()
            }
            .to_platform_input(),
        );
        let after = assert_viewed_on_screen(cx, "scrolled sideways");
        assert_eq!(after, before, "Viewed stays put while the lines scroll");

        click(cx, after.center());
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.files[0].viewed),
            ViewedState::Viewed,
            "a click on Viewed marks the file"
        );

        cx.update(|_, cx| update_settings(cx, |settings| settings.diff_wrap = true));
        assert_viewed_on_screen(cx, "lines wrap");
    }
}
