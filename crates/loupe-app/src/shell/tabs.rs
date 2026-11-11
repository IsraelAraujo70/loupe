//! The tab model: each tab has its own history with back and forward, and
//! each pane keeps its tabs in order with one active. Generic over the view
//! type so the rules are tested without GPUI.

use crate::route::Route;

pub(super) struct Page<V> {
    pub route: Route,
    pub view: V,
}

/// One tab. `index` is the page on screen; pages after it are the forward
/// history, which a new navigation drops, as in a browser.
pub(super) struct Tab<V> {
    pub id: u64,
    history: Vec<Page<V>>,
    index: usize,
}

impl<V> Tab<V> {
    pub fn new(id: u64, page: Page<V>) -> Self {
        Self {
            id,
            history: vec![page],
            index: 0,
        }
    }

    pub fn current(&self) -> &Page<V> {
        &self.history[self.index]
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    pub fn go_back(&mut self) -> bool {
        let moved = self.can_go_back();
        if moved {
            self.index -= 1;
        }
        moved
    }

    pub fn go_forward(&mut self) -> bool {
        let moved = self.can_go_forward();
        if moved {
            self.index += 1;
        }
        moved
    }

    /// Shows `page` after the current one, dropping the forward history.
    pub fn push(&mut self, page: Page<V>) {
        self.history.truncate(self.index + 1);
        self.history.push(page);
        self.index += 1;
    }

    /// Every page of the history, for rebuilding views.
    pub fn pages_mut(&mut self) -> impl Iterator<Item = &mut Page<V>> {
        self.history.iter_mut()
    }

    /// Forgets the history and shows only `page`.
    pub fn reset(&mut self, page: Page<V>) {
        self.history = vec![page];
        self.index = 0;
    }
}

/// The tabs of one pane, in order, with one active. Tab ids come from the
/// caller ([`super::panes::Panes`]), so they stay unique across panes and a
/// tab keeps its id when it moves to another one.
pub(super) struct Tabs<V> {
    tabs: Vec<Tab<V>>,
    active: usize,
    /// The active tab's id and the tab last opened in the background from
    /// it, so the next one lands after it.
    last_background: Option<(u64, u64)>,
}

impl<V> Tabs<V> {
    /// The tabs `tabs` with the one at `active` (clamped) on screen.
    /// `tabs` is not empty.
    pub fn new(tabs: Vec<Tab<V>>, active: usize) -> Self {
        debug_assert!(!tabs.is_empty(), "a pane has at least one tab");
        let active = active.min(tabs.len().saturating_sub(1));
        Self {
            tabs,
            active,
            last_background: None,
        }
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active(&self) -> &Tab<V> {
        &self.tabs[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Tab<V> {
        &mut self.tabs[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab<V>> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab<V>> {
        self.tabs.iter_mut()
    }

    /// Opens `tab` right after the active one and activates it.
    pub fn open(&mut self, tab: Tab<V>) {
        let at = self.active + 1;
        self.tabs.insert(at, tab);
        self.active = at;
    }

    /// Opens `tab` without leaving the active one, as a browser's middle
    /// click does. Tabs opened this way from the same tab stay in the order
    /// they were opened, right after it.
    pub fn open_in_background(&mut self, tab: Tab<V>) {
        let from = self.active().id;
        let after = match self.last_background {
            Some((active, last)) if active == from => self.index_of(last),
            _ => None,
        };
        let at = after.unwrap_or(self.active) + 1;
        self.last_background = Some((from, tab.id));
        self.tabs.insert(at, tab);
    }

    /// Puts `tab` at `index` (clamped to the end) and activates it.
    pub fn insert(&mut self, index: usize, tab: Tab<V>) {
        let at = index.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
    }

    /// Takes the tab at `index` out, even the last one, which leaves the
    /// pane empty for its owner to drop. The tab that takes its place is
    /// active, or the one before when it was the last.
    pub fn remove(&mut self, index: usize) -> Option<Tab<V>> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if (index < self.active || self.active >= self.tabs.len()) && self.active > 0 {
            self.active -= 1;
        }
        Some(tab)
    }

    /// Closes the tab at `index`; the last tab never closes. Activates the
    /// tab that takes its place, or the one before when it was the last.
    pub fn close(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        self.remove(index).is_some()
    }

    pub fn select(&mut self, index: usize) {
        self.active = index.min(self.tabs.len() - 1);
    }

    pub fn select_last(&mut self) {
        self.active = self.tabs.len() - 1;
    }

    /// The next tab, wrapping around; `step` is 1 or -1.
    pub fn cycle(&mut self, step: isize) {
        let len = self.tabs.len() as isize;
        self.active = (self.active as isize + step).rem_euclid(len) as usize;
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// The route each tab shows, for saving the open tabs.
    pub fn routes(&self) -> Vec<Route> {
        self.tabs
            .iter()
            .map(|tab| tab.current().route.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{Page, Tab, Tabs};
    use crate::route::Route;

    thread_local! {
        static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    }

    fn tab(route: Route) -> Tab<()> {
        let id = NEXT_ID.with(|next| next.replace(next.get() + 1));
        Tab::new(id, page(route))
    }

    fn page(route: Route) -> Page<()> {
        Page { route, view: () }
    }

    fn issue(id: &str) -> Route {
        Route::LinearIssue {
            identifier: id.into(),
        }
    }

    fn one_tab() -> Tabs<()> {
        Tabs::new(vec![tab(Route::Inbox)], 0)
    }

    #[test]
    fn each_tab_has_its_own_back_and_forward() {
        let mut tabs = one_tab();
        tabs.active_mut().push(page(Route::Notifications));
        tabs.open(tab(issue("ENG-1")));
        assert_eq!(tabs.active_index(), 1);
        assert!(
            !tabs.active().can_go_back(),
            "a new tab starts its own history"
        );

        tabs.select(0);
        assert!(tabs.active_mut().go_back());
        assert_eq!(tabs.active().current().route, Route::Inbox);
        assert!(tabs.active_mut().go_forward());
        assert_eq!(tabs.active().current().route, Route::Notifications);
    }

    #[test]
    fn navigating_drops_the_forward_history() {
        let mut tabs = one_tab();
        tabs.active_mut().push(page(Route::Notifications));
        tabs.active_mut().go_back();
        tabs.active_mut().push(page(Route::Settings));
        assert!(!tabs.active().can_go_forward());
        assert!(tabs.active_mut().go_back());
        assert_eq!(tabs.active().current().route, Route::Inbox);
    }

    #[test]
    fn closing_keeps_one_tab_and_a_sensible_active_one() {
        let mut tabs = one_tab();
        assert!(!tabs.close(0), "the last tab stays");
        tabs.open(tab(issue("A")));
        tabs.open(tab(issue("B")));
        tabs.select(0);
        assert!(tabs.close(2));
        assert_eq!(
            tabs.active_index(),
            0,
            "closing a later tab keeps the active one"
        );
        tabs.select(1);
        assert!(tabs.close(1));
        assert_eq!(
            tabs.active_index(),
            0,
            "closing the last tab activates the one before"
        );
        assert_eq!(tabs.routes(), [Route::Inbox]);
    }

    #[test]
    fn background_tabs_keep_the_active_tab_and_their_order() {
        let mut tabs = one_tab();
        tabs.open(tab(issue("A")));
        tabs.select(0);
        tabs.open_in_background(tab(issue("B")));
        tabs.open_in_background(tab(issue("C")));
        assert_eq!(tabs.active_index(), 0, "the active tab stays");
        assert_eq!(
            tabs.routes(),
            [Route::Inbox, issue("B"), issue("C"), issue("A")],
            "each lands after the one opened before it"
        );

        tabs.select(3);
        tabs.open_in_background(tab(issue("D")));
        assert_eq!(tabs.active_index(), 3);
        assert_eq!(
            tabs.routes()[4],
            issue("D"),
            "a new run starts after the active tab"
        );
    }

    #[test]
    fn select_and_cycle_wrap() {
        let mut tabs = Tabs::new(
            vec![
                tab(Route::Inbox),
                tab(Route::Notifications),
                tab(Route::Settings),
            ],
            9,
        );
        assert_eq!(
            tabs.active_index(),
            2,
            "a saved index past the end is clamped"
        );
        tabs.cycle(1);
        assert_eq!(tabs.active_index(), 0);
        tabs.cycle(-1);
        assert_eq!(tabs.active_index(), 2);
        tabs.select(0);
        tabs.select_last();
        assert_eq!(tabs.active_index(), 2);
    }
}
