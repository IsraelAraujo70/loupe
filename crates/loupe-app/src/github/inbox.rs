//! `Route::Inbox` (Reviews): the three dashboard sections, and on a saved
//! view that names repositories every other open pull request in them
//! ("All open"), narrowed by filter clauses (the Linear board's "Add
//! Filter…" pattern) that can be saved as views.

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
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::reviews::{self, Clause, Op, Sort, SortDir, SortKey, ViewsFile};
use loupe_github::{
    Inbox, Notification as GitHubNotification, NotificationSubject, Page, PrRef, PrSummary,
};

use self::filter_menu::{FilterMenu, FilterOptions};
use self::selection::{CONFIRM_ABOVE, Selection};
use super::common::{self, Loadable};
use super::pr_row::{AgeFrom, OnSelect, OnSort, RowSelect, SelectGesture, pr_row, select_box};
use crate::route::Route;
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::theme::scale;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

gpui_kit::actions!(
    github,
    [
        /// F on the Reviews screen: open "Add Filter…".
        OpenReviewsFilter,
        /// Cmd+A on the Reviews screen: pick every row of the section with
        /// the keyboard cursor or the last click.
        SelectSectionRows,
        /// Escape on the Reviews screen: drop the selection.
        ClearReviewsSelection
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    ReviewRequested,
    Authored,
    Involved,
    /// Every open pull request in the view's repositories that the sections
    /// above do not list.
    AllOpen,
}

impl Section {
    /// The sections the one inbox request fills.
    const INBOX: [Section; 3] = [
        Section::ReviewRequested,
        Section::Authored,
        Section::Involved,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::ReviewRequested => "Waiting on your review",
            Section::Authored => "Your pull requests",
            Section::Involved => "Involved",
            Section::AllOpen => "All open",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::ReviewRequested => IconName::Eye,
            Section::Authored => IconName::GitPullRequest,
            Section::Involved => IconName::AtSign,
            Section::AllOpen => IconName::Users,
        }
    }

    fn empty(self) -> (IconName, &'static str) {
        match self {
            Section::ReviewRequested => (IconName::CircleCheck, "Nothing waiting on you"),
            Section::Authored => (IconName::GitPullRequest, "You have no open pull requests"),
            Section::Involved => (IconName::AtSign, "No other open pull requests involve you"),
            Section::AllOpen => (
                IconName::CircleCheck,
                "No other open pull requests in these repositories",
            ),
        }
    }

    /// The section's page in the inbox; `None` for "All open", which has a
    /// search of its own.
    fn page(self, inbox: &Inbox) -> Option<&Page<PrSummary>> {
        match self {
            Section::ReviewRequested => Some(&inbox.review_requested),
            Section::Authored => Some(&inbox.authored),
            Section::Involved => Some(&inbox.involved),
            Section::AllOpen => None,
        }
    }

    /// Whether its rows show the author: your own pull requests need none.
    fn shows_author(self) -> bool {
        self != Section::Authored
    }
}

/// What one section shows under the clauses: its rows, the number on its
/// heading, and a note when the page holds only part of the search.
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

/// "All open" under the clauses: the open pull requests that pass them and
/// that no section above lists (`above`). The heading counts the rows it
/// shows and says how many more are already above, so the two add up to
/// what GitHub lists as open.
fn shown_all_open<'a>(
    page: &'a Page<PrSummary>,
    clauses: &[Clause],
    above: &[&PrRef],
) -> Shown<'a> {
    let all = shown(page, clauses);
    let (items, listed): (Vec<&PrSummary>, Vec<&PrSummary>) = all
        .items
        .into_iter()
        .partition(|pr| !above.contains(&&pr.pr));
    let listed = (!listed.is_empty()).then(|| format!("{} more listed above", listed.len()));
    let note = match (all.note, listed) {
        (Some(partial), Some(listed)) => Some(format!("{partial} · {listed}")),
        (partial, listed) => partial.or(listed),
    };
    Shown {
        count: items.len(),
        note,
        items,
    }
}

/// The saved view on screen and the clauses and sort order edited on top of
/// it. The draft is "unsaved" while either differs from the saved view; with
/// no view, while it has any clause or a sort.
#[derive(Clone, Debug, Default, PartialEq)]
struct PageState {
    view_id: Option<String>,
    draft: Vec<Clause>,
    /// The order of each section's rows; `None` is GitHub's order.
    sort: Option<Sort>,
    /// The view's saved "All open" choice; `None` means the default (on).
    all_open: Option<bool>,
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
    /// What the last "Mark as read" did, or why it failed (`true`).
    mark_note: Option<(SharedString, bool)>,
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
            mark_note: None,
            _load: None,
            _checks: checks,
        };
        view.reload(cx);
        view
    })
}

/// Where the selection bar opens the picked pull requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bulk {
    /// One background tab each, in Loupe.
    Tabs,
    /// One browser tab each, on GitHub.
    Browser,
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

    /// Each section with rows to show, top to bottom, under the clauses.
    fn sections<'a>(&'a self, inbox: &'a Inbox) -> Vec<(Section, Shown<'a>)> {
        let mut sections: Vec<(Section, Shown<'a>)> = Section::INBOX
            .iter()
            .filter_map(|&section| Some((section, shown(section.page(inbox)?, &self.page.draft))))
            .collect();
        if let Some(page) = self.all_open_state().and_then(Loadable::value) {
            let above: Vec<&PrRef> = sections
                .iter()
                .flat_map(|(_, shown)| shown.items.iter().map(|summary| &summary.pr))
                .collect();
            let all_open = shown_all_open(page, &self.page.draft, &above);
            sections.push((Section::AllOpen, all_open));
        }
        // Each section sorts its own rows: a sort never moves a pull
        // request into another section.
        for (_, shown) in &mut sections {
            reviews::sort_rows(&mut shown.items, self.page.sort);
        }
        sections
    }

    /// Sorts every section by `sort` (`None`: GitHub's order). Like a
    /// filter, it is part of the draft until the view is saved.
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
                Section::INBOX
                    .iter()
                    .filter_map(|section| section.page(inbox))
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

    fn switch_view(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        let views = views_store::snapshot(cx);
        let before = self.page.draft.clone();
        self.page.switch(&views, id);
        if self.page.draft != before {
            self.clear_selection(cx);
        }
        self.load_all_open(false, cx);
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
        match views_store::update(cx, |views| {
            let id = views.create(name, draft, Utc::now());
            views.set_sort(&id, sort, Utc::now());
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

    /// The pull requests each section shows, in order.
    fn sections_shown(&self) -> Vec<Vec<PrRef>> {
        let Some(inbox) = self.inbox.value() else {
            return Vec::new();
        };
        self.sections(inbox)
            .into_iter()
            .map(|(_, shown)| {
                shown
                    .items
                    .iter()
                    .map(|summary| summary.pr.clone())
                    .collect()
            })
            .collect()
    }

    /// The picked pull requests on screen, top to bottom, each once.
    fn picked(&self) -> Vec<PrSummary> {
        let Some(inbox) = self.inbox.value() else {
            return Vec::new();
        };
        let sections = self.sections(inbox);
        let rows: Vec<PrRef> = sections
            .iter()
            .flat_map(|(_, shown)| shown.items.iter().map(|summary| summary.pr.clone()))
            .collect();
        let summaries: Vec<&PrSummary> = sections
            .iter()
            .flat_map(|(_, shown)| shown.items.iter().copied())
            .collect();
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
                let rows: Vec<PrRef> = self.sections_shown().into_iter().flatten().collect();
                self.selection.extend_to(&rows, pr);
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

    /// Picks every row of `rows`, one section's, or drops them all when
    /// every one is already picked.
    fn toggle_section(&mut self, rows: &[PrRef], cx: &mut Context<Self>) {
        if !rows.is_empty() && rows.iter().all(|pr| self.selection.contains(pr)) {
            self.selection.deselect_all(rows);
        } else {
            self.selection.select_all(rows);
        }
        self.selection_changed(cx);
    }

    /// Cmd+A: picks every row of the section with the keyboard cursor, or
    /// else the last row clicked, or else the first section with rows.
    fn select_section(&mut self, cx: &mut Context<Self>) {
        let sections = self.sections_shown();
        let holds =
            |pr: Option<&PrRef>| pr.and_then(|pr| sections.iter().find(|rows| rows.contains(pr)));
        let rows = holds(self.cursor.as_ref())
            .or_else(|| holds(self.selection.anchor()))
            .or_else(|| sections.iter().find(|rows| !rows.is_empty()));
        if let Some(rows) = rows {
            self.selection.select_all(rows);
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
        for summary in picked {
            match bulk {
                Bulk::Tabs => {
                    crate::shell::open_in_new_tab(Route::PullRequest(summary.pr), window, cx)
                }
                Bulk::Browser => cx.open_url(&summary.url),
            }
        }
        cx.notify();
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
        let sections = self.sections_shown();
        let rows: Vec<PrRef> = sections.iter().flatten().cloned().collect();
        let Some(next) = step_cursor(&rows, self.cursor.as_ref(), down) else {
            return;
        };
        // The row's place in the scrolled content, from the fixed heights
        // of what is above it: 8 px of padding, then per section a 32 px
        // heading, a 24 px column heading when it has rows, its rows, and a
        // 12 px gap.
        let rem = window.rem_size();
        let row = scale::row_height().to_pixels(rem);
        let (heading, columns, gap, pad) = (
            rems(2.).to_pixels(rem),
            rems(1.5).to_pixels(rem),
            rems(0.75).to_pixels(rem),
            rems(0.5).to_pixels(rem),
        );
        let mut top = pad;
        for section in &sections {
            top += heading;
            if !section.is_empty() {
                top += columns;
            }
            if let Some(index) = section.iter().position(|pr| *pr == next) {
                top += row * index as f32;
                break;
            }
            top += row * section.len() as f32 + gap;
        }
        let viewport = self.scroll.bounds().size.height;
        let mut offset = self.scroll.offset();
        if top < -offset.y + columns {
            // Room for the column heading above the first row of a section.
            offset.y = -(top - heading - columns).max(px(0.));
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
            .filter(|pr| self.sections_shown().iter().flatten().any(|row| row == *pr));
        match on_screen {
            Some(pr) => crate::shell::navigate(Route::PullRequest(pr.clone()), window, cx),
            None => cx.propagate(),
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.inbox.is_busy();
        let header = ui::page_header("Reviews").icon(IconName::GitPullRequest);
        let waiting = self.inbox.value().map_or(0, |inbox| {
            shown(&inbox.review_requested, &self.page.draft).count
        });
        let header = if waiting > 0 {
            header.count(waiting)
        } else {
            header
        };
        header
            .after_title(self.tabs(cx))
            .action(
                ui::icon_button("inbox-add-repo", IconName::Plus, "Add repository…")
                    .on_click(|_, window, cx| super::repo::open_add_repository(window, cx)),
            )
            .action(
                ui::icon_button(
                    "inbox-refresh",
                    IconName::RefreshCw,
                    if busy { "Refreshing…" } else { "Refresh" },
                )
                .loading(busy)
                .disabled(busy)
                .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
    }

    /// "All" and the saved views as tabs, save as new, the active view's
    /// menu, Filter, and the unsaved-changes controls. Filter comes before
    /// the controls that appear with a change, so neither it nor its open
    /// menu moves when a filter is picked.
    fn tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let views = views_store::snapshot(cx);
        let dirty = self.page.is_dirty(&views);
        let active = self.page.view_id.clone();
        let view = cx.weak_entity();
        let muted = cx.theme().muted_foreground;
        let all = ui::view_tab("reviews-view-all", "All", active.is_none(), cx)
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
                    .accessibility_label(format!("View {}", saved.name))
                    .on_click(cx.listener(move |this, _, _, cx| this.switch_view(Some(&id), cx)))
                    .into_any_element()
            })
            .collect();
        let save_as = {
            let view = view.clone();
            ui::icon_button("reviews-view-save-as", IconName::Plus, "Save as new view…")
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
                .menu_trigger()
                .dropdown_menu(move |menu, _, _| {
                    // Without a repository or an organization the section
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
        // In the header's own row, after the title: one view needs no row
        // of its own.
        h_flex()
            .min_w_0()
            .ml_2()
            .gap_1()
            .items_center()
            .overflow_hidden()
            .child(all)
            .children(tabs)
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
            })
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
            None => "Sort the pull requests in each section".to_owned(),
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
        let muted = cx.theme().muted_foreground;
        let action = |id: &'static str, icon: IconName, label: &'static str| {
            Button::new(id)
                .debug_selector(move || id.into())
                .outline()
                .small()
                .icon(icon)
                .label(label)
        };
        let bar = ui::toolbar()
            .debug_selector(|| "reviews-selection-bar".into())
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .mr_1()
                    .child(format!("{count} selected")),
            );
        let bar = match self.confirm {
            Some(bulk) => {
                let question = match bulk {
                    Bulk::Tabs => format!("Open {count} pull requests in new tabs?"),
                    Bulk::Browser => format!("Open {count} pull requests on GitHub?"),
                };
                bar.child(div().text_sm().text_color(muted).child(question))
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
                                this.open_picked(bulk, true, window, cx)
                            })),
                    )
            }
            None => bar
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
                ),
        };
        let danger = cx.theme().danger;
        Some(
            bar.child(div().flex_1())
                .children(self.mark_note.clone().map(|(text, failed)| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::meta())
                        .text_color(if failed { danger } else { muted })
                        .child(text)
                }))
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

    fn section(
        &self,
        section: Section,
        shown: Shown<'_>,
        on_sort: OnSort,
        on_select: OnSelect,
        showing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows: Vec<PrRef> = shown
            .items
            .iter()
            .map(|summary| summary.pr.clone())
            .collect();
        let age_from = AgeFrom::for_sort(self.page.sort);
        let (empty_icon, empty_text) = if self.page.draft.is_empty() {
            section.empty()
        } else {
            (IconName::ListFilter, "No pull requests match the filters")
        };
        // An empty section is one line: its heading says what is missing.
        let empty = shown
            .items
            .is_empty()
            .then(|| ui::empty_line(empty_icon, empty_text, cx).ml_2());
        let list = v_flex().children(shown.items.iter().map(|summary| {
            pr_row(
                summary,
                true,
                section.shows_author(),
                self.cursor.as_ref() == Some(&summary.pr),
                Some(RowSelect {
                    checked: self.selection.contains(&summary.pr),
                    showing,
                    on_select: on_select.clone(),
                }),
                age_from,
                cx,
            )
        }));
        // The heading's box picks or drops the whole section.
        let all_picked = !rows.is_empty() && rows.iter().all(|pr| self.selection.contains(pr));
        let lead = (!rows.is_empty()).then(|| {
            let view = cx.weak_entity();
            select_box(
                SharedString::from(format!("reviews-select-section-{}", section.title())),
                SECTION_GROUP,
                all_picked,
                showing,
                format!("Select every pull request in {}", section.title()),
                move |_, cx| {
                    view.update(cx, |this, cx| this.toggle_section(&rows, cx))
                        .ok();
                },
            )
        });
        let heading = h_flex()
            .gap_2()
            .items_center()
            .child(ui::count_badge(shown.count, cx))
            .children(
                shown
                    .note
                    .map(|text| div().text_size(scale::meta()).text_color(muted).child(text)),
            )
            .children(empty);
        v_flex()
            // Its own id scope: every section's column heading has the same
            // ids, and a click must reach the one under the pointer.
            .id(SharedString::from(section.title()))
            .w_full()
            .child(section_heading(section, lead, heading, cx))
            // Each section heads its own columns: the viewer's own pull
            // requests have no author lane.
            .when(!shown.items.is_empty(), |column| {
                column.child(super::pr_row::column_header(
                    true,
                    true,
                    section.shows_author(),
                    self.page.sort,
                    Some(on_sort),
                    cx,
                ))
            })
            .child(list)
    }

    /// "All open" while its first search runs or after it failed with
    /// nothing to show; `None` once it has rows (they render as a section).
    fn all_open_placeholder(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let body = match self.all_open_state()? {
            Loadable::Loading => ui::skeleton_rows(3).into_any_element(),
            Loadable::Failed(error) => common::load_error(
                error,
                cx.listener(|view, _, _, cx| view.load_all_open(true, cx)),
            )
            .into_any_element(),
            Loadable::Loaded(_) | Loadable::Refreshing(_) => return None,
        };
        Some(
            v_flex()
                .w_full()
                .child(section_heading(Section::AllOpen, None, div(), cx))
                .child(body)
                .into_any_element(),
        )
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
        let body = match &self.inbox {
            Loadable::Failed(error) => {
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .px_0()
                    .into_any_element()
            }
            Loadable::Loading => self.loading().into_any_element(),
            Loadable::Loaded(inbox) | Loadable::Refreshing(inbox) => {
                let on_sort = self.on_sort(cx);
                let on_select = self.on_select(cx);
                let sections = self.sections(inbox);
                let showing = sections.iter().any(|(_, shown)| {
                    shown
                        .items
                        .iter()
                        .any(|summary| self.selection.contains(&summary.pr))
                });
                let sections: Vec<AnyElement> = sections
                    .into_iter()
                    .map(|(section, shown)| {
                        self.section(
                            section,
                            shown,
                            on_sort.clone(),
                            on_select.clone(),
                            showing,
                            cx,
                        )
                        .into_any_element()
                    })
                    .collect();
                v_flex()
                    .gap_3()
                    .children(sections)
                    .children(self.all_open_placeholder(cx))
                    .into_any_element()
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
            .on_action(cx.listener(|this, _: &SelectSectionRows, _, cx| this.select_section(cx)))
            .on_action(cx.listener(|this, _: &ClearReviewsSelection, _, cx| {
                if this.selection == Selection::default() {
                    cx.propagate();
                } else {
                    this.clear_selection(cx);
                }
            }))
            .child(self.header(cx))
            .children(self.filter_bar(cx))
            .children(self.notice(cx))
            .children(self.selection_bar(cx))
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

/// The hover group of a section heading, which shows its checkbox.
const SECTION_GROUP: &str = "reviews-section-heading";

/// A section's 32 px heading on the rows' own grid: `lead` (the box that
/// picks the section) over the rows' checkboxes, or an empty lane of their
/// width, its icon over the state icons, its title over the numbers, then
/// `detail` (count, note). The rows need no box around them.
fn section_heading(
    section: Section,
    lead: Option<Div>,
    detail: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .group(SECTION_GROUP)
        .h_8()
        .px_2()
        .gap_2()
        .items_center()
        // 12 px after the lane, as in a row; the icon's own 8 px gap
        // follows.
        .child(
            lead.unwrap_or_else(|| div().flex_none().w(scale::dp(16.)))
                .mr_1(),
        )
        .child(
            svg()
                .path(section.icon().path())
                .flex_none()
                .size_4()
                .text_color(muted),
        )
        .child(ui::group_title(section.title()))
        .child(detail)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use gpui_kit::{
        Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };
    use loupe_github::reviews::{Clause, Field, Op, Sort, SortDir, SortKey, ViewsFile};
    use loupe_github::{Inbox, Page, PrRef, PrState, PrSummary, RepoRef};

    use super::{InboxView, PageState, build, shown, shown_all_open, step_cursor};
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

    /// A pull request in `owner/repo` with `lines` changed.
    fn sized(owner: &str, number: u64, lines: u32) -> PrSummary {
        PrSummary {
            additions: lines,
            ..pr(owner, number)
        }
    }

    fn numbers(view: &Entity<InboxView>, cx: &mut VisualTestContext) -> Vec<Vec<u64>> {
        view.read_with(cx, |view, _| {
            view.sections_shown()
                .iter()
                .map(|section| section.iter().map(|pr| pr.number).collect())
                .collect()
        })
    }

    #[gpui_kit::test]
    fn a_column_heading_sorts_each_section_on_its_own_and_a_view_keeps_it(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        let inbox = Inbox {
            review_requested: page(
                vec![sized("a", 1, 50), sized("a", 2, 5), sized("a", 3, 500)],
                3,
                false,
            ),
            authored: page(Vec::new(), 0, false),
            involved: page(vec![sized("b", 4, 900), sized("b", 5, 1)], 2, false),
        };
        view.update(cx, |view, cx| {
            view.inbox = Loadable::Loaded(inbox);
            cx.notify();
        });
        let github = vec![vec![1, 2, 3], vec![], vec![4, 5]];
        assert_eq!(numbers(&view, cx), github);

        // Ascending, descending, then GitHub's order again; a row never
        // leaves its section.
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), [vec![2, 1, 3], vec![], vec![5, 4]]);
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), [vec![3, 1, 2], vec![], vec![4, 5]]);
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
        assert_eq!(walk, [2, 1, 3, 5, 4]);

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
        assert_eq!(numbers(&view, cx), [vec![2, 1, 3], vec![], vec![5, 4]]);
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
    fn checkboxes_pick_rows_in_the_sorted_order_and_a_filter_clears_them(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        cx.update(|_, cx| crate::shell::menus::bind_keys(cx));
        let inbox = Inbox {
            review_requested: page(
                vec![sized("a", 1, 50), sized("a", 2, 5), sized("a", 3, 500)],
                3,
                false,
            ),
            authored: page(Vec::new(), 0, false),
            involved: page(vec![sized("b", 4, 900), sized("b", 5, 1)], 2, false),
        };
        view.update(cx, |view, cx| {
            view.inbox = Loadable::Loaded(inbox);
            cx.notify();
        });
        assert_eq!(bounds(cx, "reviews-selection-bar"), None, "no bar yet");

        // Sorted smallest first, a Shift+click picks the rows between on
        // screen (2, 1, 3), not between in GitHub's order (2, 3).
        click(cx, "reviews-sort-changes");
        assert_eq!(numbers(&view, cx), [vec![2, 1, 3], vec![], vec![5, 4]]);
        click(cx, "pr-select-PR_2");
        let row = bounds(cx, "pr-row-PR_3").expect("row 3");
        cx.simulate_click(row.center(), Modifiers::shift());
        assert_eq!(picked(&view, cx), [2, 1, 3]);
        assert!(
            bounds(cx, "reviews-selection-bar").is_some(),
            "the bar shows the selection"
        );

        // Cmd+A picks the section of the last click: Involved, after 5.
        click(cx, "pr-select-PR_5");
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(picked(&view, cx), [2, 1, 3, 5, 4]);
        click(cx, "pr-select-PR_1");
        assert_eq!(picked(&view, cx), [2, 3, 5, 4]);

        // A new sort keeps the selection and reorders what the bar opens.
        click(cx, "reviews-sort-changes");
        assert_eq!(picked(&view, cx), [3, 2, 4, 5]);

        // The heading's box drops a section that is all picked.
        click(cx, "reviews-select-section-Involved");
        assert_eq!(picked(&view, cx), [3, 2]);
        click(cx, "reviews-select-section-Involved");
        assert_eq!(picked(&view, cx), [3, 2, 4, 5]);

        // Escape drops it all.
        cx.simulate_keystrokes("escape");
        assert!(picked(&view, cx).is_empty());
        assert_eq!(bounds(cx, "reviews-selection-bar"), None);

        // Changing the filters clears the selection; a sort does not.
        click(cx, "pr-select-PR_4");
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
            view.read_with(cx, |view, _| view.confirm)
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
    fn all_open_lists_only_what_the_sections_above_do_not() {
        let mut draft = pr("a", 3);
        draft.is_draft = true;
        let open = page(vec![pr("a", 1), pr("a", 2), draft, pr("b", 4)], 4, false);
        let above = PrRef::new(RepoRef::new("a", "repo"), 2);
        let numbers = |shown: &super::Shown| -> Vec<u64> {
            shown
                .items
                .iter()
                .map(|summary| summary.pr.number)
                .collect()
        };

        let all = shown_all_open(&open, &[], &[&above]);
        assert_eq!(numbers(&all), [1, 3, 4]);
        assert_eq!(all.count, 3);
        assert_eq!(all.note.as_deref(), Some("1 more listed above"));

        // The view's clauses apply as in every other section.
        let in_a = shown_all_open(&open, &org("a"), &[&above]);
        assert_eq!(numbers(&in_a), [1, 3]);
        let no_drafts = vec![Clause {
            field: Field::Status,
            op: Op::NotIn,
            values: vec!["draft".into()],
        }];
        assert_eq!(numbers(&shown_all_open(&open, &no_drafts, &[])), [1, 2, 4]);
        assert_eq!(shown_all_open(&open, &no_drafts, &[]).note, None);

        let partial = page(vec![pr("a", 1), pr("a", 2)], 700, true);
        assert_eq!(
            shown_all_open(&partial, &[], &[&above]).note.as_deref(),
            Some("Showing 2 of 700 · 1 more listed above")
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
