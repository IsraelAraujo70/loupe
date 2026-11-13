//! The pull request view: loads the detail, owns the tabs, and wires the
//! header, toolbar (review and actions), and side column around them.

mod header;
mod merge_box;
mod model;
mod poll;
mod side;
mod toolbar;

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{
    ActionsJob, ChecksEtag, ChecksPoll, GitHub, Label, MergeMethod, MergeOutcome, PrRef,
    PullRequestDetail, ReviewEvent,
};

use super::checks::{ChecksTab, Freshness};
use super::commits;
use super::conversation::ConversationTab;
use super::files::FilesTab;
use super::tab::{PrTabContext, PrTabEvent};
use crate::github::checks_feed::{self, ChecksLoaded};
use crate::github::common::{self, LoadError, Loadable, Submit};
use crate::links::PrLinkContext;
use crate::shell::item_menu::{self, Item, PrItem};
use crate::theme::scale;
use crate::ui;
use model::{Panel, PrTab, review_blocker};

/// What exists only after the first successful load.
#[derive(Clone)]
struct Loaded {
    conversation: Entity<ConversationTab>,
    files: Entity<FilesTab>,
    checks: Entity<ChecksTab>,
    /// What the Linear and T3 panels were built from; they are rebuilt when
    /// a refresh changes it.
    link_context: PrLinkContext,
    linear: AnyView,
    t3: AnyView,
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

pub struct PullRequestView {
    paint_started: std::time::Instant,
    paint_logged: bool,
    pr: PrRef,
    detail: Loadable<Arc<PullRequestDetail>>,
    tab: PrTab,
    loaded: Option<Loaded>,
    /// A detail that arrived without a window to hand it to the tabs; the
    /// next render installs it.
    incoming: Option<Arc<PullRequestDetail>>,
    panel: Panel,
    merge_method: Option<MergeMethod>,
    write: Submit,
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
    commits_scroll: ScrollHandle,
    checks_scroll: ScrollHandle,
    side_scroll: ScrollHandle,
    load_task: Option<Task<()>>,
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
    /// Whether the last tick found the pull request on screen.
    on_screen: bool,
    poll_task: Option<Task<()>>,
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
            tab: PrTab::Conversation,
            loaded: None,
            incoming: None,
            panel: Panel::Closed,
            merge_method: None,
            write: Submit::Idle,
            title_edit: None,
            labels_picker: None,
            labels_task: None,
            review_body,
            clear_review_body: false,
            focus_review_body: false,
            conversation_scroll: ScrollHandle::new(),
            commits_scroll: ScrollHandle::new(),
            checks_scroll: ScrollHandle::new(),
            side_scroll: ScrollHandle::new(),
            load_task: None,
            write_task: None,
            subscriptions: vec![submit_on_secondary_enter],
            last_fetch: Instant::now(),
            checked_at: None,
            checks_etag: None,
            incoming_etag: None,
            retried_same_checks: false,
            rerun: None,
            paused_until: None,
            on_screen: true,
            poll_task: None,
            _ticker: ticker,
        };
        let page = cx.entity_id();
        cx.on_release(move |_, cx| item_menu::set_page_item(page, None, cx))
            .detach();
        view.reload(cx);
        view
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
            self.incoming = Some(detail.clone());
            self.detail = Loadable::Loaded(detail);
        }
        self.detail.start_refresh();
        self.last_fetch = Instant::now();
        let pr = self.pr.clone();
        self.load_task = Some(common::run(
            cx,
            move |gh| gh.pull_request(&pr),
            move |view, result, _cx| match result {
                Ok(detail) => {
                    if let Some(key) = &key {
                        crate::cache::put(key, &detail);
                    }
                    if let Some(key) = crate::cache::github_key(_cx, "reviews")
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
                    let detail = Arc::new(detail);
                    view.incoming = Some(detail.clone());
                    view.detail.finish(Ok(detail));
                }
                Err(error) => view.finish_failed_load(error),
            },
        ));
        cx.notify();
    }

    fn finish_failed_load(&mut self, error: LoadError) {
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
            url: detail.url.clone(),
            title: detail.title.clone(),
            body: detail.body.clone(),
            head_ref: detail.head_ref.clone(),
            head_oid: detail.head_oid.clone(),
            head_fork: detail.head_repo.as_ref().map(ToString::to_string),
            author: detail.author.as_ref().map(|author| author.login.clone()),
            viewer_login: detail.viewer_login.clone(),
            labels: detail
                .labels
                .iter()
                .map(|label| label.name.clone())
                .collect(),
        };
        if let Some(loaded) = &mut self.loaded {
            loaded
                .conversation
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
        let files = cx.new(|cx| FilesTab::new(context.clone(), window, cx));
        let checks = cx.new(|_| ChecksTab::new(context));
        self.subscriptions.push(cx.subscribe_in(
            &conversation,
            window,
            Self::reload_on_tab_event::<ConversationTab>,
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PrTabEvent::Reload => self.reload(cx),
            PrTabEvent::ChecksRequested(jobs) => {
                self.rerun = Some((Instant::now(), jobs.clone()));
                self.reload(cx);
            }
        }
    }

    /// After a load: keeps the ETags of the poll that asked for it, tells
    /// the Reviews lists, and stamps the time.
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
        let loaded = ChecksLoaded {
            pr: self.pr.clone(),
            checks: detail.checks.state,
        };
        checks_feed::feed(cx).update(cx, |_, cx| cx.emit(loaded));
    }

    fn since_rerun(&self) -> Option<Duration> {
        self.rerun.as_ref().map(|(at, _)| at.elapsed())
    }

    /// Every `poll::TICK`: loads again a pull request that comes back on
    /// screen after a while, and polls its checks while they can change.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let on_screen = crate::shell::is_on_screen(cx.entity_id(), cx);
        let was_on_screen = std::mem::replace(&mut self.on_screen, on_screen);
        if self.detail.is_busy() || self.poll_task.is_some() {
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
        }
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
        self.write = Submit::Busy;
        self.write_task = Some(common::run(
            cx,
            move |gh| write.perform(gh, &detail),
            move |view, result, cx| match result {
                Ok(note) => {
                    view.write = note.map_or(Submit::Idle, |note| Submit::Failed(note.into()));
                    view.panel = Panel::Closed;
                    view.title_edit = None;
                    view.clear_review_body |= clears_review_body;
                    view.reload(cx);
                }
                Err(error) => view.write = Submit::Failed(error.detail()),
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

    fn render_loading(&self, cx: &App) -> Div {
        let border = cx.theme().border;
        v_flex()
            .size_full()
            .child(self.placeholder_header())
            .child(
                h_flex()
                    .flex_none()
                    .px(scale::content_inset())
                    .py_2()
                    .gap_3()
                    .border_b_1()
                    .border_color(border)
                    .child(Skeleton::new().size_4().rounded_full())
                    .child(Skeleton::new().h_3().w_40())
                    .child(Skeleton::new().secondary().h_3().w_24()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(div().flex_1().py_3().child(ui::skeleton_rows(8)))
                    .child(
                        v_flex()
                            .w(scale::side_width())
                            .h_full()
                            .p_4()
                            .gap_3()
                            .border_l_1()
                            .border_color(border)
                            .child(Skeleton::new().h_3().w_20())
                            .child(Skeleton::new().secondary().h_3().w_32())
                            .child(Skeleton::new().h_3().w_20()),
                    ),
            )
    }

    fn render_failed(&self, error: &LoadError, cx: &mut Context<Self>) -> Div {
        v_flex()
            .size_full()
            .child(self.placeholder_header())
            .child(common::load_error(
                error,
                cx.listener(|this, _, _, cx| this.reload(cx)),
            ))
    }

    fn render_tabs(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_none()
            .h(scale::toolbar_height())
            .px(scale::content_inset())
            .items_end()
            .border_b_1()
            .border_color(crate::theme::tokens::border_subtle())
            .child(
                TabBar::new("pr-tabs")
                    .underline()
                    .small()
                    .selected_index(self.tab.index())
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        if let Some(tab) = PrTab::from_index(*index) {
                            this.tab = tab;
                            cx.notify();
                        }
                    }))
                    .children(PrTab::ALL.map(|tab| {
                        Tab::new()
                            // An icon prefix keeps the label; `Tab::icon` drops it.
                            .prefix(gpui_kit::component::Icon::new(tab.icon()).small())
                            .label(tab.title())
                            .when_some(tab.count(detail), |item, count| {
                                item.suffix(crate::ui::count_badge(count, cx).ml_1())
                            })
                    })),
            )
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
        match self.tab {
            PrTab::Conversation => scrolling(
                "pr-conversation",
                &self.conversation_scroll,
                v_flex()
                    .w_full()
                    .child(self.render_merge_box(detail, cx))
                    .child(loaded.conversation.clone())
                    .into_any_element(),
            ),
            PrTab::Files => div()
                .flex_1()
                .min_h_0()
                .child(loaded.files.clone())
                .into_any_element(),
            PrTab::Commits => scrolling(
                "pr-commits",
                &self.commits_scroll,
                commits::render(detail, cx),
            ),
            PrTab::Checks => scrolling(
                "pr-checks",
                &self.checks_scroll,
                loaded.checks.clone().into_any_element(),
            ),
        }
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
        if let Some(loaded) = &self.loaded {
            let freshness = self.freshness();
            loaded
                .checks
                .update(cx, |tab, cx| tab.set_freshness(freshness, cx));
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
        v_flex()
            .size_full()
            .child(self.render_header(&detail, cx))
            .children(self.render_title_edit(cx))
            .child(self.render_panel_band(&detail, cx))
            .child(self.render_tabs(&detail, cx))
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
                    .when(self.tab.has_side_column(), |row| {
                        row.child(self.render_side(&detail, &loaded, cx))
                    }),
            )
            .into_any_element()
    }
}
