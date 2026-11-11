//! `Route::Inbox` (Reviews): the three dashboard sections, and on a saved
//! view that names repositories every other open pull request in them
//! ("All open"), narrowed by filter clauses (the Linear board's "Add
//! Filter…" pattern) that can be saved as views.

mod dialogs;
mod filter_menu;
mod views_store;

use std::collections::HashMap;
use std::rc::Rc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::reviews::{self, Clause, Op, ViewsFile};
use loupe_github::{Inbox, Page, PrRef, PrSummary};

use self::filter_menu::{FilterMenu, FilterOptions};
use super::common::{self, Loadable};
use super::pr_row::pr_row;
use crate::route::Route;
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::theme::scale;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

gpui_kit::actions!(
    github,
    [
        /// F on the Reviews screen: open "Add Filter…".
        OpenReviewsFilter
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

/// The saved view on screen and the clauses edited on top of it. The draft
/// is "unsaved" while it differs from the saved view; with no view, while it
/// has any clause.
#[derive(Clone, Debug, Default, PartialEq)]
struct PageState {
    view_id: Option<String>,
    draft: Vec<Clause>,
    /// The view's saved "All open" choice; `None` means the default (on).
    all_open: Option<bool>,
}

impl PageState {
    fn saved<'a>(&self, views: &'a ViewsFile) -> Option<&'a [Clause]> {
        views
            .get(self.view_id.as_deref()?)
            .map(|view| view.filters.as_slice())
    }

    fn is_dirty(&self, views: &ViewsFile) -> bool {
        match self.saved(views) {
            Some(saved) => saved != self.draft.as_slice(),
            None => !self.draft.is_empty(),
        }
    }

    /// Shows view `id` (or no view) with its saved clauses.
    fn switch(&mut self, views: &ViewsFile, id: Option<&str>) {
        let view = id.and_then(|id| views.get(id));
        self.view_id = view.map(|view| view.id.clone());
        self.draft = view.map(|view| view.filters.clone()).unwrap_or_default();
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
    _load: Option<Task<()>>,
    _checks: Subscription,
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
            _load: None,
            _checks: checks,
        };
        view.reload(cx);
        view
    })
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
        sections
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
        change(&mut self.page.draft);
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
        self.page.switch(&views, id);
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
        let draft = self.page.draft.clone();
        let result = views_store::update(cx, |views| views.update_filters(&id, draft, Utc::now()));
        self.report(result.err(), cx);
    }

    fn save_as_new(&mut self, name: &str, cx: &mut Context<Self>) {
        let draft = self.page.draft.clone();
        match views_store::update(cx, |views| views.create(name, draft, Utc::now())) {
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

    fn section(&self, section: Section, shown: Shown<'_>, cx: &App) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
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
                cx,
            )
        }));
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
            .w_full()
            .child(section_heading(section, heading, cx))
            // Each section heads its own columns: the viewer's own pull
            // requests have no author lane.
            .when(!shown.items.is_empty(), |column| {
                column.child(super::pr_row::column_header(
                    true,
                    section.shows_author(),
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
                .child(section_heading(Section::AllOpen, div(), cx))
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
                let sections: Vec<AnyElement> = self
                    .sections(inbox)
                    .into_iter()
                    .map(|(section, shown)| self.section(section, shown, cx).into_any_element())
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
            .child(self.header(cx))
            .children(self.filter_bar(cx))
            .children(self.notice(cx))
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

/// A section's 32 px heading on the rows' own grid: its icon over the state
/// icons, its title over the numbers, then `detail` (count, note). The rows
/// need no box around them.
fn section_heading(section: Section, detail: impl IntoElement, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .h_8()
        .px_2()
        .gap_2()
        .items_center()
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
        Bounds, Pixels, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
    };
    use loupe_github::reviews::{Clause, Field, Op, ViewsFile};
    use loupe_github::{Page, PrRef, PrState, PrSummary, RepoRef};

    use super::{PageState, build, shown, shown_all_open, step_cursor};
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

    #[gpui_kit::test]
    fn changing_a_filter_moves_neither_the_filter_button_nor_its_menu(cx: &mut TestAppContext) {
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
        let cx = &mut VisualTestContext::from_window(handle, cx);

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
    }
}
