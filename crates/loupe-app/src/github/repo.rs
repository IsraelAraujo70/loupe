//! `Route::Repo`: one repository's pull requests, watch toggle, add
//! repository; and the header and tabs (Pull requests · Actions) every page
//! of a repository shares.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::queue::{self, QueueGroup};
use loupe_github::{Page, PrListFilter, PrRef, PrSummary, RepoRef, RepoSummary};

use super::common::{self, LoadError, Loadable};
use super::pr_row::{AgeFrom, Lanes, StackPlace, StackRow, column_header, pr_row, stack_row};
use crate::route::Route;
use crate::shell::Refreshable as _;
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::shell::navigate;
use crate::store::{Settings, update_settings};
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const FILTERS: [(PrListFilter, &str); 4] = [
    (PrListFilter::Open, "Open"),
    (PrListFilter::Closed, "Closed"),
    (PrListFilter::Merged, "Merged"),
    (PrListFilter::All, "All"),
];

fn filter_icon(filter: PrListFilter) -> IconName {
    match filter {
        PrListFilter::Open => IconName::GitPullRequest,
        PrListFilter::Closed => IconName::GitPullRequestClosed,
        PrListFilter::Merged => IconName::GitMerge,
        PrListFilter::All => IconName::Layers,
    }
}

fn search_text(raw: &str) -> Option<String> {
    let text = raw.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A place the keyboard cursor rests on: a pull request's row, or a stack's
/// row by the pull request at its base.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Spot {
    Pr(PrRef),
    Stack(PrRef),
}

/// One line of the list: a pull request, or a stack of them base first, as
/// indices into the loaded page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Pr(usize),
    Stack(Vec<usize>),
}

/// The lines under one heading. A list that does not group, or whose rows
/// all fall in one group, has one section without a heading.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Section {
    group: Option<QueueGroup>,
    lines: Vec<Line>,
}

impl Section {
    fn count(&self) -> usize {
        self.lines
            .iter()
            .map(|line| match line {
                Line::Pr(_) => 1,
                Line::Stack(members) => members.len(),
            })
            .sum()
    }
}

/// The pull requests the list shows: the loaded pages, and on Open the
/// ones that wait on the viewer from `queue` (asked of them or reviewed by
/// them) that no loaded page holds yet, so a review asked of you is never
/// left on a page not loaded. The pages keep GitHub's order (latest update
/// first), and each one from the queue goes where its update falls in it.
fn listed(page: &Page<PrSummary>, queue: &[PrSummary], filter: PrListFilter) -> Vec<PrSummary> {
    let mut items = page.items.clone();
    if filter != PrListFilter::Open {
        return items;
    }
    for pr in queue {
        if items.iter().any(|known| known.pr == pr.pr) {
            continue;
        }
        let at = items
            .iter()
            .position(|known| known.updated_at < pr.updated_at)
            .unwrap_or(items.len());
        items.insert(at, pr.clone());
    }
    items
}

/// The open pull requests as a review queue: grouped by what they ask of
/// the viewer, in QueueGroup's order, each group in GitHub's order (latest
/// update first). A stack is one line in the group of its most urgent pull
/// request, where its first one comes. Closed, merged, and all pull
/// requests keep GitHub's order, ungrouped and unstacked.
fn sections(items: &[PrSummary], filter: PrListFilter) -> Vec<Section> {
    if filter != PrListFilter::Open {
        return vec![Section {
            group: None,
            lines: (0..items.len()).map(Line::Pr).collect(),
        }];
    }
    let refs: Vec<&PrSummary> = items.iter().collect();
    let stacks = queue::stacks(&refs);
    let in_stack: HashMap<usize, usize> = stacks
        .iter()
        .enumerate()
        .flat_map(|(at, members)| members.iter().map(move |&member| (member, at)))
        .collect();
    let mut lines: Vec<(QueueGroup, Line)> = Vec::new();
    for (at, item) in items.iter().enumerate() {
        let Some(&stack) = in_stack.get(&at) else {
            lines.push((QueueGroup::of(item), Line::Pr(at)));
            continue;
        };
        let members = &stacks[stack];
        if members.iter().min() != Some(&at) {
            continue;
        }
        let group = members
            .iter()
            .map(|&member| QueueGroup::of(&items[member]))
            .min()
            .unwrap_or(QueueGroup::NotReviewed);
        lines.push((group, Line::Stack(members.clone())));
    }
    lines.sort_by_key(|(group, _)| *group);
    let mut out: Vec<Section> = Vec::new();
    for (group, line) in lines {
        match out.last_mut() {
            Some(section) if section.group == Some(group) => section.lines.push(line),
            _ => out.push(Section {
                group: Some(group),
                lines: vec![line],
            }),
        }
    }
    if let [only] = out.as_mut_slice() {
        only.group = None;
    }
    out
}

/// A group's 32 px heading on the rows' grid: a chevron over the state
/// icons that says whether it is folded, its name over the numbers, then
/// its count. A click folds or unfolds it.
fn group_heading(group: QueueGroup, folded: bool, count: usize, cx: &App) -> Stateful<Div> {
    let label = group.label();
    let id = format!("repo-group-{}", label.to_lowercase().replace(' ', "-"));
    ui::list_row(SharedString::from(id.clone()), cx)
        .debug_selector(move || id.clone())
        .role(Role::Button)
        .aria_label(format!("{label}, {count}"))
        .aria_expanded(!folded)
        .child(
            Icon::new(if folded {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            })
            .with_size(scale::icon_md())
            .text_color(cx.theme().muted_foreground),
        )
        .child(ui::group_title(label))
        .child(ui::count_badge(count, cx))
}

/// The pages of a repository, as the tabs under its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepoTab {
    PullRequests,
    Actions,
}

/// A repository page's header: the repository's full name. Its pages add
/// their own actions.
pub(crate) fn repo_header(repo: &RepoRef) -> ui::PageHeader {
    ui::page_header(repo.to_string()).icon(IconName::FolderGit2)
}

/// The tabs under a repository's header, Pull requests and Actions, with
/// `count` beside the open one. Each is a page of its own, opened in place.
pub(crate) fn repo_tabs(repo: &RepoRef, active: RepoTab, count: Option<usize>) -> impl IntoElement {
    let tab =
        |id: &'static str, label: &'static str, icon: IconName, which: RepoTab, route: Route| {
            ui::section_tab(id, label)
                .icon(icon)
                .maybe_count(if which == active { count } else { None })
                .debug_selector(move || id.to_owned())
                .on_click(move |_, window, cx| navigate(route.clone(), window, cx))
        };
    ui::section_tabs(
        "repo-tabs",
        match active {
            RepoTab::PullRequests => 0,
            RepoTab::Actions => 1,
        },
    )
    .tabs([
        tab(
            "repo-tab-pull-requests",
            "Pull requests",
            IconName::GitPullRequest,
            RepoTab::PullRequests,
            Route::Repo(repo.clone()),
        ),
        tab(
            "repo-tab-actions",
            "Actions",
            IconName::Workflow,
            RepoTab::Actions,
            Route::Actions {
                repo: repo.clone(),
                branch: None,
            },
        ),
    ])
}

pub struct RepoView {
    repo: RepoRef,
    filter: PrListFilter,
    search: Entity<InputState>,
    list: Loadable<Page<PrSummary>>,
    /// On Open, the pull requests that wait on the viewer wherever they are
    /// in the list's pages.
    queue: Vec<PrSummary>,
    /// The row at the top of the reading area when Load more was pressed,
    /// and how far below the area's top it was, to put it back there once
    /// the new rows are laid out.
    anchor: Option<(Spot, Pixels)>,
    more: Option<Result<(), LoadError>>,
    scroll: ScrollHandle,
    /// The list's children (the column heading, then a row per pull
    /// request), to scroll the keyboard cursor's row into view.
    rows: ScrollHandle,
    focus: FocusHandle,
    /// The row under the keyboard cursor (J and K, or the arrows).
    cursor: Option<Spot>,
    /// Stacks shown open, by the pull request at their base.
    expanded: HashSet<PrRef>,
    /// Groups folded to their heading.
    folded: HashSet<QueueGroup>,
    _load: Option<Task<()>>,
    _more: Option<Task<()>>,
    _queue: Option<Task<()>>,
    _debounce: Option<Task<()>>,
    _search_changes: Subscription,
}

pub fn view(repo: RepoRef, window: &mut Window, cx: &mut App) -> AnyView {
    build(repo, window, cx).into()
}

fn build(repo: RepoRef, window: &mut Window, cx: &mut App) -> Entity<RepoView> {
    cx.new(|cx| {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search pull requests"));
        let changes = cx.subscribe(&search, |view: &mut RepoView, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.debounce_reload(cx);
            }
        });
        let mut view = RepoView {
            repo,
            filter: PrListFilter::Open,
            search,
            list: Loadable::Loading,
            more: None,
            scroll: ScrollHandle::new(),
            rows: ScrollHandle::new(),
            focus: crate::shell::page_focus(cx),
            cursor: None,
            expanded: HashSet::new(),
            folded: HashSet::new(),
            _load: None,
            queue: Vec::new(),
            anchor: None,
            _queue: None,
            _more: None,
            _debounce: None,
            _search_changes: changes,
        };
        view.reload(cx);
        view
    })
}

impl RepoView {
    /// The rows the cursor can rest on, top to bottom, each with the
    /// children of the list to scroll into view for it: its own, from the
    /// column heading for the first and from its group's heading for the
    /// first of a group. Rows of a folded group or stack are not on screen.
    fn spots(&self) -> Vec<(Spot, RangeInclusive<usize>)> {
        let Some(items) = self.items() else {
            return Vec::new();
        };
        let mut out: Vec<(Spot, RangeInclusive<usize>)> = Vec::new();
        let mut child = 1;
        for section in sections(&items, self.filter) {
            let mut top = if out.is_empty() { 0 } else { child };
            if let Some(group) = section.group {
                child += 1;
                if self.folded.contains(&group) {
                    continue;
                }
            }
            for line in section.lines {
                let (stack, members) = match line {
                    Line::Pr(at) => (None, vec![at]),
                    Line::Stack(members) => {
                        let base = items[members[0]].pr.clone();
                        let open = self.expanded.contains(&base);
                        (Some(base), if open { members } else { Vec::new() })
                    }
                };
                if let Some(base) = stack {
                    out.push((Spot::Stack(base), top.min(child)..=child));
                    top = child;
                    child += 1;
                }
                for member in members {
                    out.push((Spot::Pr(items[member].pr.clone()), top.min(child)..=child));
                    top = child;
                    child += 1;
                }
            }
        }
        out
    }

    /// Moves the keyboard cursor one row and scrolls it into view, with the
    /// heading over it when it is the first row of the list or its group.
    fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let spots = self.spots();
        let listed: Vec<Spot> = spots.iter().map(|(spot, _)| spot.clone()).collect();
        let Some(next) = ui::step_cursor(&listed, self.cursor.as_ref(), down) else {
            return;
        };
        if let Some((_, children)) = spots.into_iter().find(|(spot, _)| *spot == next) {
            ui::scroll_to_rows(&self.scroll, &self.rows, children);
        }
        self.cursor = Some(next);
        cx.notify();
    }

    /// Enter or O: opens the pull request under the cursor in this tab, or
    /// opens or folds the stack under it. With no cursor the key goes on to
    /// whoever handles it next.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_screen = self
            .cursor
            .clone()
            .filter(|cursor| self.spots().iter().any(|(spot, _)| spot == cursor));
        match on_screen {
            Some(Spot::Pr(pr)) => navigate(Route::PullRequest(pr), window, cx),
            Some(Spot::Stack(base)) => self.toggle_stack(&base, cx),
            None => cx.propagate(),
        }
    }

    fn toggle_stack(&mut self, base: &PrRef, cx: &mut Context<Self>) {
        if !self.expanded.remove(base) {
            self.expanded.insert(base.clone());
        }
        cx.notify();
    }

    fn toggle_group(&mut self, group: QueueGroup, cx: &mut Context<Self>) {
        if !self.folded.remove(&group) {
            self.folded.insert(group);
        }
        cx.notify();
    }

    /// The base of the stack whose rows hold pr.
    fn stack_of(&self, pr: &PrRef) -> Option<PrRef> {
        let items = self.items()?;
        sections(&items, self.filter)
            .into_iter()
            .flat_map(|section| section.lines)
            .find_map(|line| match line {
                Line::Stack(members) if members.iter().any(|&member| items[member].pr == *pr) => {
                    Some(items[members[0]].pr.clone())
                }
                _ => None,
            })
    }

    /// Right opens the folded stack under the cursor; Left folds the open
    /// one under it, or the one whose row it is on, and puts the cursor on
    /// the stack. Whether the key did anything.
    fn arrow(&mut self, open: bool, cx: &mut Context<Self>) -> bool {
        let base = match (self.cursor.clone(), open) {
            (Some(Spot::Stack(base)), true) if !self.expanded.contains(&base) => base,
            (Some(Spot::Stack(base)), false) if self.expanded.contains(&base) => base,
            (Some(Spot::Pr(pr)), false) => match self.stack_of(&pr) {
                Some(base) if self.expanded.contains(&base) => base,
                _ => return false,
            },
            _ => return false,
        };
        self.cursor = Some(Spot::Stack(base.clone()));
        self.toggle_stack(&base, cx);
        true
    }

    /// The pull requests on screen, before grouping: [`listed`].
    fn items(&self) -> Option<Vec<PrSummary>> {
        let page = self.list.value()?;
        Some(listed(page, &self.queue, self.filter))
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.list.start_refresh();
        self.more = None;
        self._more = None;
        self.anchor = None;
        let repo = self.repo.clone();
        let filter = self.filter;
        let text = search_text(&self.search.read(cx).value());
        let (queue_repo, queue_text) = (repo.clone(), text.clone());
        self._load = Some(common::run(
            cx,
            move |gh| gh.repo_pull_requests(&repo, filter, text.as_deref(), None),
            |view, result, _| view.list.finish(result),
        ));
        // Without it the list still reads by the pages loaded, so a failed
        // read leaves the queue empty rather than the page in error.
        self._queue = (filter == PrListFilter::Open).then(|| {
            common::run(
                cx,
                move |gh| gh.repo_review_queue(&queue_repo, queue_text.as_deref()),
                |view, result, cx| {
                    view.queue = result.unwrap_or_default();
                    cx.notify();
                },
            )
        });
        if filter != PrListFilter::Open {
            self.queue.clear();
        }
        cx.notify();
    }

    /// The row at the top of the reading area, and how far below the
    /// area's top it starts.
    fn reading_anchor(&self) -> Option<(Spot, Pixels)> {
        let viewport = self.scroll.bounds();
        self.spots().into_iter().find_map(|(spot, children)| {
            let bounds = self.rows.bounds_for_item(*children.end())?;
            (bounds.bottom() > viewport.top()).then(|| (spot, bounds.top() - viewport.top()))
        })
    }

    /// Scrolls the row Load more left at the top of the reading area back
    /// to where it was, once the new rows are laid out above or below it.
    fn restore_anchor(&mut self, cx: &mut Context<Self>) {
        let Some((spot, at)) = self.anchor.take() else {
            return;
        };
        let Some((_, children)) = self.spots().into_iter().find(|(known, _)| *known == spot) else {
            return;
        };
        let Some(bounds) = self.rows.bounds_for_item(*children.end()) else {
            return;
        };
        let viewport = self.scroll.bounds();
        let mut offset = self.scroll.offset();
        offset.y -= bounds.top() - viewport.top() - at;
        self.scroll.set_offset(offset);
        cx.notify();
    }

    fn debounce_reload(&mut self, cx: &mut Context<Self>) {
        self._debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |view, cx| view.reload(cx)).ok();
        }));
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.list.value().and_then(|page| page.next_cursor.clone()) else {
            return;
        };
        self.more = Some(Ok(()));
        let repo = self.repo.clone();
        let filter = self.filter;
        let text = search_text(&self.search.read(cx).value());
        self._more = Some(common::run(
            cx,
            move |gh| gh.repo_pull_requests(&repo, filter, text.as_deref(), Some(&cursor)),
            |view, result, _| match result {
                Ok(next) => {
                    view.more = None;
                    view.anchor = view.reading_anchor();
                    if let Loadable::Loaded(page) = &mut view.list {
                        page.items.extend(next.items);
                        page.next_cursor = next.next_cursor;
                        page.total_count = next.total_count;
                    }
                }
                Err(error) => view.more = Some(Err(error)),
            },
        ));
        cx.notify();
    }

    fn toggle_watch(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        update_settings(cx, move |settings| {
            if let Some(index) = settings.watched_repos.iter().position(|r| r == &repo) {
                settings.watched_repos.remove(index);
            } else {
                settings.watched_repos.push(repo);
            }
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let watching = Settings::get(cx).watched_repos.contains(&self.repo);
        let url = self.repo.url();
        let busy = self.list.is_busy();
        repo_header(&self.repo)
            .action(
                Button::new("watch")
                    .ghost()
                    .small()
                    .icon(if watching {
                        IconName::EyeOff
                    } else {
                        IconName::Eye
                    })
                    .label(if watching { "Unwatch" } else { "Watch" })
                    .tooltip(if watching {
                        "Remove from the sidebar"
                    } else {
                        "Show in the sidebar"
                    })
                    .on_click(cx.listener(|view, _, _, cx| view.toggle_watch(cx))),
            )
            .action(ui::vertical_divider(cx))
            .action(
                ui::icon_button("add-repo", IconName::Plus, "Add repository…")
                    .on_click(|_, window, cx| open_add_repository(window, cx)),
            )
            .action(
                ui::icon_button("browser", IconName::ExternalLink, "Open in browser")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .action(
                ui::icon_button("refresh", IconName::RefreshCw, "Refresh")
                    .loading(busy)
                    .disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| view.refresh_page(cx))),
            )
    }

    fn filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = FILTERS
            .iter()
            .position(|(filter, _)| *filter == self.filter)
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        ui::toolbar()
            .child(ui::segmented(
                "repo-filter",
                "Pull request state",
                FILTERS.map(|(filter, label)| (Some(filter_icon(filter)), label)),
                selected,
                move |index, _, cx| {
                    let Some((filter, _)) = FILTERS.get(index).copied() else {
                        return;
                    };
                    this.update(cx, |view, cx| {
                        if view.filter != filter {
                            view.filter = filter;
                            view.reload(cx);
                        }
                    })
                    .ok();
                },
            ))
            .child(div().flex_1())
            .child(
                div().w(scale::search_width()).child(
                    Input::new(&self.search)
                        .small()
                        .prefix(Icon::new(IconName::Search).xsmall())
                        .cleanable(true),
                ),
            )
    }
}

impl crate::shell::Refreshable for RepoView {
    fn refresh_page(&mut self, cx: &mut Context<Self>) {
        if !self.list.is_busy() {
            self.reload(cx);
        }
    }
}

impl Render for RepoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        let focused = self.focus.is_focused(window);
        let body = match &self.list {
            Loadable::Failed(error) => {
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .into_any_element()
            }
            Loadable::Loading => ui::loading_rows(10).into_any_element(),
            Loadable::Loaded(page) | Loadable::Refreshing(page) if page.items.is_empty() => {
                ui::empty_state(
                    IconName::GitPullRequest,
                    "No pull requests",
                    "Nothing matches this filter.",
                )
                .into_any_element()
            }
            Loadable::Loaded(page) | Loadable::Refreshing(page) => {
                let page = page.clone();
                let items = listed(&page, &self.queue, self.filter);
                let loading_more = matches!(self.more, Some(Ok(())));
                let rows: Vec<&PrSummary> = items.iter().collect();
                let lanes = Lanes::for_rows(&rows, true, AgeFrom::Updated);
                if self.anchor.is_some() {
                    cx.on_next_frame(window, |view, _, cx| view.restore_anchor(cx));
                }
                let cursor = self.cursor.clone().filter(|_| focused);
                let mut children: Vec<AnyElement> =
                    vec![column_header(false, lanes, None, None, cx).into_any_element()];
                for section in sections(&items, self.filter) {
                    let lanes = Lanes {
                        group: section.group,
                        ..lanes
                    };
                    if let Some(group) = section.group {
                        let folded = self.folded.contains(&group);
                        children.push(
                            group_heading(group, folded, section.count(), cx)
                                .on_click(
                                    cx.listener(move |view, _, _, cx| view.toggle_group(group, cx)),
                                )
                                .into_any_element(),
                        );
                        if folded {
                            continue;
                        }
                    }
                    for line in section.lines {
                        let members = match line {
                            Line::Pr(at) => {
                                let summary = &items[at];
                                let at_cursor = cursor == Some(Spot::Pr(summary.pr.clone()));
                                children.push(
                                    pr_row(summary, lanes, at_cursor, None, None, None, cx)
                                        .into_any_element(),
                                );
                                continue;
                            }
                            Line::Stack(members) => members,
                        };
                        let stacked: Vec<&PrSummary> =
                            members.iter().map(|&member| &items[member]).collect();
                        let base = stacked[0].pr.clone();
                        let expanded = self.expanded.contains(&base);
                        let toggle = base.clone();
                        children.push(
                            stack_row(
                                StackRow {
                                    id: SharedString::from(format!("repo-stack-{}", base.number)),
                                    name: &stacked[0].title,
                                    rows: &stacked,
                                    total: stacked.len(),
                                    expanded,
                                },
                                lanes,
                                cursor == Some(Spot::Stack(base)),
                                None,
                                cx,
                            )
                            .on_click(
                                cx.listener(move |view, _, _, cx| view.toggle_stack(&toggle, cx)),
                            )
                            .into_any_element(),
                        );
                        if !expanded {
                            continue;
                        }
                        let total = stacked.len();
                        for (at, summary) in stacked.iter().enumerate() {
                            let place = StackPlace {
                                position: at + 1,
                                total,
                                last: at + 1 == total,
                            };
                            let at_cursor = cursor == Some(Spot::Pr(summary.pr.clone()));
                            children.push(
                                pr_row(summary, lanes, at_cursor, None, Some(place), None, cx)
                                    .into_any_element(),
                            );
                        }
                    }
                }
                ui::list_container()
                    .id("repo-rows")
                    .track_scroll(&self.rows)
                    .children(children)
                    .when(page.next_cursor.is_some(), |column| {
                        column.child(
                            h_flex().py_3().justify_center().child(
                                Button::new("more")
                                    .outline()
                                    .small()
                                    .icon(IconName::ChevronDown)
                                    .label(format!(
                                        "Load more ({} of {})",
                                        page.items.len(),
                                        page.total_count
                                    ))
                                    .loading(loading_more)
                                    .disabled(loading_more)
                                    .on_click(cx.listener(|view, _, _, cx| view.load_more(cx))),
                            ),
                        )
                    })
                    .children(match &self.more {
                        Some(Err(error)) => Some(common::load_error(
                            error,
                            cx.listener(|view, _, _, cx| view.load_more(cx)),
                        )),
                        _ => None,
                    })
                    .into_any_element()
            }
        };
        v_flex()
            .id("repo-page")
            .role(Role::Group)
            .aria_label(self.repo.to_string())
            .size_full()
            .key_context("RepoView")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_cursor(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_cursor(false, cx)))
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.open_cursor(window, cx)),
            )
            // Right and Left open and fold the stack under the cursor, as
            // on Reviews, while the list itself has the focus.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                if !this.focus.is_focused(window)
                    || keystroke.modifiers.modified()
                    || !matches!(keystroke.key.as_str(), "right" | "left")
                {
                    return;
                }
                if this.arrow(keystroke.key == "right", cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.header(cx))
            .child(repo_tabs(
                &self.repo,
                RepoTab::PullRequests,
                self.list.value().map(|page| page.total_count as usize),
            ))
            .child(self.filters(cx))
            .child(
                div()
                    .id("repo")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}

/// Opens the "add repository" dialog: searches the viewer's repositories and
/// adds the chosen one to `Settings::watched_repos`.
pub fn open_add_repository(window: &mut Window, cx: &mut App) {
    let picker = cx.new(|cx| AddRepository::new(window, cx));
    let focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            picker.update(cx, |picker, cx| {
                picker.query.update(cx, |query, cx| query.focus(window, cx))
            });
        }
        dialog
            .title("Add repository")
            .w(dp(560.))
            .child(picker.clone())
    });
}

/// Repositories to offer for `query`: the viewer's repositories whose name
/// contains it, then search results not already listed, at most 30.
fn candidates(viewer: &[RepoSummary], searched: &[RepoSummary], query: &str) -> Vec<RepoSummary> {
    let needle = query.trim().to_lowercase();
    let mut out: Vec<RepoSummary> = viewer
        .iter()
        .filter(|repo| needle.is_empty() || repo.repo.to_string().to_lowercase().contains(&needle))
        .cloned()
        .collect();
    for repo in searched {
        if !out.iter().any(|known| known.repo == repo.repo) {
            out.push(repo.clone());
        }
    }
    out.truncate(30);
    out
}

struct AddRepository {
    query: Entity<InputState>,
    viewer: Loadable<Vec<RepoSummary>>,
    searched: Vec<RepoSummary>,
    _viewer: Option<Task<()>>,
    _search: Option<Task<()>>,
    _changes: Subscription,
}

impl AddRepository {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter your repositories or search owner/name")
        });
        let changes = cx.subscribe(&query, |picker: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                picker.search(cx);
                cx.notify();
            }
        });
        let viewer = common::run(
            cx,
            |gh| gh.viewer_repositories(),
            |picker: &mut Self, result, _| picker.viewer.finish(result),
        );
        Self {
            query,
            viewer: Loadable::Loading,
            searched: Vec::new(),
            _viewer: Some(viewer),
            _search: None,
            _changes: changes,
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).value().trim().to_owned();
        if text.chars().count() < 3 {
            self.searched.clear();
            self._search = None;
            return;
        }
        let Some(github) = common::client(cx) else {
            return;
        };
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let query = format!("{text} in:name");
            let found = cx
                .background_executor()
                .spawn(async move { github.search_repositories(&query) })
                .await;
            this.update(cx, |picker, cx| {
                picker.searched = found.unwrap_or_default();
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for AddRepository {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let watched = Settings::get(cx).watched_repos.clone();
        let query = self.query.read(cx).value().to_string();
        let list = match &self.viewer {
            Loadable::Failed(error) => ui::error_banner(error.message(), |_, window, cx| {
                window.close_dialog(cx);
                open_add_repository(window, cx);
            })
            .into_any_element(),
            Loadable::Loading => ui::skeleton_rows(5).into_any_element(),
            Loadable::Loaded(viewer) | Loadable::Refreshing(viewer) => {
                let items = candidates(viewer, &self.searched, &query);
                if items.is_empty() {
                    div()
                        .p_3()
                        .child(ui::empty_line(
                            IconName::FolderSearch,
                            "No repositories match",
                            cx,
                        ))
                        .into_any_element()
                } else {
                    v_flex()
                        .children(items.into_iter().map(|summary| {
                            let is_watched = watched.contains(&summary.repo);
                            let repo = summary.repo.clone();
                            h_flex()
                                .id(SharedString::from(format!("pick-{}", summary.repo)))
                                .role(Role::Button)
                                .aria_label(format!("Watch {}", summary.repo))
                                .px_2()
                                .py_1p5()
                                .gap_3()
                                .items_center()
                                .rounded(cx.theme().radius)
                                .hover(|row| row.bg(cx.theme().list_hover))
                                .on_click(move |_, window, cx| {
                                    let added = repo.clone();
                                    update_settings(cx, move |settings| {
                                        if !settings.watched_repos.contains(&added) {
                                            settings.watched_repos.push(added);
                                        }
                                    });
                                    window.close_dialog(cx);
                                    navigate(Route::Repo(repo.clone()), window, cx);
                                })
                                .child(
                                    Icon::new(IconName::FolderGit2)
                                        .small()
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(scale::body())
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(summary.repo.to_string()),
                                        )
                                        .children(summary.description.clone().map(|text| {
                                            div()
                                                .text_xs()
                                                .truncate()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(text)
                                        })),
                                )
                                .when(summary.is_private, |row| {
                                    row.child(
                                        Icon::new(IconName::Lock)
                                            .xsmall()
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                })
                                .when(is_watched, |row| {
                                    row.child(crate::ui::chip().small().child("Watching"))
                                })
                        }))
                        .into_any_element()
                }
            }
        };
        v_flex()
            .gap_3()
            .child(Input::new(&self.query).prefix(Icon::new(IconName::Search).small()))
            .child(
                div()
                    .id("repo-candidates")
                    .max_h(dp(380.))
                    .overflow_y_scroll()
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};
    use loupe_github::queue::QueueGroup;
    use loupe_github::{Page, PrListFilter, PrSummary, RepoRef, RepoSummary, ReviewStanding};

    use super::{Line, Spot, build, candidates, listed, search_text, sections};
    use crate::github::common::Loadable;
    use crate::github::pr_row::tests::summary;

    fn repo(slug: &str) -> RepoSummary {
        RepoSummary {
            repo: RepoRef::parse(slug).expect("slug"),
            description: None,
            is_private: false,
            pushed_at: None,
        }
    }

    #[test]
    fn candidates_filter_viewer_repos_then_add_new_search_results() {
        let viewer = [repo("acme/web"), repo("acme/api"), repo("me/dotfiles")];
        let searched = [repo("acme/web"), repo("other/webkit")];
        let names: Vec<String> = candidates(&viewer, &searched, " WEB ")
            .into_iter()
            .map(|r| r.repo.to_string())
            .collect();
        assert_eq!(names, ["acme/web", "other/webkit"]);
        assert_eq!(candidates(&viewer, &[], "").len(), 3);
    }

    #[test]
    fn blank_search_is_none() {
        assert_eq!(search_text("  "), None);
        assert_eq!(search_text(" fix "), Some("fix".into()));
    }

    /// J and K walk the pull requests from the first, stopping at either
    /// end; no row holds the cursor before a key.
    #[gpui_kit::test]
    fn j_and_k_walk_the_pull_requests(cx: &mut TestAppContext) {
        let repo = RepoRef::new("a", "x");
        let (mut cx, view) = crate::ui::open_list(cx, move |window, cx| build(repo, window, cx));
        let cx = &mut cx;
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.list = Loadable::Loaded(Page {
                items: vec![summary("x", 1, 0), summary("x", 2, 0), summary("x", 3, 0)],
                total_count: 3,
                next_cursor: None,
            });
            cx.notify();
        });
        cx.run_until_parked();
        let cursor = |cx: &mut VisualTestContext| {
            view.read_with(cx, |view, _| match &view.cursor {
                Some(Spot::Pr(pr)) => Some(pr.number),
                _ => None,
            })
        };
        assert_eq!(cursor(cx), None);
        cx.simulate_keystrokes("j");
        assert_eq!(cursor(cx), Some(1), "the first J lands on the first row");
        cx.simulate_keystrokes("j j j");
        assert_eq!(cursor(cx), Some(3), "and stops at the last");
        cx.simulate_keystrokes("k");
        assert_eq!(cursor(cx), Some(2));
    }

    fn on(number: u64, base: &str, head: &str) -> PrSummary {
        PrSummary {
            base_ref: base.into(),
            head_ref: head.into(),
            ..summary("x", number, 0)
        }
    }

    fn with(review: Option<ReviewStanding>, draft: bool, pr: PrSummary) -> PrSummary {
        PrSummary {
            viewer_review: review,
            is_draft: draft,
            ..pr
        }
    }

    /// The loupe fixture as GitHub lists it (newest first): what asks for
    /// the viewer comes first, the draft stack is one line in merge order,
    /// and the drafts close the list.
    fn queue() -> Vec<PrSummary> {
        vec![
            with(
                Some(ReviewStanding::Authored),
                false,
                on(196, "main", "refactor"),
            ),
            with(
                Some(ReviewStanding::Authored),
                true,
                on(186, "base", "stack-1"),
            ),
            with(
                Some(ReviewStanding::Authored),
                true,
                on(188, "stack-2", "stack-3"),
            ),
            with(
                Some(ReviewStanding::Authored),
                true,
                on(187, "stack-1", "stack-2"),
            ),
            with(
                Some(ReviewStanding::Reviewed),
                false,
                on(50, "main", "done"),
            ),
            with(
                Some(ReviewStanding::Authored),
                true,
                on(189, "base", "large"),
            ),
            with(None, false, on(60, "main", "other")),
            with(
                Some(ReviewStanding::ChangesSinceReview),
                false,
                on(44, "main", "deps"),
            ),
            with(
                Some(ReviewStanding::Requested),
                false,
                on(70, "main", "asked"),
            ),
        ]
    }

    #[test]
    fn open_pull_requests_read_as_a_review_queue() {
        let items = queue();
        let queue = sections(&items, PrListFilter::Open);
        let groups: Vec<Option<QueueGroup>> = queue.iter().map(|section| section.group).collect();
        assert_eq!(
            groups,
            [
                Some(QueueGroup::ReviewRequested),
                Some(QueueGroup::ChangesSinceReview),
                Some(QueueGroup::NotReviewed),
                Some(QueueGroup::Yours),
                Some(QueueGroup::Reviewed),
                Some(QueueGroup::Draft),
            ]
        );
        let firsts: Vec<&Line> = queue.iter().map(|section| &section.lines[0]).collect();
        assert_eq!(
            firsts,
            [
                &Line::Pr(8),
                &Line::Pr(7),
                &Line::Pr(6),
                &Line::Pr(0),
                &Line::Pr(4),
                &Line::Stack(Vec::from([1, 3, 2])),
            ],
            "the draft stack is one line, base first"
        );
        assert_eq!(queue[5].lines.get(1), Some(&Line::Pr(5)));
        assert_eq!(queue[5].count(), 4);
        let closed = sections(&items, PrListFilter::Closed);
        assert_eq!(closed.len(), 1, "other lists keep GitHub's order");
        assert_eq!(closed[0].group, None);
        assert_eq!(closed[0].lines.len(), items.len());
    }

    #[test]
    fn a_list_in_one_group_has_no_heading() {
        let items = vec![summary("x", 1, 0), summary("x", 2, 0)];
        let only = sections(&items, PrListFilter::Open);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].group, None);
    }

    /// A review asked of you on a page the list has not loaded still comes
    /// first; another list keeps only what its pages hold.
    #[test]
    fn a_review_asked_on_a_page_not_loaded_still_leads_the_queue() {
        let page = Page {
            items: vec![summary("x", 1, 0), summary("x", 2, 0)],
            total_count: 40,
            next_cursor: Some("next".into()),
        };
        let mut asked = with(
            Some(ReviewStanding::Requested),
            false,
            on(39, "main", "far"),
        );
        asked.updated_at = page.items[0].updated_at - chrono::Duration::days(30);
        let queue = [asked.clone(), page.items[0].clone()];
        let items = listed(&page, &queue, PrListFilter::Open);
        let numbers: Vec<u64> = items.iter().map(|pr| pr.pr.number).collect();
        assert_eq!(numbers.len(), 3, "each pull request once");
        assert_eq!(numbers[2], 39, "newest update first");
        let groups = sections(&items, PrListFilter::Open);
        assert_eq!(groups[0].group, Some(QueueGroup::ReviewRequested));
        assert_eq!(groups[0].lines, [Line::Pr(2)]);
        assert_eq!(listed(&page, &queue, PrListFilter::Closed).len(), 2);
    }

    /// Enter opens the stack under the cursor, J walks into its rows in
    /// merge order, Left folds it back with the cursor on its row.
    #[gpui_kit::test]
    fn the_keys_open_and_fold_a_stack(cx: &mut TestAppContext) {
        let repo = RepoRef::new("a", "x");
        let (mut cx, view) = crate::ui::open_list(cx, move |window, cx| build(repo, window, cx));
        let cx = &mut cx;
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            let items: Vec<PrSummary> = queue().into_iter().skip(1).take(3).collect();
            view.list = Loadable::Loaded(Page {
                total_count: items.len() as u32,
                items,
                next_cursor: None,
            });
            cx.notify();
        });
        cx.run_until_parked();
        let cursor = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.cursor.clone());
        let number = |spot: Option<Spot>| match spot {
            Some(Spot::Pr(pr)) => Some(pr.number),
            Some(Spot::Stack(base)) => Some(base.number + 1000),
            None => None,
        };
        cx.simulate_keystrokes("j");
        assert_eq!(number(cursor(cx)), Some(1186), "the stack's row first");
        cx.simulate_keystrokes("j");
        assert_eq!(number(cursor(cx)), Some(1186), "folded, it is the only row");
        cx.simulate_keystrokes("enter");
        cx.simulate_keystrokes("j j j");
        assert_eq!(number(cursor(cx)), Some(188), "base first: 186, 187, 188");
        cx.simulate_keystrokes("k");
        assert_eq!(number(cursor(cx)), Some(187));
        cx.simulate_keystrokes("left");
        assert_eq!(number(cursor(cx)), Some(1186));
        assert!(view.read_with(cx, |view, _| view.expanded.is_empty()));
        cx.simulate_keystrokes("right");
        assert!(!view.read_with(cx, |view, _| view.expanded.is_empty()));
    }
}
