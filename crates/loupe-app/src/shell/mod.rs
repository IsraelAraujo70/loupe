//! The window frame: the top bar with the tabs, the sidebar, and the content
//! card that shows the active tab's page. Owned by the shell front. Other
//! fronts navigate only through [`navigate`], [`open`], and
//! [`open_in_new_tab`].
//!
//! `Workspace` is the window's root view. It renders, by `Accounts.github`:
//! `Checking` a quiet splash, `Disconnected`/`Failed` the full-window
//! `settings::sign_in_view`, `Connected` the frame. The frame splits into
//! panes (`panes::Panes`), each with its own tabs; each tab has its own
//! history with back and forward (`tabs::Tabs`), and a pane's tabs can be
//! grouped. The panes, their tabs, and their groups are saved in `Settings` and come back after a restart. It observes the `Services`
//! global and rebuilds every page when the GitHub client changes, since
//! GitHub views read it once when they load. Linear and T3 views follow their
//! own clients, so connecting those keeps every page and its drafts. It
//! handles the navigation, tab, sidebar, palette, and scrolling actions;
//! `menus` handles `OpenSettings`.

pub mod actions;
pub mod http;
pub mod item_menu;
mod links;
pub mod menus;
pub mod palette;
mod panes;
pub mod router;
mod sidebar;
mod tab_bar;
mod tab_groups;
mod tabs;
mod workspace_menu;

pub use links::{open_link, open_system_urls};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{ResizeHandleRenderer, ResizeHandleState};
use gpui_kit::component::resizable::{self, HandleEdge};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::linear::issue_menu::Field;
use crate::route::Route;
use crate::services::{Accounts, Connection, Services};
use crate::settings;
use crate::shell::actions::{
    ActivateLastTab, ActivateTab1, ActivateTab2, ActivateTab3, ActivateTab4, ActivateTab5,
    ActivateTab6, ActivateTab7, ActivateTab8, CloseTab, ComposeIssue, CopyItemBranch, CopyItemId,
    CopyItemLink, GoBack, GoForward, NewTab, NextTab, OpenItemInNewTab, OpenItemSideBySide,
    OpenPalette, PreviousTab, ScrollPageDown, ScrollPageUp, SearchTabs, SplitDown, SplitLeft,
    SplitRight, SplitUp, SwitchWorkspace, ToggleSidebar,
};
use crate::shell::item_menu::Command;
use crate::shell::panes::{Node, Pane, PaneId, Panes, Zone};
use crate::shell::tab_bar::DraggedTab;
use crate::shell::tabs::Page;
use crate::shell::tabs::Tabs;
use crate::store::{self, Settings, SplitAxis};
use crate::theme::{scale, tokens};
use crate::ui::WindowDragArea as _;

pub(crate) use crate::ui::HEADER_HEIGHT as TITLE_BAR_HEIGHT;

/// The gap between the window edge and the content card, on its right and
/// bottom; the top bar sits above it.
pub(crate) const CARD_INSET: Pixels = px(6.);

/// One command palette result.
#[derive(Clone, Debug)]
pub struct PaletteItem {
    pub title: SharedString,
    pub subtitle: SharedString,
    pub route: Route,
    /// A pull request's comment count, shown at the row's end when above zero.
    pub comments: u32,
}

pub struct Workspace {
    panes: Panes<AnyView>,
    /// The tab being dragged, set when its drag starts. It counts only while
    /// GPUI still has a drag (`lifted_tab`), which ends at the mouse up.
    tab_drag: Option<DraggedTab>,
    /// Where each pane's page was drawn in the last frame, for the drop
    /// zones and for paging the active pane.
    pane_bounds: Rc<RefCell<HashMap<PaneId, Bounds<Pixels>>>>,
    /// Whether a divider between panes is being dragged; the new share is
    /// saved once, when it is released.
    resizing_panes: bool,
    focus: FocusHandle,
    /// Built while GitHub is signed out, dropped once it connects so its
    /// device-flow polling stops.
    sign_in: Option<AnyView>,
    /// The GitHub client the pages were built with.
    github: Option<Arc<loupe_github::GitHub>>,
    sidebar: Entity<sidebar::SidebarData>,
    /// The sidebar's width while its edge is dragged, saved to `Settings`
    /// once, when the drag ends.
    sidebar_drag: Option<f32>,
    /// The menu of a right-clicked link in prose, while it is open.
    link_menu: Option<links::FloatingMenu>,
    /// The Linear workspace switcher, while it is open.
    workspace_menu: Option<links::FloatingMenu>,
    /// The page each pane showed in the last frame. When one changes, the
    /// row under the pointer may have gone without a word, so it is
    /// forgotten.
    shown_pages: Vec<EntityId>,
    /// Each pane's row of tabs: its sideways scroll and the open tab it
    /// follows.
    tab_rows: HashMap<PaneId, tab_bar::TabRow>,
    _subscriptions: Vec<Subscription>,
}

/// A drag on the sidebar's edge, and the (empty) view it drags.
#[derive(Clone)]
struct ResizeSidebar;

impl Render for ResizeSidebar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// A drag on the divider of split `0`, and the (empty) view it drags.
#[derive(Clone)]
struct ResizePanes(u64);

impl Render for ResizePanes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// The workspace, and its root focus handle for [`page_may_take_focus`].
struct WorkspaceHandle(WeakEntity<Workspace>, FocusHandle);

impl Global for WorkspaceHandle {}

fn with_workspace(cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Context<Workspace>)) {
    if let Some(workspace) = cx
        .try_global::<WorkspaceHandle>()
        .and_then(|handle| handle.0.upgrade())
    {
        workspace.update(cx, f);
    }
}

/// Whether a page may focus itself while it renders: nothing is focused, or
/// only the workspace root is, so it takes focus from no input. A page with
/// its own key bindings (the team screen's F) focuses itself this way.
pub fn page_may_take_focus(window: &Window, cx: &App) -> bool {
    match window.focused(cx) {
        None => true,
        Some(focused) => cx
            .try_global::<WorkspaceHandle>()
            .is_some_and(|handle| handle.1 == focused),
    }
}

/// Whether `view` is a page on screen: the current page of a pane's open
/// tab, in the active window. Pages in a tab's history or in another tab are
/// not.
pub fn is_on_screen(view: EntityId, cx: &App) -> bool {
    cx.active_window().is_some()
        && cx
            .try_global::<WorkspaceHandle>()
            .and_then(|handle| handle.0.upgrade())
            .is_some_and(|workspace| {
                workspace
                    .read(cx)
                    .panes
                    .panes()
                    .iter()
                    .any(|pane| pane.tabs.active().current().view.entity_id() == view)
            })
}

/// Shows `route` in the active tab, after its current page; from a pinned
/// tab, in a new tab right after the pinned ones, so the pinned tab keeps
/// its page. Re-navigating to the current route does nothing.
pub fn navigate(route: Route, window: &mut Window, cx: &mut App) {
    with_workspace(cx, |workspace, cx| workspace.push(route, window, cx));
}

/// Opens `route` in a new background tab, as a browser opens a link in a new
/// tab: the active tab stays on screen.
pub fn open_in_new_tab(route: Route, window: &mut Window, cx: &mut App) {
    with_workspace(cx, |workspace, cx| {
        workspace.open_background_tab(route, window, cx)
    });
}

/// Opens `route` for a click: in a new background tab for a middle click or
/// a Cmd-click, as browsers and Linear do, in the active tab otherwise.
pub fn open(route: Route, event: &ClickEvent, window: &mut Window, cx: &mut App) {
    if event.is_middle_click() || event.modifiers().platform {
        open_in_new_tab(route, window, cx);
    } else {
        navigate(route, window, cx);
    }
}

/// Middle clicks for elements that open a page. GPUI's `on_click` sees only
/// the primary button, so a row that should open a background tab on a middle
/// click registers its handler here too and passes the event to [`open`].
pub trait OpenOnClick: InteractiveElement + Sized {
    /// Runs `listener` for a middle click and stops the event there, so a
    /// link inside a row opens only its own page.
    fn on_middle_click(
        mut self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.interactivity().on_aux_click(move |event, window, cx| {
            if event.is_middle_click() {
                cx.stop_propagation();
                listener(event, window, cx);
            }
        });
        self
    }

    /// Runs `listener` for a primary click and for a middle click.
    fn on_open(mut self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self
    where
        Self: StatefulInteractiveElement,
    {
        let listener = Rc::new(listener);
        let primary = listener.clone();
        self.interactivity()
            .on_click(move |event, window, cx| primary(event, window, cx));
        self.on_middle_click(move |event, window, cx| listener(event, window, cx))
    }

    /// Opens `route` on a click, through [`open`].
    fn opens(self, route: Route) -> Self
    where
        Self: StatefulInteractiveElement,
    {
        self.on_open(move |event, window, cx| open(route.clone(), event, window, cx))
    }
}

impl<E: InteractiveElement> OpenOnClick for E {}

/// Splits the active pane on `zone`'s side with its active tab, as the Split
/// actions do. For the command palette.
fn split_active_tab(zone: Zone, window: &mut Window, cx: &mut App) {
    with_workspace(cx, |workspace, cx| workspace.split_active(zone, window, cx));
}

/// Shows tab `tab` and makes its pane the active one. For the command
/// palette.
fn activate_tab(tab: u64, cx: &mut App) {
    with_workspace(cx, |workspace, cx| workspace.select_tab_id(tab, cx));
}

/// Opens `route` in a new pane on the right of the active pane, as "Open
/// to the side" in VS Code.
pub fn open_side_by_side(route: Route, window: &mut Window, cx: &mut App) {
    with_workspace(cx, |workspace, cx| workspace.open_beside(route, window, cx));
}

impl Workspace {
    pub fn new(initial: Route, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        cx.set_global(WorkspaceHandle(cx.entity().downgrade(), focus.clone()));
        focus.focus(window, cx);
        let saved = Settings::get(cx).clone();
        let panes = Panes::restore(&saved.pane_layout(), saved.active_pane, initial, |route| {
            router::build(route, window, cx)
        });
        let sidebar = cx.new(sidebar::SidebarData::new);
        let subscriptions = vec![
            // The focused element can leave the tree while its handle lives
            // on (a resolved thread's reply box, a sent composer, a closed
            // editor). Focus then points nowhere, and the shortcuts (Cmd+K,
            // Page Down, back) and focus-dispatched actions reach nothing.
            cx.on_focus_lost(window, |this, window, cx| {
                match window.focus_lost_restore_target(cx) {
                    Some(target) => target.focus(window, cx),
                    None => this.focus.focus(window, cx),
                }
            }),
            cx.observe(&sidebar, |_, _, cx| cx.notify()),
            cx.observe_global_in::<Services>(window, Self::services_changed),
            cx.observe_global::<Accounts>(|_, cx| cx.notify()),
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
            // Color scheme System follows the system's light and dark.
            cx.observe_window_appearance(window, |_, _, cx| {
                if Settings::get(cx).appearance.color_scheme == crate::theme::ColorScheme::System {
                    crate::theme::apply(cx);
                }
            }),
        ];
        Self {
            panes,
            tab_drag: None,
            pane_bounds: Rc::default(),
            resizing_panes: false,
            focus,
            sign_in: None,
            github: Services::get(cx).github.clone(),
            sidebar,
            sidebar_drag: None,
            link_menu: None,
            workspace_menu: None,
            shown_pages: Vec::new(),
            tab_rows: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Rebuilds every page when the GitHub client changed, so they load with
    /// the new one. Signing out of GitHub leaves one tab on the inbox.
    /// Settings is kept: it observes the globals itself, and rebuilding it
    /// would drop what the user is typing.
    fn services_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let github = Services::get(cx).github.clone();
        let same = match (&self.github, &github) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.github = github;
        if self.github.is_none() {
            let inbox = Page {
                view: router::build(&Route::Inbox, window, cx),
                route: Route::Inbox,
            };
            self.panes = Panes::single(inbox);
            // Pane ids start over with the new panes.
            self.pane_bounds.borrow_mut().clear();
        }
        for tab in self.panes.tabs_iter_mut() {
            for page in tab.pages_mut() {
                if page.route != Route::Settings {
                    page.view = router::build(&page.route, window, cx);
                }
            }
        }
        self.save_tabs(cx);
        cx.notify();
    }

    fn page(&self, route: Route, window: &mut Window, cx: &mut Context<Self>) -> Page<AnyView> {
        Page {
            view: router::build(&route, window, cx),
            route,
        }
    }

    /// Shows `route` after the active tab's page; a pinned tab keeps its
    /// page and the route opens in a new tab after the pinned ones.
    fn push(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.panes.active().active();
        if active.current().route == route {
            return;
        }
        if active.pinned {
            self.open_tab(route, window, cx);
            return;
        }
        let page = self.page(route, window, cx);
        self.panes.active_mut().active_mut().push(page);
        self.save_tabs(cx);
        cx.notify();
    }

    fn open_tab(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.page(route, window, cx);
        let tab = self.panes.tab(page);
        self.panes.active_mut().open(tab);
        self.save_tabs(cx);
        cx.notify();
    }

    /// Brings forward the tab that shows `route`, in whichever pane, or opens
    /// it in a new active tab.
    fn reveal(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        match self.panes.find(&route) {
            Some((pane, index)) => {
                self.panes.focus(pane);
                self.select_tab(Some(index), cx)
            }
            None => self.open_tab(route, window, cx),
        }
    }

    fn open_beside(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.page(route, window, cx);
        let tab = self.panes.tab(page);
        let pane = self.panes.active_pane();
        self.panes.split(pane, Zone::Right, tab);
        self.save_tabs(cx);
        cx.notify();
    }

    /// The page of the active pane's open tab.
    fn active_page(&self) -> EntityId {
        self.panes.active().active().current().view.entity_id()
    }

    /// Runs an item command's key on the row under the pointer, else on the
    /// page on screen. A command the item lacks does nothing there.
    fn run_on_target(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let Some((item, _)) = item_menu::target(self.active_page(), cx) else {
            return;
        };
        // After this update: opening a page updates the workspace again.
        window.defer(cx, move |window, cx| {
            item_menu::run(command, &item, window, cx)
        });
    }

    /// Keeps one row of tabs per pane, for the panes there are now.
    fn track_tab_rows(&mut self) {
        let panes: Vec<PaneId> = self.panes.panes().iter().map(|pane| pane.id).collect();
        self.tab_rows.retain(|pane, _| panes.contains(pane));
        for pane in panes {
            self.tab_rows.entry(pane).or_default();
        }
    }

    fn tab_row(&self, pane: PaneId) -> tab_bar::TabRow {
        self.tab_rows.get(&pane).cloned().unwrap_or_default()
    }

    /// Forgets the row under the pointer when a pane shows another page.
    fn track_shown_pages(&mut self, cx: &mut Context<Self>) {
        let shown: Vec<EntityId> = self
            .panes
            .panes()
            .iter()
            .map(|pane| pane.tabs.active().current().view.entity_id())
            .collect();
        if shown != self.shown_pages {
            self.shown_pages = shown;
            item_menu::forget_hovered(cx);
        }
    }

    fn open_background_tab(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.page(route, window, cx);
        let tab = self.panes.tab(page);
        self.panes.active_mut().open_in_background(tab);
        self.save_tabs(cx);
        cx.notify();
    }

    /// Back (-1) or forward (1) in the active tab.
    fn step(&mut self, direction: isize, cx: &mut Context<Self>) {
        let tab = self.panes.active_mut().active_mut();
        let moved = if direction < 0 {
            tab.go_back()
        } else {
            tab.go_forward()
        };
        if moved {
            self.save_tabs(cx);
            cx.notify();
        }
    }

    /// Closes the tab at `index` of `pane`; the last tab of a pane closes the
    /// pane.
    fn close_tab(
        &mut self,
        pane: PaneId,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.panes.close(pane, index) {
            // The window's last tab stays open; it goes back to the inbox
            // instead.
            let page = self.page(Route::Inbox, window, cx);
            self.panes.active_mut().active_mut().reset(page);
        }
        self.save_tabs(cx);
        cx.notify();
    }

    fn close_tab_id(&mut self, tab: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((pane, index)) = self.panes.locate(tab) {
            self.close_tab(pane, index, window, cx);
        }
    }

    /// Selects a tab of the active pane, or its last one for `None`.
    fn select_tab(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let tabs = self.panes.active_mut();
        match index {
            Some(index) => tabs.select(index),
            None => tabs.select_last(),
        }
        self.save_tabs(cx);
        cx.notify();
    }

    /// Selects tab `tab` and makes its pane the active one.
    fn select_tab_id(&mut self, tab: u64, cx: &mut Context<Self>) {
        if let Some((pane, index)) = self.panes.locate(tab) {
            self.panes.focus(pane);
            self.select_tab(Some(index), cx);
        }
    }

    fn cycle_tab(&mut self, step: isize, cx: &mut Context<Self>) {
        self.panes.active_mut().cycle(step);
        self.save_tabs(cx);
        cx.notify();
    }

    fn focus_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if self.panes.focus(pane) {
            self.save_tabs(cx);
            cx.notify();
        }
    }

    /// The pane and the tab being dragged, while a tab is.
    fn lifted_tab(&self, cx: &App) -> Option<(PaneId, u64)> {
        self.tab_drag
            .as_ref()
            .filter(|_| cx.has_active_drag())
            .map(|drag| (drag.pane, drag.tab))
    }

    /// A tab dropped on a row of tabs: on the tab at `index`, whose place it
    /// takes, or after the last one for `None`.
    fn drop_on_row(
        &mut self,
        dragged: &DraggedTab,
        pane: PaneId,
        index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.tab_drag = None;
        self.panes.move_tab(dragged.pane, dragged.tab, pane, index);
        self.save_tabs(cx);
        cx.notify();
    }

    /// A tab dropped on the name of group `group` of `pane`: it joins the
    /// group, at its end.
    fn drop_on_group(
        &mut self,
        dragged: &DraggedTab,
        pane: PaneId,
        group: u64,
        cx: &mut Context<Self>,
    ) {
        self.tab_drag = None;
        self.panes
            .move_into_group(dragged.pane, dragged.tab, pane, group);
        self.save_tabs(cx);
        cx.notify();
    }

    /// Runs `edit` on the tabs of `pane`, and saves them when it says it
    /// changed something.
    fn edit_tabs(
        &mut self,
        pane: PaneId,
        edit: impl FnOnce(&mut Tabs<AnyView>) -> bool,
        cx: &mut Context<Self>,
    ) {
        let Some(tabs) = self.panes.tabs_mut(pane) else {
            return;
        };
        if edit(tabs) {
            self.save_tabs(cx);
            cx.notify();
        }
    }

    /// Runs `edit` on the tabs of the pane that holds tab `tab`, with the
    /// tab's index there.
    fn edit_tab(
        &mut self,
        tab: u64,
        edit: impl FnOnce(&mut Tabs<AnyView>, usize) -> bool,
        cx: &mut Context<Self>,
    ) {
        if let Some((pane, index)) = self.panes.locate(tab) {
            self.edit_tabs(pane, |tabs| edit(tabs, index), cx);
        }
    }

    /// The zone of `pane`'s page under the pointer, if the pointer is on it.
    fn zone_under_pointer(&self, pane: PaneId, window: &Window) -> Option<Zone> {
        let bounds = *self.pane_bounds.borrow().get(&pane)?;
        let at = window.mouse_position();
        if !bounds.contains(&at) || bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return None;
        }
        let x = f32::from(at.x - bounds.left()) / f32::from(bounds.size.width);
        let y = f32::from(at.y - bounds.top()) / f32::from(bounds.size.height);
        Some(Zone::at(x, y))
    }

    /// A tab dropped on `pane`'s page: into the pane in the middle, beside
    /// it in a new pane near an edge.
    fn drop_on_page(
        &mut self,
        dragged: &DraggedTab,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_drag = None;
        if let Some(zone) = self.zone_under_pointer(pane, window) {
            self.panes.drop_tab(dragged.pane, dragged.tab, pane, zone);
            self.save_tabs(cx);
        }
        cx.notify();
    }

    /// Puts tab `tab` in a new pane on `zone`'s side of its pane. The tab
    /// moves when its pane has others; when it is the pane's only tab, its
    /// page opens again in the new pane.
    fn split_tab(&mut self, tab: u64, zone: Zone, window: &mut Window, cx: &mut Context<Self>) {
        let Some((pane, index)) = self.panes.locate(tab) else {
            return;
        };
        let Some(tabs) = self.panes.pane(pane).map(|pane| &pane.tabs) else {
            return;
        };
        if tabs.len() > 1 {
            self.panes.drop_tab(pane, tab, pane, zone);
        } else {
            let route = tabs
                .iter()
                .nth(index)
                .map(|tab| tab.current().route.clone())
                .unwrap_or(Route::Inbox);
            let page = self.page(route, window, cx);
            let copy = self.panes.tab(page);
            self.panes.split(pane, zone, copy);
        }
        self.save_tabs(cx);
        cx.notify();
    }

    fn split_active(&mut self, zone: Zone, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.panes.active().active().id;
        self.split_tab(tab, zone, window, cx);
    }

    fn drag_divider(
        &mut self,
        split: u64,
        axis: SplitAxis,
        event: &DragMoveEvent<ResizePanes>,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).0 != split {
            return;
        }
        let bounds = event.bounds;
        let at = event.event.position;
        let ratio = match axis {
            SplitAxis::Horizontal => {
                f32::from(at.x - bounds.left()) / f32::from(bounds.size.width).max(1.)
            }
            SplitAxis::Vertical => {
                f32::from(at.y - bounds.top()) / f32::from(bounds.size.height).max(1.)
            }
        };
        self.resizing_panes = true;
        if self.panes.set_ratio(split, ratio) {
            cx.notify();
        }
    }

    /// Ends a drag: saves the panes' shares once a divider is released, and
    /// forgets the dragged tab.
    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resizing_panes) {
            self.save_tabs(cx);
        }
        if self.tab_drag.take().is_some() {
            cx.notify();
        }
    }

    /// Saves the panes and the page each tab shows, unless nothing changed.
    fn save_tabs(&self, cx: &mut Context<Self>) {
        let (layout, active) = self.panes.layout();
        let settings = Settings::get(cx);
        if settings.panes.as_ref() == Some(&layout) && settings.active_pane == active {
            return;
        }
        store::update_settings(cx, move |settings| {
            settings.panes = Some(layout);
            settings.active_pane = active;
        });
    }

    /// Pages whatever scrolls under the middle of the active pane, as a
    /// wheel there would, so every screen scrolls from the keyboard (and from
    /// UI agents, which cannot scroll GPUI content through accessibility on
    /// macOS) without wiring each scroll container. `direction` is 1 for down.
    fn scroll_page(&mut self, direction: f32, window: &mut Window, cx: &mut Context<Self>) {
        let size = window.viewport_size();
        let sidebar = sidebar::width(
            Settings::get(cx).sidebar_collapsed,
            self.sidebar_expanded(cx),
        );
        // The active pane's page once it has been drawn, else the middle of
        // the content.
        let active = self.panes.active_pane();
        let position = self
            .pane_bounds
            .borrow()
            .get(&active)
            .map(|bounds| bounds.center())
            .unwrap_or_else(|| point(sidebar + (size.width - sidebar) / 2., size.height / 2.));
        let page = size.height * 0.8;
        let event = ScrollEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), page * -direction)),
            ..ScrollEvent::default()
        };
        window.dispatch_event(PlatformInput::Scroll(event), cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        store::update_settings(cx, |settings| {
            settings.sidebar_collapsed = !settings.sidebar_collapsed;
        });
    }

    /// The expanded sidebar's width in points: the one being dragged, else
    /// the saved one.
    fn sidebar_expanded(&self, cx: &App) -> f32 {
        sidebar::expanded_width(self.sidebar_drag.or(Settings::get(cx).sidebar_width))
    }

    fn drag_sidebar(
        &mut self,
        event: &DragMoveEvent<ResizeSidebar>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dragged = event.event.position.x - event.bounds.left();
        let points = f32::from(dragged) / Settings::get(cx).zoom.factor();
        let width = sidebar::expanded_width(Some(points.round()));
        if self.sidebar_drag != Some(width) {
            self.sidebar_drag = Some(width);
            cx.notify();
        }
    }

    fn finish_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(width) = self.sidebar_drag.take() {
            store::update_settings(cx, move |settings| settings.sidebar_width = Some(width));
        }
    }

    /// The band on the expanded sidebar's right edge: drag to resize, double
    /// click to restore the default width.
    fn sidebar_edge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        resizable::resize_handle::<ResizeSidebar, ResizeSidebar>("sidebar-edge", Axis::Horizontal)
            .inside(HandleEdge::Trailing)
            .with_appearance(sidebar_edge_appearance())
            .role(Role::Splitter)
            .aria_label("Resize sidebar")
            .aria_description("Drag to resize. Double-click to restore the default width.")
            .debug_selector(|| "sidebar-edge".into())
            .on_drag(ResizeSidebar, |_, _, _, cx| cx.new(|_| ResizeSidebar))
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.sidebar_drag = None;
                    store::update_settings(cx, |settings| settings.sidebar_width = None);
                }
            }))
    }

    /// The top bar with the tabs over the sidebar and the panes. The sidebar
    /// sits on the window glass and each pane's page on an inset card with
    /// its own tint, after Zeron.
    fn frame(
        &self,
        viewer: &loupe_github::Viewer,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.panes.active().active().current().route.clone();
        let collapsed = Settings::get(cx).sidebar_collapsed;
        let top_bar = tab_bar::render(self, cx);
        let expanded = self.sidebar_expanded(cx);
        let sidebar = sidebar::render(&current, viewer, &self.sidebar, collapsed, expanded, cx);
        let sidebar = div()
            .relative()
            .h_full()
            .flex_none()
            .child(sidebar)
            .when(!collapsed, |sidebar| {
                // A drag ends at the mouse up, wherever in the window it lands;
                // both capture listeners together see every one.
                sidebar
                    .child(self.sidebar_edge(cx))
                    .on_drag_move(cx.listener(Self::drag_sidebar))
                    .capture_any_mouse_up(
                        cx.listener(|this, _, _, cx| this.finish_sidebar_drag(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.finish_sidebar_drag(cx)),
                    )
            })
            .into_any_element();
        let lifted = self.lifted_tab(cx);
        let mut number = 0;
        let panes = self.node(self.panes.root(), lifted, &mut number, window, cx);
        v_flex().size_full().child(top_bar).child(
            h_flex().flex_1().min_h_0().w_full().child(sidebar).child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .pr(CARD_INSET)
                    .pb(CARD_INSET)
                    .child(panes),
            ),
        )
    }

    /// A pane's card, or a split's two sides with the divider between them.
    /// `number` counts the panes drawn so far, for their names.
    fn node(
        &self,
        node: &Node<AnyView>,
        lifted: Option<(PaneId, u64)>,
        number: &mut usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let split = match node {
            Node::Pane(pane) => {
                *number += 1;
                return self.pane_card(pane, *number, lifted, window, cx);
            }
            Node::Split(split) => split,
        };
        let first = self.node(&split.first, lifted, number, window, cx);
        let second = self.node(&split.second, lifted, number, window, cx);
        let (id, axis, ratio) = (split.id, split.axis, split.ratio);
        let horizontal = axis == SplitAxis::Horizontal;
        let handle = resizable::resize_handle::<ResizePanes, ResizePanes>(
            ("pane-divider", id),
            if horizontal {
                Axis::Horizontal
            } else {
                Axis::Vertical
            },
        )
        .inside(HandleEdge::Leading)
        .with_appearance(sidebar_edge_appearance())
        .role(Role::Splitter)
        .aria_label("Resize panes")
        .aria_description("Drag to resize. Double-click to share the space evenly.")
        .debug_selector(move || format!("pane-divider-{id}"))
        .on_drag(ResizePanes(id), |drag, _, _, cx| cx.new(|_| drag.clone()))
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 && this.panes.set_ratio(id, 0.5) {
                this.save_tabs(cx);
                cx.notify();
            }
        }));
        let divider = div()
            .relative()
            .flex_none()
            .when(horizontal, |divider| divider.w(CARD_INSET).h_full())
            .when(!horizontal, |divider| divider.h(CARD_INSET).w_full())
            .child(handle);
        let first = div()
            .flex_none()
            .min_w_0()
            .min_h_0()
            .when(horizontal, |side| side.w(relative(ratio)).h_full())
            .when(!horizontal, |side| side.h(relative(ratio)).w_full())
            .child(first);
        let second = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(horizontal, |side| side.h_full())
            .when(!horizontal, |side| side.w_full())
            .child(second);
        let container = if horizontal { h_flex() } else { v_flex() };
        container
            .id(("pane-split", id))
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<ResizePanes>, _, cx| {
                    this.drag_divider(id, axis, event, cx)
                }),
            )
            .child(first)
            .child(divider)
            .child(second)
            .into_any_element()
    }

    /// One pane: its row of tabs once the window is split, then its page,
    /// which a dragged tab covers with the zones it can drop on.
    fn pane_card(
        &self,
        pane: &Pane<AnyView>,
        number: usize,
        lifted: Option<(PaneId, u64)>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane_id = pane.id;
        let split = self.panes.pane_count() > 1;
        let active = pane_id == self.panes.active_pane();
        let view = pane.tabs.active().current().view.clone();
        let strip = split.then(|| {
            let row = self.tab_row(pane_id);
            tab_bar::pane_strip(pane, &row, number, active, lifted, cx)
        });
        let bounds = self.pane_bounds.clone();
        let record_bounds = canvas(
            move |drawn, _, _| {
                bounds.borrow_mut().insert(pane_id, drawn);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let overlay = lifted.map(|(from, tab)| self.drop_zones(pane_id, from, tab, window, cx));
        let theme = cx.theme();
        let page = v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .debug_selector(move || format!("pane-{pane_id}"))
            .child(view)
            .child(record_bounds)
            .children(overlay);
        v_flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .rounded(theme.radius * 2.)
            .border_1()
            .border_color(theme.border)
            .bg(tokens::panel())
            // A press anywhere in a pane makes it the one the keys, the
            // navigation buttons, and new tabs act on.
            .capture_any_mouse_down(
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.focus_pane(pane_id, cx)),
            )
            .children(strip)
            .child(page)
            .into_any_element()
    }

    /// The layer over `pane`'s page while tab `tab` of pane `from` is
    /// dragged: it takes the drop, and lights the zone under the pointer,
    /// half the page beside an edge or the whole page in the middle.
    fn drop_zones(
        &self,
        pane: PaneId,
        from: PaneId,
        tab: u64,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zone = self
            .zone_under_pointer(pane, window)
            .filter(|zone| self.panes.can_drop(from, tab, pane, *zone));
        let theme = cx.theme();
        let accent = tokens::accent();
        let lit = zone.map(|zone| {
            let area = div()
                .absolute()
                .m_1()
                .rounded(theme.radius * 2.)
                .bg(accent.opacity(0.12))
                .border_2()
                .border_color(accent.opacity(0.6))
                .debug_selector(move || format!("drop-zone-{zone:?}"));
            match zone {
                Zone::Center => area.inset_0(),
                Zone::Left => area.top_0().bottom_0().left_0().w(relative(0.5)),
                Zone::Right => area.top_0().bottom_0().right_0().w(relative(0.5)),
                Zone::Up => area.left_0().right_0().top_0().h(relative(0.5)),
                Zone::Down => area.left_0().right_0().bottom_0().h(relative(0.5)),
            }
        });
        div()
            .id(("drop-zones", pane))
            .absolute()
            .inset_0()
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_on_page(dragged, pane, window, cx)
            }))
            .children(lit)
            .into_any_element()
    }
}

/// gpui-component's divider without its resting hairline, which would double
/// the content card's border right beside it: the indicator shows only while
/// the pointer is on the edge.
fn sidebar_edge_appearance() -> ResizeHandleRenderer {
    let stock = resizable::resize_handle_appearance();
    Rc::new(move |handle, window, cx| {
        // Rendered even when hidden, so the indicator's transitions keep
        // sampling and the next hover animates from rest.
        let painted = stock(handle, window, cx);
        match handle.state() {
            ResizeHandleState::Idle => Some(div().into_any_element()),
            _ => painted,
        }
    })
}

/// An icon from the bundled Lucide catalog.
pub(crate) fn svg_icon(name: IconName, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(name.path())
        .flex_none()
        .size(size)
        .text_color(color)
}

/// A full-window splash with a draggable title strip, for the checking state.
fn splash(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .child(
            div()
                .id("splash-drag")
                .h(TITLE_BAR_HEIGHT)
                .w_full()
                .flex_none()
                .window_drag_area(),
        )
        .child(
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .text_size(scale::heading())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Loupe"),
                )
                .child(
                    div()
                        .text_size(scale::meta())
                        .text_color(theme.muted_foreground)
                        .child("Checking GitHub…"),
                ),
        )
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.track_shown_pages(cx);
        self.track_tab_rows();
        let github = Accounts::get(cx).github.clone();
        let body = match &github {
            Connection::Connected(viewer) => {
                self.sign_in = None;
                self.frame(viewer, window, cx).into_any_element()
            }
            Connection::Checking => splash(cx).into_any_element(),
            Connection::Disconnected | Connection::Failed(_) => self
                .sign_in
                .get_or_insert_with(|| settings::sign_in_view(window, cx))
                .clone()
                .into_any_element(),
        };
        let signed_in = matches!(github, Connection::Connected(_));
        let theme = cx.theme();
        // No background: gpui-component's root already paints the theme's
        // glass tint under every window.
        div()
            .size_full()
            .text_color(theme.foreground)
            .key_context("Workspace")
            .track_focus(&self.focus)
            .when(signed_in, |root| {
                root.on_action(cx.listener(|this, _: &GoBack, _, cx| this.step(-1, cx)))
                    .on_action(cx.listener(|this, _: &GoForward, _, cx| this.step(1, cx)))
                    .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                        this.open_tab(Route::Inbox, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                        let pane = this.panes.active_pane();
                        let active = this.panes.active().active_index();
                        this.close_tab(pane, active, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                        this.split_active(Zone::Right, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SplitLeft, window, cx| {
                        this.split_active(Zone::Left, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SplitUp, window, cx| {
                        this.split_active(Zone::Up, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                        this.split_active(Zone::Down, window, cx)
                    }))
                    // A drag ends at the mouse up, wherever in the window it
                    // lands; both listeners together see every one.
                    .capture_any_mouse_up(cx.listener(|this, _, _, cx| this.finish_drag(cx)))
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.finish_drag(cx)),
                    )
                    .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle_tab(1, cx)))
                    .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.cycle_tab(-1, cx)))
                    .on_action(
                        cx.listener(|this, _: &ActivateTab1, _, cx| this.select_tab(Some(0), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab2, _, cx| this.select_tab(Some(1), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab3, _, cx| this.select_tab(Some(2), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab4, _, cx| this.select_tab(Some(3), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab5, _, cx| this.select_tab(Some(4), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab6, _, cx| this.select_tab(Some(5), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab7, _, cx| this.select_tab(Some(6), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateTab8, _, cx| this.select_tab(Some(7), cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &ActivateLastTab, _, cx| this.select_tab(None, cx)),
                    )
                    .on_action(cx.listener(Self::toggle_sidebar))
                    .on_action(cx.listener(|this, _: &ScrollPageDown, window, cx| {
                        this.scroll_page(1., window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ScrollPageUp, window, cx| {
                        this.scroll_page(-1., window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                        let target = item_menu::target(this.active_page(), cx);
                        palette::toggle(target, this.panes.open_tabs(), window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SearchTabs, window, cx| {
                        palette::toggle_tabs(this.panes.open_tabs(), window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &OpenItemInNewTab, window, cx| {
                        this.run_on_target(Command::OpenInNewTab, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &OpenItemSideBySide, window, cx| {
                        this.run_on_target(Command::OpenSideBySide, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CopyItemLink, window, cx| {
                        this.run_on_target(Command::CopyLink, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CopyItemId, window, cx| {
                        this.run_on_target(Command::CopyId, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CopyItemBranch, window, cx| {
                        this.run_on_target(Command::CopyBranch, window, cx)
                    }))
                    // An issue's page answers S, P, A, and L itself; here
                    // they act on the row under the pointer.
                    .on_action(
                        cx.listener(|this, _: &crate::linear::ChangeState, window, cx| {
                            this.run_on_target(Command::Field(Field::Status), window, cx)
                        }),
                    )
                    .on_action(cx.listener(
                        |this, _: &crate::linear::ChangePriority, window, cx| {
                            this.run_on_target(Command::Field(Field::Priority), window, cx)
                        },
                    ))
                    .on_action(cx.listener(
                        |this, _: &crate::linear::ChangeAssignee, window, cx| {
                            this.run_on_target(Command::Field(Field::Assignee), window, cx)
                        },
                    ))
                    .on_action(
                        cx.listener(|this, _: &crate::linear::ChangeLabels, window, cx| {
                            this.run_on_target(Command::Field(Field::Labels), window, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &crate::linear::SetProject, window, cx| {
                            this.run_on_target(Command::Field(Field::Project), window, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &SwitchWorkspace, window, cx| {
                        workspace_menu::show(this, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ComposeIssue, window, cx| {
                        let team_id = match &this.panes.active().active().current().route {
                            Route::LinearTeam { team_id, .. } => Some(team_id.clone()),
                            _ => None,
                        };
                        crate::linear::compose_issue(team_id, window, cx);
                    }))
            })
            .child(body)
            .children(self.link_menu.as_ref().map(links::FloatingMenu::render))
            .children(
                self.workspace_menu
                    .as_ref()
                    .map(links::FloatingMenu::render),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Entity, InputEvent as _, Modifiers, MouseButton,
        MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollEvent,
        TestAppContext, VisualTestContext, WindowBounds, WindowOptions, point, px, size,
    };

    use super::Workspace;
    use crate::route::Route;
    use crate::services::{Accounts, Connection, Services};
    use crate::shell::actions::{ActivateTab1, CloseTab, NewTab, SplitDown, SplitRight};
    use crate::store::{PaneLayout, SavedTabGroup, Settings, SplitAxis, TabGroupColor};

    /// The real window: GitHub signed in with no client, so nothing loads
    /// from the network, on one tab showing Reviews.
    fn open_workspace(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts {
                github: Connection::Connected(loupe_github::Viewer {
                    login: "octocat".into(),
                    name: None,
                    avatar_url: String::new(),
                }),
                ..Accounts::default()
            });
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Workspace::new(Route::Inbox, window, cx)),
            )
            .expect("open the workspace window")
        })
    }

    /// Clicks the middle of the element with `selector` the way the platform
    /// does: move there, press, release, with a frame between each step so
    /// hit testing sees the pointer.
    fn click(
        cx: &mut VisualTestContext,
        selector: &'static str,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let position = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
            .center();
        let events = [
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }
            .to_platform_input(),
            MouseDownEvent {
                position,
                button,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            MouseUpEvent {
                position,
                button,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
        ];
        for event in events {
            cx.update(|window, cx| {
                window.dispatch_event(event, cx);
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        cx.run_until_parked();
    }

    fn dispatch(cx: &mut VisualTestContext, event: gpui_kit::PlatformInput) {
        cx.update(|window, cx| {
            window.dispatch_event(event, cx);
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    fn center(cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
            .center()
    }

    /// Presses at `start`, moves to `end` in small steps, and releases there.
    fn drag(cx: &mut VisualTestContext, start: Point<Pixels>, end: Point<Pixels>) {
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
        for step in 1..=10 {
            let at = start + (end - start) * (step as f32 / 10.);
            dispatch(
                cx,
                MouseMoveEvent {
                    position: at,
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
        cx.run_until_parked();
    }

    /// Presses on the middle of the sidebar's edge, moves `by` to the right
    /// in small steps, and releases there. Returns where it released.
    fn drag_sidebar_edge(cx: &mut VisualTestContext, by: f32) -> f32 {
        let start = center(cx, "sidebar-edge");
        let end = point(start.x + px(by), start.y);
        drag(cx, start, end);
        f32::from(end.x)
    }

    fn saved_width(cx: &mut VisualTestContext) -> Option<f32> {
        cx.update(|_, cx| Settings::get(cx).sidebar_width)
    }

    #[gpui_kit::test]
    fn dragging_the_sidebar_edge_saves_the_width_when_released(cx: &mut TestAppContext) {
        let (handle, _) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);

        let released = drag_sidebar_edge(cx, 120.);
        assert_eq!(
            saved_width(cx),
            Some(released.round()),
            "the edge follows the pointer, and is saved once released"
        );

        drag_sidebar_edge(cx, -400.);
        assert_eq!(saved_width(cx), Some(180.), "no narrower than the minimum");

        drag_sidebar_edge(cx, 600.);
        assert_eq!(saved_width(cx), Some(420.), "no wider than the maximum");
    }

    #[gpui_kit::test]
    fn double_clicking_the_sidebar_edge_restores_the_default_width(cx: &mut TestAppContext) {
        let (handle, _) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        drag_sidebar_edge(cx, 100.);
        assert!(saved_width(cx).is_some());

        let position = cx
            .debug_bounds("sidebar-edge")
            .expect("the sidebar edge is on screen")
            .center();
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
        for click_count in [1, 2] {
            dispatch(
                cx,
                MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    modifiers: none,
                    click_count,
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
                    click_count,
                }
                .to_platform_input(),
            );
        }
        assert_eq!(saved_width(cx), None);
    }

    fn tabs(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (Vec<Route>, usize) {
        workspace.read_with(cx, |workspace, _| {
            let tabs = workspace.panes.active();
            (tabs.routes(), tabs.active_index())
        })
    }

    #[gpui_kit::test]
    fn middle_and_cmd_clicks_open_background_tabs(cx: &mut TestAppContext) {
        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);

        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        assert_eq!(
            tabs(&workspace, cx),
            (vec![Route::Inbox, Route::Notifications], 0),
            "a middle click on a row opens its page in a new tab and keeps the current one"
        );

        click(
            cx,
            "sidebar-my-issues",
            MouseButton::Left,
            Modifiers::command(),
        );
        assert_eq!(
            tabs(&workspace, cx),
            (
                vec![Route::Inbox, Route::Notifications, Route::LinearMyIssues],
                0
            ),
            "a Cmd-click does the same, after the tab opened before it"
        );

        click(
            cx,
            "sidebar-my-issues",
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(
            tabs(&workspace, cx),
            (
                vec![
                    Route::LinearMyIssues,
                    Route::Notifications,
                    Route::LinearMyIssues
                ],
                0
            ),
            "a plain click still navigates the current tab"
        );
    }

    /// Where tab `id` was drawn, after two frames: the row of tabs follows
    /// its open tab on the frame after it was laid out.
    fn tab_bounds(cx: &mut VisualTestContext, id: u64) -> Bounds<Pixels> {
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        let selector: &'static str = format!("tab-{id}").leak();
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
    }

    fn in_view(row: Bounds<Pixels>, tab: Bounds<Pixels>) -> bool {
        row.left() <= tab.left() && tab.right() <= row.right()
    }

    #[gpui_kit::test]
    fn a_full_row_of_tabs_scrolls_and_keeps_the_open_tab_and_new_tab_in_view(
        cx: &mut TestAppContext,
    ) {
        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        for _ in 0..15 {
            cx.dispatch_action(NewTab);
            cx.run_until_parked();
        }
        assert_eq!(tabs(&workspace, cx).1, 15, "each new tab is the open one");

        let last = tab_bounds(cx, 15);
        let row = cx
            .debug_bounds("tabs-0")
            .expect("the row of tabs is on screen");
        for id in 0..16 {
            assert!(
                tab_bounds(cx, id).size.width >= px(128.),
                "tab {id} keeps a readable width instead of shrinking away"
            );
        }
        assert!(in_view(row, last), "the open tab, the last one, is in view");
        let plus = cx.debug_bounds("new-tab").expect("+ is on screen");
        assert!(
            plus.left() >= row.right() && plus.right() <= px(1280.),
            "+ stays after the tabs, inside the window"
        );

        cx.simulate_resize(size(px(1000.), px(820.)));
        cx.run_until_parked();
        let last = tab_bounds(cx, 15);
        let row = cx
            .debug_bounds("tabs-0")
            .expect("the row of tabs is on screen");
        assert!(
            in_view(row, last),
            "a narrower window keeps the open tab in view"
        );

        cx.dispatch_action(ActivateTab1);
        cx.run_until_parked();
        let first = tab_bounds(cx, 0);
        assert!(in_view(row, first), "Cmd+1 scrolls the first tab into view");

        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        assert_eq!(tabs(&workspace, cx).0.len(), 17);
        assert_eq!(
            tab_bounds(cx, 0),
            first,
            "a tab opened in the background leaves the row where it is"
        );

        let none = Modifiers::none();
        dispatch(
            cx,
            MouseMoveEvent {
                position: row.center(),
                pressed_button: None,
                modifiers: none,
            }
            .to_platform_input(),
        );
        dispatch(
            cx,
            ScrollEvent {
                position: row.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                modifiers: none,
                ..ScrollEvent::default()
            }
            .to_platform_input(),
        );
        assert_eq!(
            tab_bounds(cx, 0).left(),
            first.left() - px(200.),
            "the wheel scrolls the row sideways"
        );
    }

    #[gpui_kit::test]
    fn middle_click_on_a_tab_closes_it(cx: &mut TestAppContext) {
        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        assert_eq!(tabs(&workspace, cx).0.len(), 2);

        // The tab opened in the background has id 1.
        click(cx, "tab-1", MouseButton::Middle, Modifiers::none());
        assert_eq!(tabs(&workspace, cx), (vec![Route::Inbox], 0));
    }

    fn saved_panes(cx: &mut VisualTestContext) -> (PaneLayout, usize) {
        cx.update(|_, cx| {
            let settings = Settings::get(cx);
            (settings.pane_layout(), settings.active_pane)
        })
    }

    fn one_pane(tabs: &[Route]) -> PaneLayout {
        PaneLayout::Pane {
            tabs: tabs.to_vec(),
            active: 0,
            groups: Vec::new(),
            pinned: 0,
        }
    }

    #[gpui_kit::test]
    fn dragging_a_tab_to_a_side_of_the_page_splits_it_there(cx: &mut TestAppContext) {
        for (side, axis) in [
            ("right", SplitAxis::Horizontal),
            ("bottom", SplitAxis::Vertical),
        ] {
            let (handle, _) = open_workspace(cx);
            let cx = &mut VisualTestContext::from_window(handle, cx);
            // A second tab, opened in the background: id 1.
            click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());

            let start = center(cx, "tab-1");
            let page = cx.debug_bounds("pane-0").expect("the page is on screen");
            let end = match side {
                "right" => point(page.right() - px(24.), page.center().y),
                _ => point(page.center().x, page.bottom() - px(24.)),
            };
            drag(cx, start, end);
            assert_eq!(
                saved_panes(cx),
                (
                    PaneLayout::Split {
                        axis,
                        ratio: 0.5,
                        first: Box::new(one_pane(&[Route::Inbox])),
                        second: Box::new(one_pane(&[Route::Notifications])),
                    },
                    1
                ),
                "dropped near the page's {side} edge, the tab gets its own pane there, active"
            );
            assert!(
                cx.debug_bounds("pane-1").is_some(),
                "the new pane is drawn with its page"
            );
        }
    }

    #[gpui_kit::test]
    fn a_tab_dropped_in_the_middle_of_its_own_page_stays(cx: &mut TestAppContext) {
        let (handle, _) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        let start = center(cx, "tab-1");
        let end = cx.debug_bounds("pane-0").unwrap().center();
        drag(cx, start, end);
        assert_eq!(
            saved_panes(cx).0,
            one_pane(&[Route::Inbox, Route::Notifications])
        );
    }

    #[gpui_kit::test]
    fn dragging_a_tab_onto_another_reorders_the_row(cx: &mut TestAppContext) {
        let (handle, _) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        click(
            cx,
            "sidebar-my-issues",
            MouseButton::Middle,
            Modifiers::none(),
        );
        let start = center(cx, "tab-0");
        let end = center(cx, "tab-2");
        drag(cx, start, end);
        assert_eq!(
            saved_panes(cx).0,
            PaneLayout::Pane {
                tabs: vec![Route::Notifications, Route::LinearMyIssues, Route::Inbox],
                active: 2,
                groups: Vec::new(),
                pinned: 0,
            },
            "the dragged tab takes the place of the one it is dropped on, and stays active"
        );
    }

    /// The saved groups of the window's one pane, as (first, len, collapsed).
    fn saved_groups(cx: &mut VisualTestContext) -> Vec<(usize, usize, bool)> {
        let PaneLayout::Pane { groups, .. } = saved_panes(cx).0 else {
            panic!("one pane")
        };
        groups
            .iter()
            .map(|group| (group.first, group.len, group.collapsed))
            .collect()
    }

    #[gpui_kit::test]
    fn a_group_s_name_collapses_it_and_takes_dropped_tabs(cx: &mut TestAppContext) {
        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        // Tabs 1 and 2, opened in the background: Inbox, Notifications,
        // My issues.
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        click(
            cx,
            "sidebar-my-issues",
            MouseButton::Middle,
            Modifiers::none(),
        );
        // What "Add to new group…" does once the name is confirmed.
        workspace.update_in(cx, |this, _, cx| {
            this.edit_tab(
                1,
                |tabs, index| tabs.new_group(index, "Review").is_some(),
                cx,
            )
        });
        assert_eq!(saved_groups(cx), [(1, 1, false)]);

        let start = center(cx, "tab-2");
        let end = center(cx, "tab-group-0-0");
        drag(cx, start, end);
        assert_eq!(
            saved_groups(cx),
            [(1, 2, false)],
            "a tab dropped on a group's name joins the group"
        );

        click(cx, "tab-0", MouseButton::Left, Modifiers::none());
        click(cx, "tab-group-0-0", MouseButton::Left, Modifiers::none());
        assert_eq!(saved_groups(cx), [(1, 2, true)]);
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("tab-1").is_none() && cx.debug_bounds("tab-2").is_none(),
            "a collapsed group hides its tabs"
        );
        assert!(cx.debug_bounds("tab-0").is_some());

        click(cx, "tab-group-0-0", MouseButton::Left, Modifiers::none());
        assert_eq!(saved_groups(cx), [(1, 2, false)]);
        assert!(
            cx.debug_bounds("tab-1").is_some(),
            "and expanding shows them"
        );

        let start = center(cx, "tab-1");
        let end = center(cx, "tab-0");
        drag(cx, start, end);
        assert_eq!(
            saved_panes(cx).0,
            PaneLayout::Pane {
                tabs: vec![Route::Notifications, Route::Inbox, Route::LinearMyIssues],
                active: 0,
                groups: vec![SavedTabGroup {
                    name: "Review".into(),
                    color: TabGroupColor::Grey,
                    collapsed: false,
                    first: 2,
                    len: 1,
                }],
                pinned: 0,
            },
            "a tab dropped on a tab outside the group leaves it"
        );
    }

    /// A pinned Reviews tab keeps its list: a pull request opened from it
    /// (what a click on a row does) and a page the sidebar opens go to a new
    /// tab right after the pinned ones, and Cmd+click still opens one in the
    /// background. The pinned tab is narrower than the others.
    #[gpui_kit::test]
    fn a_page_opened_from_a_pinned_tab_opens_in_a_new_tab(cx: &mut TestAppContext) {
        use loupe_github::{PrRef, RepoRef};

        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        // Reviews (0), and Inbox (1) in the background; Reviews pinned, as
        // "Pin tab" does.
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        workspace.update_in(cx, |this, _, cx| {
            this.edit_tab(0, |tabs, index| tabs.pin(index), cx)
        });
        assert!(
            center(cx, "tab-0").x < center(cx, "tab-1").x,
            "the pinned tab comes first"
        );
        let width = |cx: &mut VisualTestContext, selector| {
            center(cx, selector);
            f32::from(cx.debug_bounds(selector).expect("on screen").size.width)
        };
        assert!(
            width(cx, "tab-0") <= 112. && width(cx, "tab-1") >= 128.,
            "a pinned tab is narrower than the others"
        );

        let pr = Route::PullRequest(PrRef::new(RepoRef::new("octo", "app"), 7));
        cx.update(|window, cx| super::navigate(pr.clone(), window, cx));
        cx.run_until_parked();
        let pinned_pane = |tabs: Vec<Route>, active| PaneLayout::Pane {
            tabs,
            active,
            groups: Vec::new(),
            pinned: 1,
        };
        assert_eq!(
            saved_panes(cx).0,
            pinned_pane(vec![Route::Inbox, pr.clone(), Route::Notifications], 1),
            "the pull request opens in a new tab after the pinned one"
        );
        workspace.read_with(cx, |workspace, _| {
            let pinned = workspace.panes.active().iter().next().expect("a tab");
            assert!(pinned.pinned);
            assert_eq!(
                pinned.current().route,
                Route::Inbox,
                "the pinned tab still shows Reviews"
            );
            assert!(!pinned.can_go_back());
        });

        click(cx, "tab-0", MouseButton::Left, Modifiers::none());
        click(
            cx,
            "sidebar-my-issues",
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(
            saved_panes(cx).0,
            pinned_pane(
                vec![
                    Route::Inbox,
                    Route::LinearMyIssues,
                    pr.clone(),
                    Route::Notifications
                ],
                1
            ),
            "a sidebar row opened from a pinned tab opens a new tab too"
        );

        click(cx, "tab-0", MouseButton::Left, Modifiers::none());
        click(cx, "sidebar-inbox", MouseButton::Left, Modifiers::command());
        assert_eq!(
            saved_panes(cx).0,
            pinned_pane(
                vec![
                    Route::Inbox,
                    Route::LinearMyIssues,
                    pr,
                    Route::Notifications,
                    Route::Notifications
                ],
                0
            ),
            "Cmd+click opens in the background, after the last tab opened that way \
             from the pinned tab, and the pinned tab stays open"
        );

        workspace.update_in(cx, |this, _, cx| {
            this.edit_tab(0, |tabs, index| tabs.unpin(index), cx)
        });
        let PaneLayout::Pane { tabs, pinned, .. } = saved_panes(cx).0 else {
            panic!("one pane")
        };
        assert_eq!(
            (tabs[0].clone(), pinned),
            (Route::Inbox, 0),
            "unpinned in place"
        );
    }

    #[gpui_kit::test]
    fn the_split_keys_split_and_closing_a_pane_s_last_tab_closes_it(cx: &mut TestAppContext) {
        let (handle, _) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        cx.dispatch_action(SplitDown);
        cx.run_until_parked();
        assert_eq!(
            saved_panes(cx),
            (
                PaneLayout::Split {
                    axis: SplitAxis::Vertical,
                    ratio: 0.5,
                    first: Box::new(one_pane(&[Route::Inbox])),
                    second: Box::new(one_pane(&[Route::Inbox])),
                },
                1
            ),
            "a pane's only tab opens again below it"
        );
        assert!(
            center(cx, "tab-1").y > center(cx, "tab-0").y,
            "each pane has its own row of tabs, one over the other"
        );

        cx.dispatch_action(CloseTab);
        cx.run_until_parked();
        assert_eq!(saved_panes(cx), (one_pane(&[Route::Inbox]), 0));
    }

    #[gpui_kit::test]
    fn dragging_the_divider_resizes_the_panes(cx: &mut TestAppContext) {
        let (handle, _) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        cx.dispatch_action(SplitRight);
        cx.run_until_parked();
        let left = cx.debug_bounds("pane-0").unwrap().size.width;

        // The split takes the next ids after the new pane (1).
        let start = center(cx, "pane-divider-2");
        drag(cx, start, point(start.x - px(200.), start.y));
        let PaneLayout::Split { ratio, .. } = saved_panes(cx).0 else {
            panic!("still split")
        };
        assert!(ratio < 0.4, "the divider followed the pointer: {ratio}");
        assert!(
            cx.debug_bounds("pane-0").unwrap().size.width < left - px(150.),
            "the left pane is narrower"
        );
    }

    #[gpui_kit::test]
    fn the_open_page_of_every_pane_is_on_screen(cx: &mut TestAppContext) {
        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        // Pages count as on screen only in the active window.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        let views = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |workspace, _| {
                workspace
                    .panes
                    .panes()
                    .iter()
                    .flat_map(|pane| pane.tabs.iter())
                    .map(|tab| (tab.id, tab.current().view.entity_id()))
                    .collect::<Vec<_>>()
            })
        };
        let on_screen = |cx: &mut VisualTestContext| {
            let views = views(cx);
            cx.update(|_, cx| {
                views
                    .into_iter()
                    .map(|(tab, view)| (tab, super::is_on_screen(view, cx)))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            on_screen(cx),
            [(0, true), (1, false)],
            "a background tab's page is not on screen"
        );

        // The active tab moves to a pane of its own on the right; the tab
        // left behind is now its pane's open one.
        cx.dispatch_action(SplitRight);
        cx.run_until_parked();
        assert_eq!(
            on_screen(cx),
            [(1, true), (0, true)],
            "each pane shows its page"
        );
    }
    /// Cmd+Shift+A opens the palette on the open tabs of every pane; typing
    /// narrows them by title and Enter shows the tab, in its own pane.
    #[gpui_kit::test]
    fn the_tab_search_lists_every_pane_s_tabs_and_switches(cx: &mut TestAppContext) {
        use gpui_kit::base::actions::Confirm;
        use gpui_kit::component::WindowExt as _;

        use crate::shell::actions::{OpenPalette, SearchTabs};

        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        // Reviews (0) and, in the background, Inbox (1); Reviews moves to
        // a pane of its own on the right, which then shows Settings and
        // opens Reviews in the background (2).
        click(cx, "sidebar-inbox", MouseButton::Middle, Modifiers::none());
        cx.dispatch_action(SplitRight);
        cx.run_until_parked();
        cx.update(|window, cx| super::navigate(Route::Settings, window, cx));
        cx.update(|window, cx| super::open_in_new_tab(Route::Inbox, window, cx));
        // Left pane: Inbox (1). Right pane, active: Settings (0), Reviews (2).
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |workspace, _| {
                let active = workspace.panes.active_pane();
                let tab = workspace.panes.active().active();
                (active, tab.id, tab.current().route.clone())
            })
        };
        let before = shown(cx);
        assert_eq!(before.2, Route::Settings);

        // Cmd+K opens the palette from the window, which hands it the tabs.
        cx.dispatch_action(OpenPalette);
        cx.run_until_parked();
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.update(|window, cx| window.close_dialog(cx));
        cx.run_until_parked();

        cx.dispatch_action(SearchTabs);
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        let tab_rows = |cx: &mut VisualTestContext| {
            [
                (0, "palette-tab-0"),
                (1, "palette-tab-1"),
                (2, "palette-tab-2"),
                (3, "palette-tab-3"),
            ]
            .into_iter()
            .filter(|(_, selector)| cx.debug_bounds(selector).is_some())
            .map(|(id, _)| id)
            .collect::<Vec<u64>>()
        };
        // Rows are drawn top to bottom; their ids say which tab each is.
        let order = |cx: &mut VisualTestContext, ids: &[u64]| {
            let mut ids = ids.to_vec();
            ids.sort_by(|a, b| {
                let y = |cx: &mut VisualTestContext, id: u64| {
                    let selector = [
                        "palette-tab-0",
                        "palette-tab-1",
                        "palette-tab-2",
                        "palette-tab-3",
                    ][id as usize];
                    cx.debug_bounds(selector).expect("a drawn row").origin.y
                };
                y(cx, *a).partial_cmp(&y(cx, *b)).expect("finite")
            });
            ids
        };
        let rows = tab_rows(cx);
        assert_eq!(
            order(cx, &rows),
            [1, 0, 2],
            "both panes' tabs, the left pane's first"
        );

        cx.simulate_input("inb");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert_eq!(tab_rows(cx), [1], "typing narrows the tabs by title");

        cx.dispatch_action(Confirm { secondary: false });
        cx.run_until_parked();
        assert!(
            !cx.update(|window, cx| window.has_active_dialog(cx)),
            "choosing a tab closes the palette"
        );
        let after = shown(cx);
        assert_eq!(after.1, 1, "Enter shows the first match, in the left pane");
        assert_eq!(after.2, Route::Notifications);
        assert_ne!(after.0, before.0, "its pane becomes the active one");
    }

    /// With the pointer on no row, the item keys act on the page on screen:
    /// a pull request's link and branch are copied, an issue's ID is not
    /// one of its commands, and side by side opens it in a pane on the
    /// right.
    #[gpui_kit::test]
    fn the_item_keys_act_on_the_page_on_screen(cx: &mut TestAppContext) {
        use loupe_github::{PrRef, RepoRef};

        use crate::shell::actions::{CopyItemBranch, CopyItemId, CopyItemLink, OpenItemSideBySide};
        use crate::shell::item_menu::{self, Item, PrItem};

        let (handle, workspace) = open_workspace(cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        let pr = PrRef::new(RepoRef::new("acme", "api"), 7);
        cx.update(|_, cx| {
            let page = workspace.read(cx).active_page();
            item_menu::set_page_item(
                page,
                Some(Item::PullRequest(PrItem {
                    pr: pr.clone(),
                    title: "Fix the login".into(),
                    url: pr.url(),
                    head_ref: Some("fix/login".into()),
                })),
                cx,
            );
        });
        let clipboard = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        };

        cx.dispatch_action(CopyItemLink);
        assert_eq!(
            clipboard(cx).as_deref(),
            Some("https://github.com/acme/api/pull/7")
        );
        cx.dispatch_action(CopyItemBranch);
        assert_eq!(clipboard(cx).as_deref(), Some("fix/login"));
        cx.dispatch_action(CopyItemId);
        assert_eq!(
            clipboard(cx).as_deref(),
            Some("fix/login"),
            "a pull request has no ID to copy"
        );

        cx.dispatch_action(OpenItemSideBySide);
        cx.run_until_parked();
        assert_eq!(
            saved_panes(cx),
            (
                PaneLayout::Split {
                    axis: SplitAxis::Horizontal,
                    ratio: 0.5,
                    first: Box::new(one_pane(&[Route::Inbox])),
                    second: Box::new(one_pane(&[Route::PullRequest(pr)])),
                },
                1
            ),
            "the pull request opens in a new pane on the right"
        );
    }
}
