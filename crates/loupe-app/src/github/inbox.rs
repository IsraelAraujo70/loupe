//! `Route::Inbox` (Reviews): a tab per dashboard search (To review, Mine,
//! Involved) and All, which adds on a saved view that names repositories
//! every other open pull request in them. Inside a tab, pull requests group
//! by their next step. Filter clauses (the Linear board's "Add Filter…"
//! pattern) narrow every tab and can be saved as views.

mod dialogs;
mod filter_menu;
mod selection;
mod views_store;

use std::collections::HashMap;
use std::rc::Rc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::reviews::{
    self, Clause, NextStep, Op, ReviewTab, Sort, SortDir, SortKey, ViewsFile,
};
use loupe_github::{
    Inbox, Notification as GitHubNotification, NotificationSubject, Page, PrRef, PrSummary,
};

use self::filter_menu::{FilterMenu, FilterOptions};
use self::selection::{CONFIRM_ABOVE, Selection};
use super::common::{self, Loadable};
use super::pr_row::{AgeFrom, OnSelect, OnSort, RowSelect, SelectGesture, pr_row, select_box};
use crate::route::Route;
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::shell::{self, GroupTarget, PaneGroup};
use crate::theme::scale;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

gpui_kit::actions!(
    github,
    [
        /// F on the Reviews screen: open "Add Filter…".
        OpenReviewsFilter,
        /// Cmd+A on the Reviews screen: pick every row of the open tab.
        SelectTabRows,
        /// Escape on the Reviews screen: drop the selection.
        ClearReviewsSelection
    ]
);

/// The tabs the one inbox request fills.
const INBOX_TABS: [ReviewTab; 3] = [ReviewTab::ToReview, ReviewTab::Mine, ReviewTab::Involved];

/// The line an empty tab shows when no filter is set.
fn tab_empty(tab: ReviewTab) -> (IconName, &'static str) {
    match tab {
        ReviewTab::ToReview => (IconName::CircleCheck, "Nothing waiting on you"),
        ReviewTab::Mine => (IconName::GitPullRequest, "You have no open pull requests"),
        ReviewTab::Involved => (IconName::AtSign, "No other open pull requests involve you"),
        ReviewTab::All => (IconName::CircleCheck, "No open pull requests"),
    }
}

/// The tab's page in the inbox; `None` for All, which gathers the others.
fn tab_page(tab: ReviewTab, inbox: &Inbox) -> Option<&Page<PrSummary>> {
    match tab {
        ReviewTab::ToReview => Some(&inbox.review_requested),
        ReviewTab::Mine => Some(&inbox.authored),
        ReviewTab::Involved => Some(&inbox.involved),
        ReviewTab::All => None,
    }
}

/// Whether its rows show the author: your own pull requests need none.
fn shows_author(tab: ReviewTab) -> bool {
    tab != ReviewTab::Mine
}

/// What one tab shows under the clauses: its rows, the number on the tab,
/// and a note when the page holds only part of the search.
struct Shown<'a> {
    items: Vec<&'a PrSummary>,
    count: usize,
    note: Option<String>,
}

fn shown<'a>(page: &'a Page<PrSummary>, clauses: &[Clause]) -> Shown<'a> {
    let items: Vec<&PrSummary> = page
        .items
        .iter()
        .filter(|pr| reviews::matches(pr, clauses))
        .collect();
    let truncated = page.next_cursor.is_some() || page.total_count as usize > page.items.len();
    if clauses.is_empty() {
        return Shown {
            count: page.total_count as usize,
            note: truncated
                .then(|| format!("Showing {} of {}", page.items.len(), page.total_count)),
            items,
        };
    }
    Shown {
        count: items.len(),
        note: truncated.then(|| {
            format!(
                "Filtered from the first {} of {}",
                page.items.len(),
                page.total_count
            )
        }),
        items,
    }
}

/// The All tab under the clauses: every pull request the other tabs show,
/// then those of the view's "All open" search (`all_open`) that none of them
/// shows, each once. Its count is the rows it shows; the note says when the
/// search, or any other tab, holds more than its first page.
fn shown_all<'a>(
    tabs: &[&Shown<'a>],
    all_open: Option<&'a Page<PrSummary>>,
    clauses: &[Clause],
) -> Shown<'a> {
    let mut items: Vec<&'a PrSummary> = Vec::new();
    let open = all_open.map(|page| shown(page, clauses));
    let candidates = tabs
        .iter()
        .flat_map(|shown| shown.items.iter().copied())
        .chain(open.iter().flat_map(|open| open.items.iter().copied()));
    for pr in candidates {
        if !items.iter().any(|known| known.pr == pr.pr) {
            items.push(pr);
        }
    }
    let partial = tabs.iter().any(|shown| shown.note.is_some());
    let note = open.and_then(|open| open.note).or_else(|| {
        partial.then(|| format!("Showing the {} loaded; GitHub has more", items.len()))
    });
    Shown {
        count: items.len(),
        note,
        items,
    }
}

/// What a Reviews page and its tab are called with view `view_id` open:
/// the saved view's name, or "Reviews" on All (or when the view is gone).
fn title(view_id: Option<&str>, views: &ViewsFile) -> SharedString {
    view_id
        .and_then(|id| views.get(id))
        .map_or_else(|| "Reviews".into(), |view| view.name.clone().into())
}

/// The first tab with a row to show, in the tab row's order; To review when
/// every tab is empty.
fn first_with_rows(tabs: &[(ReviewTab, Shown<'_>)]) -> ReviewTab {
    tabs.iter()
        .find(|(_, shown)| !shown.items.is_empty())
        .map_or(ReviewTab::ToReview, |(tab, _)| *tab)
}

/// One group of the open tab: its pull requests in their order on screen,
/// and whether it is folded.
struct Group<'a> {
    step: NextStep,
    collapsed: bool,
    rows: Vec<&'a PrSummary>,
}

/// `items` in their groups ([`reviews::next_step`]), in [`NextStep::ALL`]'s
/// order, each sorted by `sort` on its own. A group with no pull request is
/// left out.
fn grouped<'a>(
    items: &[&'a PrSummary],
    sort: Option<Sort>,
    collapsed: &[NextStep],
) -> Vec<Group<'a>> {
    NextStep::ALL
        .iter()
        .filter_map(|&step| {
            let mut rows: Vec<&PrSummary> = items
                .iter()
                .copied()
                .filter(|pr| reviews::next_step(pr) == step)
                .collect();
            reviews::sort_rows(&mut rows, sort);
            (!rows.is_empty()).then(|| Group {
                step,
                collapsed: collapsed.contains(&step),
                rows,
            })
        })
        .collect()
}

/// The saved view on screen and the clauses and sort order edited on top of
/// it. The draft is "unsaved" while either differs from the saved view; with
/// no view, while it has any clause or a sort.
#[derive(Clone, Debug, Default, PartialEq)]
struct PageState {
    view_id: Option<String>,
    draft: Vec<Clause>,
    /// The order of each group's rows; `None` is GitHub's order.
    sort: Option<Sort>,
    /// The view's saved "All open" choice; `None` means the default (on).
    all_open: Option<bool>,
    /// The open tab; `None` until the first tab with rows is known.
    tab: Option<ReviewTab>,
    /// The groups folded in every tab.
    collapsed: Vec<NextStep>,
}

impl PageState {
    fn is_dirty(&self, views: &ViewsFile) -> bool {
        match self.view_id.as_deref().and_then(|id| views.get(id)) {
            Some(saved) => saved.filters != self.draft || saved.sort != self.sort,
            None => !self.draft.is_empty() || self.sort.is_some(),
        }
    }

    /// Shows view `id` (or no view) with its saved clauses and sort order.
    fn switch(&mut self, views: &ViewsFile, id: Option<&str>) {
        let view = id.and_then(|id| views.get(id));
        self.view_id = view.map(|view| view.id.clone());
        self.draft = view.map(|view| view.filters.clone()).unwrap_or_default();
        self.sort = view.and_then(|view| view.sort);
        self.all_open = view.and_then(|view| view.all_open);
        self.tab = view.and_then(|view| view.tab);
        self.collapsed = view.map(|view| view.collapsed.clone()).unwrap_or_default();
    }

    /// Whether "All open" is on for the view on screen. Only a saved view
    /// holds the choice; it applies once the clauses name a repository or
    /// an organization.
    fn all_open_on(&self) -> bool {
        self.view_id.is_some() && self.all_open.unwrap_or(true)
    }

    /// The search "All open" shows now, if it shows.
    fn all_open_query(&self) -> Option<String> {
        self.all_open_on()
            .then(|| reviews::all_open_query(&self.draft))
            .flatten()
    }
}

pub struct InboxView {
    inbox: Loadable<Inbox>,
    scroll: ScrollHandle,
    /// The sideways scroll of the views' tabs, keyed by the open tab's
    /// place in the row ("All" is 0).
    views_row: ui::ScrollRow<usize>,
    page: PageState,
    /// Focused while nothing more specific is, so F reaches the screen.
    focus: FocusHandle,
    filter_menu: Entity<FilterMenu>,
    filter_open: bool,
    /// A failed save or an unreadable views file.
    notice: Option<SharedString>,
    /// The pull request under the keyboard cursor (J and K, or the arrows).
    /// Kept by reference, so a refresh that reorders the list keeps it.
    cursor: Option<PrRef>,
    /// "All open" results by search, so going back to a view shows its
    /// rows without asking GitHub again; Refresh asks again.
    all_open: HashMap<String, Loadable<Page<PrSummary>>>,
    /// The search in flight for each query, so leaving a view does not
    /// cancel it.
    all_open_loads: HashMap<String, Task<()>>,
    /// The pull requests picked with the checkboxes.
    selection: Selection,
    /// Set while the selection bar asks before opening more than
    /// [`CONFIRM_ABOVE`] pull requests.
    confirm: Option<Bulk>,
    /// "Mark as read" in flight.
    marking: Option<Task<()>>,
    /// Whether the last frame showed the selection bar.
    bar_shown: bool,
    /// What the last "Mark as read" did, or why it failed (`true`).
    mark_note: Option<(SharedString, bool)>,
    /// The tab the page opened on, chosen once fresh results arrived; it
    /// stands while no tab was picked or saved (`PageState::tab`), so a
    /// refresh never switches tabs under the user. Never saved with a view.
    opened_on: Option<ReviewTab>,
    _load: Option<Task<()>>,
    _checks: Subscription,
}

/// The unread notification threads about any pull request of `prs`.
fn threads_of(notifications: &[GitHubNotification], prs: &[PrRef]) -> Vec<String> {
    notifications
        .iter()
        .filter(|notification| notification.unread)
        .filter(|notification| {
            matches!(&notification.subject, NotificationSubject::PullRequest(pr) if prs.contains(pr))
        })
        .map(|notification| notification.id.clone())
        .collect()
}

/// Where the keyboard cursor goes next among `rows`, the pull requests on
/// screen in order: one step from `current`, staying at either end. With no
/// cursor yet, or one whose pull request left the list, it starts at the
/// first row going down and at the last going up.
fn step_cursor(rows: &[PrRef], current: Option<&PrRef>, down: bool) -> Option<PrRef> {
    let last = rows.len().checked_sub(1)?;
    let at = current.and_then(|pr| rows.iter().position(|row| row == pr));
    let next = match (at, down) {
        (Some(at), true) => (at + 1).min(last),
        (Some(at), false) => at.saturating_sub(1),
        (None, true) => 0,
        (None, false) => last,
    };
    rows.get(next).cloned()
}

pub fn view(window: &mut Window, cx: &mut App) -> AnyView {
    build(window, cx).into()
}

fn build(window: &mut Window, cx: &mut App) -> Entity<InboxView> {
    cx.new(|cx| {
        let weak = cx.weak_entity();
        let filter_menu = cx.new(|cx| FilterMenu::new(weak, window, cx));
        let feed = super::checks_feed::feed(cx);
        let checks = cx.subscribe(&feed, |view: &mut InboxView, _, loaded, cx| {
            if let Some(inbox) = view.inbox.value_mut()
                && loaded.apply(inbox)
            {
                cx.notify();
            }
        });
        let mut view = InboxView {
            inbox: Loadable::Loading,
            scroll: ScrollHandle::new(),
            views_row: ui::ScrollRow::default(),
            page: PageState::default(),
            focus: cx.focus_handle(),
            filter_menu,
            filter_open: false,
            notice: views_store::take_warning(cx),
            cursor: None,
            all_open: HashMap::new(),
            all_open_loads: HashMap::new(),
            selection: Selection::default(),
            confirm: None,
            marking: None,
            bar_shown: false,
            mark_note: None,
            opened_on: None,
            _load: None,
            _checks: checks,
        };
        view.reload(cx);
        view
    })
}

/// Where the selection bar opens the picked pull requests.
#[derive(Clone, Debug, PartialEq)]
enum Bulk {
    /// One background tab each, in Loupe.
    Tabs,
    /// One browser tab each, on GitHub.
    Browser,
    /// One background tab each, in a group of the active pane.
    Group(GroupPick),
}

/// The group "Open in group" opens the picked pull requests in.
#[derive(Clone, Debug, PartialEq)]
enum GroupPick {
    /// A new group; a dialog asks for its name and color.
    New,
    /// A new group named after the saved view on screen.
    View(String),
    /// A group the pane has.
    Existing(PaneGroup),
}

impl GroupPick {
    /// Opens `routes` in the background, in order, in this group. A page a
    /// tab already shows moves that tab into the group.
    fn open(self, routes: Vec<Route>, window: &mut Window, cx: &mut App) {
        match self {
            GroupPick::New => shell::ask_new_group(
                move |name, color, window, cx| {
                    let target = GroupTarget::New {
                        name: name.to_owned(),
                        color: Some(color),
                    };
                    shell::open_in_group(target, routes.clone(), window, cx)
                },
                window,
                cx,
            ),
            GroupPick::View(name) => {
                shell::open_in_group(GroupTarget::New { name, color: None }, routes, window, cx)
            }
            GroupPick::Existing(group) => {
                shell::open_in_group(GroupTarget::Existing(group.group), routes, window, cx)
            }
        }
    }
}

type Edit = Rc<dyn Fn(&mut Vec<Clause>)>;

/// A menu entry that edits the draft when picked.
fn edit_item(
    label: impl Into<SharedString>,
    checked: bool,
    view: &WeakEntity<InboxView>,
    edit: impl Fn(&mut Vec<Clause>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    let edit: Edit = Rc::new(edit);
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, _, cx| {
            let edit = edit.clone();
            view.update(cx, |this, cx| this.edit_draft(|draft| edit(draft), cx))
                .ok();
        })
}

/// One clause as an outlined chip with a caret; its menu flips the operator,
/// toggles values, or removes it.
fn chip(
    ix: usize,
    clause: Clause,
    options: Rc<FilterOptions>,
    view: &WeakEntity<InboxView>,
) -> AnyElement {
    let view = view.clone();
    let text = reviews::describe(&clause);
    Button::new(("reviews-filter-chip", ix))
        .outline()
        .small()
        .label(text.clone())
        .accessibility_label(format!("Filter: {text}"))
        .dropdown_caret(true)
        .menu_trigger()
        .dropdown_menu(move |menu: PopupMenu, _, _| {
            let op = |label: &'static str, wanted: Op| {
                edit_item(label, clause.op == wanted, &view, move |draft| {
                    reviews::set_op(draft, ix, wanted)
                })
            };
            let field = clause.field;
            let menu = menu
                .min_w(px(220.))
                .max_h(px(360.))
                .scrollable(true)
                .item(op("is", Op::In))
                .item(op("is not", Op::NotIn))
                .item(PopupMenuItem::separator());
            let menu = options.of(field).iter().fold(menu, |menu, option| {
                let value = option.value.clone();
                menu.item(edit_item(
                    option.label.clone(),
                    clause.values.contains(&option.value),
                    &view,
                    move |draft| reviews::toggle_value(draft, field, &value),
                ))
            });
            let remove = edit_item("Remove filter", false, &view, move |draft| {
                if ix < draft.len() {
                    draft.remove(ix);
                }
            })
            .icon(IconName::Close);
            menu.item(PopupMenuItem::separator()).item(remove)
        })
        .into_any_element()
}

impl InboxView {
    /// The page's title, which its tab shows too: the open saved view's
    /// name, or "Reviews" on All.
    pub fn title(&self, cx: &App) -> SharedString {
        let none = ViewsFile::default();
        title(
            self.page.view_id.as_deref(),
            views_store::loaded(cx).unwrap_or(&none),
        )
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let key = crate::cache::github_key(cx, "reviews");
        if self.inbox.value().is_none()
            && let Some(value) = key.as_deref().and_then(crate::cache::get)
        {
            self.inbox = Loadable::Loaded(value);
        }
        self.inbox.start_refresh();
        self._load = Some(common::run(
            cx,
            |gh| gh.inbox(),
            move |view, result, _| {
                if let (Some(key), Ok(value)) = (&key, &result) {
                    crate::cache::put(key, value);
                }
                view.inbox.finish(result);
            },
        ));
        self.load_all_open(true, cx);
        cx.notify();
    }

    /// Starts the search "All open" shows now, unless this run already has
    /// it and `again` is false. A stored copy shows while it loads.
    fn load_all_open(&mut self, again: bool, cx: &mut Context<Self>) {
        let Some(query) = self.page.all_open_query() else {
            return;
        };
        if !again && self.all_open.contains_key(&query) {
            return;
        }
        let key = crate::cache::github_key(cx, &format!("reviews/all-open/{query}"));
        let state = self.all_open.entry(query.clone()).or_default();
        if state.value().is_none()
            && let Some(value) = key.as_deref().and_then(crate::cache::get)
        {
            *state = Loadable::Loaded(value);
        }
        state.start_refresh();
        let (search, slot) = (query.clone(), query.clone());
        let task = common::run(
            cx,
            move |gh| gh.search_all_pull_requests(&search),
            move |view: &mut InboxView, result, _| {
                if let (Some(key), Ok(value)) = (&key, &result) {
                    crate::cache::put(key, value);
                }
                view.all_open.entry(query).or_default().finish(result);
            },
        );
        self.all_open_loads.insert(slot, task);
        cx.notify();
    }

    /// "All open" as it stands for the view on screen; `None` when it is
    /// off or the view names no repository.
    fn all_open_state(&self) -> Option<&Loadable<Page<PrSummary>>> {
        self.all_open.get(&self.page.all_open_query()?)
    }

    /// Every tab in the tab row's order with what it shows under the
    /// clauses, in GitHub's order.
    fn tab_lists<'a>(&'a self, inbox: &'a Inbox) -> Vec<(ReviewTab, Shown<'a>)> {
        let mut tabs: Vec<(ReviewTab, Shown<'a>)> = INBOX_TABS
            .iter()
            .filter_map(|&tab| Some((tab, shown(tab_page(tab, inbox)?, &self.page.draft))))
            .collect();
        let all = {
            let others: Vec<&Shown<'a>> = tabs.iter().map(|(_, shown)| shown).collect();
            shown_all(
                &others,
                self.all_open_state().and_then(Loadable::value),
                &self.page.draft,
            )
        };
        tabs.push((ReviewTab::All, all));
        tabs
    }

    /// The open tab: the one picked or saved with the view, else the one
    /// the page opened on, else the first with rows.
    fn active_tab(&self, tabs: &[(ReviewTab, Shown<'_>)]) -> ReviewTab {
        self.page
            .tab
            .or(self.opened_on)
            .unwrap_or_else(|| first_with_rows(tabs))
    }

    /// The open tab, once the inbox has loaded.
    fn current_tab(&self) -> Option<ReviewTab> {
        let inbox = self.inbox.value()?;
        Some(self.active_tab(&self.tab_lists(inbox)))
    }

    /// The open tab's groups, each sorted on its own: a sort never moves a
    /// pull request into another group.
    fn groups<'a>(&'a self, inbox: &'a Inbox) -> Vec<Group<'a>> {
        let tabs = self.tab_lists(inbox);
        let active = self.active_tab(&tabs);
        tabs.into_iter()
            .find(|(tab, _)| *tab == active)
            .map(|(_, shown)| grouped(&shown.items, self.page.sort, &self.page.collapsed))
            .unwrap_or_default()
    }

    /// Fixes the tab the page opened on once the results are fresh: a
    /// stored copy shown while they load may no longer have rows where it
    /// did, and "All open" may be the only tab with rows.
    fn settle_tab(&mut self) {
        let fresh = |state: Option<&Loadable<Page<PrSummary>>>| {
            !matches!(state, Some(Loadable::Loading | Loadable::Refreshing(_)))
        };
        if self.page.tab.is_some()
            || self.opened_on.is_some()
            || !matches!(self.inbox, Loadable::Loaded(_))
            || !fresh(self.all_open_state())
        {
            return;
        }
        self.opened_on = self.current_tab();
    }

    /// Opens `tab`. The selection and the cursor belong to the tab they were
    /// made in; a saved view keeps the tab.
    fn set_tab(&mut self, tab: ReviewTab, cx: &mut Context<Self>) {
        if self.current_tab() == Some(tab) && self.page.tab.is_some() {
            return;
        }
        let moved = self.current_tab() != Some(tab);
        self.page.tab = Some(tab);
        if moved {
            self.left_tab(cx);
        }
        self.save_display(cx);
    }

    /// What leaving a tab drops: the cursor, the scroll, and the selection.
    fn left_tab(&mut self, cx: &mut Context<Self>) {
        self.cursor = None;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.clear_selection(cx);
    }

    /// Folds or unfolds `step` in every tab; a saved view keeps it. The
    /// rows a fold hides leave the selection, so unfolding never brings
    /// back picks the bar no longer counted.
    fn toggle_group(&mut self, step: NextStep, cx: &mut Context<Self>) {
        match self
            .page
            .collapsed
            .iter()
            .position(|folded| *folded == step)
        {
            Some(at) => {
                self.page.collapsed.remove(at);
            }
            None => {
                self.page.collapsed.push(step);
                self.page.collapsed.sort();
            }
        }
        // Folded or not, the group's rows leave the selection: a fold hides
        // them, and a row that moved into a folded group (a refresh, new
        // checks) was picked while the bar no longer counted it.
        let rows: Vec<PrRef> = self
            .groups_shown()
            .into_iter()
            .filter(|(group, _, _)| *group == step)
            .flat_map(|(_, _, rows)| rows)
            .collect();
        self.selection.deselect_all(&rows);
        self.selection_changed(cx);
        self.save_display(cx);
    }

    /// Writes the open tab and the folded groups to the saved view on
    /// screen. They are where the user is, not a change to the view, so they
    /// save at once instead of waiting for Save.
    fn save_display(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.page.view_id.clone() {
            let (tab, collapsed) = (self.page.tab, self.page.collapsed.clone());
            let result = views_store::update(cx, |views| {
                let now = Utc::now();
                views.set_tab(&id, tab, now);
                views.set_collapsed(&id, collapsed, now);
            });
            if let Err(message) = result {
                self.notice = Some(message);
            }
        }
        cx.notify();
    }

    /// Sorts every group by `sort` (`None`: GitHub's order). Like a filter,
    /// it is part of the draft until the view is saved.
    fn set_sort(&mut self, sort: Option<Sort>, cx: &mut Context<Self>) {
        self.page.sort = sort;
        cx.notify();
    }

    /// What a column heading does on a click: the next step of its cycle.
    fn on_sort(&self, cx: &Context<Self>) -> OnSort {
        let view = cx.weak_entity();
        Rc::new(move |key, _, cx| {
            view.update(cx, |this, cx| {
                this.set_sort(reviews::cycle_sort(this.page.sort, key), cx)
            })
            .ok();
        })
    }

    /// Turns "All open" on or off for the view on screen and saves it.
    fn set_all_open(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(id) = self.page.view_id.clone() else {
            return;
        };
        match views_store::update(cx, |views| views.set_all_open(&id, on, Utc::now())) {
            Ok(_) => {
                self.page.all_open = Some(on);
                self.load_all_open(false, cx);
            }
            Err(message) => self.notice = Some(message),
        }
        cx.notify();
    }

    /// The values each field offers, from every pull request the inbox
    /// loaded (before filtering, so picking one value never hides the rest).
    fn filter_options(&self) -> FilterOptions {
        match self.inbox.value() {
            Some(inbox) => FilterOptions::new(
                INBOX_TABS
                    .iter()
                    .filter_map(|&tab| tab_page(tab, inbox))
                    .chain(self.all_open_state().and_then(Loadable::value))
                    .flat_map(|page| page.items.iter()),
                &self.page.draft,
            ),
            None => FilterOptions::new(std::iter::empty(), &self.page.draft),
        }
    }

    fn edit_draft(&mut self, change: impl FnOnce(&mut Vec<Clause>), cx: &mut Context<Self>) {
        let before = self.page.draft.clone();
        change(&mut self.page.draft);
        if self.page.draft != before {
            self.clear_selection(cx);
        }
        self.load_all_open(false, cx);
        cx.notify();
    }

    fn report(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        if error.is_some() {
            self.notice = error;
        }
        cx.notify();
    }

    /// Shows view `id` (or no view). Showing the same view again (its tab
    /// clicked twice, Discard) keeps the open tab and the folded groups,
    /// which a view without a saved copy holds only here.
    fn switch_view(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        let views = views_store::snapshot(cx);
        let (draft, tab) = (self.page.draft.clone(), self.current_tab());
        let same = self.page.view_id.as_deref() == id;
        let display = (self.page.tab, self.page.collapsed.clone());
        self.page.switch(&views, id);
        if same {
            (self.page.tab, self.page.collapsed) = display;
        } else {
            self.opened_on = None;
        }
        self.load_all_open(false, cx);
        if self.current_tab() != tab {
            self.left_tab(cx);
        } else if self.page.draft != draft {
            self.clear_selection(cx);
        }
        cx.notify();
    }

    fn discard_draft(&mut self, cx: &mut Context<Self>) {
        let id = self.page.view_id.clone();
        self.switch_view(id.as_deref(), cx);
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.page.view_id.clone() else {
            return;
        };
        let (draft, sort) = (self.page.draft.clone(), self.page.sort);
        let result = views_store::update(cx, |views| {
            let now = Utc::now();
            views.update_filters(&id, draft, now);
            views.set_sort(&id, sort, now);
        });
        self.report(result.err(), cx);
    }

    fn save_as_new(&mut self, name: &str, cx: &mut Context<Self>) {
        let (draft, sort) = (self.page.draft.clone(), self.page.sort);
        let (tab, collapsed) = (self.page.tab, self.page.collapsed.clone());
        match views_store::update(cx, |views| {
            let now = Utc::now();
            let id = views.create(name, draft, now);
            views.set_sort(&id, sort, now);
            views.set_tab(&id, tab, now);
            views.set_collapsed(&id, collapsed, now);
            id
        }) {
            Ok(id) => self.page.view_id = Some(id),
            Err(message) => self.notice = Some(message),
        }
        cx.notify();
    }

    fn rename_view(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let result = views_store::update(cx, |views| views.rename(id, name, Utc::now()));
        self.report(result.err(), cx);
    }

    fn delete_view(&mut self, id: &str, cx: &mut Context<Self>) {
        match views_store::update(cx, |views| views.delete(id)) {
            Ok(_) if self.page.view_id.as_deref() == Some(id) => self.switch_view(None, cx),
            Ok(_) => cx.notify(),
            Err(message) => self.report(Some(message), cx),
        }
    }

    /// Opens or closes "Add Filter…"; it opens on its first page, and focus
    /// comes back to the screen when it closes.
    fn set_filter_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open && !self.filter_open {
            self.filter_menu
                .update(cx, |menu, cx| menu.reset(window, cx));
        }
        if !open && self.filter_open {
            self.focus.focus(window, cx);
        }
        self.filter_open = open;
        cx.notify();
    }

    /// The open tab's groups as the list shows them: each group's pull
    /// requests in order, and whether it is folded.
    fn groups_shown(&self) -> Vec<(NextStep, bool, Vec<PrRef>)> {
        let Some(inbox) = self.inbox.value() else {
            return Vec::new();
        };
        self.groups(inbox)
            .into_iter()
            .map(|group| {
                let rows = group
                    .rows
                    .iter()
                    .map(|summary| summary.pr.clone())
                    .collect();
                (group.step, group.collapsed, rows)
            })
            .collect()
    }

    /// The rows on screen, top to bottom: those of every unfolded group.
    fn rows_shown(&self) -> Vec<PrRef> {
        self.groups_shown()
            .into_iter()
            .filter(|(_, collapsed, _)| !collapsed)
            .flat_map(|(_, _, rows)| rows)
            .collect()
    }

    /// The picked pull requests on screen, top to bottom, each once.
    fn picked(&self) -> Vec<PrSummary> {
        let Some(inbox) = self.inbox.value() else {
            return Vec::new();
        };
        let summaries: Vec<&PrSummary> = self
            .groups(inbox)
            .into_iter()
            .filter(|group| !group.collapsed)
            .flat_map(|group| group.rows)
            .collect();
        let rows: Vec<PrRef> = summaries.iter().map(|summary| summary.pr.clone()).collect();
        self.selection
            .in_order(&rows)
            .iter()
            .filter_map(|pr| summaries.iter().find(|summary| summary.pr == *pr))
            .map(|summary| (*summary).clone())
            .collect()
    }

    /// A row's checkbox, or a Shift+click on it.
    fn pick(&mut self, pr: &PrRef, gesture: SelectGesture, cx: &mut Context<Self>) {
        match gesture {
            SelectGesture::Toggle => self.selection.toggle(pr),
            SelectGesture::Range => {
                self.selection.extend_to(&self.rows_shown(), pr);
            }
        }
        self.selection_changed(cx);
    }

    fn on_select(&self, cx: &Context<Self>) -> OnSelect {
        let view = cx.weak_entity();
        Rc::new(move |pr, gesture, _, cx| {
            view.update(cx, |this, cx| this.pick(pr, gesture, cx)).ok();
        })
    }

    /// Picks every row of `rows`, one group's, or drops them all when every
    /// one is already picked.
    fn toggle_rows(&mut self, rows: &[PrRef], cx: &mut Context<Self>) {
        if !rows.is_empty() && rows.iter().all(|pr| self.selection.contains(pr)) {
            self.selection.deselect_all(rows);
        } else {
            self.selection.select_all(rows);
        }
        self.selection_changed(cx);
    }

    /// Cmd+A: picks every row the open tab shows.
    fn select_tab(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows_shown();
        if !rows.is_empty() {
            self.selection.select_all(&rows);
            self.selection_changed(cx);
        }
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        self.selection_changed(cx);
    }

    /// A new selection drops a pending confirmation and the last note.
    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        self.mark_note = None;
        cx.notify();
    }

    /// Opens every picked pull request, in list order, in a background tab
    /// or in the browser. Above [`CONFIRM_ABOVE`] the bar asks first, unless
    /// `confirmed`.
    fn open_picked(
        &mut self,
        bulk: Bulk,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picked = self.picked();
        if picked.len() > CONFIRM_ABOVE && !confirmed {
            self.confirm = Some(bulk);
            cx.notify();
            return;
        }
        self.confirm = None;
        match bulk {
            Bulk::Tabs => {
                for summary in picked {
                    shell::open_in_new_tab(Route::PullRequest(summary.pr), window, cx)
                }
            }
            Bulk::Browser => {
                for summary in picked {
                    cx.open_url(&summary.url)
                }
            }
            Bulk::Group(pick) => {
                let routes = picked
                    .into_iter()
                    .map(|summary| Route::PullRequest(summary.pr))
                    .collect();
                pick.open(routes, window, cx);
            }
        }
        cx.notify();
    }

    /// The name of the saved view on screen while every row on screen is
    /// picked, for "Open as group <name>".
    fn view_group_name(&self, cx: &mut App) -> Option<String> {
        let id = self.page.view_id.as_deref()?;
        let rows = self.rows_shown();
        if rows.is_empty() || !rows.iter().all(|pr| self.selection.contains(pr)) {
            return None;
        }
        views_store::snapshot(cx)
            .get(id)
            .map(|view| view.name.clone())
    }

    /// "Open in group": a new group, a group named after the saved view
    /// when every row is picked, and the active pane's groups.
    fn group_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.weak_entity();
        let view_name = self.view_group_name(cx);
        Button::new("reviews-selection-group")
            .debug_selector(|| "reviews-selection-group".into())
            .outline()
            .small()
            .icon(IconName::Layers)
            .label("Open in group")
            .dropdown_caret(true)
            .menu_trigger()
            .dropdown_menu(move |menu: PopupMenu, _, cx| {
                let item = |label: SharedString, pick: GroupPick| {
                    let view = view.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let bulk = Bulk::Group(pick.clone());
                        view.update(cx, |this, cx| this.open_picked(bulk, false, window, cx))
                            .ok();
                    })
                };
                let menu = menu
                    .min_w(px(220.))
                    .item(item("New group…".into(), GroupPick::New));
                let menu = match &view_name {
                    Some(name) => menu.item(item(
                        format!("Open as group {name}").into(),
                        GroupPick::View(name.clone()),
                    )),
                    None => menu,
                };
                let groups = shell::active_pane_groups(cx);
                if groups.is_empty() {
                    return menu;
                }
                groups.into_iter().fold(menu.separator(), |menu, group| {
                    let (name, color) = (group.name.clone(), group.color);
                    let view = view.clone();
                    let pick = GroupPick::Existing(group);
                    menu.item(
                        PopupMenuItem::element(move |_, _| {
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .flex_none()
                                        .size_3()
                                        .rounded_full()
                                        .bg(crate::theme::tokens::tab_group(color)),
                                )
                                .child(div().min_w_0().truncate().child(name.clone()))
                        })
                        .on_click(move |_, window, cx| {
                            let bulk = Bulk::Group(pick.clone());
                            view.update(cx, |this, cx| this.open_picked(bulk, false, window, cx))
                                .ok();
                        }),
                    )
                })
            })
    }

    /// Copies the picked pull requests' links, one per line, in list order.
    fn copy_picked_links(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let links: Vec<String> = self
            .picked()
            .into_iter()
            .map(|summary| summary.url)
            .collect();
        let note = match links.len() {
            1 => "Copied 1 link".to_owned(),
            count => format!("Copied {count} links"),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(links.join("\n")));
        window.push_notification(Notification::success(note), cx);
    }

    /// Marks the unread GitHub notifications of the picked pull requests as
    /// read, and says in the bar how many there were.
    fn mark_picked_read(&mut self, cx: &mut Context<Self>) {
        let picked: Vec<PrRef> = self
            .picked()
            .into_iter()
            .map(|summary| summary.pr)
            .collect();
        if picked.is_empty() {
            return;
        }
        self.mark_note = None;
        self.marking = Some(common::run(
            cx,
            move |gh| {
                let ids = threads_of(&gh.notifications(false)?, &picked);
                super::notifications::mark_threads_read(gh, &ids)?;
                Ok(ids.len())
            },
            |view: &mut InboxView, result, _| {
                view.marking = None;
                view.mark_note = Some(match result {
                    Ok(0) => ("No unread notifications for these".into(), false),
                    Ok(1) => ("Marked 1 notification as read".into(), false),
                    Ok(count) => (
                        format!("Marked {count} notifications as read").into(),
                        false,
                    ),
                    Err(error) => (error.message().into(), true),
                });
            },
        ));
        cx.notify();
    }

    /// Moves the keyboard cursor one row and scrolls it into view.
    fn move_cursor(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let groups = self.groups_shown();
        let rows = self.rows_shown();
        let Some(next) = step_cursor(&rows, self.cursor.as_ref(), down) else {
            return;
        };
        // The row's place in the scrolled content, from the fixed heights
        // of what is above it: 8 px of padding, then per group a 32 px
        // heading, its rows unless it is folded, and a 12 px gap.
        let rem = window.rem_size();
        let row = scale::row_height().to_pixels(rem);
        let (heading, gap, pad) = (
            rems(2.).to_pixels(rem),
            rems(0.75).to_pixels(rem),
            rems(0.5).to_pixels(rem),
        );
        let mut top = pad;
        for (_, collapsed, group) in &groups {
            top += heading;
            if *collapsed {
                top += gap;
                continue;
            }
            if let Some(index) = group.iter().position(|pr| *pr == next) {
                top += row * index as f32;
                break;
            }
            top += row * group.len() as f32 + gap;
        }
        let viewport = self.scroll.bounds().size.height;
        let mut offset = self.scroll.offset();
        if top < -offset.y {
            // Room for the group's heading above its first row.
            offset.y = -(top - heading).max(px(0.));
        } else if top + row > -offset.y + viewport {
            offset.y = -(top + row + pad - viewport);
        }
        self.scroll.set_offset(offset);
        self.cursor = Some(next);
        cx.notify();
    }

    /// Opens the pull request under the cursor in this tab. With no cursor
    /// the key goes on to whoever handles it next.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_screen = self
            .cursor
            .as_ref()
            .filter(|pr| self.rows_shown().contains(pr));
        match on_screen {
            Some(pr) => crate::shell::navigate(Route::PullRequest(pr.clone()), window, cx),
            None => cx.propagate(),
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.inbox.is_busy();
        let header = ui::page_header(self.title(cx)).icon(IconName::GitPullRequest);
        let waiting = self.inbox.value().map_or(0, |inbox| {
            shown(&inbox.review_requested, &self.page.draft).count
        });
        let header = if waiting > 0 {
            header.count(waiting)
        } else {
            header
        };
        header
            .after_title(self.view_tabs(cx))
            .action(
                ui::icon_button("inbox-add-repo", IconName::Plus, "Add repository…")
                    .debug_selector(|| "inbox-add-repo".into())
                    .on_click(|_, window, cx| super::repo::open_add_repository(window, cx)),
            )
            .action(
                ui::icon_button(
                    "inbox-refresh",
                    IconName::RefreshCw,
                    if busy { "Refreshing…" } else { "Refresh" },
                )
                .debug_selector(|| "inbox-refresh".into())
                .loading(busy)
                .disabled(busy)
                .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
    }

    /// "All" and the saved views as tabs, save as new, the active view's
    /// menu, Filter, and the unsaved-changes controls. Filter comes before
    /// the controls that appear with a change, so neither it nor its open
    /// menu moves when a filter is picked. The tabs scroll sideways when
    /// they do not fit, and the open one scrolls into view when it changes;
    /// the controls after them never scroll or shrink.
    fn view_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let views = views_store::snapshot(cx);
        let dirty = self.page.is_dirty(&views);
        let active = self.page.view_id.clone();
        let view = cx.weak_entity();
        let muted = cx.theme().muted_foreground;
        let all = ui::view_tab("reviews-view-all", "All", active.is_none(), cx)
            .debug_selector(|| "reviews-view-all".into())
            .accessibility_label("All pull requests")
            .on_click(cx.listener(|this, _, _, cx| this.switch_view(None, cx)));
        let tabs: Vec<AnyElement> = views
            .views
            .iter()
            .enumerate()
            .map(|(ix, saved)| {
                let id = saved.id.clone();
                let selected = active.as_deref() == Some(saved.id.as_str());
                ui::view_tab(("reviews-view-tab", ix), saved.name.clone(), selected, cx)
                    .debug_selector(move || format!("reviews-view-tab-{ix}"))
                    .accessibility_label(format!("View {}", saved.name))
                    .on_click(cx.listener(move |this, _, _, cx| this.switch_view(Some(&id), cx)))
                    .into_any_element()
            })
            .collect();
        let save_as = {
            let view = view.clone();
            ui::icon_button("reviews-view-save-as", IconName::Plus, "Save as new view…")
                .debug_selector(|| "reviews-view-save-as".into())
                .on_click(move |_, window, cx| dialogs::save_as(view.clone(), window, cx))
        };
        let active_view = active
            .as_deref()
            .and_then(|id| views.get(id))
            .map(|saved| (saved.id.clone(), saved.name.clone()));
        let all_open_on = self.page.all_open_on();
        let has_scope = reviews::all_open_query(&self.page.draft).is_some();
        let view_menu = active_view.map(|(id, name)| {
            let view = view.clone();
            ui::icon_button("reviews-view-menu", IconName::Ellipsis, "View actions")
                .debug_selector(|| "reviews-view-menu".into())
                .menu_trigger()
                .dropdown_menu(move |menu, _, _| {
                    // Without a repository or an organization the All tab
                    // would list all of GitHub, so the switch waits for one.
                    let all_open = {
                        let view = view.clone();
                        PopupMenuItem::new(if has_scope {
                            "Show all open pull requests"
                        } else {
                            "Show all open pull requests (filter by repository first)"
                        })
                        .checked(all_open_on && has_scope)
                        .disabled(!has_scope)
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.set_all_open(!all_open_on, cx))
                                .ok();
                        })
                    };
                    let rename = {
                        let (view, id, name) = (view.clone(), id.clone(), name.clone());
                        PopupMenuItem::new("Rename view…").on_click(move |_, window, cx| {
                            dialogs::rename(view.clone(), id.clone(), name.clone(), window, cx)
                        })
                    };
                    let delete = {
                        let (view, id, name) = (view.clone(), id.clone(), name.clone());
                        PopupMenuItem::new("Delete view…")
                            .icon(IconName::Delete)
                            .on_click(move |_, window, cx| {
                                dialogs::confirm_delete(
                                    view.clone(),
                                    id.clone(),
                                    name.clone(),
                                    window,
                                    cx,
                                )
                            })
                    };
                    menu.min_w(px(200.))
                        .item(all_open)
                        .item(PopupMenuItem::separator())
                        .item(rename)
                        .item(PopupMenuItem::separator())
                        .item(delete)
                })
        });
        let open_tab = active
            .as_deref()
            .and_then(|id| views.views.iter().position(|saved| saved.id == id))
            .map_or(0, |ix| ix + 1);
        self.views_row.follow(open_tab, open_tab);
        let tab_row = h_flex()
            .on_children_prepainted(self.views_row.watch_width(cx.entity_id()))
            .id("reviews-views")
            .debug_selector(|| "reviews-views".into())
            .role(Role::TabList)
            .aria_label("Views")
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(self.views_row.handle())
            .gap_1()
            .items_center()
            .child(all)
            .children(tabs);
        // In the header's own row, after the title: one view needs no row
        // of its own.
        h_flex()
            .min_w_0()
            // The header shares a short row among the title and this; the
            // tabs give way first, so the title keeps its width while they
            // can scroll.
            .flex_shrink(1000.)
            .ml_2()
            .gap_1()
            .items_center()
            .child(tab_row)
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .items_center()
                    .child(save_as)
                    .children(view_menu)
                    .child(div().w_2())
                    .child(self.filter_button(cx))
                    .child(self.sort_button(cx))
                    .when(dirty, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .mx_1()
                                .child("Unsaved changes"),
                        )
                        .when(self.page.view_id.is_some(), |row| {
                            row.child(
                                Button::new("reviews-view-discard")
                                    .debug_selector(|| "reviews-view-discard".into())
                                    .ghost()
                                    .small()
                                    .label("Discard")
                                    .on_click(cx.listener(|this, _, _, cx| this.discard_draft(cx))),
                            )
                            .child(
                                Button::new("reviews-view-save")
                                    .debug_selector(|| "reviews-view-save".into())
                                    .small()
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
                            )
                        })
                    }),
            )
    }

    /// Chips for every clause and Clear, shown only while a filter is set.
    fn filter_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let view = cx.weak_entity();
        let options = Rc::new(self.filter_options());
        let chips: Vec<AnyElement> = self
            .page
            .draft
            .iter()
            .enumerate()
            .map(|(ix, clause)| chip(ix, clause.clone(), options.clone(), &view))
            .collect();
        if self.page.draft.is_empty() {
            return None;
        }
        Some(
            ui::toolbar()
                .gap_1p5()
                .overflow_hidden()
                .children(chips)
                .child(
                    Button::new("reviews-filters-clear")
                        .ghost()
                        .small()
                        .label("Clear")
                        .accessibility_label("Clear filters")
                        .on_click(
                            cx.listener(|this, _, _, cx| {
                                this.edit_draft(|draft| draft.clear(), cx)
                            }),
                        ),
                ),
        )
    }

    /// The filter icon and its "Add Filter…" popover, controlled by
    /// `filter_open` so a click from an accessibility client or F opens it.
    fn filter_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let menu = self.filter_menu.clone();
        let on_change = cx.weak_entity();
        let on_click = cx.weak_entity();
        Popover::new("reviews-filter-popover")
            // The button is at the leading side of the header: the menu
            // opens under it, toward the content, not over the sidebar.
            .anchor(Anchor::TopLeft)
            .p_1()
            .w(px(280.))
            .open(self.filter_open)
            .on_open_change(move |open, window, cx| {
                on_change
                    .update(cx, |this, cx| this.set_filter_open(*open, window, cx))
                    .ok();
            })
            .trigger(
                Button::new("reviews-filter-open")
                    .debug_selector(|| "reviews-filter-open".into())
                    .ghost()
                    .small()
                    .icon(IconName::ListFilter)
                    .label("Filter")
                    .selected(self.filter_open)
                    .tooltip("Filter by organization, repository, people, or status (F)")
                    .accessibility_label("Filter")
                    .on_click(move |_, window, cx| {
                        on_click
                            .update(cx, |this, cx| this.set_filter_open(true, window, cx))
                            .ok();
                    }),
            )
            .content(move |_, _, _| menu.clone())
            .into_any_element()
    }

    /// "Sort" and its menu: every field the column headings sort by (and
    /// the creation date, which has no column of its own), the direction,
    /// and the default order.
    fn sort_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let sort = self.page.sort;
        let view = cx.weak_entity();
        // An icon alone: the arrow on the sorted column already says what
        // the order is, and a label would push Save out of a narrow header.
        let tip = match sort {
            Some(sort) => format!(
                "Sorted by {}, {}",
                sort.key.label().to_lowercase(),
                sort.describe().to_lowercase()
            ),
            None => "Sort the pull requests in each group".to_owned(),
        };
        ui::icon_button("reviews-sort-open", IconName::ArrowDownUp, tip)
            .debug_selector(|| "reviews-sort-open".into())
            .selected(sort.is_some())
            .menu_trigger()
            .dropdown_menu(move |menu: PopupMenu, _, _| {
                let set = |label: &'static str, checked: bool, next: Option<Sort>| {
                    let view = view.clone();
                    PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.set_sort(next, cx)).ok();
                        })
                };
                let menu = SortKey::ALL
                    .iter()
                    .fold(menu.min_w(px(200.)), |menu, &key| {
                        // A new field keeps the direction already picked.
                        let dir = sort.map_or(SortDir::Ascending, |sort| sort.dir);
                        menu.item(set(
                            key.label(),
                            sort.is_some_and(|sort| sort.key == key),
                            Some(Sort { key, dir }),
                        ))
                    });
                let menu = match sort {
                    Some(sort) => {
                        let (ascending, descending) = sort.key.directions();
                        let with = |dir| Some(Sort { dir, ..sort });
                        menu.item(PopupMenuItem::separator())
                            .item(set(
                                ascending,
                                sort.dir == SortDir::Ascending,
                                with(SortDir::Ascending),
                            ))
                            .item(set(
                                descending,
                                sort.dir == SortDir::Descending,
                                with(SortDir::Descending),
                            ))
                    }
                    None => menu,
                };
                menu.item(PopupMenuItem::separator()).item(set(
                    "Default order",
                    sort.is_none(),
                    None,
                ))
            })
            .into_any_element()
    }

    /// The bar over the list while pull requests are picked: how many, and
    /// what to do with them. Opening more than [`CONFIRM_ABOVE`] asks here
    /// first.
    fn selection_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.picked().len();
        if count == 0 {
            return None;
        }
        let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
        let action = |id: &'static str, icon: IconName, label: &'static str| {
            Button::new(id)
                .debug_selector(move || id.into())
                .outline()
                .small()
                .icon(icon)
                .label(label)
        };
        let controls = h_flex().gap_2().items_center();
        let controls = match self.confirm.clone() {
            Some(bulk) => {
                let question = match &bulk {
                    Bulk::Tabs => format!("Open {count} pull requests in new tabs?"),
                    Bulk::Browser => format!("Open {count} pull requests on GitHub?"),
                    Bulk::Group(GroupPick::New) => {
                        format!("Open {count} pull requests in a new group?")
                    }
                    Bulk::Group(GroupPick::View(name)) => {
                        format!("Open {count} pull requests as group {name}?")
                    }
                    Bulk::Group(GroupPick::Existing(group)) => {
                        format!("Open {count} pull requests in group {}?", group.name)
                    }
                };
                controls
                    .child(div().text_sm().text_color(muted).child(question))
                    .child(
                        Button::new("reviews-selection-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("reviews-selection-confirm")
                            .debug_selector(|| "reviews-selection-confirm".into())
                            .primary()
                            .small()
                            .label(format!("Open {count}"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_picked(bulk.clone(), true, window, cx)
                            })),
                    )
            }
            None => controls
                .child(
                    action(
                        "reviews-selection-tabs",
                        IconName::AppWindow,
                        "Open in new tabs",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_picked(Bulk::Tabs, false, window, cx)
                    })),
                )
                .child(self.group_menu(cx))
                .child(
                    action(
                        "reviews-selection-github",
                        IconName::Github,
                        "Open on GitHub",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_picked(Bulk::Browser, false, window, cx)
                    })),
                )
                .child(
                    action("reviews-selection-copy", IconName::Link, "Copy links").on_click(
                        cx.listener(|this, _, window, cx| this.copy_picked_links(window, cx)),
                    ),
                )
                .child(
                    action("reviews-selection-read", IconName::Check, "Mark as read")
                        .tooltip("Mark their GitHub notifications as read")
                        .loading(self.marking.is_some())
                        .disabled(self.marking.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.mark_picked_read(cx))),
                )
                .child({
                    let t3 = crate::services::Services::get(cx).t3.is_some();
                    action("reviews-selection-t3", IconName::Bot, "Review with T3 Code")
                        .when(!t3, |button| {
                            button.disabled(true).tooltip(crate::t3::CONNECT_HINT)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            crate::t3::review_pull_requests(this.picked(), window, cx)
                        }))
                }),
        };
        let note = self.mark_note.clone().map(|(text, failed)| {
            div()
                .min_w_0()
                .truncate()
                .text_size(scale::meta())
                .text_color(if failed { danger } else { muted })
                .child(text)
        });
        Some(
            ui::toolbar()
                .debug_selector(|| "reviews-selection-bar".into())
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .mr_1()
                        .child(format!("{count} selected")),
                )
                .child(controls)
                .child(div().flex_1())
                .children(note)
                .child(
                    ui::icon_button(
                        "reviews-selection-clear",
                        IconName::Close,
                        "Clear selection (Esc)",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx))),
                )
                .into_any_element(),
        )
    }

    fn notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.notice.clone()?;
        Some(
            ui::toolbar()
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(cx.theme().danger)
                        .child(text),
                )
                .child(
                    ui::icon_button("reviews-notice-dismiss", IconName::Close, "Dismiss").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element(),
        )
    }

    /// The tab row over the list: each tab with its count under the
    /// filters, and the open tab's note at the trailing edge.
    fn tab_strip(
        &self,
        tabs: &[(ReviewTab, Shown<'_>)],
        active: ReviewTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let buttons: Vec<AnyElement> = tabs
            .iter()
            .map(|(tab, shown)| {
                let (tab, count) = (*tab, shown.count);
                let id = format!("reviews-tab-{}", tab.label());
                ui::view_tab(
                    SharedString::from(id.clone()),
                    tab.label(),
                    tab == active,
                    cx,
                )
                .debug_selector(move || id.clone())
                .accessibility_label(format!("{}, {count}", tab.label()))
                .child(ui::count_badge(count, cx))
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                .into_any_element()
            })
            .collect();
        let note = tabs
            .iter()
            .find(|(tab, _)| *tab == active)
            .and_then(|(_, shown)| shown.note.clone())
            .map(|text| {
                div()
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(text)
            });
        ui::toolbar()
            .gap_1()
            .children(buttons)
            .child(div().flex_1())
            .children(note)
            .into_any_element()
    }

    /// One group of the open tab: its heading, which folds it, and its rows
    /// unless it is folded.
    fn group(
        &self,
        tab: ReviewTab,
        group: Group<'_>,
        on_select: OnSelect,
        showing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let step = group.step;
        let rows: Vec<PrRef> = group
            .rows
            .iter()
            .map(|summary| summary.pr.clone())
            .collect();
        // The heading's box picks or drops the whole group; a folded group
        // has no rows on screen to pick.
        let all_picked = rows.iter().all(|pr| self.selection.contains(pr));
        let lead = (!group.collapsed).then(|| {
            let view = cx.weak_entity();
            select_box(
                SharedString::from(format!("reviews-select-group-{}", step.label())),
                GROUP_HEADING,
                all_picked,
                showing,
                format!("Select every pull request in {}", step.label()),
                move |_, cx| {
                    view.update(cx, |this, cx| this.toggle_rows(&rows, cx)).ok();
                },
            )
        });
        let heading = group_heading(step, group.collapsed, group.rows.len(), lead, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(step, cx)));
        let age_from = AgeFrom::for_sort(self.page.sort);
        let rows = (!group.collapsed).then(|| {
            v_flex().children(group.rows.iter().map(|summary| {
                pr_row(
                    summary,
                    true,
                    shows_author(tab),
                    self.cursor.as_ref() == Some(&summary.pr),
                    Some(RowSelect {
                        checked: self.selection.contains(&summary.pr),
                        showing,
                        on_select: on_select.clone(),
                    }),
                    age_from,
                    cx,
                )
            }))
        });
        v_flex()
            .w_full()
            .child(heading)
            .children(rows)
            .into_any_element()
    }

    /// "All open" while its first search runs or after it failed with
    /// nothing to show; `None` once it has rows (they join the All tab).
    fn all_open_placeholder(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.all_open_state()? {
            Loadable::Loading => Some(ui::skeleton_rows(3).into_any_element()),
            Loadable::Failed(error) => Some(
                common::load_error(
                    error,
                    cx.listener(|view, _, _, cx| view.load_all_open(true, cx)),
                )
                .into_any_element(),
            ),
            Loadable::Loaded(_) | Loadable::Refreshing(_) => None,
        }
    }

    fn loading(&self) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(div().px_2().child(Skeleton::new().h_3().w_32()))
            .child(ui::skeleton_rows(8))
    }
}

impl Render for InboxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        self.settle_tab();
        let selection_bar = self.selection_bar(cx);
        if selection_bar.is_some() != self.bar_shown {
            // The bar pushes the list down by its height; scrolling as far
            // keeps every row under the pointer, so the next click lands on
            // the row it aimed at. A list too short to scroll moves once.
            self.bar_shown = selection_bar.is_some();
            let height = scale::toolbar_height().to_pixels(window.rem_size());
            let mut offset = self.scroll.offset();
            offset.y = (offset.y + if self.bar_shown { -height } else { height }).min(px(0.));
            self.scroll.set_offset(offset);
        }
        let (strip, columns, body) = match &self.inbox {
            Loadable::Failed(error) => (
                None,
                None,
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .px_0()
                    .into_any_element(),
            ),
            Loadable::Loading => (None, None, self.loading().into_any_element()),
            Loadable::Loaded(inbox) | Loadable::Refreshing(inbox) => {
                let tabs = self.tab_lists(inbox);
                let active = self.active_tab(&tabs);
                let strip = self.tab_strip(&tabs, active, cx);
                let items: Vec<&PrSummary> = tabs
                    .into_iter()
                    .find(|(tab, _)| *tab == active)
                    .map(|(_, shown)| shown.items)
                    .unwrap_or_default();
                let groups = grouped(&items, self.page.sort, &self.page.collapsed);
                let showing = groups.iter().filter(|group| !group.collapsed).any(|group| {
                    group
                        .rows
                        .iter()
                        .any(|summary| self.selection.contains(&summary.pr))
                });
                // One column heading for the whole tab, outside the scroll,
                // so its labels stay over the rows' lanes.
                let columns = (!items.is_empty()).then(|| {
                    div()
                        .w_full()
                        .px_2()
                        .child(super::pr_row::column_header(
                            true,
                            true,
                            shows_author(active),
                            self.page.sort,
                            Some(self.on_sort(cx)),
                            cx,
                        ))
                        .into_any_element()
                });
                let placeholder = (active == ReviewTab::All)
                    .then(|| self.all_open_placeholder(cx))
                    .flatten();
                let empty = (items.is_empty() && placeholder.is_none()).then(|| {
                    let (icon, text) = if self.page.draft.is_empty() {
                        tab_empty(active)
                    } else {
                        (IconName::ListFilter, "No pull requests match the filters")
                    };
                    ui::empty_line(icon, text, cx).px_2().h_8()
                });
                let on_select = self.on_select(cx);
                let groups: Vec<AnyElement> = groups
                    .into_iter()
                    .map(|group| self.group(active, group, on_select.clone(), showing, cx))
                    .collect();
                let body = v_flex()
                    .gap_3()
                    .children(groups)
                    .children(empty)
                    .children(placeholder)
                    .into_any_element();
                (Some(strip), columns, body)
            }
        };
        v_flex()
            .size_full()
            .key_context("InboxView")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenReviewsFilter, window, cx| {
                this.set_filter_open(true, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectNext, window, cx| this.move_cursor(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                this.move_cursor(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.open_cursor(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectTabRows, _, cx| this.select_tab(cx)))
            .on_action(cx.listener(|this, _: &ClearReviewsSelection, _, cx| {
                // Picks hidden in a folded group do not hold the key.
                if this.picked().is_empty() {
                    this.selection.clear();
                    cx.propagate();
                } else {
                    this.clear_selection(cx);
                }
            }))
            .child(self.header(cx))
            .children(self.filter_bar(cx))
            .children(self.notice(cx))
            .children(strip)
            .children(selection_bar)
            .children(columns)
            .child(
                div()
                    .id("inbox")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    // 8 px here and 8 px inside each row put every label on
                    // the 16 px content edge while the hover fill reaches past it.
                    .child(div().w_full().px_2().py_2().child(body)),
            )
    }
}

/// The hover group of a group heading, which shows its checkbox.
const GROUP_HEADING: &str = "reviews-group-heading";

/// A group's 32 px heading on the rows' own grid: `lead` (the box that
/// picks the group) over the rows' checkboxes, or an empty lane of their
/// width, a chevron that says whether it is folded over the state icons,
/// its title over the numbers, then its count. A click folds or unfolds it.
fn group_heading(
    step: NextStep,
    collapsed: bool,
    count: usize,
    lead: Option<Div>,
    cx: &App,
) -> Stateful<Div> {
    let (muted, hover) = (cx.theme().muted_foreground, cx.theme().list_hover);
    let label = step.label();
    h_flex()
        .id(SharedString::from(format!("reviews-group-{label}")))
        .debug_selector(move || format!("reviews-group-{label}"))
        .role(Role::Button)
        .aria_label(format!("{label}, {count}"))
        .aria_expanded(!collapsed)
        .group(GROUP_HEADING)
        .h_8()
        .px_2()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .hover(move |row| row.bg(hover))
        // 12 px after the lane, as in a row; the chevron's own 8 px gap
        // follows.
        .child(
            lead.unwrap_or_else(|| div().flex_none().w(scale::dp(16.)))
                .mr_1(),
        )
        .child(
            Icon::new(if collapsed {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            })
            .with_size(scale::dp(16.))
            .text_color(muted),
        )
        .child(ui::group_title(label))
        .child(ui::count_badge(count, cx))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use gpui_kit::{
        AnyView, Bounds, Entity, InputEvent as _, Modifiers, MouseMoveEvent, Pixels, PlatformInput,
        ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
        point, px, size,
    };
    use loupe_github::reviews::{Clause, Field, Op, Sort, SortDir, SortKey, ViewsFile};
    use loupe_github::{Inbox, Page, PrRef, PrState, PrSummary, RepoRef};

    use loupe_github::reviews::{NextStep, ReviewTab};
    use loupe_github::{ChecksState, ReviewDecision};

    use super::{InboxView, PageState, build, shown, shown_all, step_cursor, title};
    use crate::github::common::Loadable;
    use crate::services::{Accounts, Services};
    use crate::store::Settings;

    fn pr(owner: &str, number: u64) -> PrSummary {
        let at = Utc::now();
        PrSummary {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new(owner, "repo"), number),
            title: String::new(),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            author: None,
            created_at: at,
            updated_at: at,
            comment_count: 0,
            additions: 0,
            deletions: 0,
            checks: None,
            review_decision: None,
            head_ref: String::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: loupe_github::Mergeable::Unknown,
        }
    }

    fn page(items: Vec<PrSummary>, total: u32, more: bool) -> Page<PrSummary> {
        Page {
            items,
            total_count: total,
            next_cursor: more.then(|| "c".to_owned()),
        }
    }

    fn org(name: &str) -> Vec<Clause> {
        vec![Clause {
            field: Field::Org,
            op: Op::In,
            values: vec![name.into()],
        }]
    }

    #[test]
    fn the_cursor_steps_through_the_rows_and_stops_at_the_ends() {
        let rows: Vec<PrRef> = (1..=3)
            .map(|number| PrRef::new(RepoRef::new("a", "repo"), number))
            .collect();
        let number = |pr: Option<PrRef>| pr.map(|pr| pr.number);
        assert_eq!(number(step_cursor(&rows, None, true)), Some(1));
        assert_eq!(number(step_cursor(&rows, None, false)), Some(3));
        assert_eq!(number(step_cursor(&rows, Some(&rows[0]), true)), Some(2));
        assert_eq!(number(step_cursor(&rows, Some(&rows[2]), true)), Some(3));
        assert_eq!(number(step_cursor(&rows, Some(&rows[0]), false)), Some(1));
        // A cursor whose pull request left the list starts over.
        let gone = PrRef::new(RepoRef::new("a", "repo"), 9);
        assert_eq!(number(step_cursor(&rows, Some(&gone), true)), Some(1));
        assert_eq!(step_cursor(&[], None, true), None);
    }

    #[test]
    fn the_title_is_the_open_saved_view_or_reviews() {
        let mut views = ViewsFile::default();
        let mine = views.create("My PRs", Vec::new(), Utc::now());
        assert_eq!(title(Some(&mine), &views), "My PRs");
        assert_eq!(title(None, &views), "Reviews", "All is Reviews");
        assert_eq!(
            title(Some("deleted"), &views),
            "Reviews",
            "a view that is gone"
        );
        views.rename(&mine, "Mine", Utc::now());
        assert_eq!(title(Some(&mine), &views), "Mine");
    }

    /// The title the shell gives the page's tab.
    fn tab_title(view: &Entity<InboxView>, cx: &mut VisualTestContext) -> String {
        let any: AnyView = view.clone().into();
        cx.update(|_, cx| crate::github::inbox_title(&any, cx))
            .expect("a Reviews page has a title")
            .to_string()
    }

    /// The tab's title, after checking the header draws the same one.
    fn titles(view: &Entity<InboxView>, cx: &mut VisualTestContext) -> String {
        let title = tab_title(view, cx);
        let selector: &'static str = format!("page-title:{title}").leak();
        assert!(
            bounds(cx, selector).is_some(),
            "the header says {title:?} too"
        );
        title
    }

    #[gpui_kit::test]
    fn the_tab_and_the_header_follow_the_open_view(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        assert_eq!(titles(&view, cx), "Reviews");
        view.update(cx, |view, cx| view.save_as_new("Title follows", cx));
        assert_eq!(titles(&view, cx), "Title follows");
        let id = view
            .read_with(cx, |view, _| view.page.view_id.clone())
            .expect("the new view is open");
        view.update(cx, |view, cx| view.rename_view(&id, "Renamed", cx));
        assert_eq!(titles(&view, cx), "Renamed", "a rename shows at once");
        view.update(cx, |view, cx| view.switch_view(None, cx));
        assert_eq!(titles(&view, cx), "Reviews");
        view.update(cx, |view, cx| view.switch_view(Some(&id), cx));
        assert_eq!(titles(&view, cx), "Renamed");
        view.update(cx, |view, cx| view.delete_view(&id, cx));
        assert_eq!(titles(&view, cx), "Reviews");
    }

    #[test]
    fn counts_follow_the_filters() {
        let full = page(vec![pr("a", 1), pr("b", 2)], 2, false);
        let all = shown(&full, &[]);
        assert_eq!((all.items.len(), all.count, all.note), (2, 2, None));
        let some = shown(&full, &org("b"));
        assert_eq!(some.count, 1);
        assert_eq!(some.items[0].pr.number, 2);
        assert_eq!(some.note, None);
    }

    #[test]
    fn a_partial_page_says_what_it_holds() {
        let partial = page(vec![pr("a", 1)], 40, true);
        assert_eq!(
            shown(&partial, &[]).note.as_deref(),
            Some("Showing 1 of 40")
        );
        assert_eq!(shown(&partial, &[]).count, 40);
        let filtered = shown(&partial, &org("a"));
        assert_eq!(filtered.count, 1);
        assert_eq!(
            filtered.note.as_deref(),
            Some("Filtered from the first 1 of 40")
        );
    }

    /// Where the element with `selector` was last painted.
    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Option<Bounds<Pixels>> {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.debug_bounds(selector)
    }

    /// The Reviews screen in a 1280 × 820 window, with no account.
    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<InboxView>) {
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

    /// Clicks the element with `selector` where it was last painted.
    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is on screen"));
        cx.simulate_click(at.center(), Modifiers::none());
    }

    #[gpui_kit::test]
    fn changing_a_filter_moves_neither_the_filter_button_nor_its_menu(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;

        // A saved view with one filter, and "Add Filter…" open on it.
        view.update_in(cx, |view, window, cx| {
            view.edit_draft(|draft| *draft = org("hooli-inc"), cx);
            view.save_as_new("Hooli", cx);
            view.set_filter_open(true, window, cx);
        });
        let button = bounds(cx, "reviews-filter-open").expect("the Filter button");
        let menu = bounds(cx, "reviews-filter-menu").expect("the open filter menu");
        assert_eq!(bounds(cx, "reviews-view-save"), None, "nothing to save yet");

        // Picking a value makes the view unsaved: Discard and Save appear.
        view.update(cx, |view, cx| {
            view.edit_draft(|draft| draft.extend(org("acme")), cx)
        });
        let save = bounds(cx, "reviews-view-save").expect("Save shows once the view changed");
        assert_eq!(bounds(cx, "reviews-filter-open"), Some(button));
        assert_eq!(bounds(cx, "reviews-filter-menu"), Some(menu));
        assert!(
            save.left() >= button.right(),
            "Save sits after Filter, not before it"
        );

        // With no view open, "Unsaved changes" alone appears: same place.
        view.update(cx, |view, cx| {
            view.switch_view(None, cx);
            view.edit_draft(|draft| draft.extend(org("acme")), cx);
        });
        let unsaved = bounds(cx, "reviews-filter-open").expect("the Filter button");
        view.update(cx, |view, cx| view.edit_draft(|draft| draft.clear(), cx));
        assert_eq!(bounds(cx, "reviews-filter-open"), Some(unsaved));
    }

    /// Where the element with `selector` was drawn, after two frames: the
    /// row of views follows its open tab on the frame after it was laid out.
    fn settled(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
        bounds(cx, "reviews-views");
        cx.run_until_parked();
        let selector: &'static str = selector.to_owned().leak();
        bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is on screen"))
    }

    fn in_view(row: Bounds<Pixels>, tab: Bounds<Pixels>) -> bool {
        row.left() <= tab.left() && tab.right() <= row.right()
    }

    fn wheel(cx: &mut VisualTestContext, at: Bounds<Pixels>, dy: f32) {
        let events: [PlatformInput; 2] = [
            MouseMoveEvent {
                position: at.center(),
                pressed_button: None,
                modifiers: Modifiers::none(),
            }
            .to_platform_input(),
            ScrollEvent {
                position: at.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Modifiers::none(),
                ..ScrollEvent::default()
            }
            .to_platform_input(),
        ];
        for event in events {
            cx.update(|window, cx| {
                window.dispatch_event(event, cx);
            });
        }
    }

    #[gpui_kit::test]
    fn a_row_of_views_too_long_for_the_header_scrolls_and_keeps_every_control_whole(
        cx: &mut TestAppContext,
    ) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;

        // While the views fit, the row is as wide as its tabs and does not
        // scroll.
        let row = settled(cx, "reviews-views");
        let all = settled(cx, "reviews-view-all");
        assert_eq!(all.left(), row.left(), "nothing is scrolled");
        let first_view = view.update(cx, |_, cx| super::views_store::snapshot(cx).views.len());
        if first_view > 0 {
            let last = settled(cx, &format!("reviews-view-tab-{}", first_view - 1));
            assert_eq!(last.right(), row.right(), "the row ends at its last tab");
        }

        // Eight more views, the last one open with an unsaved change, in a
        // narrow window.
        view.update(cx, |view, cx| {
            for n in 1..=8 {
                view.switch_view(None, cx);
                view.edit_draft(|draft| *draft = org(&format!("org-{n}")), cx);
                view.save_as_new(&format!("Review queue {n}"), cx);
            }
            view.edit_draft(|draft| draft.extend(org("acme")), cx);
        });
        cx.simulate_resize(size(px(900.), px(820.)));
        cx.run_until_parked();
        let last_ix = view.update(cx, |_, cx| super::views_store::snapshot(cx).views.len()) - 1;
        let last = format!("reviews-view-tab-{last_ix}");

        let row = settled(cx, "reviews-views");
        assert!(
            in_view(row, settled(cx, &last)),
            "the open view, the last one, is scrolled into view"
        );
        let actions = settled(cx, "inbox-add-repo");
        assert!(
            settled(cx, "inbox-refresh").right() <= px(900.),
            "Add repository and Refresh stay in the window"
        );
        let mut edge = row.right();
        for control in [
            "reviews-view-save-as",
            "reviews-view-menu",
            "reviews-filter-open",
            "reviews-sort-open",
            "reviews-view-discard",
            "reviews-view-save",
        ] {
            let at = settled(cx, control);
            assert!(
                at.left() >= edge && at.right() <= actions.left(),
                "{control} is whole, after the tabs and before Add repository: {at:?}"
            );
            edge = at.right();
        }

        // The wheel scrolls the row back toward the first views.
        let before = settled(cx, &last);
        wheel(cx, row, 200.);
        assert_eq!(
            settled(cx, &last).left(),
            before.left() + px(200.),
            "the wheel scrolls the row sideways"
        );

        // Opening "All" scrolls back to it, and the wheel reaches the last
        // view again.
        view.update(cx, |view, cx| view.switch_view(None, cx));
        let all = settled(cx, "reviews-view-all");
        // "All" has no unsaved change: the controls after the row are fewer
        // and the row is wider.
        let row = settled(cx, "reviews-views");
        assert!(
            in_view(row, all),
            "the open view, All, is scrolled into view"
        );
        wheel(cx, row, -5000.);
        assert!(
            in_view(row, settled(cx, &last)),
            "the wheel reaches the last view"
        );
    }

    /// A pull request in `owner/repo` with `lines` changed.
    fn sized(owner: &str, number: u64, lines: u32) -> PrSummary {
        PrSummary {
            additions: lines,
            ..pr(owner, number)
        }
    }

    /// Approved with passing checks: ready to merge.
    fn ready(pr: PrSummary) -> PrSummary {
        PrSummary {
            review_decision: Some(ReviewDecision::Approved),
            checks: Some(ChecksState::Passing),
            ..pr
        }
    }

    /// The open tab's groups, folded or not, as pull request numbers.
    fn numbers(view: &Entity<InboxView>, cx: &mut VisualTestContext) -> Vec<Vec<u64>> {
        view.read_with(cx, |view, _| {
            view.groups_shown()
                .iter()
                .map(|(_, _, rows)| rows.iter().map(|pr| pr.number).collect())
                .collect()
        })
    }

    fn active(view: &Entity<InboxView>, cx: &mut VisualTestContext) -> Option<ReviewTab> {
        view.read_with(cx, |view, _| view.current_tab())
    }

    /// To review holds two ready to merge (1, 6) and two waiting for a
    /// review (2, 3); Involved two waiting (4, 5); Mine none.
    fn inbox() -> Inbox {
        Inbox {
            review_requested: page(
                vec![
                    ready(sized("a", 1, 50)),
                    sized("a", 2, 5),
                    sized("a", 3, 500),
                    ready(sized("a", 6, 20)),
                ],
                4,
                false,
            ),
            authored: page(Vec::new(), 0, false),
            involved: page(vec![sized("b", 4, 900), sized("b", 5, 1)], 2, false),
        }
    }

    fn load(view: &Entity<InboxView>, inbox: Inbox, cx: &mut VisualTestContext) {
        view.update(cx, |view, cx| {
            view.inbox = Loadable::Loaded(inbox);
            cx.notify();
        });
        bounds(cx, "reviews-tab-All");
    }

    #[gpui_kit::test]
    fn a_column_heading_sorts_each_group_on_its_own_and_a_view_keeps_it(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        load(&view, inbox(), cx);
        let github = vec![vec![1, 6], vec![2, 3]];
        assert_eq!(numbers(&view, cx), github, "ready to merge, then waiting");

        // Ascending, descending, then GitHub's order again; a row never
        // leaves its group.
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), [vec![6, 1], vec![2, 3]]);
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), [vec![1, 6], vec![3, 2]]);
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), github);

        // The keyboard cursor walks the rows in the order on screen.
        click(cx, "reviews-sort-changes");
        let walk: Vec<u64> = (0..5)
            .map(|_| {
                view.update_in(cx, |view, window, cx| {
                    view.move_cursor(true, window, cx);
                    view.cursor.as_ref().map(|pr| pr.number)
                })
                .expect("a row under the cursor")
            })
            .collect();
        assert_eq!(walk, [6, 1, 2, 3, 3]);

        // The sort holds in every tab.
        click(cx, "reviews-tab-Involved");
        assert_eq!(numbers(&view, cx), [vec![5, 4]]);
        click(cx, "reviews-tab-To review");

        // A sort is a change to the view: saved with it, and back with it.
        let smallest = Some(Sort {
            key: SortKey::Changes,
            dir: SortDir::Ascending,
        });
        view.update(cx, |view, cx| {
            assert!(view.page.is_dirty(&super::views_store::snapshot(cx)));
            view.save_as_new("Small first", cx);
            assert!(!view.page.is_dirty(&super::views_store::snapshot(cx)));
        });
        let id = view.read_with(cx, |view, _| view.page.view_id.clone());
        view.update(cx, |view, cx| view.switch_view(None, cx));
        assert_eq!(numbers(&view, cx), github, "All has its own order");
        view.update(cx, |view, cx| view.switch_view(id.as_deref(), cx));
        assert_eq!(view.read_with(cx, |view, _| view.page.sort), smallest);
        assert_eq!(numbers(&view, cx), [vec![6, 1], vec![2, 3]]);
    }

    #[gpui_kit::test]
    fn the_page_opens_on_the_first_tab_with_rows_unless_the_view_keeps_another(
        cx: &mut TestAppContext,
    ) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        // Nothing to review and nothing of yours: Involved comes first.
        let only_involved = Inbox {
            review_requested: page(Vec::new(), 0, false),
            ..inbox()
        };
        load(&view, only_involved, cx);
        assert_eq!(active(&view, cx), Some(ReviewTab::Involved));
        assert_eq!(numbers(&view, cx), [vec![4, 5]]);

        // A refresh that empties it does not switch tabs under the user.
        load(
            &view,
            Inbox {
                involved: page(Vec::new(), 0, false),
                ..inbox()
            },
            cx,
        );
        assert_eq!(active(&view, cx), Some(ReviewTab::Involved));
        assert!(numbers(&view, cx).is_empty());

        // A view keeps the tab and the folded groups it was left on.
        load(&view, inbox(), cx);
        view.update(cx, |view, cx| view.save_as_new("Mine first", cx));
        click(cx, "reviews-tab-Mine");
        let id = view.read_with(cx, |view, _| view.page.view_id.clone());
        view.update(cx, |view, cx| view.switch_view(None, cx));
        assert_eq!(
            active(&view, cx),
            Some(ReviewTab::ToReview),
            "no view: the first tab with rows"
        );
        view.update(cx, |view, cx| view.switch_view(id.as_deref(), cx));
        assert_eq!(
            active(&view, cx),
            Some(ReviewTab::Mine),
            "saved, though empty"
        );
        click(cx, "reviews-tab-All");
        click(cx, "reviews-group-Ready to merge");
        let saved = view.update(cx, |_, cx| {
            let id = id.as_deref().expect("a saved view");
            super::views_store::snapshot(cx)
                .get(id)
                .cloned()
                .expect("still saved")
        });
        assert_eq!(saved.tab, Some(ReviewTab::All));
        assert_eq!(saved.collapsed, [NextStep::ReadyToMerge]);
        let dirty = view.update(cx, |view, cx| {
            view.page.is_dirty(&super::views_store::snapshot(cx))
        });
        assert!(!dirty, "a tab or a fold is no unsaved change");
    }

    #[gpui_kit::test]
    fn a_stored_copy_does_not_choose_the_tab_the_fresh_results_do(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        // The stored copy had something to review; since then it was
        // reviewed, and two of yours opened.
        let stored = Inbox {
            involved: page(Vec::new(), 0, false),
            ..inbox()
        };
        view.update(cx, |view, cx| {
            view.inbox = Loadable::Refreshing(stored);
            cx.notify();
        });
        bounds(cx, "reviews-tab-All");
        assert_eq!(active(&view, cx), Some(ReviewTab::ToReview));
        let fresh = Inbox {
            review_requested: page(Vec::new(), 0, false),
            authored: page(vec![pr("c", 11), pr("c", 12)], 2, false),
            involved: page(Vec::new(), 0, false),
        };
        load(&view, fresh, cx);
        assert_eq!(active(&view, cx), Some(ReviewTab::Mine));
    }

    #[gpui_kit::test]
    fn views_keep_the_tab_only_when_it_was_picked_and_a_second_click_keeps_it(
        cx: &mut TestAppContext,
    ) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        load(&view, inbox(), cx);
        let saved = |view: &Entity<InboxView>, id: &Option<String>, cx: &mut VisualTestContext| {
            let id = id.clone().expect("a saved view");
            view.update(cx, |_, cx| {
                super::views_store::snapshot(cx)
                    .get(&id)
                    .cloned()
                    .expect("saved")
            })
        };
        // Folding under a view that keeps no tab does not save the tab the
        // page opened on.
        view.update(cx, |view, cx| view.save_as_new("Plain", cx));
        let plain = view.read_with(cx, |view, _| view.page.view_id.clone());
        click(cx, "reviews-group-Ready to merge");
        let kept = saved(&view, &plain, cx);
        assert_eq!(
            (kept.tab, kept.collapsed),
            (None, vec![NextStep::ReadyToMerge])
        );
        click(cx, "reviews-group-Ready to merge");

        // With no view, a second click on "All" keeps the tab and the folds.
        view.update(cx, |view, cx| view.switch_view(None, cx));
        click(cx, "reviews-tab-Involved");
        click(cx, "reviews-group-Waiting for review");
        click(cx, "reviews-view-all");
        assert_eq!(active(&view, cx), Some(ReviewTab::Involved));
        let folded = view.read_with(cx, |view, _| view.page.collapsed.clone());
        assert_eq!(folded, [NextStep::WaitingForReview]);
        click(cx, "reviews-group-Waiting for review");

        // A view with the same filters that opens on another tab drops the
        // selection, though that tab lists the picked pull request too.
        click(cx, "reviews-tab-All");
        view.update(cx, |view, cx| view.save_as_new("Everything", cx));
        let everything = view.read_with(cx, |view, _| view.page.view_id.clone());
        view.update(cx, |view, cx| view.switch_view(None, cx));
        assert_eq!(active(&view, cx), Some(ReviewTab::ToReview));
        click(cx, "pr-select-PR_2");
        assert_eq!(picked(&view, cx), [2]);
        view.update(cx, |view, cx| view.switch_view(everything.as_deref(), cx));
        assert_eq!(active(&view, cx), Some(ReviewTab::All));
        assert!(
            picked(&view, cx).is_empty(),
            "the tab changed with the view"
        );
    }

    #[gpui_kit::test]
    fn tabs_count_what_the_filters_leave_and_all_lists_each_once(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        load(&view, inbox(), cx);
        let counts = |view: &Entity<InboxView>, cx: &mut VisualTestContext| -> Vec<usize> {
            view.read_with(cx, |view, _| {
                let inbox = view.inbox.value().expect("loaded");
                view.tab_lists(inbox)
                    .iter()
                    .map(|(_, shown)| shown.count)
                    .collect()
            })
        };
        assert_eq!(counts(&view, cx), [4, 0, 2, 6]);
        view.update(cx, |view, cx| {
            view.edit_draft(|draft| *draft = org("b"), cx)
        });
        assert_eq!(counts(&view, cx), [0, 0, 2, 2]);
        assert_eq!(
            active(&view, cx),
            Some(ReviewTab::ToReview),
            "a filter keeps the tab"
        );
        assert!(numbers(&view, cx).is_empty());
        assert!(
            bounds(cx, "reviews-tab-Involved").is_some(),
            "every tab stays"
        );
    }

    /// The picked pull requests, in the order the bar acts on them.
    fn picked(view: &Entity<InboxView>, cx: &mut VisualTestContext) -> Vec<u64> {
        view.read_with(cx, |view, _| {
            view.picked()
                .iter()
                .map(|summary| summary.pr.number)
                .collect()
        })
    }

    #[gpui_kit::test]
    fn checkboxes_pick_rows_of_the_open_tab_and_a_filter_or_a_tab_clears_them(
        cx: &mut TestAppContext,
    ) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        cx.update(|_, cx| crate::shell::menus::bind_keys(cx));
        load(&view, inbox(), cx);
        assert_eq!(bounds(cx, "reviews-selection-bar"), None, "no bar yet");

        // Sorted smallest first, a Shift+click picks the rows between on
        // screen, across groups (1, 2), not between in GitHub's order.
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), [vec![6, 1], vec![2, 3]]);
        click(cx, "pr-select-PR_1");
        let row = bounds(cx, "pr-row-PR_2").expect("row 2");
        cx.simulate_click(row.center(), Modifiers::shift());
        assert_eq!(picked(&view, cx), [1, 2]);
        assert!(
            bounds(cx, "reviews-selection-bar").is_some(),
            "the bar shows the selection"
        );

        // Cmd+A picks the whole tab, every group.
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(picked(&view, cx), [6, 1, 2, 3]);

        // A new sort keeps the selection and reorders what the bar opens.
        click(cx, "reviews-sort-changes");
        assert_eq!(picked(&view, cx), [1, 6, 3, 2]);

        // The group heading's box drops a group that is all picked.
        click(cx, "reviews-select-group-Ready to merge");
        assert_eq!(picked(&view, cx), [3, 2]);
        click(cx, "reviews-select-group-Ready to merge");
        assert_eq!(picked(&view, cx), [1, 6, 3, 2]);

        // A folded group's rows leave the screen and the selection; Cmd+A
        // skips them.
        click(cx, "reviews-group-Ready to merge");
        assert_eq!(picked(&view, cx), [3, 2]);
        assert!(bounds(cx, "pr-row-PR_1").is_none(), "folded");
        click(cx, "reviews-group-Ready to merge");
        assert!(bounds(cx, "pr-row-PR_1").is_some(), "unfolded");
        assert_eq!(picked(&view, cx), [3, 2], "unfolding picks nothing back");
        click(cx, "reviews-group-Ready to merge");
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(picked(&view, cx), [3, 2]);
        click(cx, "reviews-group-Ready to merge");
        assert_eq!(
            picked(&view, cx),
            [3, 2],
            "Cmd+A left the folded rows alone"
        );

        // Escape drops it all.
        cx.simulate_keystrokes("escape");
        assert!(picked(&view, cx).is_empty());
        assert_eq!(bounds(cx, "reviews-selection-bar"), None);

        // The selection belongs to its tab: another tab starts empty, even
        // All, which lists the same pull request, and Cmd+A there picks
        // that tab's rows.
        click(cx, "pr-select-PR_3");
        click(cx, "reviews-tab-All");
        assert!(picked(&view, cx).is_empty(), "another tab clears it");
        click(cx, "reviews-tab-Involved");
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(picked(&view, cx), [4, 5]);

        // Changing the filters clears the selection; a sort does not.
        view.update(cx, |view, cx| {
            view.edit_draft(|draft| *draft = org("b"), cx)
        });
        assert!(picked(&view, cx).is_empty(), "a filter clears it");
        click(cx, "pr-select-PR_4");
        view.update(cx, |view, cx| {
            view.save_as_new("B", cx);
            view.switch_view(None, cx);
        });
        assert!(picked(&view, cx).is_empty(), "another view clears it");
    }

    #[gpui_kit::test]
    fn the_bar_offers_a_t3_review_of_the_picked_pull_requests(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        view.update(cx, |view, cx| {
            view.inbox = Loadable::Loaded(Inbox {
                review_requested: page(vec![pr("a", 1), pr("a", 2)], 2, false),
                authored: page(Vec::new(), 0, false),
                involved: page(Vec::new(), 0, false),
            });
            cx.notify();
        });
        assert_eq!(bounds(cx, "reviews-selection-t3"), None);
        click(cx, "pr-select-PR_2");
        click(cx, "pr-select-PR_1");
        assert!(
            bounds(cx, "reviews-selection-t3").is_some(),
            "the bar offers Review with T3 Code"
        );
        // Without T3 it is disabled and says why; the selection stays.
        click(cx, "reviews-selection-t3");
        assert_eq!(picked(&view, cx), [1, 2]);
    }

    #[gpui_kit::test]
    fn the_bar_leaves_the_rows_under_the_pointer(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        view.update(cx, |view, cx| {
            view.inbox = Loadable::Loaded(Inbox {
                review_requested: page((1..=40).map(|n| pr("a", n)).collect(), 40, false),
                authored: page(Vec::new(), 0, false),
                involved: page(Vec::new(), 0, false),
            });
            cx.notify();
        });
        let row = bounds(cx, "pr-row-PR_2").expect("row 2");
        click(cx, "pr-select-PR_1");
        assert!(bounds(cx, "reviews-selection-bar").is_some());
        assert_eq!(bounds(cx, "pr-row-PR_2"), Some(row), "the bar moved no row");
        click(cx, "pr-select-PR_2");
        assert_eq!(picked(&view, cx), [1, 2], "the second click hit row 2");
        view.update(cx, |view, cx| view.clear_selection(cx));
        assert_eq!(bounds(cx, "reviews-selection-bar"), None);
        assert_eq!(bounds(cx, "pr-row-PR_2"), Some(row), "nor did its leaving");
    }

    #[gpui_kit::test]
    fn opening_more_than_ten_asks_first(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        cx.update(|_, cx| crate::shell::menus::bind_keys(cx));
        let many = |count: u64| page((1..=count).map(|n| pr("a", n)).collect(), 0, false);
        let load = |view: &Entity<InboxView>, count: u64, cx: &mut VisualTestContext| {
            view.update(cx, |view, cx| {
                view.inbox = Loadable::Loaded(Inbox {
                    review_requested: many(count),
                    authored: page(Vec::new(), 0, false),
                    involved: page(Vec::new(), 0, false),
                });
                view.selection = super::Selection::default();
                cx.notify();
            });
            cx.simulate_keystrokes("cmd-a");
            assert_eq!(picked(view, cx).len(), count as usize);
        };
        let confirm = |view: &Entity<InboxView>, cx: &mut VisualTestContext| {
            view.read_with(cx, |view, _| view.confirm.clone())
        };

        // Ten open at once.
        load(&view, 10, cx);
        click(cx, "reviews-selection-tabs");
        assert_eq!(confirm(&view, cx), None);
        click(cx, "reviews-selection-github");
        assert_eq!(confirm(&view, cx), None);

        // Eleven ask in the bar, for tabs and for the browser alike.
        load(&view, 11, cx);
        click(cx, "reviews-selection-tabs");
        assert_eq!(confirm(&view, cx), Some(super::Bulk::Tabs));
        assert!(bounds(cx, "reviews-selection-tabs").is_none());
        click(cx, "reviews-selection-confirm");
        assert_eq!(confirm(&view, cx), None, "confirmed and opened");
        click(cx, "reviews-selection-github");
        assert_eq!(confirm(&view, cx), Some(super::Bulk::Browser));
        let group = super::Bulk::Group(super::GroupPick::New);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_picked(group.clone(), false, window, cx)
            })
        });
        assert_eq!(confirm(&view, cx), Some(group), "a group asks as well");
    }

    #[gpui_kit::test]
    fn open_as_group_needs_a_saved_view_and_every_row_on_screen_picked(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        cx.update(|_, cx| crate::shell::menus::bind_keys(cx));
        load(&view, inbox(), cx);
        let offered = |view: &Entity<InboxView>, cx: &mut VisualTestContext| {
            view.update(cx, |view, cx| view.view_group_name(cx))
        };
        cx.simulate_keystrokes("cmd-a");
        assert!(!picked(&view, cx).is_empty());
        assert_eq!(offered(&view, cx), None, "no saved view is open");

        view.update(cx, |view, cx| view.save_as_new("Ready to ship", cx));
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(offered(&view, cx).as_deref(), Some("Ready to ship"));

        view.update(cx, |view, cx| {
            let first = view.rows_shown()[0].clone();
            view.selection.toggle(&first);
            view.selection_changed(cx);
        });
        assert_eq!(offered(&view, cx), None, "one row is left out");
    }

    #[test]
    fn mark_as_read_takes_only_unread_threads_of_the_picked_pull_requests() {
        use loupe_github::{Notification, NotificationSubject};
        let pr_ref = |number| PrRef::new(RepoRef::new("a", "repo"), number);
        let thread = |id: &str, subject, unread| Notification {
            id: id.into(),
            repo: RepoRef::new("a", "repo"),
            title: String::new(),
            subject,
            reason: "mention".into(),
            unread,
            updated_at: Utc::now(),
        };
        let notifications = vec![
            thread("1", NotificationSubject::PullRequest(pr_ref(1)), true),
            thread("2", NotificationSubject::PullRequest(pr_ref(1)), false),
            thread("3", NotificationSubject::PullRequest(pr_ref(2)), true),
            thread(
                "4",
                NotificationSubject::Issue {
                    repo: RepoRef::new("a", "repo"),
                    number: 1,
                },
                true,
            ),
        ];
        assert_eq!(super::threads_of(&notifications, &[pr_ref(1)]), ["1"]);
        assert_eq!(
            super::threads_of(&notifications, &[pr_ref(1), pr_ref(2)]),
            ["1", "3"]
        );
    }

    #[test]
    fn all_lists_every_tab_then_the_rest_of_all_open_each_once() {
        let mut draft = pr("a", 3);
        draft.is_draft = true;
        let open = page(vec![pr("a", 1), pr("a", 2), draft, pr("b", 4)], 4, false);
        let mine = page(vec![pr("a", 2), pr("c", 9)], 2, false);
        let mine = shown(&mine, &[]);
        let numbers = |shown: &super::Shown| -> Vec<u64> {
            shown
                .items
                .iter()
                .map(|summary| summary.pr.number)
                .collect()
        };

        let all = shown_all(&[&mine], Some(&open), &[]);
        assert_eq!(numbers(&all), [2, 9, 1, 3, 4]);
        assert_eq!((all.count, all.note), (5, None));
        assert_eq!(numbers(&shown_all(&[&mine], None, &[])), [2, 9]);

        // The view's clauses apply as in every other tab.
        let in_a = shown_all(&[], Some(&open), &org("a"));
        assert_eq!(numbers(&in_a), [1, 2, 3]);
        let no_drafts = vec![Clause {
            field: Field::Status,
            op: Op::NotIn,
            values: vec!["draft".into()],
        }];
        assert_eq!(numbers(&shown_all(&[], Some(&open), &no_drafts)), [1, 2, 4]);

        let partial = page(vec![pr("a", 1), pr("a", 2)], 700, true);
        assert_eq!(
            shown_all(&[&mine], Some(&partial), &[]).note.as_deref(),
            Some("Showing 2 of 700")
        );
        let first_page = page(vec![pr("a", 1)], 120, true);
        let to_review = shown(&first_page, &[]);
        assert_eq!(
            shown_all(&[&to_review, &mine], None, &[]).note.as_deref(),
            Some("Showing the 3 loaded; GitHub has more")
        );
    }

    #[test]
    fn all_open_follows_the_view_and_its_repositories() {
        let repo = vec![Clause {
            field: Field::Repo,
            op: Op::In,
            values: vec!["hooli-inc/hooli-api".into()],
        }];
        let mut views = ViewsFile::default();
        let id = views.create("Back end Hooli", repo.clone(), Utc::now());
        let mut page = PageState {
            draft: repo,
            ..PageState::default()
        };
        assert_eq!(page.all_open_query(), None, "only a saved view has it");
        page.switch(&views, Some(&id));
        assert_eq!(
            page.all_open_query().as_deref(),
            Some(
                "is:pr is:open archived:false repo:hooli-inc/hooli-api \
                 sort:updated-desc"
            )
        );
        page.draft.clear();
        assert_eq!(page.all_open_query(), None, "no repository, no search");
        views.set_all_open(&id, false, Utc::now());
        page.switch(&views, Some(&id));
        assert_eq!(page.all_open_query(), None, "switched off and saved");
        assert!(!page.is_dirty(&views), "the switch is not a filter edit");
    }

    #[test]
    fn the_draft_is_unsaved_until_it_matches_its_view() {
        let mut views = ViewsFile::default();
        let id = views.create("Hooli", org("hooli-inc"), Utc::now());
        let mut page = PageState::default();
        assert!(!page.is_dirty(&views));
        page.draft = org("x");
        assert!(page.is_dirty(&views), "clauses with no view are unsaved");
        page.switch(&views, Some(&id));
        assert_eq!(page.draft, org("hooli-inc"));
        assert!(!page.is_dirty(&views));
        page.draft.clear();
        assert!(page.is_dirty(&views));
        page.switch(&views, None);
        assert_eq!(page, PageState::default());

        // The sort order is part of the draft, like the clauses.
        page.switch(&views, Some(&id));
        page.sort = Some(Sort {
            key: SortKey::Checks,
            dir: SortDir::Ascending,
        });
        assert!(page.is_dirty(&views), "a new sort is unsaved");
        views.set_sort(&id, page.sort, Utc::now());
        assert!(!page.is_dirty(&views));
        page.switch(&views, None);
        assert_eq!(page.sort, None);
        page.switch(&views, Some(&id));
        assert_eq!(page.sort.map(|sort| sort.key), Some(SortKey::Checks));
    }
}
