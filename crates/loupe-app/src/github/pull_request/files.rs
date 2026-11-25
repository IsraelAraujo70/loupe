//! The files tab of the pull request view: a folder tree of the changed
//! files beside a virtualized diff, with existing review threads and new inline comments drawn as diff
//! annotations.

mod composer;
mod emphasis;
mod expand;
mod header;
mod model;
mod on_screen;
mod search;
mod sticky;
mod tree;
mod tree_view;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use chrono::{Local, Utc};

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffDecoration, DiffFile, DiffFileStatus, DiffHoverHighlight,
    DiffLinePosition, DiffLineRange, DiffState,
};
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, Search};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, Theme, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::diff::{Hunk, anchor_for_range, find_line, git_patch, parse_patch};
use loupe_github::{
    ChangedFile, CommitFiles, NodeId, PullRequestDetail, ViewedFailure, ViewedState,
};

use self::composer::{ComposerEvent, ComposerTarget, ComposerView};
use self::emphasis::{Emphasis, SharedEmphasis, SyntaxCache};
use self::expand::{Direction, Gap, Source};
use self::model::{
    LoadedFiles, Missing, PlacedThread, Placement, anchor_error_message, build_files, diff_mode,
    file_doc, is_viewed, place_threads, restore_viewed, set_viewed, split_path, status_letter,
    to_diff_side, to_side, viewed_counts,
};
use self::on_screen::{CardWidth, on_screen};
use self::search::{Match, Needle};
use self::sticky::HeaderWatch;
use self::tree::{
    GitTint, Move, RowKind, TreeRow, ViewedMark, active_folder, ancestors, distinct_starts,
    file_tint, folder_tint, into_tree_order, move_down, move_left, move_right, move_up, tree_rows,
    viewed_mark,
};
use self::tree_view::{RowView, StickyFolders};
use super::tab::{PrTabContext, PrTabEvent};
use super::thread::{HeadRef, ThreadEvent, ThreadView};

pub(super) use self::model::resolved_summary;
use crate::github::common::{self, LoadError, Loadable};
use crate::github::{live, widgets};
use crate::store::{DiffLayout, FileTreePosition, Settings, update_settings};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::ControlSize as _;
use crate::{assets, ui};

/// The files tree's starting width and the range its edge drags within.
const FILE_LIST_WIDTH: f32 = 340.;
/// A diff line longer than this runs past the panel at its default width.
pub(super) const LONG_LINE: usize = 100;
/// What the toolbar's note about long lines explains.
const LONG_LINES_HINT: &str = "These files have lines longer than the view. Scroll sideways with Shift + wheel or ← →, or choose Wrap.";
/// What the toolbar's note about diffs GitHub left out explains.
const OMITTED_HINT: &str = "GitHub leaves some diffs out of a pull request this large. Their line counts are included; choose Load diff on a file, or Load all, to see them.";
/// The same note and hint for one commit.
const COMMIT_OMITTED_NOTE: &str = "GitHub left this diff out because the commit is too large.";
const COMMIT_OMITTED_HINT: &str = "GitHub leaves some diffs out of a commit this large. Their line counts are included; choose Load diff on a file, or Load all, to see them.";
const FILE_LIST_MIN: f32 = 220.;
const FILE_LIST_MAX: f32 = 640.;
/// How wide the diff's trailing edge fades while long lines run past it.
pub(super) const FADE_WIDTH: f32 = 32.;

/// What the find bar's note about diffs GitHub left out explains.
const NOT_SEARCHED_HINT: &str = "GitHub left these diffs out of a pull request this large, so their lines are not searched. Choose Load all to search them too.";
const COMMIT_NOT_SEARCHED_HINT: &str = "GitHub left these diffs out of a commit this large, so their lines are not searched. Choose Load all to search them too.";
/// How many left-out diffs Load all reads at once, so a large pull request
/// does not send GitHub a burst of calls.
const LOAD_ALL_AT_ONCE: usize = 4;

gpui_kit::actions!(
    github,
    [
        /// Cmd+F (Ctrl+F off macOS) in a pull request's or a commit's files:
        /// find a text in the diff.
        FindInDiff
    ]
);

/// The note under a file with no diff, and for a diff GitHub left out, the
/// path to load and whether it is loading.
type NoDiffNote = (ElementId, SharedString, Option<(String, bool)>);

/// Why Load all stopped: a short line for the toolbar and the whole reason.
fn stopped_by(error: &LoadError) -> (SharedString, SharedString) {
    (error.title().into(), error.message().into())
}

/// What a file's header in the diff shows.
type HeaderFor = Rc<dyn Fn(&DiffFile) -> header::HeaderContent>;

struct ThreadSlot {
    view: Entity<ThreadView>,
    _events: Subscription,
}

struct ComposerSlot {
    view: Entity<ComposerView>,
    _events: Subscription,
}

/// The lines a file shows between its hunks beyond its patch.
#[derive(Default)]
struct Expansion {
    /// The file at the commit the diff shows, once read.
    source: Option<Arc<Source>>,
    /// The new-side lines revealed, in the order asked.
    revealed: Vec<Range<u32>>,
    /// The patch's hunks with the revealed lines merged in.
    shown: Option<Arc<Vec<Hunk>>>,
    /// The read of the file under way, and the gap that asked for it.
    read: Option<(Task<()>, u32)>,
    /// Why the file could not be read, and the gap that asked.
    error: Option<(u32, String)>,
}

/// One gap between hunks and what its controls show.
struct GapBar {
    path: String,
    gap: Gap,
    busy: bool,
    error: Option<SharedString>,
}

/// The find bar over the diff and the lines it found.
struct Find {
    input: Entity<InputState>,
    _events: Subscription,
    open: bool,
    /// The text the matches are for.
    query: SharedString,
    /// The text the diff's highlighter marks: the query while the bar is
    /// open, else nothing.
    marked: SharedString,
    matches: Vec<Match>,
    current: Option<usize>,
}

/// The one commit a tab shows instead of the whole pull request.
struct CommitSource {
    oid: String,
    /// Its first parent, once its files load.
    parent: Option<String>,
}

/// Load all: every diff GitHub left out, read a few at a time and put in the
/// viewer together once the last one arrives, so the diff moves only once.
struct LoadAll {
    /// Every path the run loads, to show their Load diff as busy.
    paths: HashSet<String>,
    /// Paths not asked for yet, in the tree's order.
    queue: VecDeque<String>,
    /// The reads under way, by path.
    reading: HashMap<String, Task<()>>,
    /// The diffs read so far (`None` for a binary file).
    read: Vec<(String, Option<String>)>,
    /// The commit the diffs are read against, once GitHub said which.
    merge_base: Option<String>,
    _merge_base: Task<()>,
    started: Instant,
}

impl LoadAll {
    /// How many diffs the run has read or given up on.
    fn done(&self) -> usize {
        self.paths.len() - self.queue.len() - self.reading.len()
    }
}

pub struct FilesTab {
    context: PrTabContext,
    commit: Option<CommitSource>,
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
    /// Diffs GitHub left out that are being loaded, by path.
    patch_loads: HashMap<String, Task<()>>,
    /// Why loading a left-out diff failed, by path.
    patch_errors: HashMap<String, String>,
    /// Load all, while it runs.
    load_all: Option<LoadAll>,
    /// Why the last Load all stopped early: a short line and the whole reason.
    load_all_error: Option<(SharedString, SharedString)>,
    /// The lines revealed between hunks, by path.
    expansions: HashMap<String, Expansion>,
    diff: Option<Entity<DiffState>>,
    /// The changed words the diff marks, read by its highlighter.
    emphasis: SharedEmphasis,
    /// The syntax colors the highlighter worked out, kept for the next one.
    syntax: SyntaxCache,
    /// The syntax colors the diff's highlighter was made with.
    syntax_theme: Arc<HighlightTheme>,
    _theme_changes: Subscription,
    /// Where the diff drew its file headers, to tell the file at its top.
    headers: Rc<RefCell<HeaderWatch>>,
    /// How wide the threads and the composer are in the diff on screen.
    card_width: CardWidth,
    layout: DiffLayout,
    viewed_error: Option<String>,
    viewed_pending: HashSet<String>,
    viewed_tasks: HashMap<usize, Task<()>>,
    viewed_writes: usize,
    placed: Vec<PlacedThread>,
    threads: HashMap<NodeId, ThreadSlot>,
    threads_dirty: bool,
    composer: Option<ComposerSlot>,
    pending_reveal: Option<(String, Option<u32>, loupe_github::Side)>,
    /// The tab's own focus, so Find reaches it while nothing in it is focused.
    focus: FocusHandle,
    find: Find,
}

impl FilesTab {
    pub fn new(context: PrTabContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(context, None, None, window, cx)
    }

    /// The tab for what commit `oid` of the pull request changes against its
    /// first parent. Its Viewed marks are the pull request's, as on GitHub;
    /// it takes no threads or new comments, which belong to the pull
    /// request's whole diff. `find`, the text of the find bar open in the
    /// commit shown before, opens the bar with it.
    pub fn for_commit(
        context: PrTabContext,
        oid: String,
        find: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let commit = CommitSource { oid, parent: None };
        Self::build(context, Some(commit), find, window, cx)
    }

    fn build(
        context: PrTabContext,
        commit: Option<CommitSource>,
        find: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let filter_changes = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let find_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Find in diff");
            if let Some(text) = &find {
                input.set_value(text.clone(), window, cx);
            }
            input
        });
        let find_events =
            cx.subscribe(&find_input, |this, _, event: &InputEvent, cx| match event {
                // Enter also reports a change; only a new text starts over.
                InputEvent::Change => {
                    if this.find.input.read(cx).value() != this.find.query {
                        this.run_find(true, cx)
                    }
                }
                InputEvent::PressEnter { shift, .. } => {
                    this.step_find(if *shift { -1 } else { 1 }, cx)
                }
                _ => {}
            });
        let mut tab = Self {
            context,
            commit,
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
            patch_loads: HashMap::new(),
            patch_errors: HashMap::new(),
            load_all: None,
            load_all_error: None,
            expansions: HashMap::new(),
            diff: None,
            emphasis: SharedEmphasis::default(),
            syntax: SyntaxCache::default(),
            syntax_theme: cx.theme().highlight_theme.clone(),
            _theme_changes: cx.observe_global::<Theme>(Self::theme_changed),
            headers: Rc::default(),
            card_width: CardWidth::default(),
            layout: Settings::get(cx).diff_layout,
            viewed_error: None,
            viewed_pending: HashSet::new(),
            viewed_tasks: HashMap::new(),
            viewed_writes: 0,
            placed: Vec::new(),
            threads: HashMap::new(),
            threads_dirty: false,
            composer: None,
            pending_reveal: None,
            focus: cx.focus_handle(),
            find: Find {
                input: find_input,
                _events: find_events,
                open: find.is_some(),
                query: SharedString::default(),
                marked: find.unwrap_or_default(),
                matches: Vec::new(),
                current: None,
            },
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
        // A commit's changes stay what they are whatever the pull request
        // does next.
        if self.commit.is_some() {
            self.context.detail = detail;
            if matches!(self.load, Loadable::Failed(_)) {
                self.load_files(cx);
            }
            cx.notify();
            return;
        }
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
            self.patch_loads.clear();
            self.patch_errors.clear();
            self.load_all = None;
            self.load_all_error = None;
            self.expansions.clear();
            self.diff = None;
        }
        if head_changed || matches!(self.load, Loadable::Failed(_)) {
            self.load_files(cx);
        }
        self.sync_threads(window, cx);
        cx.notify();
    }

    fn load_files(&mut self, cx: &mut Context<Self>) {
        if self.commit.is_some() {
            self.load_commit_files(cx);
            return;
        }
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
                        let current = gh.pull_request_head(&pr)?;
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

    /// Loads the commit's files, kept by commit since they never change, and
    /// reads their viewed marks in the pull request every time.
    fn load_commit_files(&mut self, cx: &mut Context<Self>) {
        let Some(oid) = self.commit.as_ref().map(|commit| commit.oid.clone()) else {
            return;
        };
        let pr = self.context.pr.clone();
        let key = crate::cache::github_key(cx, &format!("commit-files/{}/{oid}", pr.repo.url()));
        self.load.start_refresh();
        let read_key = key.clone();
        self._load = Some(common::run(
            cx,
            move |gh| {
                let mut commit = match read_key
                    .as_deref()
                    .and_then(crate::cache::get::<CommitFiles>)
                {
                    Some(commit) => commit,
                    None => gh.commit_files(&pr.repo, &oid)?,
                };
                gh.merge_viewed_states(&pr, &mut commit.files)?;
                let loaded =
                    build_files(into_tree_order(commit.files.clone()), git_patch, |patch| {
                        parse_patch(patch).ok()
                    });
                Ok((commit, loaded))
            },
            move |this, result, cx| {
                let loaded = result.map(|(commit, loaded)| {
                    if let Some(key) = &key {
                        crate::cache::put(key, &commit);
                    }
                    if let Some(source) = &mut this.commit {
                        source.parent = commit.parent_oid;
                    }
                    loaded
                });
                this.finish_load(loaded, cx);
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
        let first_load = self.diff.is_none();
        self.files = loaded.files;
        eprintln!(
            "review-files-ready pr={} files={} timestamp={}",
            self.context.pr.url(),
            self.files.len(),
            chrono::Utc::now().to_rfc3339()
        );
        self.hunks = loaded.hunks;
        self.missing = loaded.missing;
        self.expansions.clear();
        self.syntax
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.refresh_emphasis();
        let mode = diff_mode(self.layout);
        match &self.diff {
            Some(state) => state.update(cx, |state, cx| state.set_files(loaded.docs, cx)),
            None => {
                let highlighter = self.highlighter(cx);
                // Every line given is shown: the lines revealed between
                // hunks are context the component would fold away again.
                self.diff = Some(cx.new(|cx| {
                    DiffState::new(loaded.docs, cx)
                        .with_mode(mode)
                        .with_context_lines(None)
                        .with_inline_unit(None)
                        .with_syntax_highlighter(highlighter)
                }));
            }
        }
        self.load_all = None;
        self.load_all_error = None;
        self.collapse_viewed(cx);
        // Collapsing a file keeps the line at the top of the diff in place,
        // and at the very top that is the first file's first line, under its
        // header and its hunk's: open on the header instead.
        if let Some(state) = &self.diff {
            state.update(cx, |state, cx| {
                if let Some(first) = state.files().first().map(|file| file.path().clone()) {
                    state.scroll_to_file(&first, cx);
                }
            });
        }
        // The diff starts over on its first file before any header is drawn
        // to tell it.
        self.headers.borrow_mut().jumped_to(0);
        if let Some(slot) = &self.composer
            && !self
                .files
                .iter()
                .any(|file| file.path == slot.view.read(cx).target().path)
        {
            self.composer = None;
        }
        self.threads_dirty = true;
        let reveal = self.pending_reveal.take();
        // A find carried from the commit shown before goes to its first match.
        let jump = first_load && reveal.is_none();
        if let Some((path, line, side)) = reveal {
            self.apply_reveal(&path, line, side, cx);
        }
        self.run_find(jump, cx);
    }

    /// Collapses viewed files, and files whose diff GitHub left out so the
    /// header stands alone with its Load diff note. A commit keeps viewed
    /// files open: one viewed in the pull request may change again in it.
    fn collapse_viewed(&self, cx: &mut Context<Self>) {
        let Some(state) = &self.diff else { return };
        let collapse_viewed = self.commit.is_none();
        state.update(cx, |state, cx| {
            for file in &self.files {
                let omitted = self.missing.get(&file.path) == Some(&Missing::Omitted);
                let viewed = collapse_viewed && is_viewed(file.viewed);
                state.set_file_collapsed(&file.path, viewed || omitted, cx);
            }
        });
    }

    /// Whether a Viewed mark was set here, so another view of the same pull
    /// request should read the marks again.
    pub fn wrote_viewed(&self) -> bool {
        self.viewed_writes > 0
    }

    /// Reads the viewed marks again after another view of the pull request
    /// changed them, and collapses or opens the files whose mark changed.
    pub fn refresh_viewed(&mut self, cx: &mut Context<Self>) {
        if self.commit.is_some() || self.load.value().is_none() || self.load.is_busy() {
            return;
        }
        let pr = self.context.pr.clone();
        let mut files = self.files.clone();
        let expected_head = self.context.detail.head_oid.clone();
        self._load = Some(common::run(
            cx,
            move |gh| {
                gh.merge_viewed_states(&pr, &mut files)?;
                Ok(files)
            },
            move |this, result, cx| {
                let Ok(files) = result else { return };
                if this.context.detail.head_oid != expected_head {
                    return;
                }
                for fresh in files {
                    let viewed = is_viewed(fresh.viewed);
                    let changed = this.files.iter_mut().find(|file| {
                        file.path == fresh.path
                            && is_viewed(file.viewed) != viewed
                            && !this.viewed_pending.contains(&file.path)
                    });
                    if let Some(file) = changed {
                        file.viewed = fresh.viewed;
                        this.set_collapsed(&fresh.path, viewed, cx);
                    }
                }
            },
        ));
    }

    /// Loads the diff GitHub left out of `path` and puts it in the viewer in
    /// place of the file's placeholder.
    fn load_omitted_patch(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.loading_patch(path) {
            return;
        }
        let Some(file) = self.files.iter().find(|file| file.path == path).cloned() else {
            return;
        };
        self.patch_errors.remove(path);
        let pr = self.context.pr.clone();
        let (base, head) = self.diff_ends();
        let follows_head = self.commit.is_none();
        let expected_head = self.context.detail.head_oid.clone();
        let owned_path = path.to_owned();
        let task = common::run(
            cx,
            move |gh| gh.omitted_patch(&pr, &base, &head, &file),
            move |this, result, cx| {
                this.patch_loads.remove(&owned_path);
                if follows_head && this.context.detail.head_oid != expected_head {
                    return;
                }
                match result {
                    Ok(patch) => this.apply_omitted_patch(&owned_path, patch, cx),
                    Err(error) => {
                        this.patch_errors.insert(owned_path, error.message());
                    }
                }
            },
        );
        self.patch_loads.insert(path.to_owned(), task);
        cx.notify();
    }

    /// The commits the diff runs between. A commit's diff is against its
    /// first parent; a root commit only adds files, so it needs no older side.
    fn diff_ends(&self) -> (String, String) {
        match &self.commit {
            Some(commit) => (
                commit.parent.clone().unwrap_or_else(|| commit.oid.clone()),
                commit.oid.clone(),
            ),
            None => (
                self.context.detail.base_oid.clone(),
                self.context.detail.head_oid.clone(),
            ),
        }
    }

    /// Whether the diff GitHub left out of `path` is being loaded, on its
    /// own or by Load all.
    fn loading_patch(&self, path: &str) -> bool {
        self.patch_loads.contains_key(path)
            || self
                .load_all
                .as_ref()
                .is_some_and(|run| run.paths.contains(path))
    }

    /// The diffs GitHub left out that nothing is loading yet, in the tree's
    /// order.
    fn unloaded_patches(&self) -> VecDeque<String> {
        self.files
            .iter()
            .filter(|file| {
                self.missing.get(&file.path) == Some(&Missing::Omitted)
                    && !self.loading_patch(&file.path)
            })
            .map(|file| file.path.clone())
            .collect()
    }

    /// Loads every diff GitHub left out: one call for the merge base they
    /// share, then each file's versions, `LOAD_ALL_AT_ONCE` files at a time.
    fn load_all_patches(&mut self, cx: &mut Context<Self>) {
        if self.load_all.is_some() {
            return;
        }
        let queue = self.unloaded_patches();
        if queue.is_empty() {
            return;
        }
        for path in &queue {
            self.patch_errors.remove(path);
        }
        self.load_all_error = None;
        eprintln!("diff-load-all start files={}", queue.len());
        let repo = self.context.pr.repo.clone();
        let (base, head) = self.diff_ends();
        let merge_base = common::run(
            cx,
            move |gh| gh.merge_base(&repo, &base, &head),
            |this, result, cx| match result {
                Ok(merge_base) => {
                    if let Some(run) = &mut this.load_all {
                        run.merge_base = Some(merge_base);
                    }
                    this.next_patches(cx);
                }
                Err(error) => this.stop_load_all(Some(stopped_by(&error)), cx),
            },
        );
        self.load_all = Some(LoadAll {
            paths: queue.iter().cloned().collect(),
            queue,
            reading: HashMap::new(),
            read: Vec::new(),
            merge_base: None,
            _merge_base: merge_base,
            started: Instant::now(),
        });
        cx.notify();
    }

    /// Asks for the next diffs until `LOAD_ALL_AT_ONCE` are under way, and
    /// puts them all in the viewer once none is left. Asks for no more while
    /// GitHub's rate limit runs low, so the rest of the app keeps working.
    fn next_patches(&mut self, cx: &mut Context<Self>) {
        let paused = common::client(cx)
            .and_then(|github| live::paused_until(github.rate_limits(), Utc::now()));
        let (_, head) = self.diff_ends();
        let Some(run) = &mut self.load_all else {
            return;
        };
        let Some(merge_base) = run.merge_base.clone() else {
            return;
        };
        if let Some(until) = paused
            && !run.queue.is_empty()
        {
            run.queue.clear();
            let at = until.with_timezone(&Local).format("%H:%M");
            self.load_all_error = Some((
                format!("Paused until {at}").into(),
                format!(
                    "GitHub's rate limit is running low, so Load all stopped to leave calls for the rest of the app. It fills up again at {at}."
                )
                .into(),
            ));
        }
        while run.reading.len() < LOAD_ALL_AT_ONCE
            && let Some(path) = run.queue.pop_front()
        {
            let Some(file) = self.files.iter().find(|file| file.path == path).cloned() else {
                continue;
            };
            let repo = self.context.pr.repo.clone();
            let (merge_base, head, read_path) = (merge_base.clone(), head.clone(), path.clone());
            let task = common::run(
                cx,
                move |gh| gh.patch_since(&repo, &merge_base, &head, &file),
                move |this, result, cx| this.patch_read(read_path, result, cx),
            );
            run.reading.insert(path, task);
        }
        if run.queue.is_empty() && run.reading.is_empty() {
            self.finish_load_all(cx);
        }
        cx.notify();
    }

    /// One of Load all's diffs arrived, or failed. A file GitHub does not
    /// have keeps its error and the rest go on; any other failure (offline,
    /// the token, the rate limit) would fail the rest too, so the run stops.
    fn patch_read(
        &mut self,
        path: String,
        result: Result<Option<String>, LoadError>,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = &mut self.load_all else {
            return;
        };
        run.reading.remove(&path);
        match result {
            Ok(patch) => run.read.push((path, patch)),
            Err(error) => {
                if error != LoadError::NotFound {
                    run.queue.clear();
                    self.load_all_error = Some(stopped_by(&error));
                }
                self.patch_errors.insert(path, error.message());
            }
        }
        self.next_patches(cx);
    }

    /// Stops Load all, keeping the diffs it read, and says why when `why`
    /// is given.
    fn stop_load_all(&mut self, why: Option<(SharedString, SharedString)>, cx: &mut Context<Self>) {
        if why.is_some() {
            self.load_all_error = why;
        }
        if let Some(run) = &mut self.load_all {
            run.queue.clear();
            run.reading.clear();
        }
        self.finish_load_all(cx);
    }

    /// Puts what Load all read in the viewer in one go, keeping the file at
    /// the top of the diff where it is.
    fn finish_load_all(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.load_all.take() else {
            return;
        };
        let top = self
            .top_file(cx)
            .or(Some(0))
            .and_then(|ix| self.files.get(ix))
            .map(|file| file.path.clone());
        let (loaded, asked) = (run.read.len(), run.paths.len());
        let applied = Instant::now();
        self.apply_omitted_patches(run.read, top, cx);
        eprintln!(
            "diff-load-all done loaded={loaded} of={asked} ms={} apply_ms={}",
            run.started.elapsed().as_millis(),
            applied.elapsed().as_millis()
        );
        cx.notify();
    }

    /// Puts a loaded diff (`None` for a binary file) in the viewer, opens the
    /// file, and keeps it on screen.
    fn apply_omitted_patch(&mut self, path: &str, patch: Option<String>, cx: &mut Context<Self>) {
        self.apply_omitted_patches(vec![(path.to_owned(), patch)], Some(path.to_owned()), cx);
    }

    /// Puts loaded diffs (`None` for a binary file) in the viewer with one
    /// change of its files, opens those not viewed, and shows the header of
    /// `scroll_to`.
    fn apply_omitted_patches(
        &mut self,
        patches: Vec<(String, Option<String>)>,
        scroll_to: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut docs = HashMap::new();
        for (path, patch) in patches {
            if let Some(doc) = self.take_omitted_patch(&path, patch) {
                docs.insert(path, doc);
            }
        }
        self.refresh_emphasis();
        if let Some(state) = &self.diff
            && !docs.is_empty()
        {
            state.update(cx, |state, cx| {
                let files = state
                    .files()
                    .iter()
                    .map(|existing| match docs.get(existing.path().as_ref()) {
                        Some((doc, _)) => doc.clone(),
                        None => existing.clone(),
                    })
                    .collect::<Vec<_>>();
                state.set_files(files, cx);
                for (path, (_, viewed)) in &docs {
                    state.set_file_collapsed(path, *viewed, cx);
                }
                if let Some(path) = &scroll_to {
                    state.scroll_to_file(path, cx);
                }
            });
        }
        self.threads_dirty = true;
        self.run_find(false, cx);
    }

    /// Takes the diff loaded for `path` into the files, its hunks, and what
    /// is missing, and returns the file for the viewer with whether it is
    /// viewed.
    fn take_omitted_patch(
        &mut self,
        path: &str,
        patch: Option<String>,
    ) -> Option<(DiffFile, bool)> {
        self.expansions.remove(path);
        let file = self.files.iter_mut().find(|file| file.path == path)?;
        file.patch = patch;
        let viewed = is_viewed(file.viewed);
        let file = file.clone();
        let (doc, reason) = match git_patch(&file) {
            Some(patch) => file_doc(&file, Some(patch)),
            // Binary: GitHub's counts say lines changed, but there is no
            // text to show, so the placeholder says binary.
            None => file_doc(
                &ChangedFile {
                    additions: 0,
                    deletions: 0,
                    ..file.clone()
                },
                None,
            ),
        };
        match reason {
            Some(reason) => {
                self.missing.insert(file.path.clone(), reason);
            }
            None => {
                self.missing.remove(&file.path);
            }
        }
        if reason.is_none()
            && let Some(hunks) = file
                .patch
                .as_deref()
                .and_then(|patch| parse_patch(patch).ok())
        {
            self.hunks.insert(file.path.clone(), Arc::new(hunks));
        }
        doc.map(|doc| (doc, viewed))
    }

    /// Gives the diff's highlighter the hunks the diff is about to show.
    fn refresh_emphasis(&self) {
        let shown = Emphasis::new(
            self.files
                .iter()
                .filter_map(|file| self.shown_hunks(&file.path).cloned()),
        );
        *self
            .emphasis
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(shown);
    }

    /// The diff's highlighter, in the current theme's syntax colors, the
    /// component's own wash for changed words, and the find's mark over the
    /// text found.
    fn highlighter(
        &self,
        cx: &App,
    ) -> impl Fn(&str, &str) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync + 'static {
        let theme = cx.theme();
        let words = emphasis::highlighter(
            self.emphasis.clone(),
            self.syntax.clone(),
            theme.highlight_theme.clone(),
            theme.success.opacity(0.3),
            theme.danger.opacity(0.3),
        );
        let needle = Needle::new(&self.find.marked);
        let found = HighlightStyle {
            background_color: Some(theme.warning.opacity(0.4)),
            ..Default::default()
        };
        move |language, text| {
            let mut styles = words(language, text);
            if let Some(needle) = &needle {
                styles.extend(
                    search::marks(needle, text)
                        .into_iter()
                        .map(|range| (range, found)),
                );
            }
            styles
        }
    }

    /// The highlighter keeps the styles it returned, so a new palette needs a
    /// new one; a change of zoom or font keeps the same syntax colors.
    fn theme_changed(&mut self, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&self.syntax_theme, &cx.theme().highlight_theme) {
            return;
        }
        self.syntax_theme = cx.theme().highlight_theme.clone();
        self.syntax = SyntaxCache::default();
        if let Some(state) = self.diff.clone() {
            let highlighter = self.highlighter(cx);
            state.update(cx, |state, cx| {
                state.set_syntax_highlighter(highlighter, cx)
            });
        }
    }

    /// The hunks the diff shows for `path`: its patch's, with any lines
    /// revealed between them.
    fn shown_hunks(&self, path: &str) -> Option<&Arc<Vec<Hunk>>> {
        self.expansions
            .get(path)
            .and_then(|expansion| expansion.shown.as_ref())
            .or_else(|| self.hunks.get(path))
    }

    /// Reveals what `direction` asks of the gap at new-side line `start` in
    /// `path`, reading the file at the commit the diff shows the first time.
    /// The text is kept by commit and path, which never change.
    fn expand_gap(&mut self, path: &str, start: u32, direction: Direction, cx: &mut Context<Self>) {
        if !self.hunks.contains_key(path) {
            return;
        }
        match self.expansions.get(path) {
            Some(expansion) if expansion.source.is_some() => {
                return self.reveal_gap(path, start, direction, cx);
            }
            Some(expansion) if expansion.read.is_some() => return,
            _ => {}
        }
        let repo = self.context.pr.repo.clone();
        let sha = match &self.commit {
            Some(commit) => commit.oid.clone(),
            None => self.context.detail.head_oid.clone(),
        };
        let key = crate::cache::github_key(cx, &format!("file-text/{}/{sha}/{path}", repo.url()));
        let (read_path, owned_path) = (path.to_owned(), path.to_owned());
        let task = common::run(
            cx,
            move |gh| {
                if let Some(text) = key.as_deref().and_then(crate::cache::get::<String>) {
                    return Ok(text);
                }
                let bytes = gh.raw_contents(&repo, &read_path, &sha)?;
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if let Some(key) = &key {
                    crate::cache::put(key, &text);
                }
                Ok(text)
            },
            move |this, result, cx| {
                let Some(expansion) = this.expansions.get_mut(&owned_path) else {
                    return;
                };
                expansion.read = None;
                match result {
                    Ok(text) => {
                        expansion.source = Some(Arc::new(Source::new(&text)));
                        this.reveal_gap(&owned_path, start, direction, cx);
                    }
                    Err(error) => expansion.error = Some((start, error.message())),
                }
            },
        );
        let expansion = self.expansions.entry(path.to_owned()).or_default();
        expansion.error = None;
        expansion.read = Some((task, start));
        cx.notify();
    }

    /// Reveals what `direction` asks of the gap at new-side line `start` in
    /// `path` from the file read, and scrolls the revealed lines under the
    /// pinned header.
    fn reveal_gap(&mut self, path: &str, start: u32, direction: Direction, cx: &mut Context<Self>) {
        let Some(hunks) = self.hunks.get(path).cloned() else {
            return;
        };
        let Some(file) = self.files.iter().find(|file| file.path == path).cloned() else {
            return;
        };
        let Some(expansion) = self.expansions.get_mut(path) else {
            return;
        };
        let Some(source) = expansion.source.clone() else {
            return;
        };
        let current = expansion.shown.clone().unwrap_or_else(|| hunks.clone());
        let Some(gap) = expand::gaps(&current, Some(source.line_count()))
            .into_iter()
            .find(|gap| gap.start == start)
        else {
            return;
        };
        let lines = gap.reveal(direction);
        expansion.error = None;
        expansion.revealed.push(lines.clone());
        let shown = Arc::new(expand::expanded(&hunks, &source, &expansion.revealed));
        expansion.shown = Some(shown.clone());
        let file = ChangedFile {
            patch: Some(expand::patch_text(&shown)),
            ..file
        };
        let (Some(doc), None) = file_doc(&file, git_patch(&file)) else {
            return;
        };
        self.refresh_emphasis();
        let lead = expand::lead(&shown, lines.start, search::LEAD);
        if let Some(state) = &self.diff {
            let headers = self.headers.clone();
            state.update(cx, |state, cx| {
                let docs = state
                    .files()
                    .iter()
                    .map(|existing| {
                        if existing.path().as_ref() == path {
                            doc.clone()
                        } else {
                            existing.clone()
                        }
                    })
                    .collect::<Vec<_>>();
                state.set_files(docs, cx);
                if let Some(ix) = state
                    .files()
                    .iter()
                    .position(|file| file.path().as_ref() == path)
                {
                    headers.borrow_mut().jumped_to(ix);
                }
                match lead {
                    Some((line, side)) => state.scroll_to_line(
                        DiffLinePosition::new(path.to_owned(), to_diff_side(side), line as usize),
                        cx,
                    ),
                    None => state.scroll_to_file(path, cx),
                }
            });
        }
        self.run_find(false, cx);
        cx.notify();
    }

    /// Places the pull request's threads in the diff and keeps one entity per
    /// thread, so drafts and expanded state survive a reload.
    fn sync_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.threads_dirty = false;
        if self.load.value().is_none() || self.commit.is_some() {
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

    /// Selects `path` in the file tree and scrolls the diff to `line` on `side`
    /// (falling back to the file header when `line` is not in a diff hunk). If
    /// the diff is still loading, queues the reveal until files land.
    pub fn reveal_line(
        &mut self,
        path: &str,
        line: Option<u32>,
        side: loupe_github::Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.filter.read(cx).value().is_empty() {
            self.filter
                .update(cx, |filter, cx| filter.set_value("", window, cx));
        }
        if self.diff.is_none() {
            self.pending_reveal = Some((path.to_owned(), line, side));
            cx.notify();
            return;
        }
        self.apply_reveal(path, line, side, cx);
        cx.notify();
    }

    fn apply_reveal(
        &mut self,
        path: &str,
        line: Option<u32>,
        side: loupe_github::Side,
        cx: &mut Context<Self>,
    ) {
        // Expand every parent folder so the file is visible in the tree.
        let mut cursor = path;
        while let Some((dir, _)) = cursor.rsplit_once('/') {
            self.collapsed.remove(dir);
            cursor = dir;
        }
        self.selected = Some(path.to_owned());
        let rows = self.current_rows(cx);
        if let Some(ix) = rows.iter().position(|row| row.path == path) {
            self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        let Some(state) = &self.diff else {
            return;
        };
        let has_exact_line = line.is_some_and(|line_no| {
            self.hunks
                .get(path)
                .is_some_and(|hunks| find_line(hunks, line_no, side).is_some())
        });
        let headers = self.headers.clone();
        state.update(cx, |state, cx| {
            state.set_file_collapsed(path, false, cx);
            if let Some(line_no) = line.filter(|_| has_exact_line) {
                // The line may sit far from its file's header.
                if let Some(file) = state
                    .files()
                    .iter()
                    .position(|file| file.path().as_ref() == path)
                {
                    headers.borrow_mut().jumped_to(file);
                }
                state.scroll_to_line(
                    DiffLinePosition::new(path.to_owned(), to_diff_side(side), line_no as usize),
                    cx,
                );
            } else {
                state.scroll_to_file(path, cx);
            }
        });
    }

    /// Opens the find bar, or takes the focus back to it, with its text
    /// selected to type over.
    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diff.is_none() {
            return;
        }
        let reopened = !self.find.open;
        self.find.open = true;
        self.find.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        if reopened {
            self.run_find(true, cx);
        }
        cx.notify();
    }

    /// Closes the find bar and its marks; its text stays for the next time.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = false;
        self.find.matches.clear();
        self.find.current = None;
        self.mark_found(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The find bar's text while it is open, for the commit shown next.
    pub fn open_find_text(&self, cx: &App) -> Option<SharedString> {
        self.find.open.then(|| self.find.input.read(cx).value())
    }

    /// Gives the diff a highlighter that marks the text to find, when that
    /// text changed: the query while the bar is open, else none. A new
    /// highlighter paints every file again.
    fn mark_found(&mut self, cx: &mut Context<Self>) {
        let marked = if self.find.open {
            self.find.query.clone()
        } else {
            SharedString::default()
        };
        if marked == self.find.marked {
            return;
        }
        self.find.marked = marked;
        if let Some(state) = self.diff.clone() {
            let highlighter = self.highlighter(cx);
            state.update(cx, |state, cx| {
                state.set_syntax_highlighter(highlighter, cx)
            });
        }
    }

    /// Finds the bar's text in the diffs loaded. A new text goes to its first
    /// match from the file on screen; a reload or a diff loaded later keeps
    /// the current match where it was.
    fn run_find(&mut self, jump: bool, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        let query = self.find.input.read(cx).value();
        let started = std::time::Instant::now();
        let previous = self
            .find
            .current
            .and_then(|ix| self.find.matches.get(ix))
            .cloned();
        let files = self.files.iter().map(|file| {
            let hunks = self.shown_hunks(&file.path);
            (
                file.path.as_str(),
                hunks.map_or(&[][..], |hunks| hunks.as_slice()),
            )
        });
        self.find.matches = search::find(files, &query);
        self.find.query = query.clone();
        self.mark_found(cx);
        if !query.is_empty() {
            let lines: usize = self
                .files
                .iter()
                .filter_map(|file| self.shown_hunks(&file.path))
                .flat_map(|hunks| hunks.iter())
                .map(|hunk| hunk.lines.len())
                .sum();
            eprintln!(
                "diff-search files={} lines={lines} matches={} ms={:.2}",
                self.hunks.len(),
                self.find.matches.len(),
                started.elapsed().as_secs_f64() * 1000.
            );
        }
        let kept = previous.filter(|_| !jump).and_then(|previous| {
            self.find.matches.iter().position(|found| {
                found.path == previous.path
                    && found.line == previous.line
                    && found.side == previous.side
                    && found.range == previous.range
                    && found.in_header == previous.in_header
            })
        });
        self.find.current =
            kept.or_else(|| search::first_from(&self.find.matches, self.top_file(cx)));
        if jump && let Some(ix) = self.find.current {
            self.show_match(ix, cx);
        }
        cx.notify();
    }

    /// Goes `delta` matches on, round from the last to the first.
    fn step_find(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(ix) = search::step(self.find.matches.len(), self.find.current, delta) {
            self.show_match(ix, cx);
            cx.notify();
        }
    }

    /// Selects match `ix`'s file in the tree and scrolls the diff a few
    /// lines above the match, opening its file if it was collapsed.
    fn show_match(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(found) = self.find.matches.get(ix).cloned() else {
            return;
        };
        self.find.current = Some(ix);
        if let Some(state) = &self.diff {
            // Opens the fold the line may be in before scrolling above it.
            let position = DiffLinePosition::new(
                found.path.clone(),
                to_diff_side(found.side),
                found.line as usize,
            );
            state.update(cx, |state, cx| state.scroll_to_line(position, cx));
        }
        match found.lead {
            Some((line, side)) => self.apply_reveal(&found.path, Some(line), side, cx),
            None => self.apply_reveal(&found.path, None, found.side, cx),
        }
    }

    /// The place in `files` of the file at the top of the diff.
    fn top_file(&self, cx: &App) -> Option<usize> {
        let top = self.headers.borrow().top()?;
        let state = self.diff.as_ref()?.read(cx);
        let path = state.files().get(top)?.path();
        self.files
            .iter()
            .position(|file| file.path == path.as_ref())
    }

    /// The mark of the current match's line, on both halves of an unchanged
    /// line in a split diff; the highlighter marks the text found in every
    /// line. A match in a hunk's header has no line to mark.
    fn find_decorations(&self, cx: &App) -> Vec<DiffDecoration> {
        let Some(found) = self
            .find
            .current
            .and_then(|ix| self.find.matches.get(ix))
            .filter(|found| self.find.open && !found.in_header)
        else {
            return Vec::new();
        };
        let line = |side, line: u32| {
            DiffDecoration::new(DiffLineRange::new(
                found.path.clone(),
                to_diff_side(side),
                line as usize,
                line as usize,
            ))
            .with_background(cx.theme().warning.opacity(0.2))
        };
        let mut decorations = vec![line(found.side, found.line)];
        decorations.extend(
            found
                .old_line
                .map(|old| line(loupe_github::Side::Left, old)),
        );
        decorations
    }

    /// The bar over the diff: the text, "3 of 41", previous and next, how
    /// many files GitHub left out of the search, and Close.
    fn find_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let total = self.find.matches.len();
        let searching = !self.find.input.read(cx).value().is_empty();
        let count = match self.find.current {
            _ if !searching => None,
            Some(ix) => Some((
                format!("{} of {total}", ix + 1),
                format!("Match {} of {total}", ix + 1),
            )),
            None => Some(("No results".to_owned(), "No match in the diff".to_owned())),
        };
        let not_searched = self
            .missing
            .values()
            .filter(|reason| **reason == Missing::Omitted)
            .count();
        let hint = if self.commit.is_some() {
            COMMIT_NOT_SEARCHED_HINT
        } else {
            NOT_SEARCHED_HINT
        };
        let focus_input = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.find
                .input
                .update(cx, |input, cx| input.focus(window, cx));
        };
        ui::toolbar()
            .id("files-find")
            .debug_selector(|| "files-find".into())
            .role(Role::Search)
            .aria_label("Find in diff")
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.close_find(window, cx)))
            .child(
                Input::new(&self.find.input)
                    .small()
                    .h(rems(1.75))
                    .w(scale::search_width())
                    .text_size(scale::body())
                    .prefix(Icon::new(IconName::Search).with_size(dp(14.)))
                    .aria_label("Find in diff"),
            )
            .children(count.map(|(text, label)| {
                div()
                    .id("files-find-count")
                    .role(Role::Status)
                    .aria_label(label)
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(text)
            }))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        ui::icon_button(
                            "files-find-previous",
                            IconName::ChevronUp,
                            "Previous match",
                        )
                        .tooltip(format!("Previous match ({})", ui::shortcut("shift-enter")))
                        .disabled(total == 0)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.step_find(-1, cx);
                                focus_input(this, window, cx);
                            },
                        )),
                    )
                    .child(
                        ui::icon_button("files-find-next", IconName::ChevronDown, "Next match")
                            .tooltip(format!("Next match ({})", ui::shortcut("enter")))
                            .disabled(total == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.step_find(1, cx);
                                focus_input(this, window, cx);
                            })),
                    ),
            )
            .when(self.load_all.is_some(), |bar| {
                bar.child(self.load_all_progress("files-find-load-all-progress", cx))
            })
            .when(self.load_all.is_none() && not_searched > 0, |bar| {
                let text = match not_searched {
                    1 => "1 file not searched".to_owned(),
                    count => format!("{count} files not searched"),
                };
                bar.child(
                    div()
                        .id("files-find-not-searched")
                        .role(Role::Note)
                        .aria_label(format!("{text}. {hint}"))
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
                        .child(text),
                )
                .children(self.load_all_button("files-find-load-all", cx))
            })
            .child(div().flex_1())
            .child(
                ui::icon_button("files-find-close", IconName::Close, "Close find")
                    .tooltip(format!("Close ({})", ui::shortcut("escape")))
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
            .into_any_element()
    }

    /// Marks a file viewed or unviewed at once, collapses or expands it, and
    /// reverts with an inline error when GitHub refuses.
    fn toggle_viewed(&mut self, path: &str, viewed: bool, cx: &mut Context<Self>) {
        self.set_files_viewed(&[path.to_owned()], viewed, cx);
    }

    /// Marks `paths` viewed or unviewed on screen at once (a folder's files
    /// go through here together), sends them to GitHub in one task, and reverts the files GitHub refuses with a single
    /// inline error. Files already in that state, or with a write in flight,
    /// are skipped: dropping a task does not stop its request, so a second
    /// write could reach GitHub first and leave it opposite to the screen.
    fn set_files_viewed(&mut self, paths: &[String], viewed: bool, cx: &mut Context<Self>) {
        let mut previous: HashMap<String, ViewedState> = HashMap::new();
        for path in paths {
            if self.viewed_pending.contains(path) {
                continue;
            }
            let differs = self
                .files
                .iter()
                .any(|file| &file.path == path && is_viewed(file.viewed) != viewed);
            if differs && let Some(state) = set_viewed(&mut self.files, path, viewed) {
                previous.insert(path.clone(), state);
            }
        }
        if previous.is_empty() {
            return;
        }
        let batch: Vec<String> = paths
            .iter()
            .filter(|path| previous.contains_key(*path))
            .cloned()
            .collect();
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
        for path in &batch {
            self.set_collapsed(path, viewed, cx);
        }
        self.viewed_pending.extend(batch.iter().cloned());
        let write = self.viewed_writes;
        self.viewed_writes += 1;
        let pull_request_id = self.context.detail.id.clone();
        let expected_head = self.context.detail.head_oid.clone();
        let expected_base = self.context.detail.base_oid.clone();
        let request_paths = batch.clone();
        let task = common::run(
            cx,
            move |gh| Ok(gh.set_files_viewed(&pull_request_id, &request_paths, viewed)),
            move |this, result, cx| {
                this.viewed_tasks.remove(&write);
                for path in &batch {
                    this.viewed_pending.remove(path);
                }
                if this.context.detail.head_oid != expected_head
                    || this.context.detail.base_oid != expected_base
                {
                    return;
                }
                let failures = result.unwrap_or_else(|error| {
                    let message = error.detail();
                    batch
                        .iter()
                        .map(|path| ViewedFailure {
                            path: path.clone(),
                            message: message.clone(),
                        })
                        .collect()
                });
                for failure in &failures {
                    if let Some(state) = previous.get(&failure.path) {
                        restore_viewed(&mut this.files, &failure.path, *state);
                        this.set_collapsed(&failure.path, is_viewed(*state), cx);
                    }
                }
                if let Some(first) = failures.first() {
                    this.viewed_error = Some(if failures.len() == 1 {
                        format!(
                            "Could not update the viewed mark of {}: {}",
                            first.path, first.message
                        )
                    } else {
                        format!(
                            "Could not update the viewed mark of {} of {} files: {}",
                            failures.len(),
                            batch.len(),
                            first.message
                        )
                    });
                }
                if let Some(key) = &cache_key {
                    let failed: HashSet<&str> = failures
                        .iter()
                        .map(|failure| failure.path.as_str())
                        .collect();
                    crate::cache::update::<Vec<ChangedFile>>(key, |files| {
                        for path in batch.iter().filter(|path| !failed.contains(path.as_str())) {
                            set_viewed(files, path, viewed);
                        }
                    });
                }
            },
        );
        self.viewed_tasks.insert(write, task);
        cx.notify();
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
            match self
                .expansions
                .get(&file.path)
                .and_then(|expansion| expansion.shown.as_ref())
            {
                Some(hunks) => hunks
                    .iter()
                    .flat_map(|hunk| &hunk.lines)
                    .any(|line| line.text.chars().count() > LONG_LINE),
                None => file.patch.as_deref().is_some_and(|patch| {
                    patch.lines().any(|line| line.chars().count() > LONG_LINE)
                }),
            }
        })
    }

    /// Load all, and why the last run stopped early when it did.
    fn load_all_button(&self, id: &'static str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let error = self.load_all_error.clone().map(|(short, reason)| {
            div()
                .id(SharedString::from(format!("{id}-stopped")))
                .role(Role::Status)
                .aria_label(reason.clone())
                .flex_none()
                .text_size(scale::meta())
                .text_color(muted)
                .tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx))
                .child(short)
                .into_any_element()
        });
        error
            .into_iter()
            .chain([Button::new(id)
                .outline()
                .control()
                .label("Load all")
                .accessibility_label("Load all diffs")
                .on_click(cx.listener(|this, _, _, cx| this.load_all_patches(cx)))
                .into_any_element()])
            .collect()
    }

    /// How far Load all got: a spinner and "Loading diffs: 12 of 69".
    fn load_all_progress(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let text = self.load_all.as_ref().map_or_else(String::new, |run| {
            format!("Loading diffs: {} of {}", run.done(), run.paths.len())
        });
        h_flex()
            .id(id)
            .role(Role::Status)
            .aria_label(text.clone())
            .flex_none()
            .gap_2()
            .items_center()
            .text_size(scale::meta())
            .text_color(muted)
            .child(Spinner::new().xsmall().color(muted))
            .child(text)
            .into_any_element()
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
        let omitted = self
            .missing
            .values()
            .filter(|reason| **reason == Missing::Omitted)
            .count();
        let omitted_hint = if self.commit.is_some() {
            COMMIT_OMITTED_HINT
        } else {
            OMITTED_HINT
        };
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
        if self.load_all.is_some() {
            parts.push(self.load_all_progress("load-all-progress", cx));
            parts.push(
                Button::new("load-all-stop")
                    .outline()
                    .control()
                    .label("Stop")
                    .accessibility_label("Stop loading diffs")
                    .on_click(cx.listener(|this, _, _, cx| this.stop_load_all(None, cx)))
                    .into_any_element(),
            );
        } else if omitted > 0 {
            parts.push(
                div()
                    .id("omitted-diffs-note")
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .tooltip(move |window, cx| Tooltip::new(omitted_hint).build(window, cx))
                    .child(match omitted {
                        1 => "1 diff not loaded".to_owned(),
                        omitted => format!("{omitted} diffs not loaded"),
                    })
                    .into_any_element(),
            );
            parts.extend(self.load_all_button("load-all-diffs", cx));
        }
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
        parts.push(
            ui::icon_button("files-find-open", IconName::Search, "Find in diff")
                .tooltip(format!(
                    "Find in diff ({})",
                    ui::bound_shortcut(&FindInDiff, cx)
                ))
                .selected(self.find.open)
                .on_click(cx.listener(|this, _, window, cx| this.open_find(window, cx)))
                .into_any_element(),
        );
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

    /// Selects the file whose lines reached the top of the diff, scrolling
    /// the tree only as far as it takes to show it.
    fn follow_diff(&mut self, path: &str, cx: &mut Context<Self>) {
        let rows = self.current_rows(cx);
        let Some(ix) = rows.iter().position(|row| row.path == path) else {
            return;
        };
        self.selected = Some(path.to_owned());
        self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
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
                self.set_files_viewed(&paths, mark != ViewedMark::All, cx);
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
        let starts = distinct_starts(rows);
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
                            name_from: 0,
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
                        let missing = match self.missing.get(&file.path) {
                            Some(Missing::Omitted) => " · diff not loaded",
                            Some(Missing::Unreadable) => " · diff not shown",
                            None => "",
                        };
                        RowView {
                            depth: row.depth,
                            folder: None,
                            label: split_path(&file.path).1.to_owned().into(),
                            name_from: starts[ix],
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
                                file.path, file.additions, file.deletions, missing
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
        let mut notes: Vec<NoDiffNote> = Vec::new();
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
            let note = match self.patch_errors.get(path) {
                Some(error) => error.clone().into(),
                None if self.commit.is_some() && *reason == Missing::Omitted => {
                    COMMIT_OMITTED_NOTE.into()
                }
                None => reason.note().into(),
            };
            let load =
                (*reason == Missing::Omitted).then(|| (path.clone(), self.loading_patch(path)));
            notes.push((id, note, load));
        }
        // Each gap between hunks goes under the line above it, or under the
        // header, after any thread there, so it sits right over the hunk
        // that follows.
        let mut bars: HashMap<ElementId, GapBar> = HashMap::new();
        for file in self
            .files
            .iter()
            .filter(|file| expand::expandable(file.status))
        {
            let Some(hunks) = self.shown_hunks(&file.path) else {
                continue;
            };
            let expansion = self.expansions.get(&file.path);
            let total = expansion
                .and_then(|expansion| expansion.source.as_ref())
                .map(|source| source.line_count());
            let busy = expansion.is_some_and(|expansion| expansion.read.is_some());
            let error = expansion.and_then(|expansion| expansion.error.as_ref());
            for gap in expand::gaps(hunks, total) {
                let id = ElementId::Name(SharedString::from(format!(
                    "gap:{}:{}",
                    file.path, gap.start
                )));
                annotations.push(match gap.above {
                    Some((line, side)) => DiffAnnotation::line(
                        id.clone(),
                        DiffLinePosition::new(file.path.clone(), to_diff_side(side), line as usize),
                    ),
                    None => DiffAnnotation::file(id.clone(), file.path.clone()),
                });
                let error = error
                    .filter(|(start, _)| *start == gap.start)
                    .map(|(_, error)| SharedString::from(error.clone()));
                bars.insert(
                    id,
                    GapBar {
                        path: file.path.clone(),
                        gap,
                        busy,
                        error,
                    },
                );
            }
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
        // The fill of the component's hunk rows, so a gap's controls and the
        // next hunk's header read as one band.
        let fold = theme.muted.opacity(0.35);
        let add_weak = weak.clone();
        let load_weak = weak.clone();
        let gap_weak = weak.clone();
        // While lines scroll, whether some line runs past the panel: the
        // toolbar says so in words and the panel's edge fades. The
        // component draws its own horizontal bar only while it scrolls and
        // keeps the scroll offset to itself (`DiffState::horizontal_scroll`
        // is crate-private), so a bar of Loupe's own could not be true.
        let runs_on = !wrap && self.long_files().next().is_some();
        let fade = if runs_on { dp(FADE_WIDTH) } else { px(0.) };
        let card_width = self.card_width.clone();
        let content: HeaderFor = {
            let (viewed, sizes, weak) = (viewed.clone(), sizes.clone(), weak.clone());
            Rc::new(move |file: &DiffFile| {
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
                header::HeaderContent {
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
                    on_drawn: None,
                }
            })
        };
        let file_ixs: Rc<HashMap<SharedString, usize>> = Rc::new(
            state
                .read(cx)
                .files()
                .iter()
                .enumerate()
                .map(|(ix, file)| (file.path().clone(), ix))
                .collect(),
        );
        let header = {
            let (content, watch) = (content.clone(), self.headers.clone());
            move |file: &DiffFile, _: &mut Window, _: &mut App| {
                let mut header = content(file);
                if let Some(&ix) = file_ixs.get(file.path()) {
                    let watch = watch.clone();
                    header.on_drawn = Some(Rc::new(move |bounds, viewport| {
                        watch.borrow_mut().record(ix, bounds, viewport)
                    }));
                }
                header::file_header(header)
            }
        };
        let pinned = self.pinned_header(state, content, cx);
        let decorations = self.find_decorations(cx);
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
                // line in a card at most 760 px wide that stays on the part
                // of the diff on screen, so their buttons stay in reach
                // however wide the file or narrow the window is.
                Diff::new(state)
                    .size_full()
                    .soft_wrap(wrap)
                    .hover_highlight(DiffHoverHighlight::Both)
                    .decorations(decorations)
                    .annotations(annotations)
                    .render_annotation(move |annotation, _, _| {
                        if let Some(bar) = bars.get(annotation.id()) {
                            return gap_bar(bar, &gap_weak, muted, fold);
                        }
                        // Threads and the composer sit in a card under their
                        // line, inset from the gutter like a review comment.
                        let card = div()
                            .debug_selector(|| "diff-card".into())
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
                            return on_screen(card.child(view.clone()), card_width.clone(), fade)
                                .into_any_element();
                        }
                        let (note, load) = notes
                            .iter()
                            .find(|(id, _, _)| id == annotation.id())
                            .map(|(_, note, load)| (Some(note.clone()), load.clone()))
                            .unwrap_or_default();
                        let weak = load_weak.clone();
                        h_flex()
                            .mx_3()
                            .my_2()
                            .gap_2()
                            .items_center()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child(Icon::new(IconName::FileX).xsmall())
                            .children(note)
                            .when_some(load, |row, (path, busy)| {
                                row.child(
                                    Button::new(SharedString::from(format!("load-diff:{path}")))
                                        .outline()
                                        .xsmall()
                                        .label("Load diff")
                                        .accessibility_label(format!("Load diff of {path}"))
                                        .loading(busy)
                                        .disabled(busy)
                                        .on_click(move |_, _, cx| {
                                            weak.update(cx, |this, cx| {
                                                this.load_omitted_patch(&path, cx)
                                            })
                                            .ok();
                                        }),
                                )
                            })
                            .into_any_element()
                    })
                    // The path, the size, and Viewed in the part of the
                    // header on screen, whether lines wrap or scroll.
                    .render_header(header)
                    .when(self.commit.is_none(), |diff| {
                        diff.on_add_annotation(move |range, window, cx| {
                            let range = range.clone();
                            add_weak
                                .update(cx, |_, cx| {
                                    cx.defer_in(window, move |this, window, cx| {
                                        this.open_composer(&range, window, cx)
                                    });
                                })
                                .ok();
                        })
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
            .child(pinned)
            .into_any_element()
    }

    /// The header of the file at the top of the diff, pinned over its lines
    /// while they scroll as the files tree pins folders, and the tree's
    /// selection following that file.
    fn pinned_header(
        &self,
        state: &Entity<DiffState>,
        content: HeaderFor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let watch = self.headers.clone();
        let state = state.clone();
        let weak = cx.entity().downgrade();
        canvas(
            move |bounds, window, cx| {
                // A header's text sits under its row's top border and padding.
                let inset = window.rem_size() * 0.25 + px(1.);
                let (before, top) = watch.borrow_mut().finish_frame(f32::from(inset));
                let file = top?;
                let (path, mut header) = {
                    let state = state.read(cx);
                    let file = state.files().get(file)?;
                    (file.path().clone(), content(file))
                };
                if before.is_some() && before != top {
                    let (weak, path) = (weak.clone(), path.clone());
                    cx.defer(move |cx| {
                        weak.update(cx, |tab, cx| tab.follow_diff(&path, cx)).ok();
                    });
                }
                let (headers, viewport) = {
                    let watch = watch.borrow();
                    (watch.headers().to_vec(), watch.viewport())
                };
                // Before any header was drawn, such as when the diff opens
                // past its first one, the diff's part of the screen is ours.
                let viewport = if viewport.size.width > px(0.) {
                    viewport
                } else {
                    bounds
                };
                // Marked from here, the file folds and its header takes the
                // top, the next file's lines under it.
                header.on_viewed = {
                    let (weak, path) = (weak.clone(), path.to_string());
                    Rc::new(move |checked, _, cx| {
                        weak.update(cx, |tab, cx| {
                            tab.toggle_viewed(&path, checked, cx);
                            tab.scroll_to(&path, cx);
                        })
                        .ok();
                    })
                };
                let collapse = {
                    let (state, path) = (state.clone(), path.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        state.update(cx, |state, cx| {
                            state.set_file_collapsed(&path, true, cx);
                            state.scroll_to_file(&path, cx);
                        })
                    }
                };
                let width = viewport.size.width;
                let mut row = h_flex()
                    .id("files-pinned-header")
                    .debug_selector(|| "pinned-header".into())
                    .role(Role::Group)
                    .aria_label(format!("Current file {path}"))
                    .occlude()
                    .w(width)
                    .pl_1()
                    .pr_3()
                    .py_1()
                    .gap_1()
                    .bg(tokens::background())
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("pinned-collapse")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .accessibility_label(format!("Collapse {path}"))
                            .on_click(collapse),
                    )
                    .child(div().flex_1().min_w_0().child(header::file_header(header)))
                    .into_any_element();
                let row_size = row.layout_as_root(
                    size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                    window,
                    cx,
                );
                let offset =
                    sticky::pinned_top(&headers, file, row_size.height.into(), inset.into())?;
                let mut shade = div()
                    .w(width)
                    .h(dp(6.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(tokens::edge_shadow(), 0.),
                        linear_color_stop(tokens::edge_shadow().opacity(0.), 1.),
                    ))
                    .into_any_element();
                shade.layout_as_root(AvailableSpace::min_size(), window, cx);
                let origin = viewport.origin + point(px(0.), px(offset));
                window.with_content_mask(Some(ContentMask { bounds: viewport }), |window| {
                    row.prepaint_at(origin, window, cx);
                    shade.prepaint_at(origin + point(px(0.), row_size.height), window, cx);
                });
                Some((row, shade, viewport))
            },
            |_, pinned, window, cx| {
                let Some((mut row, mut shade, viewport)) = pinned else {
                    return;
                };
                window.with_content_mask(Some(ContentMask { bounds: viewport }), |window| {
                    row.paint(window, cx);
                    shade.paint(window, cx);
                });
            },
        )
        .absolute()
        .inset_0()
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
                    let find_bar = self.find.open.then(|| self.find_bar(cx));
                    let diff = resizable_panel().child(
                        v_flex().size_full().children(find_bar).child(
                            h_flex()
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .child(self.diff_view(&state, cx)),
                        ),
                    );
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
        // Nearly every file of a pull request is modified, so its letter
        // stays quiet and color marks the files that are new, gone, or moved.
        GitTint::Modified => tokens::text_muted(),
        GitTint::Deleted => tokens::closed(),
        GitTint::Renamed => tokens::link(),
    }
}

/// The controls of one gap between hunks, as GitHub draws them: Expand
/// down under a hunk, Expand up over one, and the whole gap with its count.
fn gap_bar(bar: &GapBar, weak: &WeakEntity<FilesTab>, muted: Hsla, fill: Hsla) -> AnyElement {
    let GapBar {
        path,
        gap,
        busy,
        error,
    } = bar;
    let start = gap.start;
    let control = |direction: Direction| {
        let lines = gap.reveal(direction);
        let name = match gap.end {
            None => format!("Show the lines after line {} of {path}", start - 1),
            Some(_) => format!("Show lines {} to {} of {path}", lines.start, lines.end - 1),
        };
        let (icon, tooltip) = match direction {
            Direction::Down => (IconName::ArrowDown, "Expand down"),
            Direction::Up => (IconName::ArrowUp, "Expand up"),
            Direction::All => (IconName::ChevronsUpDown, "Expand all"),
        };
        let (weak, path) = (weak.clone(), path.clone());
        let button = Button::new(SharedString::from(format!(
            "expand-{direction:?}:{path}:{start}"
        )))
        .ghost()
        .small()
        .icon(icon)
        .text_color(muted)
        .accessibility_label(name)
        .loading(*busy)
        .disabled(*busy)
        .on_click(move |_, _, cx| {
            weak.update(cx, |this, cx| this.expand_gap(&path, start, direction, cx))
                .ok();
        });
        match (direction, gap.hidden()) {
            (Direction::All, Some(1)) => button.label("1 unchanged line"),
            (Direction::All, Some(hidden)) => button.label(format!("{hidden} unchanged lines")),
            _ => button.tooltip(tooltip),
        }
    };
    h_flex()
        .w_full()
        .px_2()
        .gap_1()
        .items_center()
        .bg(fill)
        .text_size(scale::meta())
        .text_color(muted)
        .children(gap.directions().into_iter().map(control))
        .children(error.clone())
        .into_any_element()
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
                    if self.commit.is_some() {
                        "This commit does not change any files."
                    } else {
                        "This pull request does not change any files."
                    },
                )
                .into_any_element()
            }
            Loadable::Loaded(()) | Loadable::Refreshing(()) => self.loaded_view(window, cx),
        };
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        div()
            .size_full()
            .key_context("FilesTab")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &FindInDiff, window, cx| this.open_find(window, cx)))
            // A text field answers Cmd+F with Search and passes it on.
            .on_action(cx.listener(|this, _: &Search, window, cx| this.open_find(window, cx)))
            .child(body)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::sync::Arc;

    use chrono::{DateTime, Utc};
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::component::diff::{DiffLineRange, DiffSide};
    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, InputEvent as _, Modifiers, MouseButton,
        MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollEvent,
        TestAppContext, VisualTestContext, Window, WindowBounds, WindowOptions, point, px, size,
    };
    use loupe_github::diff::{git_patch, parse_patch};
    use loupe_github::{
        ChangedFile, Checks, FileStatus, MergeMethod, MergeStateStatus, Mergeable, PrRef, PrState,
        PullRequestDetail, RepoRef, ViewedState,
    };

    use super::emphasis::PLAIN;
    use super::expand::{Direction, Source};
    use super::model::build_files;
    use super::{FILE_LIST_MAX, FILE_LIST_MIN, FILE_LIST_WIDTH, FilesTab, PrTabContext};
    use crate::services::{Accounts, Services};
    use crate::store::{FileTreePosition, Settings, update_settings};

    const WINDOW_WIDTH: f32 = 1280.;

    pub(in crate::github::pull_request) fn detail() -> PullRequestDetail {
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
            commits_total: 0,
            commits_cursor: None,
            checks: Checks {
                state: None,
                runs: Vec::new(),
            },
            timeline: Vec::new(),
            threads: Vec::new(),
            pending_review: None,
            viewer_login: "bruno".into(),
            viewer_can_update: true,
            viewer_review: None,
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
        open_files_tab_with_files(cx, vec![file])
    }

    /// The files tab alone in a window, with `files` loaded.
    fn open_files_tab_with_files(
        cx: &mut TestAppContext,
        files: Vec<ChangedFile>,
    ) -> (VisualTestContext, Entity<FilesTab>) {
        open_tab(cx, files, FilesTab::new)
    }

    /// The tab `build` makes, alone in a window, with `files` loaded.
    fn open_tab(
        cx: &mut TestAppContext,
        files: Vec<ChangedFile>,
        build: impl FnOnce(PrTabContext, &mut Window, &mut Context<FilesTab>) -> FilesTab + 'static,
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
                    cx.new(|cx| build(context, window, cx))
                },
            )
            .expect("open the window")
        });
        let mut cx = VisualTestContext::from_window(handle, cx);
        // With no GitHub client the first load fails; then the files arrive.
        cx.run_until_parked();
        tab.update(&mut cx, |tab, cx| {
            tab.finish_load(
                Ok(build_files(files, git_patch, |patch| {
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

    fn assert_card_on_screen(cx: &mut VisualTestContext, case: &str) -> Bounds<Pixels> {
        let (_, diff) = panes(cx);
        let card = cx
            .debug_bounds("diff-card")
            .expect("the composer's card is drawn");
        assert!(
            card.left() >= diff.left() && card.right() <= diff.right(),
            "{case}: the card {card:?} is outside the diff {diff:?}"
        );
        card
    }

    #[gpui_kit::test]
    fn a_card_under_a_line_stays_on_screen_while_lines_scroll(cx: &mut TestAppContext) {
        let line = "x".repeat(400);
        let (mut cx, tab) = open_files_tab_with(
            cx,
            ChangedFile {
                path: "src/main.rs".into(),
                previous_path: None,
                status: FileStatus::Modified,
                additions: 1,
                deletions: 1,
                patch: Some(format!("@@ -1 +1 @@\n-old\n+{line}")),
                viewed: ViewedState::Unviewed,
            },
        );
        let cx = &mut cx;
        cx.update(|window, cx| {
            tab.update(cx, |tab, cx| {
                let range = DiffLineRange::new("src/main.rs", DiffSide::Modified, 1, 1);
                tab.open_composer(&range, window, cx);
            })
        });

        let before = assert_card_on_screen(cx, "lines scroll");
        let (_, diff) = panes(cx);
        dispatch(
            cx,
            ScrollEvent {
                position: point(diff.left() + px(200.), diff.top() + px(60.)),
                delta: ScrollDelta::Pixels(point(px(-300.), px(0.))),
                ..ScrollEvent::default()
            }
            .to_platform_input(),
        );
        let after = assert_card_on_screen(cx, "scrolled sideways");
        assert_eq!(
            after.left(),
            before.left(),
            "the card stays put while the lines scroll"
        );

        // The diff beside the tree is narrower than a card at its widest.
        cx.simulate_resize(size(px(960.), px(800.)));
        let narrow = assert_card_on_screen(cx, "narrow window");
        assert!(narrow.size.width < before.size.width, "the card narrows");

        cx.update(|_, cx| update_settings(cx, |settings| settings.diff_wrap = true));
        assert_card_on_screen(cx, "lines wrap");
    }

    fn added(path: &str, lines: usize) -> ChangedFile {
        let body: String = (1..=lines).map(|line| format!("+line {line}\n")).collect();
        ChangedFile {
            path: path.into(),
            previous_path: None,
            status: FileStatus::Added,
            additions: lines as u32,
            deletions: 0,
            patch: Some(format!("@@ -0,0 +1,{lines} @@\n{body}")),
            viewed: ViewedState::Unviewed,
        }
    }

    fn scroll_diff(cx: &mut VisualTestContext, by: f32) {
        let (_, diff) = panes(cx);
        dispatch(
            cx,
            ScrollEvent {
                position: point(diff.left() + px(200.), diff.top() + px(200.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-by))),
                ..ScrollEvent::default()
            }
            .to_platform_input(),
        );
        panes(cx);
    }

    #[gpui_kit::test]
    fn the_file_at_the_top_keeps_its_header_and_the_tree_follows(cx: &mut TestAppContext) {
        let (mut cx, tab) = open_files_tab_with_files(
            cx,
            vec![added("src/long.rs", 300), added("src/next.rs", 100)],
        );
        let cx = &mut cx;
        let (_, diff) = panes(cx);
        assert!(
            cx.debug_bounds("pinned-header").is_none(),
            "at the top the file's own header is in place"
        );

        scroll_diff(cx, 1500.);
        let pinned = cx
            .debug_bounds("pinned-header")
            .expect("in the middle of a long file its header stays on screen");
        // Inside the diff's own 1 px border.
        assert!(
            pinned.top() - diff.top() <= px(1.)
                && pinned.left() >= diff.left()
                && pinned.right() <= diff.right(),
            "pinned at the top of the diff: {pinned:?} in {diff:?}"
        );
        let viewed = cx
            .debug_bounds("header-viewed")
            .expect("the pinned header has Viewed");
        assert!(viewed.top() >= pinned.top() && viewed.bottom() <= pinned.bottom());

        // A wheel's steps, each shorter than the screen, into the next file.
        for _ in 0..9 {
            scroll_diff(cx, 600.);
        }
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.selected.clone()).as_deref(),
            Some("src/next.rs"),
            "the tree selects the file that reached the top"
        );
    }

    /// The current match: its place, the file, and the line.
    fn current_match(
        cx: &mut VisualTestContext,
        tab: &Entity<FilesTab>,
    ) -> Option<(usize, String, u32)> {
        tab.read_with(cx, |tab, _| {
            let ix = tab.find.current?;
            let found = &tab.find.matches[ix];
            Some((ix, found.path.clone(), found.line))
        })
    }

    #[gpui_kit::test]
    fn find_goes_through_the_lines_of_every_loaded_diff(cx: &mut TestAppContext) {
        // A diff GitHub left out: lines changed, no patch.
        let left_out = ChangedFile {
            patch: None,
            additions: 5000,
            ..added("src/huge.rs", 1)
        };
        let (mut cx, tab) = open_files_tab_with_files(
            cx,
            vec![
                added("src/long.rs", 300),
                added("src/next.rs", 100),
                left_out,
            ],
        );
        let cx = &mut cx;
        cx.update(|_, cx| crate::shell::menus::bind_keys(cx));
        draw(cx);
        assert!(
            cx.debug_bounds("files-find").is_none(),
            "the bar starts closed"
        );

        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-f");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-f");
        draw(cx);
        assert!(
            cx.debug_bounds("files-find").is_some(),
            "the shortcut opens the bar"
        );

        // `line 25` and `line 250` to `line 259` in the first file, `line 25`
        // in the second; the left-out diff is not searched.
        cx.simulate_input("LINE 25");
        draw(cx);
        assert_eq!(tab.read_with(cx, |tab, _| tab.find.matches.len()), 12);
        assert_eq!(
            current_match(cx, &tab),
            Some((0, "src/long.rs".into(), 25)),
            "the first match from the file on screen, ignoring case"
        );
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.selected.clone()).as_deref(),
            Some("src/long.rs")
        );

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            current_match(cx, &tab),
            Some((1, "src/long.rs".into(), 250))
        );
        cx.simulate_keystrokes("shift-enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("shift-enter");
        cx.run_until_parked();
        assert_eq!(
            current_match(cx, &tab),
            Some((11, "src/next.rs".into(), 25)),
            "back past the first goes round to the last"
        );
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.selected.clone()).as_deref(),
            Some("src/next.rs"),
            "the tree selects the match's file"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        draw(cx);
        assert!(
            cx.debug_bounds("files-find").is_none(),
            "Escape closes the bar"
        );
        assert!(tab.read_with(cx, |tab, _| tab.find.matches.is_empty()));

        // Load diff on the left-out file brings its lines into the search.
        cx.update(|window, cx| tab.update(cx, |tab, cx| tab.open_find(window, cx)));
        assert_eq!(tab.read_with(cx, |tab, _| tab.find.matches.len()), 12);
        let before = current_match(cx, &tab).map(|(_, path, line)| (path, line));
        tab.update(cx, |tab, cx| {
            tab.apply_omitted_patch(
                "src/huge.rs",
                Some("@@ -0,0 +1 @@\n+line 25 again".into()),
                cx,
            )
        });
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.find.matches.len()),
            13,
            "a diff loaded later is searched too"
        );
        assert_eq!(
            current_match(cx, &tab).map(|(_, path, line)| (path, line)),
            before,
            "the current match stays where it was"
        );
    }

    #[gpui_kit::test]
    fn find_marks_the_text_found_and_only_the_current_line(cx: &mut TestAppContext) {
        let (mut cx, tab) = open_files_tab_with_files(cx, vec![added("src/long.rs", 300)]);
        let cx = &mut cx;
        draw(cx);
        let marked = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                let found = cx.theme().warning.opacity(0.4);
                let highlight = tab.read(cx).highlighter(cx);
                highlight(PLAIN, "line 25\nline 250\nline 3\n")
                    .into_iter()
                    .filter(|(_, style)| style.background_color == Some(found))
                    .map(|(range, _)| range)
                    .collect::<Vec<_>>()
            })
        };
        assert!(marked(cx).is_empty(), "nothing is marked before a find");

        cx.update(|window, cx| tab.update(cx, |tab, cx| tab.open_find(window, cx)));
        cx.simulate_input("LINE 25");
        draw(cx);
        assert_eq!(
            marked(cx),
            [0..7, 8..15],
            "the text found, not the whole line"
        );
        let lines: Vec<_> = cx.update(|_, cx| {
            tab.read(cx)
                .find_decorations(cx)
                .iter()
                .map(|mark| (mark.range().start(), mark.range().end()))
                .collect()
        });
        assert_eq!(lines, [(25, 25)], "only the current match's line");
        assert_eq!(
            cx.update(|_, cx| tab.read(cx).open_find_text(cx))
                .as_deref(),
            Some("LINE 25")
        );

        cx.update(|window, cx| tab.update(cx, |tab, cx| tab.close_find(window, cx)));
        assert!(
            marked(cx).is_empty(),
            "closing the bar takes the marks away"
        );
        assert_eq!(cx.update(|_, cx| tab.read(cx).open_find_text(cx)), None);
    }

    #[gpui_kit::test]
    fn the_next_commit_opens_with_the_find_on_its_first_match(cx: &mut TestAppContext) {
        let (mut cx, tab) = open_tab(
            cx,
            vec![added("src/long.rs", 300)],
            |context, window, cx| {
                FilesTab::for_commit(context, "c0ffee".into(), Some("LINE 25".into()), window, cx)
            },
        );
        let cx = &mut cx;
        draw(cx);
        assert!(cx.debug_bounds("files-find").is_some(), "the bar is open");
        assert_eq!(
            tab.read_with(cx, |tab, cx| tab.find.input.read(cx).value().to_string()),
            "LINE 25"
        );
        assert_eq!(tab.read_with(cx, |tab, _| tab.find.matches.len()), 11);
        assert_eq!(
            current_match(cx, &tab),
            Some((0, "src/long.rs".into(), 25)),
            "the first match"
        );
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.selected.clone()).as_deref(),
            Some("src/long.rs"),
            "the diff went to it"
        );
    }

    /// A file whose diff GitHub left out: lines changed, no patch.
    fn left_out(path: &str) -> ChangedFile {
        ChangedFile {
            patch: None,
            additions: 5000,
            ..added(path, 1)
        }
    }

    #[gpui_kit::test]
    fn a_diff_with_collapsed_files_opens_on_the_first_header(cx: &mut TestAppContext) {
        let seen = ChangedFile {
            viewed: ViewedState::Viewed,
            ..added("src/seen.rs", 20)
        };
        let (mut cx, tab) = open_files_tab_with_files(
            cx,
            vec![
                added("src/first.rs", 40),
                left_out("src/huge.rs"),
                seen,
                added("src/last.rs", 40),
            ],
        );
        let cx = &mut cx;
        panes(cx);
        assert!(
            cx.debug_bounds("pinned-header").is_none(),
            "the first file's own header and its first lines are on screen"
        );
        assert_eq!(tab.read_with(cx, |tab, cx| tab.top_file(cx)), Some(0));
    }

    #[gpui_kit::test]
    fn load_all_stops_with_the_reason_when_github_cannot_answer(cx: &mut TestAppContext) {
        let (mut cx, tab) = open_files_tab_with_files(
            cx,
            vec![
                added("src/first.rs", 40),
                left_out("src/huge.rs"),
                left_out("src/other.rs"),
            ],
        );
        let cx = &mut cx;
        tab.update(cx, |tab, cx| tab.load_all_patches(cx));
        cx.run_until_parked();
        tab.read_with(cx, |tab, _| {
            assert!(tab.load_all.is_none(), "the run ended");
            assert_eq!(
                tab.load_all_error
                    .as_ref()
                    .map(|(short, _)| short.to_string()),
                Some("GitHub is not connected".into())
            );
            assert!(!tab.loading_patch("src/huge.rs"));
            assert_eq!(tab.unloaded_patches().len(), 2, "both can be loaded again");
        });
    }

    #[gpui_kit::test]
    fn diffs_loaded_together_are_searched_and_keep_the_top_in_place(cx: &mut TestAppContext) {
        let (mut cx, tab) = open_files_tab_with_files(
            cx,
            vec![
                added("src/first.rs", 40),
                left_out("src/huge.rs"),
                left_out("src/other.rs"),
            ],
        );
        let cx = &mut cx;
        cx.update(|window, cx| tab.update(cx, |tab, cx| tab.open_find(window, cx)));
        cx.simulate_input("needle");
        panes(cx);
        assert_eq!(tab.read_with(cx, |tab, _| tab.find.matches.len()), 0);

        tab.update(cx, |tab, cx| {
            tab.apply_omitted_patches(
                vec![
                    (
                        "src/huge.rs".into(),
                        Some("@@ -0,0 +1 @@\n+a needle".into()),
                    ),
                    (
                        "src/other.rs".into(),
                        Some("@@ -0,0 +1 @@\n+one more needle".into()),
                    ),
                ],
                Some("src/first.rs".into()),
                cx,
            )
        });
        panes(cx);
        tab.read_with(cx, |tab, cx| {
            assert_eq!(tab.find.matches.len(), 2, "both diffs are searched");
            assert!(tab.unloaded_patches().is_empty());
            let state = tab.diff.as_ref().expect("diff").read(cx);
            assert!(
                !state.is_file_collapsed("src/huge.rs"),
                "a loaded diff opens"
            );
            assert!(!state.is_file_collapsed("src/other.rs"));
        });
        assert!(
            cx.debug_bounds("pinned-header").is_none(),
            "the file at the top stays on its header"
        );
    }

    /// A file of 101 lines whose line 10 changed and that gained a line
    /// after line 60, with its patch and its text at the head commit.
    fn far_apart() -> (ChangedFile, String) {
        let patch = "@@ -7,7 +7,7 @@\n line 7\n line 8\n line 9\n-line 10\n+LINE 10\n line 11\n line 12\n line 13\n@@ -57,7 +57,8 @@\n line 57\n line 58\n line 59\n-line 60\n+LINE 60\n+line 60b\n line 61\n line 62\n line 63";
        let text: String = (1..=100)
            .map(|line| match line {
                10 => "LINE 10\n".to_owned(),
                60 => "LINE 60\nline 60b\n".to_owned(),
                line => format!("line {line}\n"),
            })
            .collect();
        let file = ChangedFile {
            path: "src/lib.rs".into(),
            previous_path: None,
            status: FileStatus::Modified,
            additions: 3,
            deletions: 2,
            patch: Some(patch.into()),
            viewed: ViewedState::Unviewed,
        };
        (file, text)
    }

    #[gpui_kit::test]
    fn expanding_a_gap_shows_its_lines_to_find_but_not_to_comment(cx: &mut TestAppContext) {
        let (file, text) = far_apart();
        let (mut cx, tab) = open_files_tab_with(cx, file);
        let cx = &mut cx;
        draw(cx);
        let hunks = |cx: &mut VisualTestContext| {
            tab.read_with(cx, |tab, _| {
                tab.shown_hunks("src/lib.rs").map(|hunks| hunks.len())
            })
        };
        cx.update(|window, cx| tab.update(cx, |tab, cx| tab.open_find(window, cx)));
        cx.simulate_input("line 30");
        draw(cx);
        assert_eq!(tab.read_with(cx, |tab, _| tab.find.matches.len()), 0);

        // Without GitHub the read fails, and the gap says why.
        tab.update(cx, |tab, cx| {
            tab.expand_gap("src/lib.rs", 14, Direction::Down, cx)
        });
        cx.run_until_parked();
        let error = tab.read_with(cx, |tab, _| tab.expansions["src/lib.rs"].error.clone());
        assert_eq!(error.map(|(start, _)| start), Some(14));

        tab.update(cx, |tab, cx| {
            let expansion = tab.expansions.entry("src/lib.rs".into()).or_default();
            expansion.source = Some(Arc::new(Source::new(&text)));
            tab.expand_gap("src/lib.rs", 14, Direction::Down, cx);
        });
        draw(cx);
        assert_eq!(
            tab.read_with(cx, |tab, _| tab.find.matches.len()),
            1,
            "a revealed line is searched"
        );
        assert_eq!(hunks(cx), Some(2), "lines 34 to 56 still part the hunks");
        let (additions, deletions) = tab.read_with(cx, |tab, cx| {
            let file = &tab.diff.as_ref().expect("diff").read(cx).files()[0];
            (file.additions(), file.deletions())
        });
        assert_eq!(
            (additions, deletions),
            (3, 2),
            "revealed lines are unchanged lines"
        );

        tab.update(cx, |tab, cx| {
            tab.expand_gap("src/lib.rs", 34, Direction::All, cx)
        });
        draw(cx);
        assert_eq!(hunks(cx), Some(1), "the whole gap joins the two hunks");

        let target = |line: usize, cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                tab.update(cx, |tab, cx| {
                    let range = DiffLineRange::new("src/lib.rs", DiffSide::Modified, line, line);
                    tab.open_composer(&range, window, cx);
                    let slot = tab.composer.as_ref().expect("composer");
                    slot.view.read(cx).target().anchor.is_ok()
                })
            })
        };
        assert!(target(11, cx), "a line of the patch takes a comment");
        assert!(!target(30, cx), "GitHub anchors comments only to the patch");
    }
}
