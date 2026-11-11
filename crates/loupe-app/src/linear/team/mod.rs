//! One team's issues as a board or a list, shaped by the views Linear has
//! for the team (tabs): its built-in ones and its custom views. The screen
//! works on a draft of the view's filters and display settings; Save writes
//! the draft back to Linear, and "Save as new view" makes a custom view
//! there. A custom view whose filter Loupe cannot reproduce exactly keeps
//! that filter as Linear has it, and the screen narrows it with filters of
//! its own. Also Favorites and the new issue dialog.

mod board;
mod dialogs;
mod filter;
mod filter_menu;
mod issues;
mod page;
mod toolbar;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::filter::FilterClause;
use loupe_linear::views::{
    CompletedWindow, DisplayProperty, ImportContext, Layout, Ordering, ViewConfig, ViewsFile,
    export_linear_filter, export_preferences, import_linear_filter,
};
use loupe_linear::{
    CustomViewUpdate, IssueSummary, IssueUpdate, Linear, LinearCustomView, Page, Team,
};

use self::board::DraggedIssue;
use self::filter_menu::FilterMenu;
use self::issues::DropTarget;
use self::page::PageState;
use super::data::{self, Loadable};
use super::views_store::Kind;
use super::{new_issue, remote_views, views_store, widgets};
use crate::services;
use crate::store::{self, Favorite};

gpui_kit::actions!(
    linear,
    [
        /// F on a team screen: open "Add Filter…".
        OpenFilter
    ]
);

/// Builds the team view for `Route::LinearTeam`.
pub fn view(
    team_id: String,
    name: String,
    view_id: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    cx.new(|cx| TeamView::new(team_id, name, view_id, None, window, cx))
        .into()
}

/// One project's issues, for the Issues tab of its page: the team screen
/// without its header and its views, on `team_id`'s states, labels, and
/// members (the project's lead team). Its display starts as a list grouped by
/// status with closed issues, as Linear shows a project.
pub(crate) fn project_issues(
    project_id: String,
    team_id: String,
    team_name: String,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TeamView> {
    cx.new(|cx| TeamView::new(team_id, team_name, None, Some(project_id), window, cx))
}

/// How a project's issues show at first.
fn project_config() -> ViewConfig {
    let mut config = ViewConfig {
        layout: Layout::List,
        completed: CompletedWindow::All,
        ..ViewConfig::default()
    };
    // Every issue here is in the project.
    config
        .properties
        .retain(|property| *property != DisplayProperty::Project);
    config
}

/// How a fetch treats what the view already shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fetch {
    /// A new first page: blank the issues while it loads.
    Reset,
    /// A new first page: keep showing the old issues until it arrives.
    Refresh,
    /// The page after `next_cursor`, appended to the issues.
    More,
}

/// State of the "Load more" button.
#[derive(Clone, Debug)]
enum More {
    Idle,
    Loading,
    Failed(SharedString),
}

/// A card drop sent to Linear and not answered yet. The board shows it on
/// top of the loaded issues until Linear accepts or refuses it.
#[derive(Clone, Debug)]
struct PendingDrop {
    seq: u64,
    issue_id: String,
    update: IssueUpdate,
}

/// A message under the toolbar: an import report or a failed save.
#[derive(Clone, Debug)]
struct Notice {
    text: SharedString,
    is_error: bool,
}

pub(crate) struct TeamView {
    team_id: String,
    /// Set on a project's Issues tab: the issues are the project's, from
    /// every team it spans, and there are no saved views.
    project_id: Option<String>,
    name: SharedString,
    /// The client the loads below belong to; a different client restarts them.
    client: Option<Arc<Linear>>,
    team: Loadable<Arc<Team>>,
    /// The key owner's id, for "me" in filters.
    me: Option<String>,
    issues: Loadable<Vec<IssueSummary>>,
    next_cursor: Option<String>,
    more: More,
    page: PageState,
    /// Filters set here on top of a custom view and never saved with it:
    /// the search text, and every filter while the view is locked. They
    /// narrow the issues in the app and last until the view changes.
    local_filters: Vec<FilterClause>,
    /// Why the filter of the custom view on screen stays Linear's: it uses
    /// a condition Loupe cannot reproduce exactly, or the team's labels and
    /// states (which reading it needs) are not loaded yet. `None` when the
    /// draft holds the view's filter.
    locked: Option<SharedString>,
    /// The issues on screen are a custom view's whole set, as Linear
    /// evaluated it, so a change to the draft needs no new request.
    loaded_by_linear: bool,
    /// A change to a view is on its way to Linear.
    saving: bool,
    /// Bumped by every fetch that replaces the first page, so a response to a
    /// request made under an older query is dropped.
    generation: u64,
    search: Entity<InputState>,
    debounce: Option<Task<()>>,
    requested_view: Option<String>,
    notice: Option<Notice>,
    /// The issue the new issue dialog just created, offered for opening.
    created: Option<IssueSummary>,
    /// The board's sideways scroll, to know when columns lie past its edge.
    board_scroll: ScrollHandle,
    /// The row of view tabs, tracked for its width: the row shows as many
    /// views as it holds.
    tabs_strip: ScrollHandle,
    /// How far the board scrolls each tick while a dragged card or column is
    /// near one of its sides; zero away from them.
    edge_scroll: f32,
    /// The loop that scrolls by `edge_scroll` is running.
    edge_scrolling: bool,
    /// Focused while nothing more specific is, so F reaches the screen.
    focus: FocusHandle,
    filter_menu: Entity<FilterMenu>,
    filter_open: bool,
    drops: Vec<PendingDrop>,
    /// Drops Linear confirmed while the issues were loading.
    confirmed: Vec<IssueSummary>,
    next_drop: u64,
    /// The card being dragged, as `(issue id, group key)`. Only meaningful
    /// while GPUI has an active drag.
    dragging: Option<(String, String)>,
    /// The key of the board column being dragged by its heading. Only
    /// meaningful while GPUI has an active drag.
    dragging_column: Option<String>,
    _search_subscription: Subscription,
    _issue_changes: Subscription,
}

impl TeamView {
    fn new(
        team_id: String,
        name: String,
        view_id: Option<String>,
        project_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let views = views_store::for_team(cx, &team_id);
        let page = match project_id {
            Some(_) => PageState {
                view_id: None,
                draft: project_config(),
            },
            None => PageState::open(&views, &team_id, view_id.as_deref()),
        };
        let text = filter::text_of(&page.draft.filters);
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .default_value(text)
        });
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this, input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search_changed(input.read(cx).value().to_string(), cx);
                }
            },
        );
        // A warning about the saved views is for a team screen to show.
        let warning = match project_id {
            Some(_) => None,
            None => views_store::take_warning(cx),
        };
        let notice = warning.map(|text| Notice {
            text,
            is_error: true,
        });
        let weak = cx.weak_entity();
        let filter_menu = cx.new(|cx| FilterMenu::new(weak, window, cx));
        // An issue edited on its own page: the copy listed here follows.
        let issue_changes = cx.observe_global::<data::IssueChange>(|view, cx| {
            let Some(issue) = cx.global::<data::IssueChange>().issue.clone() else {
                return;
            };
            if let Loadable::Ready(listed) = &mut view.issues
                && listed.iter().any(|known| known.id == issue.id)
            {
                issues::merge_page(listed, vec![issue]);
                cx.notify();
            }
        });
        let mut view = TeamView {
            team_id,
            project_id,
            name: name.into(),
            client: None,
            team: Loadable::Loading,
            me: None,
            issues: Loadable::Loading,
            next_cursor: None,
            more: More::Idle,
            page,
            local_filters: Vec::new(),
            locked: None,
            loaded_by_linear: false,
            saving: false,
            generation: 0,
            search,
            debounce: None,
            requested_view: view_id,
            notice,
            created: None,
            board_scroll: ScrollHandle::new(),
            tabs_strip: ScrollHandle::new(),
            edge_scroll: 0.,
            edge_scrolling: false,
            focus: cx.focus_handle(),
            filter_menu,
            filter_open: false,
            drops: Vec::new(),
            confirmed: Vec::new(),
            next_drop: 0,
            dragging: None,
            dragging_column: None,
            _search_subscription: subscription,
            _issue_changes: issue_changes,
        };
        view.sync_client(cx);
        view
    }

    /// Starts loading when the signed-in client is new to this view (first
    /// render, or the user connected Linear after opening the team).
    fn sync_client(&mut self, cx: &mut Context<Self>) {
        let client = data::client(cx);
        if data::same_client(&self.client, &client) {
            return;
        }
        if self.requested_view.is_none() {
            // A custom view is opened again once the team's catalog is
            // there, to read its filter.
            self.requested_view = self
                .page
                .view_id
                .clone()
                .filter(|id| views_store::kind(id) == Kind::Custom);
        }
        self.client = client;
        self.locked = self.lock(cx);
        self.me = data::me(cx);
        if self.client.is_some() {
            self.load_team(cx);
            self.fetch_issues(Fetch::Reset, cx);
        }
    }

    /// Reads the team and the issues again, keeping what shows meanwhile.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_team(cx);
        self.fetch_issues(Fetch::Refresh, cx);
        cx.notify();
    }

    fn reload_all(&mut self, cx: &mut Context<Self>) {
        self.load_team(cx);
        self.fetch_issues(Fetch::Reset, cx);
        cx.notify();
    }

    fn load_team(&mut self, cx: &mut Context<Self>) {
        self.team = Loadable::Loading;
        let task = data::team(self.team_id.clone(), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.team = match result {
                    Ok(team) => Loadable::Ready(team),
                    Err(message) => Loadable::Failed(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fetch_issues(&mut self, mode: Fetch, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let after = match mode {
            Fetch::More => match self.next_cursor.clone() {
                Some(cursor) => Some(cursor),
                None => return,
            },
            Fetch::Reset | Fetch::Refresh => None,
        };
        match mode {
            Fetch::Reset => {
                self.generation += 1;
                self.issues = Loadable::Loading;
                self.next_cursor = None;
                self.more = More::Idle;
            }
            Fetch::Refresh => {
                self.generation += 1;
                // The old cursor belongs to the old query: no paging until the
                // new first page arrives.
                self.next_cursor = None;
                self.more = More::Idle;
            }
            Fetch::More => self.more = More::Loading,
        }

        let generation = self.generation;
        let query = self.page.draft.query(Utc::now());
        let by_linear = self.asks_linear_for_the_view(cx);
        self.loaded_by_linear = by_linear.is_some();
        // A workspace view lists every team's issues; any other view, this
        // team's.
        let scope = match self.custom_view(cx) {
            Some(view) => view.team_id,
            None => Some(self.team_id.clone()),
        };
        let project_id = self.project_id.clone();
        let task = services::background(cx, move || match (by_linear, project_id) {
            (Some(id), _) => client.custom_view_all_issues(&id).map(|nodes| Page {
                nodes,
                next_cursor: None,
            }),
            (None, Some(project_id)) => {
                client.project_issues(&project_id, &query, after.as_deref())
            }
            (None, None) => client.issues(scope.as_deref(), &query, after.as_deref()),
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                if view.generation == generation {
                    view.apply_page(mode, result, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply_page(
        &mut self,
        mode: Fetch,
        result: Result<Page<IssueSummary>, loupe_linear::Error>,
        cx: &mut Context<Self>,
    ) {
        match (mode, result) {
            (Fetch::More, Ok(page)) => {
                if let Loadable::Ready(listed) = &mut self.issues {
                    issues::merge_page(listed, page.nodes);
                }
                self.next_cursor = page.next_cursor;
                self.more = More::Idle;
            }
            (Fetch::More, Err(error)) => self.more = More::Failed(data::error_text(&error)),
            (Fetch::Reset | Fetch::Refresh, Ok(page)) => {
                let mut listed = Vec::new();
                issues::merge_page(&mut listed, page.nodes);
                if let Loadable::Ready(old) = &self.issues {
                    issues::keep_newer(&mut listed, old);
                }
                issues::keep_newer(&mut listed, &std::mem::take(&mut self.confirmed));
                self.issues = Loadable::Ready(listed);
                self.next_cursor = page.next_cursor;
            }
            (Fetch::Reset | Fetch::Refresh, Err(error)) => {
                self.issues = Loadable::Failed(data::error_text(&error));
                self.next_cursor = None;
            }
        }
        cx.notify();
    }

    /// `listed` with the drops Linear has not answered yet applied.
    fn with_drops<'a>(&self, listed: &'a [IssueSummary], team: &Team) -> Cow<'a, [IssueSummary]> {
        if self.drops.is_empty() {
            return Cow::Borrowed(listed);
        }
        let mut shown = listed.to_vec();
        for drop in &self.drops {
            if let Some(issue) = shown.iter_mut().find(|issue| issue.id == drop.issue_id) {
                crate::linear::issue::optimistic::apply(issue, &drop.update, team);
            }
        }
        Cow::Owned(shown)
    }

    /// Column `moved` dropped on column `target` (or moved there from its
    /// menu): it takes that column's place in the draft's column order.
    fn move_column(&mut self, moved: &str, target: &str, cx: &mut Context<Self>) {
        self.dragging_column = None;
        let (Loadable::Ready(team), Loadable::Ready(listed)) = (&self.team, &self.issues) else {
            cx.notify();
            return;
        };
        let order = {
            let shown = self.with_drops(listed, team);
            let visible = self.visible_issues(&shown);
            let natural: Vec<String> =
                issues::group_issues(&visible, self.page.draft.group_by, team)
                    .into_iter()
                    .map(|group| group.key)
                    .collect();
            issues::reorder_columns(&natural, &self.page.draft.column_order, moved, target)
        };
        match order {
            Some(order) => self.edit_draft(|draft| draft.column_order = order, cx),
            None => cx.notify(),
        }
    }

    /// Moves a dropped card at once and sends the change to Linear; a refusal
    /// puts the card back and says why.
    fn drop_issue(&mut self, dragged: &DraggedIssue, target: DropTarget, cx: &mut Context<Self>) {
        self.dragging = None;
        let (Some(client), Loadable::Ready(team), Loadable::Ready(listed)) =
            (self.client.clone(), &self.team, &self.issues)
        else {
            cx.notify();
            return;
        };
        let plan = {
            let shown = self.with_drops(listed, team);
            let visible = self.visible_issues(&shown);
            let groups = issues::group_issues(&visible, self.page.draft.group_by, team);
            issues::drop_plan(
                &groups,
                self.page.draft.group_by,
                self.page.draft.order_by,
                &dragged.issue.id,
                &dragged.from_group,
                &target,
            )
        };
        let Some(plan) = plan else {
            cx.notify();
            return;
        };
        if plan.switch_to_manual {
            self.edit_draft(|draft| draft.order_by = Ordering::Manual, cx);
        }
        if plan.update == IssueUpdate::default() {
            return;
        }
        let seq = self.next_drop;
        self.next_drop += 1;
        let issue_id = dragged.issue.id.clone();
        let identifier = dragged.issue.identifier.clone();
        self.drops.push(PendingDrop {
            seq,
            issue_id: issue_id.clone(),
            update: plan.update.clone(),
        });
        let task = services::background(cx, move || client.update_issue(&issue_id, &plan.update));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.drops.retain(|drop| drop.seq != seq);
                match result {
                    Ok(issue) => match &mut view.issues {
                        Loadable::Ready(listed) => issues::merge_page(listed, vec![issue]),
                        // A first page is on its way: reconcile when it lands.
                        _ => view.confirmed.push(issue),
                    },
                    Err(error) => {
                        view.notice = Some(Notice {
                            text: format!(
                                "Couldn’t move {identifier}: {}",
                                data::error_text(&error)
                            )
                            .into(),
                            is_error: true,
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The views this team's screen shows. Once the team's labels and
    /// states are known, each custom view whose filter Loupe reproduces
    /// exactly carries it as clauses, so the screen can show and edit it.
    fn views(&self, cx: &mut App) -> ViewsFile {
        let mut file = views_store::for_team(cx, &self.team_id);
        if let Loadable::Ready(team) = &self.team {
            let context = ImportContext {
                labels: &team.labels,
                states: &team.states,
            };
            for view in &mut file.views {
                // A workspace view lists every team's issues, and this
                // team's labels, states, and cycles cannot read its filter
                // for the others: it stays Linear's.
                if let Some(remote) = remote_views::metadata(&view.id, cx)
                    && remote.team_id.is_some()
                    && let Ok(filters) = import_linear_filter(&remote.filter_data, &context)
                {
                    view.config.filters = filters;
                }
            }
        }
        file
    }

    /// Where the view on screen comes from.
    fn view_kind(&self) -> Option<Kind> {
        self.page.view_id.as_deref().map(views_store::kind)
    }

    /// The custom view on screen, as Linear has it.
    fn custom_view(&self, cx: &mut App) -> Option<LinearCustomView> {
        remote_views::metadata(self.page.view_id.as_deref()?, cx)
    }

    /// Why the filter of the view on screen stays Linear's; `None` when
    /// the draft holds it. See [`Self::locked`].
    fn lock(&self, cx: &mut App) -> Option<SharedString> {
        if self.view_kind() != Some(Kind::Custom) {
            return None;
        }
        let (Some(view), Loadable::Ready(team)) = (self.custom_view(cx), &self.team) else {
            return Some("Its filter is still loading.".into());
        };
        if view.team_id.is_none() {
            return Some("It lists the issues of every team.".into());
        }
        let context = ImportContext {
            labels: &team.labels,
            states: &team.states,
        };
        import_linear_filter(&view.filter_data, &context)
            .err()
            .map(SharedString::from)
    }

    /// The custom view to ask Linear for whole, when the screen shows it
    /// with the filter Linear has: a locked view, or one whose filter the
    /// draft has not changed. `None` when the draft's own filters are what
    /// to ask for.
    fn asks_linear_for_the_view(&self, cx: &mut App) -> Option<String> {
        let id = self.page.view_id.as_deref()?;
        let custom = id.strip_prefix("linear:")?;
        let unchanged = self
            .views(cx)
            .get(id)
            .is_some_and(|saved| saved.config.filters == self.page.draft.filters);
        (self.locked.is_some() || unchanged).then(|| custom.to_owned())
    }

    /// Changes the draft. A change to what Linear is asked for (filters, the
    /// closed window, sub-issues) refetches from the first page; a display
    /// change does not, and neither does any change while the screen holds
    /// a custom view's whole set and still shows that view's own filter.
    fn edit_draft(&mut self, change: impl FnOnce(&mut ViewConfig), cx: &mut Context<Self>) {
        let asked = |draft: &ViewConfig| {
            (
                draft.filters.clone(),
                draft.completed,
                draft.show_sub_issues,
            )
        };
        let before = asked(&self.page.draft);
        change(&mut self.page.draft);
        let whole_view = self.loaded_by_linear && self.asks_linear_for_the_view(cx).is_some();
        if before != asked(&self.page.draft) && !whole_view {
            self.debounce = None;
            self.fetch_issues(Fetch::Refresh, cx);
        }
        cx.notify();
    }

    /// The filters the filter bar shows as chips and the filter menu edits:
    /// the draft's, or the local ones on a locked view.
    fn filters(&self) -> &[FilterClause] {
        if self.locked.is_some() {
            &self.local_filters
        } else {
            &self.page.draft.filters
        }
    }

    /// Changes the filters of [`Self::filters`]. On a locked view nothing is
    /// fetched: its issues are all loaded, and the filters apply in the app.
    fn edit_filters(
        &mut self,
        change: impl FnOnce(&mut Vec<FilterClause>),
        cx: &mut Context<Self>,
    ) {
        if self.locked.is_some() {
            change(&mut self.local_filters);
            cx.notify();
        } else {
            self.edit_draft(|draft| change(&mut draft.filters), cx);
        }
    }

    /// Where the search text lives: beside a custom view, never in it (a
    /// search is not part of a view saved in Linear); in the draft otherwise.
    fn search_is_local(&self) -> bool {
        self.view_kind() == Some(Kind::Custom)
    }

    fn search_text(&self) -> String {
        if self.search_is_local() {
            filter::text_of(&self.local_filters)
        } else {
            filter::text_of(&self.page.draft.filters)
        }
    }

    /// Records the typed text now and refetches once typing pauses.
    fn search_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.search_text() == text.trim() {
            return;
        }
        if self.search_is_local() {
            filter::set_text(&mut self.local_filters, &text);
            cx.notify();
            return;
        }
        filter::set_text(&mut self.page.draft.filters, &text);
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            this.update(cx, |view, cx| {
                view.fetch_issues(Fetch::Refresh, cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Puts the search text in force in the search box.
    fn sync_search_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.search_text();
        self.search
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    fn switch_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.requested_view = None;
        self.local_filters.clear();
        let views = self.views(cx);
        self.page.switch(&views, id);
        self.locked = self.lock(cx);
        let remote = remote_views::metadata(id, cx);
        if let Loadable::Ready(team) = &self.team {
            self.name = if remote.as_ref().is_some_and(|view| view.team_id.is_none()) {
                "Workspace".into()
            } else {
                team.name.clone().into()
            };
        }
        // Why the view's filter is locked is the lock beside its title;
        // the bar is for the display settings that could not be read.
        self.notice = remote.as_ref().and_then(|remote| {
            let (_, warnings) = loupe_linear::views::linear_preferences(&remote.preferences);
            if warnings.is_empty() {
                return None;
            }
            Some(Notice {
                text: warnings.join(" ").into(),
                is_error: false,
            })
        });
        // Opened before the team's catalog arrived, the draft holds no
        // filter yet: the view is opened again once it is there, so the
        // draft never passes for a view whose filter was removed.
        if views_store::kind(id) == Kind::Custom && !matches!(self.team, Loadable::Ready(_)) {
            self.requested_view = Some(id.to_owned());
        }
        self.sync_search_box(window, cx);
        self.fetch_issues(Fetch::Reset, cx);
        cx.notify();
    }

    fn discard_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let views = self.views(cx);
        self.page.discard(&views);
        self.sync_search_box(window, cx);
        self.fetch_issues(Fetch::Refresh, cx);
        cx.notify();
    }

    /// Runs a change to Linear's views off the main thread, then `done` with
    /// what Linear answered. A refusal is shown and changes nothing here.
    fn change_views<T: Send + 'static>(
        &mut self,
        what: &'static str,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Linear) -> loupe_linear::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.saving = true;
        let task = services::background(cx, move || work(&client));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(value) => done(view, value, cx),
                    Err(error) => view.report(
                        Some(format!("Couldn’t {what}: {}", data::error_text(&error)).into()),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Saves the draft over the view on screen: in Linear for a custom view
    /// (its filter when the draft changed it, and the display settings as
    /// the key owner's own), in Loupe's file for a view still only there.
    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.page.view_id.clone() else {
            return;
        };
        let draft = self.page.draft.clone();
        match views_store::kind(&id) {
            Kind::BuiltIn => {}
            Kind::Legacy => {
                let result = views_store::update_legacy(cx, |views| {
                    views.update_config(&id, draft, Utc::now())
                });
                self.report(result.err(), cx);
            }
            Kind::Custom => {
                let (Some(remote), Some(saved)) = (
                    self.custom_view(cx),
                    self.views(cx).get(&id).map(|view| view.config.clone()),
                ) else {
                    return;
                };
                let filter = if self.locked.is_none() && draft.filters != saved.filters {
                    match export_linear_filter(&draft.filters) {
                        Ok(filter) => Some(filter),
                        Err(reason) => {
                            self.report(Some(format!("{reason}. Remove it to save.").into()), cx);
                            return;
                        }
                    }
                } else {
                    None
                };
                let display = |config: &ViewConfig| ViewConfig {
                    filters: Vec::new(),
                    ..config.clone()
                };
                let preferences = (display(&draft) != display(&saved))
                    .then(|| export_preferences(&draft, Some(&saved), &remote.own_preferences));
                let refetch = filter.is_some();
                let (sent, before) = (draft.clone(), saved.clone());
                self.change_views(
                    "save the view",
                    cx,
                    move |client| {
                        let mut stored = None;
                        if let Some(filter_data) = filter {
                            let update = CustomViewUpdate {
                                filter_data: Some(filter_data),
                                ..CustomViewUpdate::default()
                            };
                            stored = Some(client.update_custom_view(&remote.id, &update)?);
                        }
                        // The filter may be stored by now: what Linear holds
                        // is kept even when the display settings are refused.
                        let refused = match preferences {
                            Some(preferences) => {
                                match client.save_view_preferences(&remote.id, &preferences) {
                                    Ok(view) => {
                                        stored = Some(view);
                                        None
                                    }
                                    Err(error) if stored.is_some() => Some(error),
                                    Err(error) => return Err(error),
                                }
                            }
                            None => None,
                        };
                        Ok((stored, refused))
                    },
                    move |view, (stored, refused): (Option<LinearCustomView>, _), cx| {
                        if let Some(error) = refused {
                            let error: loupe_linear::Error = error;
                            view.report(
                                Some(
                                    format!(
                                        "The filter is saved in Linear, but not the display settings: {}",
                                        data::error_text(&error)
                                    )
                                    .into(),
                                ),
                                cx,
                            );
                        }
                        let Some(stored) = stored else {
                            return;
                        };
                        remote_views::upsert(stored, cx);
                        // The draft becomes what Linear stored, so nothing
                        // Linear did not keep stays on screen as if saved.
                        // A draft edited since the save was sent is left as
                        // it is (`PageState::settles_on_stored`).
                        if view.page.view_id.as_deref() == Some(id.as_str())
                            && view.page.settles_on_stored(&sent, &before)
                        {
                            let views = view.views(cx);
                            view.page.switch(&views, &id);
                            view.locked = view.lock(cx);
                            if refetch {
                                view.fetch_issues(Fetch::Refresh, cx);
                            }
                        }
                    },
                );
            }
        }
    }

    /// "Save as new view": the draft as a new custom view in Linear.
    fn save_as_new(&mut self, name: &str, cx: &mut Context<Self>) {
        self.create_in_linear(name, None, cx);
    }

    /// "Save to Linear" on a view that is only in Loupe: Linear gets it
    /// under the same name, with its star, and Loupe's copy goes.
    fn move_to_linear(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(name) = self.views(cx).get(id).map(|view| view.name.clone()) else {
            return;
        };
        if self.page.view_id.as_deref() == Some(id) {
            self.create_in_linear(&name, Some(id.to_owned()), cx);
        }
    }

    /// Saves the draft as a new custom view in Linear (see
    /// [`views_store::create_in_linear`]) and switches to it: a view of this
    /// team, or of the team the custom view on screen belongs to.
    fn create_in_linear(&mut self, name: &str, moved: Option<String>, cx: &mut Context<Self>) {
        let team_id = match self.custom_view(cx) {
            Some(view) => view.team_id,
            None => Some(self.team_id.clone()),
        };
        let draft = self.page.draft.clone();
        let task = views_store::create_in_linear(name, &draft, team_id, moved, cx);
        self.saving = true;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(created) => {
                        view.requested_view = Some(created.id);
                        view.report(created.warning, cx);
                    }
                    Err(message) => view.report(Some(message), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn rename_view(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        match views_store::kind(id) {
            Kind::BuiltIn => {}
            Kind::Legacy => {
                let result =
                    views_store::update_legacy(cx, |views| views.rename(id, name, Utc::now()));
                self.report(result.err(), cx);
            }
            Kind::Custom => {
                let Some(remote) = remote_views::metadata(id, cx) else {
                    return;
                };
                let update = CustomViewUpdate {
                    name: Some(name.trim().to_owned()),
                    ..CustomViewUpdate::default()
                };
                self.change_views(
                    "rename the view",
                    cx,
                    move |client| client.update_custom_view(&remote.id, &update),
                    |_, stored, cx| remote_views::upsert(stored, cx),
                );
            }
        }
    }

    fn move_view(&mut self, id: &str, step: i32, cx: &mut Context<Self>) {
        let team_id = self.team_id.clone();
        let result = views_store::update_legacy(cx, |views| views.move_by(id, &team_id, step));
        self.report(result.err(), cx);
    }

    /// Opens the team's first tab after the view on screen was deleted.
    fn left_view(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.page.view_id.as_deref() == Some(id) {
            let views = self.views(cx);
            self.page.after_delete(&views, &self.team_id);
            self.requested_view = self.page.view_id.clone();
        }
        cx.notify();
    }

    fn delete_view(&mut self, id: &str, cx: &mut Context<Self>) {
        match views_store::kind(id) {
            Kind::BuiltIn => {}
            Kind::Legacy => {
                let result = views_store::update_legacy(cx, |views| views.delete(id));
                self.left_view(id, cx);
                self.report(result.err(), cx);
            }
            Kind::Custom => {
                let Some(remote) = remote_views::metadata(id, cx) else {
                    return;
                };
                let id = id.to_owned();
                self.change_views(
                    "delete the view",
                    cx,
                    {
                        let remote_id = remote.id.clone();
                        move |client| client.delete_custom_view(&remote_id)
                    },
                    move |view, (), cx| {
                        remote_views::remove(&remote.id, cx);
                        view.left_view(&id, cx);
                    },
                );
            }
        }
    }

    fn is_project(&self) -> bool {
        self.project_id.is_some()
    }

    fn report(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        if let Some(text) = error {
            self.notice = Some(Notice {
                text,
                is_error: true,
            });
        }
        cx.notify();
    }

    /// What the header star stars: the saved view on screen, or the team's
    /// issues when no view is applied.
    fn favorite(&self) -> Favorite {
        match &self.page.view_id {
            Some(id) => Favorite::View { id: id.clone() },
            None => Favorite::Team {
                team_id: self.team_id.clone(),
                name: self.name.to_string(),
            },
        }
    }

    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let favorite = self.favorite();
        store::update_settings(cx, move |settings| settings.toggle_favorite(favorite));
    }

    /// Called by the new issue dialog once Linear accepted the issue.
    fn issue_created(&mut self, issue: IssueSummary, cx: &mut Context<Self>) {
        self.created = Some(issue);
        self.fetch_issues(Fetch::Refresh, cx);
        cx.notify();
    }

    fn open_new_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_issue_in(None, window, cx);
    }

    /// The new issue dialog, starting in `state_id` when given.
    fn open_new_issue_in(
        &mut self,
        state_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(client), Loadable::Ready(team)) = (self.client.clone(), &self.team) else {
            return;
        };
        let view = cx.weak_entity();
        let on_created = Box::new(
            move |issue: IssueSummary, _: bool, _: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.issue_created(issue, cx))
                    .ok();
            },
        );
        new_issue::open(team.clone(), client, state_id, on_created, window, cx);
    }
}

impl TeamView {
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
}

impl Render for TeamView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        self.sync_client(cx);
        if let Some(id) = self.requested_view.clone() {
            let views = self.views(cx);
            if views.get(&id).is_some() && matches!(self.team, Loadable::Ready(_)) {
                self.requested_view = None;
                self.switch_view(&id, window, cx);
            }
        }
        self.locked = self.lock(cx);
        if self.client.is_none() {
            return widgets::not_connected(
                gpui_kit::component::IconName::LayoutDashboard,
                self.name.clone(),
            );
        }
        if let Some(id) = self
            .requested_view
            .clone()
            .filter(|id| views_store::kind(id) == Kind::Custom)
        {
            let views = self.views(cx);
            if views.get(&id).is_none() {
                let state = cx.global::<remote_views::RemoteViews>();
                let body = if state.loading {
                    crate::ui::skeleton_rows(8).into_any_element()
                } else {
                    widgets::load_error(
                        state
                            .error
                            .clone()
                            .unwrap_or_else(|| "This Linear view is no longer available.".into()),
                        |_, _, cx| remote_views::retry(cx),
                    )
                };
                return v_flex()
                    .size_full()
                    .child(crate::ui::page_header("Linear view"))
                    .child(body)
                    .into_any_element();
            }
        }
        v_flex()
            .size_full()
            .key_context("TeamView")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenFilter, window, cx| {
                this.set_filter_open(true, window, cx)
            }))
            .when(!self.is_project(), |page| {
                page.child(self.render_header(cx))
            })
            .child(self.render_tabs(window, cx))
            .children(self.render_filter_bar(cx))
            .children(self.render_notice(cx))
            .child(self.render_body(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod project_tests {
    use loupe_linear::views::{CompletedWindow, DisplayProperty, Grouping, Layout};

    use super::project_config;

    #[test]
    fn a_projects_issues_start_as_a_list_by_status_with_the_closed_ones() {
        let config = project_config();
        assert_eq!(config.layout, Layout::List);
        assert_eq!(config.group_by, Grouping::Status);
        // Linear's progress counts the done issues: the page shows them.
        assert_eq!(config.completed, CompletedWindow::All);
        assert!(!config.shows(DisplayProperty::Project));
    }
}
