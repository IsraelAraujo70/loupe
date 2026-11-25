//! The pull request view: loads the detail, owns the tabs, and wires the
//! header, toolbar (review and actions), and side column around them.

mod header;
mod merge_box;
mod model;
mod poll;
mod side;
mod stack_panel;
mod toolbar;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use futures::StreamExt as _;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::stack_merge::run_stack_merge;
use loupe_github::{
    ActionsJob, ChecksEtag, ChecksPoll, GitHub, Label, MergeMethod, MergeOutcome, PrRef, PrState,
    PullRequestDetail, PullRequestPoll, ReviewEvent, StackGraph, StackMergeEvent, StackMergePlan,
    StackMergeState, StackMergeStep, StepStatus,
};

use super::checks::{ChecksTab, Freshness};
use super::commits;
use super::conversation::ConversationTab;
use super::files::FilesTab;
use super::guide::{GuideAction, GuideTab};
use super::tab::{PrTabContext, PrTabEvent};
use crate::github::common::{self, LoadError, Loadable, Submit};
use crate::github::pr_feed::{self, PrLoaded};
use crate::links::PrLinkContext;
use crate::shell::item_menu::{self, Item, PrItem};
use crate::theme::scale;
use crate::ui;
use model::{Panel, PrTab, review_blocker};

/// What exists only after the first successful load.
#[derive(Clone)]
struct Loaded {
    conversation: Entity<ConversationTab>,
    guide: Entity<GuideTab>,
    files: Entity<FilesTab>,
    checks: Entity<ChecksTab>,
    /// What the Linear and T3 panels were built from; they are rebuilt when
    /// a refresh changes it.
    link_context: PrLinkContext,
    linear: AnyView,
    t3: AnyView,
}

/// A commit opened from the Commits tab: the Files tab shows its changes
/// in place of the whole pull request's until All changes.
struct CommitDiff {
    oid: String,
    files: Entity<FilesTab>,
    _events: Subscription,
}

/// What the pull request poll compares GitHub's answer with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PrSeen {
    /// A load just landed: the next poll only takes its ETag.
    Unknown,
    /// The ETag of the pull request on screen.
    Etag(String),
    /// The last load failed: the next poll loads again whatever it says.
    Stale,
}

/// A write against the pull request.
#[derive(Clone, Debug)]
enum Write {
    Review {
        event: ReviewEvent,
        body: String,
        review_id: Option<String>,
    },
    DiscardReview {
        review_id: String,
    },
    Merge(MergeMethod),
    MarkReady,
    ConvertToDraft,
    Close,
    Reopen,
    EditTitle(String),
    AddLabel(String),
    RemoveLabel(String),
}

/// Shown after a merge that GitHub had not finished when Loupe stopped
/// waiting for it.
const MERGE_STILL_RUNNING: &str =
    "GitHub is still merging this pull request. Refresh in a moment to see the result.";

impl Write {
    /// Sends the write. `Ok(Some(note))` succeeded with something to tell the
    /// user once the pull request reloads.
    fn perform(
        self,
        gh: &GitHub,
        detail: &PullRequestDetail,
    ) -> loupe_github::Result<Option<&'static str>> {
        let id = detail.id.as_str();
        let done = match self {
            Write::Merge(method) => {
                return gh
                    .merge(&detail.pr, method, &detail.head_oid)
                    .map(|outcome| {
                        (outcome == MergeOutcome::Pending).then_some(MERGE_STILL_RUNNING)
                    });
            }
            Write::Review {
                event,
                body,
                review_id,
            } => gh.submit_review(id, review_id.as_deref(), event, &body),
            Write::DiscardReview { review_id } => gh.discard_review(&review_id),
            Write::MarkReady => gh.mark_ready_for_review(id),
            Write::ConvertToDraft => gh.convert_to_draft(id),
            Write::Close => gh.close_pull_request(id),
            Write::Reopen => gh.reopen_pull_request(id),
            Write::EditTitle(title) => gh.update_pull_request(id, Some(&title), None),
            Write::AddLabel(label_id) => gh.add_labels(id, &[label_id]),
            Write::RemoveLabel(label_id) => gh.remove_labels(id, &[label_id]),
        };
        done.map(|()| None)
    }
}

/// Live state of an in-progress, failed, or finished stack merge.
pub(super) struct StackMergeRun {
    pub plan: StackMergePlan,
    pub selected: Vec<u64>,
    pub steps: Vec<StackMergeStep>,
    pub method: MergeMethod,
    pub statuses: HashMap<u64, StepStatus>,
    pub state: StackMergeState,
    pub running: bool,
    pub failed_pr: Option<u64>,
    pub can_retry: bool,
}

pub struct PullRequestView {
    paint_started: std::time::Instant,
    paint_logged: bool,
    pr: PrRef,
    detail: Loadable<Arc<PullRequestDetail>>,
    /// Loaded stack graph (populated asynchronously when the PR is open).
    stack_graph: Option<StackGraph>,
    /// True once the stack graph lookup for this PR has finished (even when
    /// it returned `None`), so a pending Reviews "Merge stack" request is
    /// always consumed or cleared.
    stack_loaded: bool,
    stack_task: Option<Task<()>>,
    /// State of the active or most recent stack merge run.
    stack_merge_run: Option<StackMergeRun>,
    stack_merge_task: Option<Task<()>>,
    tab: PrTab,
    loaded: Option<Loaded>,
    commit: Option<CommitDiff>,
    /// A detail that arrived without a window to hand it to the tabs; the
    /// next render installs it.
    incoming: Option<Arc<PullRequestDetail>>,
    panel: Panel,
    merge_method: Option<MergeMethod>,
    write: Submit,
    /// Ready for review sent from the merge box's Draft block: in flight, or
    /// why GitHub refused it, which the block shows.
    ready: Submit,
    /// What a write that succeeded left to say, such as a merge GitHub was
    /// still running when Loupe stopped waiting.
    write_note: Option<&'static str>,
    /// The title being edited, while the edit row is open.
    title_edit: Option<Entity<InputState>>,
    /// The repository's labels while the label editor is open.
    labels_picker: Option<Loadable<Vec<Label>>>,
    labels_task: Option<Task<()>>,
    review_body: Entity<TextareaState>,
    clear_review_body: bool,
    /// Put the caret in the review summary on the next render: the review
    /// panel just opened, and what the user types next belongs there.
    focus_review_body: bool,
    conversation_scroll: ScrollHandle,
    /// Details opened in a window too narrow to show the side column on
    /// its own.
    details_open: bool,
    guide_scroll: ScrollHandle,
    commits_scroll: ScrollHandle,
    checks_scroll: ScrollHandle,
    side_scroll: ScrollHandle,
    load_task: Option<Task<()>>,
    /// The commits after the first page loading for the Commits tab, by
    /// the `base...head` they belong to.
    commits_load: Option<(String, Task<()>)>,
    /// Why the commits after the first page did not load.
    commits_error: Option<LoadError>,
    write_task: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
    /// When GitHub was last asked about this pull request, by a load or a
    /// poll of its checks.
    last_fetch: Instant,
    /// When GitHub last confirmed the checks on screen.
    checked_at: Option<DateTime<Local>>,
    /// The ETags of the checks on screen, which the next poll sends.
    checks_etag: Option<ChecksEtag>,
    /// The ETags of a poll that saw a change, kept when its reload lands.
    incoming_etag: Option<ChecksEtag>,
    /// A reload after a change brought the same checks once. GraphQL can
    /// trail REST for a moment, so the next poll asks with the old ETags
    /// and loads again before the view settles on what it shows.
    retried_same_checks: bool,
    /// When jobs were last re-run from the Checks tab, and which, until
    /// their replacements show up.
    rerun: Option<(Instant, Vec<ActionsJob>)>,
    /// Polling waits for a low rate limit to reset.
    paused_until: Option<DateTime<Utc>>,
    /// Whether the merge box's check list is open, once the user opened or
    /// closed it; until then it follows `model::checks_open_at_first`.
    checks_open: Option<bool>,
    /// The sections of the merge box's check list the user folded.
    folded_check_groups: HashSet<model::CheckGroup>,
    /// Whether the last tick found the pull request on screen.
    on_screen: bool,
    poll_task: Option<Task<()>>,
    /// What the pull request poll compares with.
    pr_seen: PrSeen,
    /// When that poll last went out; `None` right after a load.
    pr_polled_at: Option<Instant>,
    pr_poll_task: Option<Task<()>>,
    /// Last `(head_oid, guide_service::revision, language, stack)` synchronized
    /// into `GuideTab`.
    guide_sync: Option<(String, u64, loupe_guide::GuideLanguage, Option<StackGraph>)>,
    _ticker: Task<()>,
}

impl PullRequestView {
    pub(super) fn new(pr: PrRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let review_body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 10)
                .placeholder("Leave a comment on this pull request")
        });
        let submit_on_secondary_enter = cx.subscribe_in(
            &review_body,
            window,
            |this, _, event: &InputEvent, _window, cx| {
                if matches!(
                    event,
                    InputEvent::PressEnter {
                        secondary: true,
                        ..
                    }
                ) {
                    this.submit_review(ReviewEvent::Comment, cx);
                }
            },
        );
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(poll::TICK).await;
                if this
                    .update(cx, |view: &mut Self, cx| view.tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut view = Self {
            paint_started: std::time::Instant::now(),
            paint_logged: false,
            pr,
            detail: Loadable::Loading,
            stack_graph: None,
            stack_loaded: false,
            stack_task: None,
            stack_merge_run: None,
            stack_merge_task: None,
            tab: PrTab::Conversation,
            loaded: None,
            commit: None,
            incoming: None,
            panel: Panel::Closed,
            merge_method: None,
            write: Submit::Idle,
            ready: Submit::Idle,
            write_note: None,
            title_edit: None,
            labels_picker: None,
            labels_task: None,
            review_body,
            clear_review_body: false,
            focus_review_body: false,
            conversation_scroll: ScrollHandle::new(),
            details_open: false,
            guide_scroll: ScrollHandle::new(),
            commits_scroll: ScrollHandle::new(),
            checks_scroll: ScrollHandle::new(),
            side_scroll: ScrollHandle::new(),
            load_task: None,
            commits_load: None,
            commits_error: None,
            write_task: None,
            subscriptions: vec![submit_on_secondary_enter],
            last_fetch: Instant::now(),
            checked_at: None,
            checks_etag: None,
            incoming_etag: None,
            retried_same_checks: false,
            rerun: None,
            paused_until: None,
            checks_open: None,
            folded_check_groups: HashSet::new(),
            on_screen: true,
            poll_task: None,
            pr_seen: PrSeen::Unknown,
            pr_polled_at: None,
            pr_poll_task: None,
            guide_sync: None,
            _ticker: ticker,
        };
        let page = cx.entity_id();
        cx.on_release(move |_, cx| item_menu::set_page_item(page, None, cx))
            .detach();
        view.reload(cx);
        view
    }

    pub(in crate::github::pull_request) fn loaded_detail(&self) -> Option<&PullRequestDetail> {
        self.detail.value().map(|detail| detail.as_ref())
    }

    /// Loads the pull request again, keeping the stale one on screen. A failed
    /// refresh keeps the stale data and reports in the toolbar.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let key = crate::cache::github_key(cx, &format!("pr/{}", self.pr.url()));
        if self.detail.value().is_none()
            && let Some(detail) = key
                .as_deref()
                .and_then(crate::cache::get::<PullRequestDetail>)
        {
            let detail = Arc::new(detail);
            self.load_commits(&detail, cx);
            self.incoming = Some(detail.clone());
            self.detail = Loadable::Loaded(detail);
        }
        self.detail.start_refresh();
        self.last_fetch = Instant::now();
        // The poll after the load takes the ETag of what it brought.
        self.pr_seen = PrSeen::Unknown;
        self.pr_polled_at = None;
        let pr = self.pr.clone();
        self.load_task = Some(common::run(
            cx,
            move |gh| gh.pull_request(&pr),
            move |view, result, _cx| match result {
                Ok(mut detail) => {
                    if let Some(shown) = view.detail.value() {
                        model::carry_commits(shown, &mut detail);
                    }
                    if let Some(key) = &key {
                        crate::cache::put(key, &detail);
                    }
                    if let Some(key) = crate::cache::github_key(_cx, crate::github::inbox::CACHE)
                        && let Some(mut inbox) = crate::cache::get::<loupe_github::Inbox>(&key)
                    {
                        for page in [
                            &mut inbox.review_requested,
                            &mut inbox.authored,
                            &mut inbox.involved,
                        ] {
                            let before = page.items.len();
                            if detail.state != loupe_github::PrState::Open {
                                page.items.retain(|item| item.pr != detail.pr);
                                page.total_count = page
                                    .total_count
                                    .saturating_sub((before - page.items.len()) as u32);
                            }
                            for item in page.items.iter_mut().filter(|item| item.pr == detail.pr) {
                                item.title = detail.title.clone();
                                item.is_draft = detail.is_draft;
                                item.updated_at = detail.updated_at;
                                item.review_decision = detail.review_decision;
                                item.checks = detail.checks.state;
                            }
                        }
                        crate::cache::put(&key, &inbox);
                    }
                    view.settle_checks(&detail, _cx);
                    if detail.state == PrState::Open {
                        view.load_stack_graph(&detail, _cx);
                    } else {
                        view.stack_graph = None;
                        view.stack_loaded = true;
                        view.stack_task = None;
                    }
                    let detail = Arc::new(detail);
                    view.load_commits(&detail, _cx);
                    crate::github::enqueue_auto_guide_if_watched(detail.clone(), _cx);
                    view.incoming = Some(detail.clone());
                    view.detail.finish(Ok(detail));
                }
                Err(error) => view.finish_failed_load(error),
            },
        ));
        cx.notify();
    }

    /// Lists the commits after the first page in the background when
    /// `detail` lacks some: the page painted without them, and only the
    /// Commits tab needs them. A load already running for the same head and
    /// base goes on.
    fn load_commits(&mut self, detail: &PullRequestDetail, cx: &mut Context<Self>) {
        let range = format!("{}...{}", detail.base_oid, detail.head_oid);
        if !model::commits_missing(detail) {
            self.commits_load = None;
            self.commits_error = None;
            return;
        }
        if self
            .commits_load
            .as_ref()
            .is_some_and(|(loading, _)| *loading == range)
        {
            return;
        }
        self.commits_error = None;
        let key = crate::cache::github_key(cx, &format!("pr/{}", self.pr.url()));
        let request = detail.clone();
        let loaded = range.clone();
        let task = common::run(
            cx,
            move |gh| gh.pull_request_commits(&request),
            move |view, result, _cx| {
                view.commits_load = None;
                let Some(shown) = view.detail.value_mut() else {
                    return;
                };
                if format!("{}...{}", shown.base_oid, shown.head_oid) != loaded {
                    return;
                }
                match result {
                    Ok(commits) => {
                        let mut whole = PullRequestDetail::clone(shown);
                        whole.commits = model::with_fresh(commits, &shown.commits);
                        if let Some(key) = &key {
                            crate::cache::put(key, &whole);
                        }
                        *shown = Arc::new(whole);
                    }
                    Err(error) => view.commits_error = Some(error),
                }
            },
        );
        self.commits_load = Some((range, task));
    }

    fn finish_failed_load(&mut self, error: LoadError) {
        self.pr_seen = PrSeen::Stale;
        self.incoming_etag = None;
        match self.detail.value().cloned() {
            Some(stale) => {
                self.detail = Loadable::Loaded(stale);
                // `write` also guards a write in flight: overwriting `Busy`
                // would enable the buttons and allow a second submit.
                if !self.write.is_busy() {
                    self.write = Submit::Failed(format!("Could not refresh: {}", error.detail()));
                }
            }
            None => self.detail.finish(Err(error)),
        }
    }

    /// Hands a fresh detail to the tabs, creating them and the side panels on
    /// the first load.
    fn install(
        &mut self,
        detail: Arc<PullRequestDetail>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.panel.still_valid(&detail) {
            self.panel = Panel::Closed;
        }
        let link_context = PrLinkContext {
            pr: self.pr.clone(),
            ..PrLinkContext::from_detail(&detail)
        };
        if let Some(commit) = &self.commit {
            commit
                .files
                .update(cx, |tab, cx| tab.set_detail(detail.clone(), window, cx));
        }
        if let Some(loaded) = &mut self.loaded {
            loaded
                .conversation
                .update(cx, |tab, cx| tab.set_detail(detail.clone(), window, cx));
            loaded
                .guide
                .update(cx, |tab, cx| tab.set_detail(detail.clone(), window, cx));
            loaded
                .files
                .update(cx, |tab, cx| tab.set_detail(detail.clone(), window, cx));
            loaded
                .checks
                .update(cx, |tab, cx| tab.set_detail(detail, cx));
            if loaded.link_context != link_context {
                loaded.linear = crate::linear::pr_linked_issues(link_context.clone(), window, cx);
                loaded.t3 = crate::t3::pr_linked_threads(link_context.clone(), window, cx);
                loaded.link_context = link_context;
            }
            return;
        }
        let context = PrTabContext {
            pr: self.pr.clone(),
            detail: detail.clone(),
        };
        let conversation = cx.new(|cx| ConversationTab::new(context.clone(), window, cx));
        let guide = cx.new(|cx| GuideTab::new(context.clone(), window, cx));
        {
            let pr_view = cx.entity().downgrade();
            guide.update(cx, |tab, _| {
                tab.set_action_handler(move |action, window, cx| match action {
                    GuideAction::Generate => {
                        pr_view
                            .update(cx, |view, cx| view.trigger_guide_generation(cx))
                            .ok();
                    }
                    GuideAction::Cancel => {
                        pr_view
                            .update(cx, |view, cx| {
                                crate::github::guide_service::cancel_generation(&view.pr, cx);
                            })
                            .ok();
                    }
                    GuideAction::OpenSettings => {
                        crate::shell::navigate(crate::route::Route::Settings, window, cx);
                    }
                    GuideAction::RecheckClaude => {
                        crate::github::guide_service::recheck_availability(cx);
                    }
                });
            });
        }
        crate::github::guide_service::probe_availability_if_needed(cx);
        let files = cx.new(|cx| FilesTab::new(context.clone(), window, cx));
        let checks_scroll = self.checks_scroll.clone();
        let checks = cx.new(|cx| ChecksTab::new(context, checks_scroll, cx));
        self.subscriptions.push(cx.subscribe_in(
            &conversation,
            window,
            Self::reload_on_tab_event::<ConversationTab>,
        ));
        self.subscriptions.push(cx.subscribe_in(
            &guide,
            window,
            Self::reload_on_tab_event::<GuideTab>,
        ));
        self.subscriptions.push(cx.subscribe_in(
            &files,
            window,
            Self::reload_on_tab_event::<FilesTab>,
        ));
        self.subscriptions.push(cx.subscribe_in(
            &checks,
            window,
            Self::reload_on_tab_event::<ChecksTab>,
        ));
        let linear = crate::linear::pr_linked_issues(link_context.clone(), window, cx);
        let t3 = crate::t3::pr_linked_threads(link_context.clone(), window, cx);
        self.loaded = Some(Loaded {
            conversation,
            guide,
            files,
            checks,
            link_context,
            linear,
            t3,
        });
    }

    fn reload_on_tab_event<T: 'static>(
        &mut self,
        _tab: &Entity<T>,
        event: &PrTabEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PrTabEvent::Reload => self.reload(cx),
            PrTabEvent::ChecksRequested(jobs) => {
                self.rerun = Some((Instant::now(), jobs.clone()));
                self.reload(cx);
            }
            PrTabEvent::OpenFile { path, line, side } => {
                self.reveal_file(path, *line, *side, window, cx);
            }
            PrTabEvent::OpenFileIn {
                pr,
                path,
                line,
                side,
            } => {
                super::request_open_file(pr.clone(), path.clone(), *line, *side, cx);
                crate::shell::navigate(crate::route::Route::PullRequest(pr.clone()), window, cx);
            }
        }
    }

    /// Shows commit `oid`'s changes in the Files tab, or with `None` the
    /// whole pull request's again.
    fn show_commit(&mut self, oid: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = PrTab::Files;
        if self.commit.as_ref().map(|commit| commit.oid.as_str()) == oid {
            cx.notify();
            return;
        }
        if let Some(closed) = self.commit.take()
            && closed.files.read(cx).wrote_viewed()
            && let Some(loaded) = &self.loaded
        {
            loaded
                .files
                .update(cx, |files, cx| files.refresh_viewed(cx));
        }
        if let (Some(oid), Some(detail)) = (oid, self.detail.value()) {
            let context = PrTabContext {
                pr: self.pr.clone(),
                detail: detail.clone(),
            };
            let oid = oid.to_owned();
            let files = cx.new(|cx| FilesTab::for_commit(context, oid.clone(), window, cx));
            let events = cx.subscribe_in(&files, window, Self::reload_on_tab_event::<FilesTab>);
            self.commit = Some(CommitDiff {
                oid,
                files,
                _events: events,
            });
        }
        cx.notify();
    }

    /// Shows the Files tab at `path` (and `line` on `side` when given).
    fn reveal_file(
        &mut self,
        path: &str,
        line: Option<u32>,
        side: loupe_github::Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_commit(None, window, cx);
        if let Some(loaded) = &self.loaded {
            loaded.files.update(cx, |files, cx| {
                files.reveal_line(path, line, side, window, cx)
            });
        }
        cx.notify();
    }

    /// After a load: keeps the ETags of the poll that asked for it, tells
    /// the Reviews lists its checks and draft state, and stamps the time.
    fn settle_checks(&mut self, detail: &PullRequestDetail, cx: &mut Context<Self>) {
        let same_checks = self
            .detail
            .value()
            .is_some_and(|shown| shown.checks == detail.checks);
        if self
            .rerun
            .as_ref()
            .is_some_and(|(_, jobs)| poll::rerun_arrived(jobs, &detail.checks))
        {
            self.rerun = None;
        }
        if let Some(etag) = self.incoming_etag.take() {
            let after_a_change = self
                .checks_etag
                .as_ref()
                .is_some_and(|seen| seen.sha() == etag.sha());
            if same_checks && after_a_change && !self.retried_same_checks {
                self.retried_same_checks = true;
            } else {
                self.checks_etag = Some(etag);
                self.retried_same_checks = false;
            }
        }
        self.checked_at = Some(Local::now());
        let loaded = PrLoaded {
            pr: self.pr.clone(),
            checks: detail.checks.state,
            is_draft: detail.is_draft,
        };
        pr_feed::feed(cx).update(cx, |_, cx| cx.emit(loaded));
    }

    fn since_rerun(&self) -> Option<Duration> {
        self.rerun.as_ref().map(|(at, _)| at.elapsed())
    }

    /// Every `poll::TICK`: loads again a pull request that comes back on
    /// screen after a while, and polls its checks while they can change.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let on_screen = crate::shell::is_on_screen(cx.entity_id(), cx);
        let was_on_screen = std::mem::replace(&mut self.on_screen, on_screen);
        if self.detail.is_busy() || self.poll_task.is_some() || self.pr_poll_task.is_some() {
            return;
        }
        let Some(detail) = self.detail.value() else {
            return;
        };
        let since_last = self.last_fetch.elapsed();
        if poll::reload_on_return(was_on_screen, on_screen, since_last) {
            self.reload(cx);
            return;
        }
        let may_change = poll::checks_may_change(detail, self.since_rerun());
        let rate = common::client(cx)
            .map(|github| github.rate_limits())
            .unwrap_or_default();
        let decision = poll::decide(may_change, on_screen, since_last, rate, Utc::now());
        let paused_until = match decision {
            poll::Poll::Paused(until) => Some(until),
            _ => None,
        };
        if paused_until != self.paused_until {
            self.paused_until = paused_until;
            cx.notify();
        }
        if decision == poll::Poll::Now {
            self.poll(cx);
        } else if crate::github::live::paused_until(rate, Utc::now()).is_none()
            && poll::pr_poll_due(self.pr_polled_at.map(|at| at.elapsed()), on_screen)
        {
            self.poll_pull_request(cx);
        }
    }

    /// Asks GitHub whether the pull request changed since it loaded (a 304
    /// costs no rate limit) and loads it again when it did. The first poll
    /// after a load only takes the ETag.
    fn poll_pull_request(&mut self, cx: &mut Context<Self>) {
        let pr = self.pr.clone();
        let seen = match &self.pr_seen {
            PrSeen::Etag(etag) => Some(etag.clone()),
            PrSeen::Unknown | PrSeen::Stale => None,
        };
        self.pr_polled_at = Some(Instant::now());
        self.pr_poll_task = Some(common::run(
            cx,
            move |github| github.poll_pull_request(&pr, seen.as_deref()),
            |view, result, cx| {
                view.pr_poll_task = None;
                let Ok(PullRequestPoll::Changed(etag)) = result else {
                    return;
                };
                if !matches!(view.pr_seen, PrSeen::Unknown) {
                    view.reload(cx);
                }
                view.pr_seen = etag.map_or(PrSeen::Unknown, PrSeen::Etag);
                view.pr_polled_at = Some(Instant::now());
            },
        ));
    }

    /// Asks GitHub whether the checks changed (a 304 costs no rate limit)
    /// and loads the pull request again when they did.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.value() else {
            return;
        };
        let repo = self.pr.repo.clone();
        let sha = detail.head_oid.clone();
        let seen = self.checks_etag.clone();
        self.last_fetch = Instant::now();
        self.poll_task = Some(common::run(
            cx,
            move |github| github.poll_checks(&repo, &sha, seen.as_ref()),
            |view, result, cx| {
                view.poll_task = None;
                eprintln!(
                    "checks-poll pr={} outcome={} timestamp={}",
                    view.pr.url(),
                    match &result {
                        Ok(ChecksPoll::Unchanged) => "unchanged",
                        Ok(ChecksPoll::Changed(_)) => "changed",
                        Err(_) => "failed",
                    },
                    Utc::now().to_rfc3339()
                );
                match result {
                    Ok(ChecksPoll::Unchanged) => view.checked_at = Some(Local::now()),
                    Ok(ChecksPoll::Changed(etag)) => {
                        view.incoming_etag = Some(etag);
                        view.reload(cx);
                    }
                    // Without the cheap answer, a full load still tells.
                    Err(_) => view.reload(cx),
                }
            },
        ));
        cx.notify();
    }

    fn freshness(&self) -> Freshness {
        Freshness {
            checked_at: self.checked_at,
            busy: self.detail.is_busy() || self.poll_task.is_some(),
            live: self
                .detail
                .value()
                .is_some_and(|detail| poll::checks_may_change(detail, self.since_rerun())),
            paused_until: self.paused_until.map(|until| until.with_timezone(&Local)),
        }
    }

    fn start_write(&mut self, write: Write, cx: &mut Context<Self>) {
        if self.write.is_busy() {
            return;
        }
        let Some(detail) = self.detail.value().cloned() else {
            return;
        };
        let clears_review_body = matches!(write, Write::Review { .. });
        let is_merge = matches!(write, Write::Merge(_));
        let marks_ready = matches!(write, Write::MarkReady);
        self.write = Submit::Busy;
        self.ready = if marks_ready {
            Submit::Busy
        } else {
            Submit::Idle
        };
        self.write_note = None;
        self.write_task = Some(common::run(
            cx,
            move |gh| write.perform(gh, &detail),
            move |view, result, cx| match result {
                Ok(note) => {
                    view.write = Submit::Idle;
                    view.ready = Submit::Idle;
                    view.write_note = note;
                    view.panel = Panel::Closed;
                    view.title_edit = None;
                    view.clear_review_body |= clears_review_body;
                    view.reload(cx);
                }
                Err(error) if marks_ready => {
                    view.write = Submit::Idle;
                    view.ready = Submit::Failed(error.detail());
                }
                Err(error) => {
                    view.write = Submit::Failed(error.detail());
                    // GitHub may still be merging after a network error while
                    // Loupe was reading the result; the reload shows the state.
                    if is_merge {
                        view.reload(cx);
                    }
                }
            },
        ));
        cx.notify();
    }

    fn submit_review(&mut self, event: ReviewEvent, cx: &mut Context<Self>) {
        if self.write.is_busy() {
            return;
        }
        let Some(detail) = self.detail.value().cloned() else {
            return;
        };
        let body = self.review_body.read(cx).value().trim_end().to_owned();
        let pending = detail.pending_review.as_ref();
        if let Some(reason) = review_blocker(event, &body, pending.map_or(0, |p| p.comment_count)) {
            self.write = Submit::Failed(reason.into());
            cx.notify();
            return;
        }
        let review_id = pending.map(|review| review.id.clone());
        self.start_write(
            Write::Review {
                event,
                body,
                review_id,
            },
            cx,
        );
    }

    fn start_title_edit(&mut self, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.title_edit = Some(input);
        cx.notify();
    }

    fn save_title(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.title_edit else {
            return;
        };
        let title = input.read(cx).value().trim().to_owned();
        if title.is_empty() {
            self.write = Submit::Failed("A pull request needs a title.".into());
            cx.notify();
            return;
        }
        self.start_write(Write::EditTitle(title), cx);
    }

    /// Opens or closes the label editor, loading the repository's labels the
    /// first time.
    fn toggle_labels_picker(&mut self, cx: &mut Context<Self>) {
        if self.labels_picker.take().is_some() {
            self.labels_task = None;
            cx.notify();
            return;
        }
        self.labels_picker = Some(Loadable::Loading);
        let repo = self.pr.repo.clone();
        self.labels_task = Some(common::run(
            cx,
            move |gh| gh.repository_labels(&repo),
            |view, result, _| {
                if let Some(picker) = &mut view.labels_picker {
                    picker.finish(result);
                }
            },
        ));
        cx.notify();
    }

    /// Adds `label` to the pull request, or removes it when it is there.
    fn toggle_label(&mut self, label: &Label, cx: &mut Context<Self>) {
        let present = self
            .detail
            .value()
            .is_some_and(|detail| detail.labels.iter().any(|shown| shown.id == label.id));
        let write = if present {
            Write::RemoveLabel(label.id.clone())
        } else {
            Write::AddLabel(label.id.clone())
        };
        self.start_write(write, cx);
    }

    fn load_stack_graph(&mut self, detail: &PullRequestDetail, cx: &mut Context<Self>) {
        let pr = detail.pr.clone();
        let base_ref = detail.base_ref.clone();
        let head_ref = detail.head_ref.clone();
        self.stack_loaded = false;
        self.stack_task = Some(common::run(
            cx,
            move |gh| gh.pull_request_stack(&pr, &base_ref, &head_ref),
            |view, result, cx| {
                view.stack_loaded = true;
                if let Ok(graph) = result {
                    view.stack_graph = graph;
                }
                cx.notify();
            },
        ));
    }

    /// Consumes a pending "Merge stack" request from Reviews once this PR's
    /// stack lookup finishes. Always clears the global for `&self.pr` so an
    /// unfulfilled request never fires on a later visit. If Reviews opened a
    /// non-root PR (e.g. the real root was not in the Reviews list), navigates
    /// to the true root PR with the request forwarded.
    fn maybe_open_requested_stack_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.stack_loaded || !super::take_stack_merge_confirm(&self.pr, cx) {
            return;
        }
        let Some(graph) = &self.stack_graph else {
            return;
        };
        if !graph.is_root(self.pr.number) {
            let root_pr = PrRef::new(self.pr.repo.clone(), graph.root);
            super::request_stack_merge_confirm(root_pr.clone(), cx);
            crate::shell::navigate(crate::route::Route::PullRequest(root_pr), window, cx);
            return;
        }
        let Some(detail) = self.detail.value() else {
            return;
        };
        if detail.state != PrState::Open || !detail.viewer_can_update {
            return;
        }
        if let Some(method) = model::resolve_method(&detail.merge_methods, self.merge_method) {
            self.panel = Panel::ConfirmStackMerge(method);
            cx.notify();
        }
    }

    fn start_stack_merge(
        &mut self,
        method: MergeMethod,
        selected: Vec<u64>,
        cx: &mut Context<Self>,
    ) {
        if self.write.is_busy() || self.stack_merge_run.as_ref().is_some_and(|run| run.running) {
            return;
        }
        let Some(graph) = &self.stack_graph else {
            return;
        };
        let plan = graph.merge_plan();
        let steps = plan.selected_steps(&selected);
        if steps.is_empty() {
            return;
        }
        let statuses: HashMap<u64, StepStatus> = steps
            .iter()
            .map(|step| (step.node.pr.number, StepStatus::Waiting))
            .collect();
        let state = StackMergeState::from_steps(&steps);
        self.stack_merge_run = Some(StackMergeRun {
            plan,
            selected,
            steps,
            method,
            statuses,
            state,
            running: true,
            failed_pr: None,
            can_retry: false,
        });
        self.panel = Panel::StackMergeProgress;
        self.write = Submit::Busy;
        self.write_note = None;
        self.drive_stack_merge(None, cx);
        cx.notify();
    }

    fn retry_stack_merge(&mut self, cx: &mut Context<Self>) {
        if self.write.is_busy() || self.stack_merge_run.as_ref().is_some_and(|run| run.running) {
            return;
        }
        let Some(run) = &mut self.stack_merge_run else {
            return;
        };
        if !run.can_retry {
            return;
        }
        let retry_from = run.failed_pr;
        for step in &run.steps {
            let num = step.node.pr.number;
            if !run.state.merged.contains(&num) {
                run.statuses.insert(num, StepStatus::Waiting);
            }
        }
        run.running = true;
        run.failed_pr = None;
        run.can_retry = false;
        self.write = Submit::Busy;
        self.drive_stack_merge(retry_from, cx);
        cx.notify();
    }

    fn dismiss_stack_merge_progress(&mut self, cx: &mut Context<Self>) {
        if self.stack_merge_run.as_ref().is_some_and(|run| run.running) {
            return;
        }
        self.stack_merge_run = None;
        if self.panel == Panel::StackMergeProgress {
            self.panel = Panel::Closed;
        }
        self.reload(cx);
        cx.notify();
    }

    /// Drives the stack merge using `loupe_github::stack_merge::run_stack_merge`
    /// on a background thread and streams [`StackMergeEvent`]s back to the view.
    fn drive_stack_merge(&mut self, retry_from: Option<u64>, cx: &mut Context<Self>) {
        let Some(github) = common::client(cx) else {
            if let Some(run) = &mut self.stack_merge_run {
                if let Some(first) = run
                    .steps
                    .iter()
                    .find(|s| !run.state.merged.contains(&s.node.pr.number))
                {
                    let num = first.node.pr.number;
                    run.statuses
                        .insert(num, StepStatus::Failed("GitHub is not connected.".into()));
                    run.failed_pr = Some(num);
                    run.can_retry = true;
                }
                run.running = false;
            }
            self.write = Submit::Idle;
            cx.notify();
            return;
        };
        let Some(run) = &self.stack_merge_run else {
            return;
        };
        let plan = run.plan.clone();
        let selected = run.selected.clone();
        let method = run.method;
        let mut state = run.state.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<StackMergeEvent>();

        let worker = cx.background_executor().spawn(async move {
            let outcome = run_stack_merge(
                &*github,
                &plan,
                &selected,
                method,
                &mut state,
                retry_from,
                |event| {
                    let _ = tx.unbounded_send(event);
                },
            );
            drop(tx);
            (state, outcome)
        });

        self.stack_merge_task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |view, cx| {
                        if let Some(run) = &mut view.stack_merge_run {
                            match event {
                                StackMergeEvent::Retargeting(num) => {
                                    run.statuses.insert(num, StepStatus::Retargeting);
                                }
                                StackMergeEvent::WaitingForChecks(num) => {
                                    run.statuses.insert(num, StepStatus::WaitingForChecks);
                                }
                                StackMergeEvent::Merging(num) => {
                                    run.statuses.insert(num, StepStatus::Merging);
                                }
                                StackMergeEvent::Merged(num) => {
                                    run.statuses.insert(num, StepStatus::Merged);
                                }
                                StackMergeEvent::Failed {
                                    number,
                                    reason,
                                    can_retry,
                                } => {
                                    run.statuses.insert(number, StepStatus::Failed(reason));
                                    run.failed_pr = Some(number);
                                    run.can_retry = can_retry;
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let (final_state, outcome) = worker.await;
            let _ = this.update(cx, |view, cx| {
                if let Some(run) = &mut view.stack_merge_run {
                    run.state = final_state;
                    run.running = false;
                    match outcome {
                        Ok(()) => {
                            run.failed_pr = None;
                            run.can_retry = false;
                        }
                        Err(failure) => {
                            run.failed_pr = Some(failure.number);
                            run.can_retry = failure.can_retry;
                        }
                    }
                }
                view.write = Submit::Idle;
                view.reload(cx);
                cx.notify();
            });
        }));
    }

    pub(super) fn summarize_pull_request(&mut self, cx: &mut Context<Self>) {
        self.tab = PrTab::Guide;
        let stack = self.stack_graph.as_ref();
        if let Some(detail) = self.detail.value().cloned()
            && !crate::github::guide_service::is_generating(cx, &detail.pr)
            && !crate::github::guide_service::has_fresh_scoped_guide(cx, &detail, stack)
        {
            crate::github::guide_service::start_scoped_generation(detail, stack, cx);
        }
        cx.notify();
    }

    /// Summarize the whole stack or this pull request alone, for this session.
    pub(super) fn summarize_scope(
        &mut self,
        scope: crate::github::guide_service::GuideScope,
        cx: &mut Context<Self>,
    ) {
        crate::github::guide_service::set_scope_for(&self.pr, scope, cx);
        self.summarize_pull_request(cx);
    }

    /// Summarize in `language` for this pull request, without changing Settings.
    pub(super) fn summarize_in(
        &mut self,
        language: loupe_guide::GuideLanguage,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.detail.value().cloned() {
            crate::github::guide_service::set_language_for(&detail, language, cx);
        }
        self.summarize_pull_request(cx);
    }

    pub(super) fn trigger_guide_generation(&mut self, cx: &mut Context<Self>) {
        if let Some(detail) = self.detail.value().cloned() {
            crate::github::guide_service::start_scoped_generation(
                detail,
                self.stack_graph.as_ref(),
                cx,
            );
        }
    }

    fn set_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == panel {
            Panel::Closed
        } else {
            panel
        };
        self.focus_review_body = self.panel == Panel::Review;
        if self.write.error().is_some() {
            self.write = Submit::Idle;
        }
        cx.notify();
    }

    /// The page header before the pull request is known: its reference as
    /// the title.
    fn placeholder_header(&self) -> impl IntoElement {
        ui::page_header(format!("{}", self.pr)).icon(gpui_kit::assets::IconName::GitPullRequest)
    }

    /// The page in the shape it will load into, so nothing moves when the
    /// pull request arrives: the state and branch band, the tabs (named from
    /// the first frame), the status card, the description, and the side
    /// column's sections.
    fn render_loading(&self, cx: &App) -> Div {
        let border = cx.theme().border;
        let tabs = PrTab::ALL.into_iter().map(|tab| {
            ui::section_tab(("pr-tab-loading", tab.index()), tab.title()).icon(tab.icon())
        });
        let line = |width: f32| Skeleton::new().secondary().h_3().w(relative(width));
        let side_section = |_| {
            v_flex()
                .gap_2()
                .child(Skeleton::new().h_3().w_20())
                .child(Skeleton::new().secondary().h_3().w_32())
        };
        v_flex()
            .size_full()
            .child(self.placeholder_header())
            .child(
                h_flex()
                    .flex_none()
                    .h(scale::band_height())
                    .px(scale::content_inset())
                    .gap_2()
                    .child(
                        Skeleton::new()
                            .h(scale::chip_height())
                            .w_16()
                            .rounded_full(),
                    )
                    .child(Skeleton::new().size_4().rounded_full())
                    .child(Skeleton::new().secondary().h_3().w_24())
                    .child(Skeleton::new().secondary().h(scale::chip_height()).w_32()),
            )
            .child(ui::section_tabs("pr-tabs-loading", self.tab.index()).tabs(tabs))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .p(scale::content_inset())
                            .gap_4()
                            .child(
                                ui::card(cx)
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(Skeleton::new().size_6().rounded_full())
                                            .child(Skeleton::new().h_3().w_32()),
                                    )
                                    .child(line(0.35)),
                            )
                            .child(
                                ui::card(cx)
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(Skeleton::new().size_4().rounded_full())
                                            .child(Skeleton::new().h_3().w_40()),
                                    )
                                    .child(line(0.8))
                                    .child(line(0.72))
                                    .child(line(0.6))
                                    .child(line(0.4)),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_none()
                            .w(scale::side_width())
                            .h_full()
                            .p_4()
                            .gap_6()
                            .border_l_1()
                            .border_color(border)
                            .children((0..4).map(side_section)),
                    ),
            )
    }

    fn render_failed(&self, error: &LoadError, cx: &mut Context<Self>) -> Div {
        v_flex()
            .size_full()
            .child(self.placeholder_header())
            .child(common::load_failed_page(
                error,
                "Pull request",
                self.pr.url(),
                cx.listener(|this, _, _, cx| this.reload(cx)),
            ))
    }

    fn render_tabs(
        &self,
        detail: &PullRequestDetail,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let guide_count = self
            .loaded
            .as_ref()
            .and_then(|loaded| loaded.guide.read(cx).step_count());
        let tabs = PrTab::ALL.into_iter().map(|tab| {
            let count = match tab {
                PrTab::Guide => guide_count,
                _ => tab.count(detail),
            };
            ui::section_tab(("pr-tab", tab.index()), tab.title())
                .icon(tab.icon())
                .maybe_count(count)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }))
        });
        let details = (narrow && self.tab.has_side_column()).then(|| {
            let name = if self.details_open {
                "Hide details"
            } else {
                "Show details"
            };
            ui::icon_button(
                "pr-details-toggle",
                gpui_kit::assets::IconName::PanelRight,
                name,
            )
            .label("Details")
            .selected(self.details_open)
            .on_click(cx.listener(|this, _, _, cx| {
                this.details_open = !this.details_open;
                cx.notify();
            }))
        });
        ui::section_tabs("pr-tabs", self.tab.index())
            .tabs(tabs)
            .maybe_trailing(details)
    }

    fn render_tab_content(
        &self,
        detail: &PullRequestDetail,
        loaded: &Loaded,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scrolling = |id: &'static str, handle: &ScrollHandle, content: AnyElement| {
            div()
                .id(id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(handle)
                .child(content)
                .into_any_element()
        };
        let view = cx.entity().downgrade();
        let show_commit: commits::OnShow = Rc::new(move |oid, window, cx| {
            view.update(cx, |view, cx| view.show_commit(oid, window, cx))
                .ok();
        });
        match self.tab {
            PrTab::Conversation => scrolling(
                "pr-conversation",
                &self.conversation_scroll,
                v_flex()
                    .w_full()
                    .child(self.render_merge_box(detail, cx))
                    .children(
                        self.stack_graph
                            .as_ref()
                            .map(|graph| stack_panel::render_stack_panel(graph, &self.pr, cx)),
                    )
                    .child(loaded.conversation.clone())
                    .into_any_element(),
            ),
            PrTab::Guide => scrolling(
                "pr-guide",
                &self.guide_scroll,
                loaded.guide.clone().into_any_element(),
            ),
            PrTab::Files => match &self.commit {
                Some(commit) => v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(commits::commit_bar(detail, &commit.oid, show_commit, cx))
                    .child(div().flex_1().min_h_0().child(commit.files.clone()))
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_h_0()
                    .child(loaded.files.clone())
                    .into_any_element(),
            },
            PrTab::Commits => {
                let rest = match (&self.commits_load, &self.commits_error) {
                    (Some(_), _) => commits::Rest::Loading,
                    (None, Some(error)) => commits::Rest::Failed(error.message().into()),
                    (None, None) => commits::Rest::Settled,
                };
                let retry: commits::OnRetry = Rc::new(cx.listener(
                    |view: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        if let Some(detail) = view.detail.value().cloned() {
                            view.load_commits(&detail, cx);
                            cx.notify();
                        }
                    },
                ));
                scrolling(
                    "pr-commits",
                    &self.commits_scroll,
                    commits::render(detail, rest, show_commit, retry, cx),
                )
            }
            PrTab::Checks => scrolling(
                "pr-checks",
                &self.checks_scroll,
                loaded.checks.clone().into_any_element(),
            ),
        }
    }
}

impl crate::shell::Refreshable for PullRequestView {
    fn refresh_page(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }
}

impl Render for PullRequestView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The palette and the item keys act on the pull request on screen.
        let shown = self.detail.value().map(|detail| {
            Item::PullRequest(PrItem {
                pr: detail.pr.clone(),
                title: detail.title.clone(),
                url: detail.url.clone(),
                head_ref: Some(detail.head_ref.clone()),
            })
        });
        item_menu::set_page_item(cx.entity_id(), shown, cx);
        if !self.paint_logged && self.detail.value().is_some() {
            eprintln!(
                "review-paint pr={} elapsed_ms={} timestamp={}",
                self.pr.url(),
                self.paint_started.elapsed().as_millis(),
                chrono::Utc::now().to_rfc3339()
            );
            self.paint_logged = true;
        }
        if let Some(detail) = self.incoming.take() {
            self.install(detail, window, cx);
        }
        if self.loaded.is_some()
            && let Some((path, line, side)) = super::take_open_file(&self.pr, cx)
        {
            self.reveal_file(&path, line, side, window, cx);
        }
        self.maybe_open_requested_stack_confirm(window, cx);
        if let Some(loaded) = &self.loaded {
            let freshness = self.freshness();
            loaded
                .checks
                .update(cx, |tab, cx| tab.set_freshness(freshness, cx));
            if let Some(detail) = self.detail.value() {
                let sync_key = (
                    detail.head_oid.clone(),
                    crate::github::guide_service::revision(cx),
                    crate::github::guide_service::wanted_language(cx, detail),
                    self.stack_graph.clone(),
                );
                if self.guide_sync.as_ref() != Some(&sync_key) {
                    let guide_state = crate::github::guide_service::view_state(
                        cx,
                        detail,
                        self.stack_graph.as_ref(),
                    );
                    loaded
                        .guide
                        .update(cx, |tab, cx| tab.set_state(guide_state, cx));
                    self.guide_sync = Some(sync_key);
                }
            }
        }
        if std::mem::take(&mut self.focus_review_body) {
            let summary = self.review_body.read(cx).focus_handle(cx);
            summary.focus(window, cx);
        }
        if std::mem::take(&mut self.clear_review_body) {
            self.review_body
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        let (Some(detail), Some(loaded)) = (self.detail.value().cloned(), self.loaded.clone())
        else {
            return match &self.detail {
                Loadable::Failed(error) => self.render_failed(error, cx),
                _ => self.render_loading(cx),
            }
            .into_any_element();
        };
        let narrow = window.viewport_size().width
            < scale::side_column_breakpoint().to_pixels(window.rem_size());
        let side = self.tab.has_side_column() && (!narrow || self.details_open);
        v_flex()
            .size_full()
            .child(self.render_header(&detail, cx))
            .children(self.render_title_edit(cx))
            .child(self.render_panel_band(&detail, cx))
            .child(self.render_tabs(&detail, narrow, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_tab_content(&detail, &loaded, cx)),
                    )
                    .when(side, |row| {
                        row.child(self.render_side(&detail, &loaded, cx))
                    }),
            )
            .into_any_element()
    }
}
