//! `Route::Notifications`: notifications grouped by repository. A failed
//! workflow run names only its workflow and branch, so the screen asks
//! GitHub for the branch's pull request and failed runs and shows those:
//! which pull request, which run, which job.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{
    BranchCi, BranchPullRequest, FailedRun, GitHub, Notification, NotificationSubject, RepoRef,
};

use super::common::{self, Loadable, Submit};
use super::notification_filter::{self as filter, Term};
use super::{CloseStack, OpenStack};
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::shell::Refreshable as _;
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::shell::item_menu::{Item, PrItem, with_item_menu};
use crate::store::{Settings, update_settings};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::{self, ListRowFocus as _};

/// Repository groups, newest group first; inside a group unread first, then
/// newest first.
fn group(notifications: &[Notification]) -> Vec<(RepoRef, Vec<&Notification>)> {
    let mut groups: HashMap<&RepoRef, Vec<&Notification>> = HashMap::new();
    for notification in notifications {
        groups
            .entry(&notification.repo)
            .or_default()
            .push(notification);
    }
    let mut groups: Vec<(RepoRef, Vec<&Notification>)> = groups
        .into_iter()
        .map(|(repo, mut items)| {
            items.sort_by(|a, b| {
                b.unread
                    .cmp(&a.unread)
                    .then(b.updated_at.cmp(&a.updated_at))
            });
            (repo.clone(), items)
        })
        .collect();
    groups.sort_by(|(a_repo, a), (b_repo, b)| {
        let newest = |items: &[&Notification]| items.iter().map(|n| n.updated_at).max();
        newest(b).cmp(&newest(a)).then(a_repo.cmp(b_repo))
    });
    groups
}

/// Width of the trailing reason lane ("Review requested").
const REASON_LANE: f32 = 120.;
/// Width of the lane for a row's age ("3mo").
const AGE_LANE: f32 = 40.;

/// A GitHub Actions notification, read out of its title: "CI workflow run
/// failed for feat/x branch".
#[derive(Debug, PartialEq, Eq)]
struct WorkflowRun<'a> {
    workflow: &'a str,
    outcome: &'a str,
    branch: &'a str,
}

fn workflow_run(title: &str) -> Option<WorkflowRun<'_>> {
    let (workflow, rest) = title.split_once(" workflow run ")?;
    let (outcome, rest) = rest.split_once(" for ")?;
    let branch = rest.strip_suffix(" branch")?;
    Some(WorkflowRun {
        workflow,
        outcome,
        branch,
    })
}

/// A repository and a branch of it that a failed run names.
type BranchKey = (RepoRef, String);

/// Branches whose pull request and runs are read for one list, at most:
/// each costs a few requests.
const MAX_BRANCHES: usize = 12;

/// The branches the list's failed workflow runs name, each once, in the
/// list's order.
fn failed_branches(list: &[Notification]) -> Vec<BranchKey> {
    let mut branches: Vec<BranchKey> = Vec::new();
    for notification in list {
        let Some(run) = workflow_run(&notification.title).filter(|run| run.outcome == "failed")
        else {
            continue;
        };
        let key = (notification.repo.clone(), run.branch.to_owned());
        if !branches.contains(&key) {
            branches.push(key);
        }
    }
    branches
}

/// The failed run each notification is about, by notification id. GitHub
/// sends the notification seconds after the run ends, so a notification
/// takes the run of its workflow and branch that finished closest to it,
/// within ten minutes; the closest pairs are matched first and a run is
/// given to one notification only, so two runs that ended seconds apart
/// each get their own.
fn match_runs(
    list: &[Notification],
    ci: &HashMap<BranchKey, BranchCi>,
) -> HashMap<String, FailedRun> {
    const WINDOW_SECONDS: i64 = 600;
    // (seconds apart, notification, branch, run)
    let mut pairs: Vec<(i64, usize, &BranchKey, usize)> = Vec::new();
    for (index, notification) in list.iter().enumerate() {
        let Some(run) = workflow_run(&notification.title).filter(|run| run.outcome == "failed")
        else {
            continue;
        };
        let Some((key, branch)) =
            ci.get_key_value(&(notification.repo.clone(), run.branch.to_owned()))
        else {
            continue;
        };
        for (position, failed) in branch.failed_runs.iter().enumerate() {
            let apart = (failed.finished_at - notification.updated_at)
                .num_seconds()
                .abs();
            if failed.workflow == run.workflow && apart <= WINDOW_SECONDS {
                pairs.push((apart, index, key, position));
            }
        }
    }
    pairs.sort_by_key(|(apart, index, _, position)| (*apart, *index, *position));
    let mut matched: HashMap<String, FailedRun> = HashMap::new();
    let mut taken: HashSet<(&BranchKey, usize)> = HashSet::new();
    for (_, index, key, position) in pairs {
        let id = &list[index].id;
        if matched.contains_key(id) || !taken.insert((key, position)) {
            continue;
        }
        matched.insert(id.clone(), ci[key].failed_runs[position].clone());
    }
    matched
}

/// "check" or "check, e2e +2": the jobs that failed, the first two by name.
fn failed_jobs_text(jobs: &[String]) -> Option<String> {
    match jobs {
        [] => None,
        [one] => Some(one.clone()),
        [one, two] => Some(format!("{one}, {two}")),
        [one, two, rest @ ..] => Some(format!("{one}, {two} +{}", rest.len())),
    }
}

/// Notifications that read the same (title and read state) as one stack,
/// newest first, in the order their first one appears.
fn stacks<'a>(items: &[&'a Notification]) -> Vec<Vec<&'a Notification>> {
    let mut stacks: Vec<Vec<&Notification>> = Vec::new();
    for &item in items {
        match stacks
            .iter_mut()
            .find(|stack| stack[0].title == item.title && stack[0].unread == item.unread)
        {
            Some(stack) => stack.push(item),
            None => stacks.push(vec![item]),
        }
    }
    stacks
}

/// What identifies a stack while the list reloads: where it is and how it
/// reads.
fn stack_key(repo: &RepoRef, first: &Notification) -> String {
    format!("{repo}\n{}\n{}", first.title, first.unread)
}

fn reason_label(reason: &str) -> String {
    match reason {
        "review_requested" => "Review requested",
        "mention" => "Mentioned",
        "team_mention" => "Team mentioned",
        "author" => "Author",
        "comment" => "Comment",
        "assign" => "Assigned",
        "state_change" => "State change",
        "ci_activity" => "CI",
        "subscribed" => "Subscribed",
        "manual" => "Subscribed",
        "security_alert" => "Security alert",
        other => other,
    }
    .to_owned()
}

/// Where clicking a notification goes: a PR opens in Loupe, the rest open on
/// the web.
enum Target {
    PullRequest(Route),
    Web(String),
}

fn target(notification: &Notification, pull: Option<&BranchPullRequest>) -> Target {
    // A failed run of a branch with a pull request opens that pull request.
    if let Some(pull) = pull {
        return Target::PullRequest(Route::PullRequest(pull.pr.clone()));
    }
    match &notification.subject {
        NotificationSubject::PullRequest(pr) => Target::PullRequest(Route::PullRequest(pr.clone())),
        NotificationSubject::Issue { repo, number } => {
            Target::Web(format!("{}/issues/{number}", repo.url()))
        }
        NotificationSubject::Other { .. } => Target::Web(notification.repo.url()),
    }
}

/// What a row's context menu acts on: the pull request it reports (for a
/// failed run, the one of its branch), or the GitHub issue. Other
/// notifications have no menu.
fn menu_item(notification: &Notification, pull: Option<&BranchPullRequest>) -> Option<Item> {
    let branch = workflow_run(&notification.title).map(|run| run.branch.to_owned());
    if let Some(pull) = pull {
        return Some(Item::PullRequest(PrItem {
            pr: pull.pr.clone(),
            title: pull.title.clone(),
            url: pull.pr.url(),
            head_ref: branch,
        }));
    }
    match &notification.subject {
        NotificationSubject::PullRequest(pr) => Some(Item::PullRequest(PrItem {
            pr: pr.clone(),
            title: notification.title.clone(),
            url: pr.url(),
            head_ref: None,
        })),
        NotificationSubject::Issue { repo, number } => Some(Item::GitHubIssue {
            title: notification.title.clone(),
            url: format!("{}/issues/{number}", repo.url()),
        }),
        NotificationSubject::Other { .. } => None,
    }
}

pub struct NotificationsView {
    all: bool,
    list: Loadable<Vec<Notification>>,
    collapsed: HashSet<RepoRef>,
    /// Stacks of repeated notifications the user opened, by [`stack_key`].
    /// A stack starts closed, as one row with its count.
    expanded: HashSet<String>,
    mark: Submit,
    confirm_all: bool,
    scroll: ScrollHandle,
    /// What GitHub knows about the branches the failed runs name. A branch
    /// that could not be read is not here, and its rows show the branch.
    ci: HashMap<BranchKey, BranchCi>,
    ci_loads: HashMap<BranchKey, Task<()>>,
    _load: Option<Task<()>>,
    marks: HashMap<String, Task<()>>,
    _mark_all: Option<Task<()>>,
    /// Where a filter is typed. The word being typed filters at once; a
    /// finished one (a space after it, or Enter) leaves the field and is
    /// saved as a chip.
    query: Entity<InputState>,
    _query_events: Subscription,
    focus: FocusHandle,
    /// The line under the keyboard cursor (J and K, or the arrows).
    cursor: Option<Spot>,
}

/// A line the keyboard cursor can stand on: a notification's row, with the
/// stack it sits in when it is one of an open stack's rows, or a stack's
/// own row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Spot {
    Row { id: String, stack: Option<String> },
    Stack(String),
}

/// One repository of the list as it is on screen: whether it is folded, and
/// its lines top to bottom.
struct GroupShown {
    collapsed: bool,
    spots: Vec<Spot>,
}

/// What "Mark all as read" marks.
#[derive(Debug, PartialEq, Eq)]
enum MarkAll {
    /// Every notification, with one request.
    Everything,
    /// While a filter is set, only the unread threads it shows.
    Threads(Vec<String>),
}

fn mark_all_plan(terms: &[Term], list: Option<&[Notification]>) -> MarkAll {
    match list {
        Some(list) if !terms.is_empty() => MarkAll::Threads(shown_unread(terms, list)),
        _ => MarkAll::Everything,
    }
}

/// The ids of the unread notifications `terms` lets through.
fn shown_unread(terms: &[Term], list: &[Notification]) -> Vec<String> {
    filter::apply(terms, list)
        .into_iter()
        .filter(|notification| notification.unread)
        .map(|notification| notification.id.clone())
        .collect()
}

/// Marks each thread read, a few at a time.
pub(super) fn mark_threads_read(gh: &GitHub, ids: &[String]) -> loupe_github::Result<()> {
    const WORKERS: usize = 6;
    let chunk = ids.len().div_ceil(WORKERS).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = ids
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || part.iter().try_for_each(|id| gh.mark_notification_read(id)))
            })
            .collect();
        workers.into_iter().try_for_each(|worker| {
            worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    })
}

pub fn view(window: &mut Window, cx: &mut App) -> AnyView {
    build(window, cx).into()
}

fn build(window: &mut Window, cx: &mut App) -> Entity<NotificationsView> {
    cx.new(|cx| {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter: org:, repo:, or text"));
        let query_events = cx.subscribe_in(
            &query,
            window,
            |view: &mut NotificationsView, input, event: &InputEvent, window, cx| {
                let value = input.read(cx).value().to_string();
                let (done, rest) = match event {
                    InputEvent::Change => filter::finished(&value),
                    InputEvent::PressEnter { .. } => (filter::parse(&value), String::new()),
                    _ => return,
                };
                view.confirm_all = false;
                if done.is_empty() && rest == value {
                    cx.notify();
                    return;
                }
                input.update(cx, |state, cx| state.set_value(rest, window, cx));
                view.add_terms(&done, cx);
            },
        );
        let mut view = NotificationsView {
            all: false,
            list: Loadable::Loading,
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            mark: Submit::Idle,
            confirm_all: false,
            scroll: ScrollHandle::new(),
            ci: HashMap::new(),
            ci_loads: HashMap::new(),
            _load: None,
            marks: HashMap::new(),
            _mark_all: None,
            query,
            _query_events: query_events,
            focus: cx.focus_handle(),
            cursor: None,
        };
        view.reload(cx);
        view
    })
}

impl NotificationsView {
    /// The list as it is on screen, one entry per repository: the same
    /// filter, groups, and stacks the rows are drawn from.
    fn groups_shown(&self, cx: &App) -> Vec<GroupShown> {
        let Some(list) = self.list.value() else {
            return Vec::new();
        };
        let terms = self.terms(cx);
        let list: Vec<Notification> = filter::apply(&terms, list).into_iter().cloned().collect();
        group(&list)
            .into_iter()
            .map(|(repo, items)| {
                let mut spots = Vec::new();
                for stack in stacks(&items) {
                    match stack.as_slice() {
                        [] => {}
                        [single] => spots.push(Spot::Row {
                            id: single.id.clone(),
                            stack: None,
                        }),
                        [first, ..] => {
                            let key = stack_key(&repo, first);
                            let open = self.expanded.contains(&key);
                            spots.push(Spot::Stack(key.clone()));
                            if open {
                                spots.extend(stack.iter().map(|item| Spot::Row {
                                    id: item.id.clone(),
                                    stack: Some(key.clone()),
                                }));
                            }
                        }
                    }
                }
                GroupShown {
                    collapsed: self.collapsed.contains(&repo),
                    spots,
                }
            })
            .collect()
    }

    /// The cursor, while its line is on screen.
    fn cursor_on_screen(&self, cx: &App) -> Option<Spot> {
        let cursor = self.cursor.clone()?;
        self.groups_shown(cx)
            .iter()
            .any(|group| !group.collapsed && group.spots.contains(&cursor))
            .then_some(cursor)
    }

    /// Moves the keyboard cursor one line and scrolls it into view.
    fn move_cursor(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let groups = self.groups_shown(cx);
        let spots: Vec<Spot> = groups
            .iter()
            .filter(|group| !group.collapsed)
            .flat_map(|group| group.spots.iter().cloned())
            .collect();
        let Some(next) = super::inbox::step_cursor(&spots, self.cursor.as_ref(), down) else {
            return;
        };
        // The line's place in the scrolled content, from the fixed heights
        // of what is above it: 8 px of padding, then per repository a
        // heading, its lines unless it is folded, and an 8 px gap.
        let rem = window.rem_size();
        let row = scale::row_height().to_pixels(rem);
        let gap = rems(0.5).to_pixels(rem);
        let mut top = gap;
        for group in &groups {
            top += row;
            if !group.collapsed {
                if let Some(index) = group.spots.iter().position(|spot| *spot == next) {
                    top += row * index as f32;
                    break;
                }
                top += row * group.spots.len() as f32;
            }
            top += gap;
        }
        let viewport = self.scroll.bounds().size.height;
        let mut offset = self.scroll.offset();
        if top < -offset.y {
            offset.y = -(top - row).max(px(0.));
        } else if top + row > -offset.y + viewport {
            offset.y = -(top + row + gap - viewport);
        }
        self.scroll.set_offset(offset);
        self.cursor = Some(next);
        cx.notify();
    }

    /// Enter or O: opens the notification under the cursor in this tab, or
    /// opens or folds the stack under it. With no cursor the key goes on to
    /// whoever handles it next.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.cursor_on_screen(cx) {
            Some(Spot::Row { id, .. }) => {
                let notification = self
                    .list
                    .value()
                    .and_then(|list| list.iter().find(|item| item.id == id))
                    .cloned();
                if let Some(notification) = notification {
                    self.open(&notification, None, window, cx);
                }
            }
            Some(Spot::Stack(key)) => self.toggle_stack(&key, cx),
            None => cx.propagate(),
        }
    }

    /// Right arrow: opens the folded stack under the cursor.
    fn open_stack(&mut self, cx: &mut Context<Self>) {
        match self.cursor_on_screen(cx) {
            Some(Spot::Stack(key)) if !self.expanded.contains(&key) => self.toggle_stack(&key, cx),
            _ => cx.propagate(),
        }
    }

    /// Left arrow: folds the open stack under the cursor, or the one whose
    /// row the cursor is on.
    fn close_stack(&mut self, cx: &mut Context<Self>) {
        let key = match self.cursor_on_screen(cx) {
            Some(Spot::Stack(key)) => Some(key),
            Some(Spot::Row { stack, .. }) => stack,
            None => None,
        };
        match key.filter(|key| self.expanded.contains(key)) {
            Some(key) => self.toggle_stack(&key, cx),
            None => cx.propagate(),
        }
    }

    /// Opens or folds a stack. Folding it brings the cursor from one of its
    /// rows up to the stack's own row.
    fn toggle_stack(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        } else if matches!(&self.cursor, Some(Spot::Row { stack: Some(stack), .. }) if stack == key)
        {
            self.cursor = Some(Spot::Stack(key.to_owned()));
        }
        cx.notify();
    }

    /// The filter in force: the saved terms, then the one being typed.
    fn terms(&self, cx: &App) -> Vec<Term> {
        let mut terms = filter::from_tokens(&Settings::get(cx).notification_filter);
        for term in filter::parse(&self.query.read(cx).value()) {
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
        terms
    }

    /// Saves `added` after the filter's terms, so they come back with the
    /// app.
    fn add_terms(&mut self, added: &[Term], cx: &mut Context<Self>) {
        let tokens = filter::with_terms(&Settings::get(cx).notification_filter, added);
        self.set_filter(tokens, cx);
    }

    fn set_filter(&mut self, tokens: Vec<String>, cx: &mut Context<Self>) {
        self.confirm_all = false;
        update_settings(cx, |settings| settings.notification_filter = tokens);
        self.load_ci(cx);
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let key = crate::cache::github_key(cx, &format!("notifications/{}", self.all));
        if self.list.value().is_none()
            && let Some(value) = key.as_deref().and_then(crate::cache::get)
        {
            self.list = Loadable::Loaded(value);
        }
        self.list.start_refresh();
        let all = self.all;
        self._load = Some(common::run(
            cx,
            move |gh| gh.notifications(all),
            move |view, result, cx| {
                if let (Some(key), Ok(value)) = (&key, &result) {
                    crate::cache::put(key, value);
                }
                view.list.finish(result);
                view.load_ci(cx);
            },
        ));
        self.load_ci(cx);
        cx.notify();
    }

    /// Reads the pull request and the failed runs of every branch a failed
    /// run names and the screen does not know yet.
    fn load_ci(&mut self, cx: &mut Context<Self>) {
        let Some(list) = self.list.value() else {
            return;
        };
        // The branches of the rows on screen come first.
        let shown: Vec<Notification> = filter::apply(&self.terms(cx), list)
            .into_iter()
            .cloned()
            .collect();
        let wanted: Vec<BranchKey> = failed_branches(&shown)
            .into_iter()
            .take(MAX_BRANCHES)
            .filter(|key| !self.ci.contains_key(key) && !self.ci_loads.contains_key(key))
            .collect();
        for key in wanted {
            let (repo, branch) = key.clone();
            let done = key.clone();
            let task = common::run(
                cx,
                move |gh| gh.branch_ci(&repo, &branch),
                move |view, result, _| {
                    view.ci_loads.remove(&done);
                    if let Ok(ci) = result {
                        view.ci.insert(done, ci);
                    }
                },
            );
            self.ci_loads.insert(key, task);
        }
    }

    /// What GitHub knows about the branch of a failed run notification.
    fn ci_of(&self, notification: &Notification) -> Option<&BranchCi> {
        let run = workflow_run(&notification.title).filter(|run| run.outcome == "failed")?;
        self.ci
            .get(&(notification.repo.clone(), run.branch.to_owned()))
    }

    fn set_unread(&mut self, id: &str, unread: bool) {
        if let Loadable::Loaded(list) | Loadable::Refreshing(list) = &mut self.list
            && let Some(item) = list.iter_mut().find(|item| item.id == id)
        {
            item.unread = unread;
        }
    }

    fn mark_read(&mut self, id: String, cx: &mut Context<Self>) {
        self.set_unread(&id, false);
        self.mark = Submit::Idle;
        let thread = id.clone();
        let key = id.clone();
        let task = common::run(
            cx,
            move |gh| gh.mark_notification_read(&thread),
            move |view, result, _| {
                view.marks.remove(&id);
                if let Err(error) = result {
                    view.set_unread(&id, true);
                    view.mark = Submit::Failed(error.detail());
                }
            },
        );
        self.marks.insert(key, task);
        cx.notify();
    }

    /// Opens a notification's pull request (in a background tab for a middle
    /// click or a Cmd-click) or its page on GitHub, and marks it read.
    /// Opens what `notification` is about. A click opens it as the click's
    /// modifiers ask; the keyboard (no `event`) opens it in this tab.
    fn open(
        &mut self,
        notification: &Notification,
        event: Option<&ClickEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if notification.unread {
            self.mark_read(notification.id.clone(), cx);
        }
        let pull = self
            .ci_of(notification)
            .and_then(|ci| ci.pull_request.clone());
        match target(notification, pull.as_ref()) {
            Target::PullRequest(route) => match event {
                Some(event) => crate::shell::open(route, event, window, cx),
                None => crate::shell::navigate(route, window, cx),
            },
            Target::Web(url) => cx.open_url(&url),
        }
    }

    /// Marks every notification read or, while a filter is set, only the
    /// unread ones it shows.
    fn mark_all(&mut self, cx: &mut Context<Self>) {
        self.confirm_all = false;
        self.mark = Submit::Busy;
        let plan = mark_all_plan(&self.terms(cx), self.list.value().map(Vec::as_slice));
        if let MarkAll::Threads(ids) = &plan {
            for id in ids {
                self.set_unread(id, false);
            }
        }
        self._mark_all = Some(common::run(
            cx,
            move |gh| match plan {
                MarkAll::Threads(ids) => mark_threads_read(gh, &ids),
                MarkAll::Everything => gh.mark_all_notifications_read(),
            },
            |view, result, cx| match result {
                Ok(()) => {
                    view.mark = Submit::Idle;
                    view.reload(cx);
                }
                Err(error) => {
                    view.mark = Submit::Failed(error.detail());
                    view.reload(cx);
                }
            },
        ));
        cx.notify();
    }

    /// Unread notifications the filter lets through.
    fn unread_count(&self, terms: &[Term]) -> usize {
        self.list
            .value()
            .map_or(0, |list| shown_unread(terms, list).len())
    }

    fn header(&self, terms: &[Term], cx: &mut Context<Self>) -> impl IntoElement {
        let unread = self.unread_count(terms);
        let busy = self.list.is_busy();
        let marking = self.mark.is_busy();
        ui::page_header("Inbox")
            .icon(IconName::Inbox)
            .when(unread > 0, |header| header.count(unread))
            .action(
                Button::new("mark-all")
                    .ghost()
                    .small()
                    .icon(IconName::CheckCheck)
                    // No ellipsis: in a header it reads as a cut label.
                    .label("Mark all as read")
                    .loading(marking)
                    .disabled(marking || unread == 0 || self.confirm_all)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.confirm_all = true;
                        cx.notify();
                    })),
            )
            .action(
                ui::icon_button("refresh", IconName::RefreshCw, "Refresh")
                    .loading(busy)
                    .disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| view.refresh_page(cx))),
            )
    }

    /// A term as a chip that removes it, with its × at the end.
    fn filter_chip(&self, ix: usize, term: &Term, cx: &mut Context<Self>) -> AnyElement {
        let token = term.token();
        let muted = cx.theme().muted_foreground;
        let (prefix, value) = match term {
            Term::Org(value) => (Some("org:"), value.clone()),
            Term::Repo(value) => (Some("repo:"), value.clone()),
            Term::Text(value) => (None, value.clone()),
        };
        let removed = term.clone();
        ui::chip_button(
            SharedString::from(format!("inbox-filter-{ix}")),
            format!("Remove filter: {token}"),
            cx,
        )
        .tooltip(|window, cx| Tooltip::new("Remove filter").build(window, cx))
        .on_click(cx.listener(move |view, _, _, cx| {
            let tokens = Settings::get(cx)
                .notification_filter
                .iter()
                .filter(|saved| Term::parse(saved).as_ref() != Some(&removed))
                .cloned()
                .collect();
            view.set_filter(tokens, cx);
        }))
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .children(prefix.map(|prefix| div().text_color(muted).child(prefix)))
                .child(div().max_w(dp(220.)).truncate().child(value))
                .child(
                    Icon::new(IconName::Close)
                        .with_size(scale::icon_xs())
                        .text_color(muted),
                ),
        )
        .into_any_element()
    }

    fn toolbar(&self, terms: &[Term], cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let this = cx.entity().downgrade();
        let saved = filter::from_tokens(&Settings::get(cx).notification_filter);
        let shown = self.unread_count(terms);
        let chips: Vec<AnyElement> = saved
            .iter()
            .enumerate()
            .map(|(ix, term)| self.filter_chip(ix, term, cx))
            .collect();
        let filtered = !terms.is_empty();
        ui::toolbar()
            .child(ui::segmented(
                "notification-filter",
                "Notifications",
                [
                    (Some(IconName::BellDot), "Unread"),
                    (Some(IconName::Bell), "All"),
                ],
                usize::from(self.all),
                move |index, _, cx| {
                    let all = index == 1;
                    this.update(cx, |view, cx| {
                        if view.all != all {
                            view.all = all;
                            view.list = Loadable::Loading;
                            view.reload(cx);
                        }
                    })
                    .ok();
                },
            ))
            .child(
                h_flex()
                    .min_w_0()
                    .gap_1p5()
                    .items_center()
                    .overflow_hidden()
                    .children(chips)
                    .when(filtered, |row| {
                        row.child(
                            Button::new("inbox-filter-clear")
                                .ghost()
                                .xsmall()
                                .label("Clear")
                                .accessibility_label("Clear filters")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.query
                                        .update(cx, |state, cx| state.set_value("", window, cx));
                                    view.set_filter(Vec::new(), cx);
                                })),
                        )
                    }),
            )
            .child(div().flex_1())
            .children(self.mark.error().map(|message| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::meta())
                    .text_color(cx.theme().danger)
                    .child(message.to_owned())
            }))
            .when(self.confirm_all, |row| {
                row.child(
                    div()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(if filtered {
                            "Mark the notifications shown as read?"
                        } else {
                            "Mark every notification as read?"
                        }),
                )
                .child(
                    Button::new("mark-all-cancel")
                        .ghost()
                        .xsmall()
                        .label("Cancel")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.confirm_all = false;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("mark-all-confirm")
                        .danger()
                        .xsmall()
                        .label(if filtered {
                            format!("Mark {shown} as read")
                        } else {
                            "Mark all as read".to_owned()
                        })
                        .on_click(cx.listener(|view, _, _, cx| view.mark_all(cx))),
                )
            })
            .child(
                Input::new(&self.query)
                    .small()
                    .flex_none()
                    .w(scale::search_width())
                    .cleanable(true)
                    .prefix(IconName::Search)
                    .aria_label("Filter notifications"),
            )
    }

    fn group_header(
        &self,
        repo: &RepoRef,
        total: usize,
        unread: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.collapsed.contains(repo);
        let toggle = repo.clone();
        let only = repo.clone();
        let hint = SharedString::from(format!("Show only repo:{repo}"));
        let muted = cx.theme().muted_foreground;
        ui::list_group_header(SharedString::from(format!("group-{repo}")), collapsed, cx)
            .aria_label(if collapsed {
                format!("Expand {repo}")
            } else {
                format!("Collapse {repo}")
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                if !view.collapsed.remove(&toggle) {
                    view.collapsed.insert(toggle.clone());
                }
                cx.notify();
            }))
            .child(
                // The title narrows the Inbox to its repository.
                div()
                    .id(SharedString::from(format!("group-filter-{repo}")))
                    .role(Role::Button)
                    .aria_label(format!("Show only {repo}"))
                    .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
                    .min_w_0()
                    .hover(|title| title.underline())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        view.add_terms(&[Term::of_repo(&only)], cx);
                    }))
                    .child(ui::group_title(repo.to_string()).truncate()),
            )
            .child(
                div()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(match unread {
                        0 => format!("{total}"),
                        unread if unread == total => format!("{unread} unread"),
                        _ => format!("{unread} unread of {total}"),
                    }),
            )
            .into_any_element()
    }

    /// The rows of one stack: a single notification is its own row; several
    /// that read the same are one row that opens into a row for each.
    /// `focused` says whether the list has the focus, and so whether the
    /// row under the keyboard cursor shows it.
    fn stack_rows(
        &self,
        repo: &RepoRef,
        stack: &[&Notification],
        runs: &HashMap<String, FailedRun>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let [first, ..] = stack else {
            return Vec::new();
        };
        let at = |spot: Spot| focused && self.cursor.as_ref() == Some(&spot);
        if stack.len() == 1 {
            let cursor = at(Spot::Row {
                id: first.id.clone(),
                stack: None,
            });
            return vec![self.row(stack, None, false, cursor, runs, cx)];
        }
        let key = stack_key(repo, first);
        let open = self.expanded.contains(&key);
        let cursor = at(Spot::Stack(key.clone()));
        let mut rows = vec![self.row(stack, Some((key.clone(), open)), false, cursor, runs, cx)];
        if open {
            rows.extend(stack.iter().map(|item| {
                let cursor = at(Spot::Row {
                    id: item.id.clone(),
                    stack: Some(key.clone()),
                });
                self.row(&[*item], None, true, cursor, runs, cx)
            }));
        }
        rows
    }

    /// One 32 px row: the subject, then what happened, with the age at its
    /// end. With `fold`, the row stands for a stack of notifications that
    /// read the same: it says in words what happened and where, how many
    /// they are, and when the latest came; a click opens or closes the list
    /// of them instead of opening one.
    ///
    /// A failed workflow run says which pull request it belongs to (its
    /// number and title, once GitHub answered), "CI failed" beside a red
    /// mark, and the jobs that failed; the row then has an Open PR button.
    /// The blue dot means unread and nothing else.
    fn row(
        &self,
        stack: &[&Notification],
        fold: Option<(String, bool)>,
        nested: bool,
        cursor: bool,
        runs: &HashMap<String, FailedRun>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(&notification) = stack.first() else {
            return div().into_any_element();
        };
        let run = workflow_run(&notification.title);
        let failed = run.as_ref().is_some_and(|run| run.outcome == "failed");
        let pull = self
            .ci_of(notification)
            .and_then(|ci| ci.pull_request.clone());
        let item = menu_item(notification, pull.as_ref());
        let failed_run = runs.get(&notification.id);
        let jobs = failed_run.and_then(|run| failed_jobs_text(&run.failed_jobs));
        // On a stack or a row of its own, a failed run's red mark sits with
        // the words "CI failed" and the leading glyph is the row's kind:
        // the pull request when GitHub named one, a workflow otherwise. A
        // row inside a stack is one run: its leading glyph is the outcome.
        let icon = match (&notification.subject, &run) {
            (_, Some(_)) if failed && nested => IconName::CircleX,
            (_, Some(_)) if failed && pull.is_some() => IconName::GitPullRequest,
            (_, Some(_)) if failed => IconName::Workflow,
            (_, Some(_)) => IconName::CircleCheck,
            (NotificationSubject::PullRequest(_), _) => IconName::GitPullRequest,
            (NotificationSubject::Issue { .. }, _) => IconName::CircleDot,
            (NotificationSubject::Other { .. }, _) => IconName::Bell,
        };
        let id = notification.id.clone();
        let unread_ids: Vec<String> = stack
            .iter()
            .filter(|item| item.unread)
            .map(|item| item.id.clone())
            .collect();
        let clicked = notification.clone();
        let unread = !unread_ids.is_empty();
        let count = stack.len();
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let icon_color = match (failed && nested, unread) {
            (true, _) => tokens::closed(),
            (false, true) => foreground,
            (false, false) => muted,
        };
        let open = fold.as_ref().map(|(_, open)| *open);
        let pull_name = pull
            .as_ref()
            .map(|pull| format!(", pull request #{}", pull.pr.number))
            .unwrap_or_default();
        let name = match open {
            Some(open) => format!(
                "{} {}{pull_name}, {count} notifications{}",
                if open { "Collapse" } else { "Expand" },
                notification.title,
                if unread { ", unread" } else { "" }
            ),
            None => format!("{}{pull_name}", notification.title),
        };
        let row = ui::list_row(
            SharedString::from(match &fold {
                Some(_) => format!("notification-stack-{id}"),
                None => format!("notification-{id}"),
            }),
            cx,
        )
        .role(if fold.is_some() {
            Role::Button
        } else {
            Role::Link
        })
        .aria_label(name)
        .relative()
        .group("notification")
        .keyboard_focus(cursor, cx)
        .when(nested, |row| row.pl_6())
        .gap_2()
        .child(ui::list_row_divider(cx));
        let row = match fold {
            Some((key, _)) => {
                row.on_click(cx.listener(move |view, _, _, cx| view.toggle_stack(&key, cx)))
            }
            None => row.on_open(cx.listener(move |view, event, window, cx| {
                view.open(&clicked, Some(event), window, cx)
            })),
        };
        let slot = || h_flex().flex_none().items_center().justify_center();
        let exact = notification
            .updated_at
            .with_timezone(&chrono::Local)
            .format("%b %-d, %H:%M")
            .to_string();
        let text = |color: Hsla, value: SharedString| {
            div()
                .min_w_0()
                .truncate()
                .text_size(scale::body())
                .text_color(color)
                .child(value)
        };
        let faint = |value: SharedString| {
            div()
                .flex_none()
                .text_size(scale::meta())
                .text_color(muted)
                .child(value)
        };
        // Unread is the foreground color and the dot, read is muted: a
        // screen of unread rows is not a wall of bold.
        let ink = if unread { foreground } else { muted };

        // What the line says. A stack and a row on its own name the
        // subject: for a failed run, the pull request (or the branch while
        // GitHub has not answered), then "CI failed" with its red mark and
        // the jobs that failed. A row inside a stack says what is its own:
        // which run, which jobs, and exactly when.
        let mut line = h_flex().flex_1().min_w_0().gap_2().items_center();
        match (&run, nested) {
            (Some(run), false) => {
                line = match &pull {
                    Some(pull) => line
                        .child(faint(format!("#{}", pull.pr.number).into()))
                        .child(text(ink, pull.title.clone().into())),
                    None => line.child(text(ink, run.branch.to_owned().into())),
                };
                line = line.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .items_center()
                        .when(failed, |status| {
                            status.child(
                                Icon::new(IconName::CircleX)
                                    .with_size(scale::icon_sm())
                                    .text_color(tokens::closed()),
                            )
                        })
                        .child(
                            div()
                                .text_size(scale::body())
                                .text_color(ink)
                                .child(format!("{} {}", run.workflow, run.outcome)),
                        ),
                );
                if let Some(jobs) = jobs.clone() {
                    line = line.child(faint(format!("· {jobs}").into()));
                }
                if pull.is_some() {
                    line = line.child(faint(format!("· {}", run.branch).into()));
                }
            }
            (Some(run), true) => {
                let which = match failed_run {
                    Some(failed_run) => {
                        format!(
                            "{} #{} {}",
                            run.workflow, failed_run.run_number, run.outcome
                        )
                    }
                    None => format!("{} workflow run {}", run.workflow, run.outcome),
                };
                line = line.child(text(ink, which.into()));
                if let Some(jobs) = jobs.clone() {
                    line = line.child(faint(format!("· {jobs}").into()));
                }
            }
            (None, _) => {
                line = line.child(text(ink, notification.title.clone().into()));
            }
        }
        if open.is_some() {
            line = line.child(ui::count_badge(count, cx));
        }
        let trailing_meta: SharedString = match (open, nested, &run) {
            (Some(_), _, _) => format!("Latest {exact}").into(),
            (None, true, _) => exact.clone().into(),
            (None, false, Some(_)) => "Workflow".into(),
            (None, false, None) => reason_label(&notification.reason).into(),
        };

        // The pull request of a failed run, one click away from the row
        // that reports it. Opening it marks what the row stands for as read.
        let open_pr = pull.filter(|_| !nested).map(|pull| {
            let route = Route::PullRequest(pull.pr.clone());
            let read = unread_ids.clone();
            Button::new(SharedString::from(format!("open-pr-{id}")))
                .outline()
                .xsmall()
                .icon(IconName::GitPullRequest)
                .label("Open PR")
                .accessibility_label(format!("Open PR #{}", pull.pr.number))
                .tooltip(format!("Open {} in this tab", pull.pr))
                .on_click(cx.listener(move |view, event, window, cx| {
                    cx.stop_propagation();
                    for id in &read {
                        view.mark_read(id.clone(), cx);
                    }
                    crate::shell::open(route.clone(), event, window, cx);
                }))
        });

        // One 32 px line with two glyphs: a leading one (a stack's chevron,
        // or the kind of a single notification) and the unread dot; then
        // the line above. At the trailing edge, fixed columns: Open PR,
        // mark as read (on hover), and the age.
        let row = row
            .child(
                slot().w_2().child(
                    div()
                        .size_1p5()
                        .rounded_full()
                        .when(unread, |dot| dot.bg(tokens::accent())),
                ),
            )
            .child(slot().w_4().map(|slot| {
                match open {
                    Some(open) => slot.child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .text_color(muted),
                    ),
                    None => slot.child(
                        Icon::new(icon)
                            .with_size(scale::icon_sm())
                            .text_color(icon_color),
                    ),
                }
            }))
            .child(line)
            .children(open_pr)
            .child(
                div()
                    .flex_none()
                    .w(dp(REASON_LANE))
                    .truncate()
                    .text_right()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(trailing_meta),
            )
            .child(
                // A shortcut: opening a notification also marks it read.
                slot().w_6().invisible().when(unread, |lane| {
                    lane.group_hover("notification", |s| s.visible()).child(
                        ui::icon_button(
                            SharedString::from(format!("read-{id}")),
                            IconName::Check,
                            "Mark as read",
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            for id in &unread_ids {
                                view.mark_read(id.clone(), cx);
                            }
                        })),
                    )
                }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("notification-age-{id}")))
                    .flex_none()
                    .w(dp(AGE_LANE))
                    .text_right()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .tooltip(move |window, cx| Tooltip::new(exact.clone()).build(window, cx))
                    .child(ui::relative_time(notification.updated_at, Utc::now())),
            );
        match item {
            Some(item) => with_item_menu(row, item).into_any_element(),
            None => row.into_any_element(),
        }
    }
}

impl crate::shell::Refreshable for NotificationsView {
    fn refresh_page(&mut self, cx: &mut Context<Self>) {
        if !self.list.is_busy() {
            self.reload(cx);
        }
    }
}

impl Render for NotificationsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        let focused = self.focus.is_focused(window);
        let terms = self.terms(cx);
        let body = match &self.list {
            Loadable::Failed(error) => {
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .into_any_element()
            }
            Loadable::Loading => ui::loading_rows(10).into_any_element(),
            Loadable::Loaded(list) | Loadable::Refreshing(list) if list.is_empty() => {
                let state = ui::empty_state(
                    IconName::BellCheck,
                    "You're all caught up",
                    if self.all {
                        "No notifications."
                    } else {
                        "No unread notifications."
                    },
                );
                if self.all {
                    state.into_any_element()
                } else {
                    state
                        .action(
                            Button::new("show-all")
                                .outline()
                                .small()
                                .label("Show all")
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.all = true;
                                    view.list = Loadable::Loading;
                                    view.reload(cx);
                                })),
                        )
                        .into_any_element()
                }
            }
            Loadable::Loaded(list) | Loadable::Refreshing(list)
                if filter::apply(&terms, list).is_empty() =>
            {
                ui::empty_state(
                    IconName::ListFilter,
                    "Nothing matches the filter",
                    if self.all {
                        "No notification matches it."
                    } else {
                        "No unread notification matches it."
                    },
                )
                .action(
                    Button::new("inbox-filter-clear-empty")
                        .outline()
                        .small()
                        .label("Clear filter")
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.query
                                .update(cx, |state, cx| state.set_value("", window, cx));
                            view.set_filter(Vec::new(), cx);
                        })),
                )
                .into_any_element()
            }
            Loadable::Loaded(list) | Loadable::Refreshing(list) => {
                let list: Vec<Notification> =
                    filter::apply(&terms, list).into_iter().cloned().collect();
                let groups = group(&list);
                let runs = match_runs(&list, &self.ci);
                ui::list_container()
                    .gap_2()
                    .children(groups.into_iter().map(|(repo, items)| {
                        let unread = items.iter().filter(|item| item.unread).count();
                        let collapsed = self.collapsed.contains(&repo);
                        v_flex()
                            .child(self.group_header(&repo, items.len(), unread, cx))
                            .when(!collapsed, |group| {
                                group.children(stacks(&items).iter().flat_map(|stack| {
                                    self.stack_rows(&repo, stack, &runs, focused, cx)
                                }))
                            })
                    }))
                    .into_any_element()
            }
        };
        v_flex()
            .id("inbox-page")
            .role(Role::Group)
            .aria_label("Inbox")
            .size_full()
            .key_context("NotificationsView")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &SelectNext, window, cx| this.move_cursor(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                this.move_cursor(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.open_cursor(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenStack, _, cx| this.open_stack(cx)))
            .on_action(cx.listener(|this, _: &CloseStack, _, cx| this.close_stack(cx)))
            .child(self.header(&terms, cx))
            .child(self.toolbar(&terms, cx))
            .child(
                div()
                    .id("notifications")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_github::{BranchCi, FailedRun, Notification, NotificationSubject, PrRef, RepoRef};

    use std::collections::HashMap;

    use gpui_kit::{
        Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
    };

    use super::{
        MarkAll, NotificationsView, Spot, WorkflowRun, build, failed_branches, failed_jobs_text,
        group, mark_all_plan, match_runs, menu_item, reason_label, stack_key, workflow_run,
    };
    use crate::github::common::Loadable;
    use crate::github::notification_filter::parse;
    use crate::services::{Accounts, Services};
    use crate::shell::item_menu::{Item, PrItem};
    use crate::store::Settings;

    fn notification(id: &str, repo: &str, unread: bool, seconds: i64) -> Notification {
        let repo = RepoRef::parse(repo).expect("repo");
        Notification {
            id: id.into(),
            repo: repo.clone(),
            title: id.into(),
            subject: NotificationSubject::PullRequest(PrRef::new(repo, 1)),
            reason: "mention".into(),
            unread,
            updated_at: DateTime::from_timestamp(seconds, 0).expect("timestamp"),
        }
    }

    #[test]
    fn groups_newest_repo_first_and_unread_first_inside() {
        let list = [
            notification("a1", "acme/a", false, 50),
            notification("b1", "acme/b", true, 10),
            notification("a2", "acme/a", true, 20),
            notification("a3", "acme/a", true, 30),
        ];
        let groups = group(&list);
        let names: Vec<_> = groups.iter().map(|(repo, _)| repo.to_string()).collect();
        assert_eq!(names, ["acme/a", "acme/b"]);
        let ids: Vec<_> = groups[0].1.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a3", "a2", "a1"]);
    }

    #[test]
    fn with_a_filter_mark_all_marks_only_the_unread_rows_shown() {
        let list = [
            notification("mine-unread", "IsraelAraujo70/loupe", true, 30),
            notification("mine-read", "IsraelAraujo70/loupe", false, 20),
            notification("theirs", "hooli-inc/hooli-api", true, 10),
        ];
        assert_eq!(
            mark_all_plan(&parse("org:IsraelAraujo70"), Some(&list)),
            MarkAll::Threads(vec!["mine-unread".into()])
        );
        assert_eq!(
            mark_all_plan(&parse("org:nobody"), Some(&list)),
            MarkAll::Threads(Vec::new())
        );
        assert_eq!(mark_all_plan(&[], Some(&list)), MarkAll::Everything);
    }

    #[test]
    fn failed_runs_name_their_branches_once() {
        let mut list = [
            notification("a", "acme/a", true, 30),
            notification("b", "acme/a", true, 20),
            notification("c", "acme/b", true, 10),
            notification("d", "acme/a", true, 5),
        ];
        list[0].title = "CI workflow run failed for feat/x branch".into();
        list[1].title = "CI workflow run failed for feat/x branch".into();
        list[2].title = "CI workflow run failed for feat/x branch".into();
        list[3].title = "CI workflow run succeeded for feat/y branch".into();
        let branches: Vec<String> = failed_branches(&list)
            .into_iter()
            .map(|(repo, branch)| format!("{repo} {branch}"))
            .collect();
        assert_eq!(branches, ["acme/a feat/x", "acme/b feat/x"]);
    }

    #[test]
    fn each_notification_gets_the_run_that_ended_next_to_it() {
        let at = |seconds: i64| DateTime::from_timestamp(seconds, 0).expect("timestamp");
        let run = |workflow: &str, number: u64, seconds: i64| FailedRun {
            workflow: workflow.into(),
            run_number: number,
            finished_at: at(seconds),
            url: String::new(),
            failed_jobs: Vec::new(),
        };
        let failed = |id: &str, seconds: i64| {
            let mut item = notification(id, "acme/a", true, seconds);
            item.title = "CI workflow run failed for feat/x branch".into();
            item
        };
        let ci = HashMap::from([(
            (RepoRef::new("acme", "a"), "feat/x".to_owned()),
            BranchCi {
                pull_request: None,
                // Runs 73 and 71 ended eight seconds apart.
                failed_runs: vec![
                    run("CI", 73, 1_000),
                    run("CI", 71, 992),
                    run("Deploy", 5, 1_001),
                    run("CI", 60, 100),
                ],
            },
        )]);
        let list = [
            failed("late", 1_004),
            failed("early", 996),
            failed("old", 105),
            failed("orphan", 9_000),
            notification("mention", "acme/a", true, 1_000),
        ];
        let matched = match_runs(&list, &ci);
        let number = |id: &str| matched.get(id).map(|run| run.run_number);
        assert_eq!(number("late"), Some(73));
        assert_eq!(number("early"), Some(71), "a run goes to one notification");
        assert_eq!(number("old"), Some(60));
        assert_eq!(number("orphan"), None, "no run within ten minutes");
        assert_eq!(number("mention"), None);
    }

    #[test]
    fn failed_jobs_read_as_a_short_list() {
        let jobs = |names: &[&str]| -> Vec<String> { names.iter().map(|n| (*n).into()).collect() };
        assert_eq!(failed_jobs_text(&jobs(&[])), None);
        assert_eq!(
            failed_jobs_text(&jobs(&["check"])).as_deref(),
            Some("check")
        );
        assert_eq!(
            failed_jobs_text(&jobs(&["check", "e2e", "lint", "docs"])).as_deref(),
            Some("check, e2e +2")
        );
    }

    #[test]
    fn reason_labels() {
        assert_eq!(reason_label("review_requested"), "Review requested");
        assert_eq!(reason_label("ci_activity"), "CI");
        assert_eq!(reason_label("something_new"), "something_new");
    }

    #[test]
    fn a_workflow_title_gives_its_workflow_outcome_and_branch() {
        let run = workflow_run("CI workflow run failed for feat/reviews-filters branch");
        assert_eq!(
            run,
            Some(WorkflowRun {
                workflow: "CI",
                outcome: "failed",
                branch: "feat/reviews-filters",
            })
        );
        assert_eq!(workflow_run("fix(auth): separate quotas"), None);
    }

    /// A row's menu acts on the pull request it is about (for a failed run,
    /// the one of its branch, whose name the title gives), on a GitHub
    /// issue in the browser, and on nothing else.
    #[test]
    fn a_rows_menu_is_about_its_pull_request_or_issue() {
        let pr = notification("1", "acme/api", true, 0);
        assert_eq!(
            menu_item(&pr, None),
            Some(Item::PullRequest(PrItem {
                pr: PrRef::new(RepoRef::new("acme", "api"), 1),
                title: "1".into(),
                url: "https://github.com/acme/api/pull/1".into(),
                head_ref: None,
            }))
        );

        let mut run = notification("2", "acme/api", true, 0);
        run.title = "CI workflow run failed for fix/login branch".into();
        run.subject = NotificationSubject::Other {
            kind: "CheckSuite".into(),
        };
        let pull = loupe_github::BranchPullRequest {
            pr: PrRef::new(RepoRef::new("acme", "api"), 9),
            title: "Fix the login".into(),
            state: loupe_github::PrState::Open,
            is_draft: false,
        };
        assert_eq!(
            menu_item(&run, Some(&pull)),
            Some(Item::PullRequest(PrItem {
                pr: PrRef::new(RepoRef::new("acme", "api"), 9),
                title: "Fix the login".into(),
                url: "https://github.com/acme/api/pull/9".into(),
                head_ref: Some("fix/login".into()),
            }))
        );
        assert_eq!(menu_item(&run, None), None, "a run without a pull request");

        let mut issue = notification("3", "acme/api", true, 0);
        issue.subject = NotificationSubject::Issue {
            repo: RepoRef::new("acme", "api"),
            number: 4,
        };
        assert_eq!(
            menu_item(&issue, None),
            Some(Item::GitHubIssue {
                title: "3".into(),
                url: "https://github.com/acme/api/issues/4".into(),
            })
        );
    }

    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<NotificationsView>) {
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                build,
            )
            .expect("open the window")
        });
        (VisualTestContext::from_window(handle, cx), view)
    }

    #[gpui_kit::test]
    fn the_keyboard_walks_the_rows_and_opens_and_folds_a_stack(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        cx.run_until_parked();
        let same = |id: &str, seconds| Notification {
            title: "CI failed".into(),
            ..notification(id, "o/r", true, seconds)
        };
        let list = vec![
            notification("a", "o/r", true, 40),
            same("b", 30),
            same("c", 20),
            notification("d", "o/r", true, 10),
        ];
        let key = stack_key(&list[1].repo, &list[1]);
        view.update(cx, |view, cx| {
            view.list = Loadable::Loaded(list);
            cx.notify();
        });
        let step = |down: bool, cx: &mut VisualTestContext| {
            view.update_in(cx, |view, window, cx| {
                view.move_cursor(down, window, cx);
                view.cursor.clone()
            })
        };
        let row = |id: &str, stack: Option<&String>| {
            Some(Spot::Row {
                id: id.into(),
                stack: stack.cloned(),
            })
        };
        let stack = Some(Spot::Stack(key.clone()));

        assert_eq!(
            step(true, cx),
            row("a", None),
            "the first J lands on the first row"
        );
        assert_eq!(step(true, cx), stack);
        assert_eq!(
            step(true, cx),
            row("d", None),
            "folded, J steps past its rows"
        );
        assert_eq!(step(true, cx), row("d", None), "and stops at the end");
        assert_eq!(step(false, cx), stack);

        view.update(cx, |view, cx| view.open_stack(cx));
        assert_eq!(step(true, cx), row("b", Some(&key)));
        assert_eq!(step(true, cx), row("c", Some(&key)));
        // Left on a row inside folds the stack and brings the cursor up.
        view.update(cx, |view, cx| view.close_stack(cx));
        assert_eq!(view.read_with(cx, |view, _| view.cursor.clone()), stack);
        assert!(view.read_with(cx, |view, _| !view.expanded.contains(&key)));
        // Enter on the stack's row opens it too.
        view.update_in(cx, |view, window, cx| view.open_cursor(window, cx));
        assert!(view.read_with(cx, |view, _| view.expanded.contains(&key)));
    }
}
