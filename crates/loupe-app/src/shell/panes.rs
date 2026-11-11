//! The window's panes, as in Zed and VS Code: a tree of splits whose leaves
//! are panes, each with its own tabs, and one pane active. Generic over the
//! view type so the rules are tested without GPUI.
//!
//! A pane is never empty: taking its last tab out removes it, and its
//! sibling takes the space. The window always has one pane.

use crate::route::Route;
use crate::store::{PaneLayout, SplitAxis};

use super::tabs::{Page, Tab, Tabs};

pub(super) type PaneId = u64;

/// The smallest share a split gives either side, so a pane cannot be dragged
/// out of sight.
pub(super) const MIN_RATIO: f32 = 0.15;

/// Where a tab dropped on a pane goes: into the pane, or beside it on one
/// side, which splits the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Zone {
    Center,
    Left,
    Right,
    Up,
    Down,
}

impl Zone {
    /// The zone under `x`, `y` (0 to 1 across the pane): an edge when the
    /// point is within a quarter of the pane from it, the nearest edge first;
    /// the middle otherwise.
    pub fn at(x: f32, y: f32) -> Zone {
        let edges = [
            (x, Zone::Left),
            (1. - x, Zone::Right),
            (y, Zone::Up),
            (1. - y, Zone::Down),
        ];
        let (distance, zone) = edges
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("four edges");
        if distance < 0.25 { zone } else { Zone::Center }
    }

    fn axis(self) -> Option<SplitAxis> {
        match self {
            Zone::Center => None,
            Zone::Left | Zone::Right => Some(SplitAxis::Horizontal),
            Zone::Up | Zone::Down => Some(SplitAxis::Vertical),
        }
    }

    /// Whether the new pane comes before the one it splits.
    fn leads(self) -> bool {
        matches!(self, Zone::Left | Zone::Up)
    }
}

pub(super) struct Pane<V> {
    pub id: PaneId,
    pub tabs: Tabs<V>,
}

pub(super) struct Split<V> {
    pub id: u64,
    pub axis: SplitAxis,
    /// The first child's share of the split, from [`MIN_RATIO`] to
    /// `1 - MIN_RATIO`.
    pub ratio: f32,
    pub first: Box<Node<V>>,
    pub second: Box<Node<V>>,
}

pub(super) enum Node<V> {
    Pane(Pane<V>),
    Split(Split<V>),
}

impl<V> Node<V> {
    fn first_pane(&self) -> PaneId {
        match self {
            Node::Pane(pane) => pane.id,
            Node::Split(split) => split.first.first_pane(),
        }
    }

    fn panes<'a>(&'a self, out: &mut Vec<&'a Pane<V>>) {
        match self {
            Node::Pane(pane) => out.push(pane),
            Node::Split(split) => {
                split.first.panes(out);
                split.second.panes(out);
            }
        }
    }

    fn panes_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Pane<V>>) {
        match self {
            Node::Pane(pane) => out.push(pane),
            Node::Split(split) => {
                split.first.panes_mut(out);
                split.second.panes_mut(out);
            }
        }
    }

    fn has_split(&self, id: u64) -> bool {
        match self {
            Node::Pane(_) => false,
            Node::Split(split) => {
                split.id == id || split.first.has_split(id) || split.second.has_split(id)
            }
        }
    }

    fn split_mut(&mut self, id: u64) -> Option<&mut Split<V>> {
        let Node::Split(split) = self else {
            return None;
        };
        if split.id == id {
            return Some(split);
        }
        if split.first.has_split(id) {
            split.first.split_mut(id)
        } else {
            split.second.split_mut(id)
        }
    }

    /// The tree without pane `id`. Its sibling takes the split's place; the
    /// second value is the pane that now holds the space, when one did.
    fn without(self, id: PaneId) -> (Option<Node<V>>, Option<PaneId>) {
        match self {
            Node::Pane(pane) if pane.id == id => (None, None),
            Node::Pane(pane) => (Some(Node::Pane(pane)), None),
            Node::Split(split) => {
                let Split {
                    id: split_id,
                    axis,
                    ratio,
                    first,
                    second,
                } = split;
                let (first, heir_first) = first.without(id);
                let (second, heir_second) = second.without(id);
                match (first, second) {
                    (Some(first), Some(second)) => (
                        Some(Node::Split(Split {
                            id: split_id,
                            axis,
                            ratio,
                            first: Box::new(first),
                            second: Box::new(second),
                        })),
                        heir_first.or(heir_second),
                    ),
                    (Some(only), None) | (None, Some(only)) => {
                        let heir = heir_first.or(heir_second).unwrap_or(only.first_pane());
                        (Some(only), Some(heir))
                    }
                    (None, None) => (None, None),
                }
            }
        }
    }

    /// The tree with pane `target` split along `zone`'s side, `new` on it.
    fn split_at(self, target: PaneId, zone: Zone, new: Pane<V>, split_id: u64) -> Node<V> {
        self.split_at_inner(target, zone, &mut Some(new), split_id)
    }

    fn split_at_inner(
        self,
        target: PaneId,
        zone: Zone,
        new: &mut Option<Pane<V>>,
        split_id: u64,
    ) -> Node<V> {
        match self {
            Node::Pane(pane) if pane.id == target => {
                let (Some(axis), Some(added)) = (zone.axis(), new.take()) else {
                    return Node::Pane(pane);
                };
                let (first, second) = if zone.leads() {
                    (Node::Pane(added), Node::Pane(pane))
                } else {
                    (Node::Pane(pane), Node::Pane(added))
                };
                Node::Split(Split {
                    id: split_id,
                    axis,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                })
            }
            Node::Pane(pane) => Node::Pane(pane),
            Node::Split(mut split) => {
                split.first = Box::new(split.first.split_at_inner(target, zone, new, split_id));
                split.second = Box::new(split.second.split_at_inner(target, zone, new, split_id));
                Node::Split(split)
            }
        }
    }

    fn layout(&self) -> PaneLayout {
        match self {
            Node::Pane(pane) => PaneLayout::Pane {
                tabs: pane.tabs.routes(),
                active: pane.tabs.active_index(),
            },
            Node::Split(split) => PaneLayout::Split {
                axis: split.axis,
                ratio: split.ratio,
                first: Box::new(split.first.layout()),
                second: Box::new(split.second.layout()),
            },
        }
    }
}

pub(super) struct Panes<V> {
    /// Always `Some`; `None` only inside an operation that rebuilds the tree.
    root: Option<Node<V>>,
    active: PaneId,
    next_tab: u64,
    /// Ids of panes and splits, apart from the tabs' so a tab's id says only
    /// how many tabs opened before it.
    next_node: u64,
}

impl<V> Panes<V> {
    /// The panes of `layout`, with the pane at `active_pane` (in reading
    /// order, clamped) active. `build` makes the
    /// view of each tab's page; a pane saved with no tabs is dropped, and a
    /// window left with none gets one tab showing `first`.
    pub fn restore(
        layout: &PaneLayout,
        active_pane: usize,
        first: Route,
        mut build: impl FnMut(&Route) -> V,
    ) -> Self {
        let mut panes = Self {
            root: None,
            active: 0,
            next_tab: 0,
            next_node: 0,
        };
        let root = panes.restore_node(layout, &mut build);
        let root = match root {
            Some(root) => root,
            None => {
                let view = build(&first);
                let tab = panes.tab(Page { route: first, view });
                let id = panes.node_id();
                Node::Pane(Pane {
                    id,
                    tabs: Tabs::new(vec![tab], 0),
                })
            }
        };
        let mut all = Vec::new();
        root.panes(&mut all);
        panes.active = all[active_pane.min(all.len() - 1)].id;
        panes.root = Some(root);
        panes
    }

    fn restore_node(
        &mut self,
        layout: &PaneLayout,
        build: &mut impl FnMut(&Route) -> V,
    ) -> Option<Node<V>> {
        match layout {
            PaneLayout::Pane { tabs, active } => {
                if tabs.is_empty() {
                    return None;
                }
                let tabs = tabs
                    .iter()
                    .map(|route| {
                        let page = Page {
                            route: route.clone(),
                            view: build(route),
                        };
                        self.tab(page)
                    })
                    .collect();
                Some(Node::Pane(Pane {
                    id: self.node_id(),
                    tabs: Tabs::new(tabs, *active),
                }))
            }
            PaneLayout::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let id = self.node_id();
                match (
                    self.restore_node(first, build),
                    self.restore_node(second, build),
                ) {
                    (Some(first), Some(second)) => Some(Node::Split(Split {
                        id,
                        axis: *axis,
                        ratio: clamp_ratio(*ratio),
                        first: Box::new(first),
                        second: Box::new(second),
                    })),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }

    /// One pane with one tab showing `page`.
    pub fn single(page: Page<V>) -> Self {
        let mut panes = Self {
            root: None,
            active: 0,
            next_tab: 0,
            next_node: 0,
        };
        let tab = panes.tab(page);
        panes.active = panes.node_id();
        panes.root = Some(Node::Pane(Pane {
            id: panes.active,
            tabs: Tabs::new(vec![tab], 0),
        }));
        panes
    }

    fn node_id(&mut self) -> u64 {
        let id = self.next_node;
        self.next_node += 1;
        id
    }

    /// A new tab showing `page`, with an id no other tab has.
    pub fn tab(&mut self, page: Page<V>) -> Tab<V> {
        let id = self.next_tab;
        self.next_tab += 1;
        Tab::new(id, page)
    }

    pub fn root(&self) -> &Node<V> {
        self.root.as_ref().expect("the root is always set")
    }

    fn root_mut(&mut self) -> &mut Node<V> {
        self.root.as_mut().expect("the root is always set")
    }

    /// Every pane, in reading order: left before right, top before bottom.
    pub fn panes(&self) -> Vec<&Pane<V>> {
        let mut all = Vec::new();
        self.root().panes(&mut all);
        all
    }

    pub fn pane_count(&self) -> usize {
        self.panes().len()
    }

    pub fn active_pane(&self) -> PaneId {
        self.active
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane<V>> {
        self.panes().into_iter().find(|pane| pane.id == id)
    }

    pub fn tabs_mut(&mut self, id: PaneId) -> Option<&mut Tabs<V>> {
        let mut all = Vec::new();
        self.root_mut().panes_mut(&mut all);
        all.into_iter()
            .find(|pane| pane.id == id)
            .map(|pane| &mut pane.tabs)
    }

    /// The active pane's tabs.
    pub fn active(&self) -> &Tabs<V> {
        &self.pane(self.active).expect("the active pane exists").tabs
    }

    pub fn active_mut(&mut self) -> &mut Tabs<V> {
        let active = self.active;
        self.tabs_mut(active).expect("the active pane exists")
    }

    /// Makes `pane` the active one. False when it already was or is gone.
    pub fn focus(&mut self, pane: PaneId) -> bool {
        if pane == self.active || self.pane(pane).is_none() {
            return false;
        }
        self.active = pane;
        true
    }

    /// Every tab of every pane, for rebuilding views.
    pub fn tabs_iter_mut(&mut self) -> impl Iterator<Item = &mut Tab<V>> {
        let mut all = Vec::new();
        self.root_mut().panes_mut(&mut all);
        all.into_iter().flat_map(|pane| pane.tabs.iter_mut())
    }

    /// The pane and the index of the first tab showing `route`.
    pub fn find(&self, route: &Route) -> Option<(PaneId, usize)> {
        self.panes().into_iter().find_map(|pane| {
            pane.tabs
                .routes()
                .iter()
                .position(|open| open == route)
                .map(|index| (pane.id, index))
        })
    }

    /// The pane that holds tab `tab`, and its index there.
    pub fn locate(&self, tab: u64) -> Option<(PaneId, usize)> {
        self.panes()
            .into_iter()
            .find_map(|pane| pane.tabs.index_of(tab).map(|index| (pane.id, index)))
    }

    /// Closes the tab at `index` of `pane`. The last tab of a pane closes
    /// the pane, unless it is the window's only one: then nothing closes and
    /// this returns false.
    pub fn close(&mut self, pane: PaneId, index: usize) -> bool {
        let Some(tabs) = self.tabs_mut(pane) else {
            return false;
        };
        if tabs.len() > 1 {
            return tabs.close(index);
        }
        if index != 0 || self.pane_count() == 1 {
            return false;
        }
        self.remove_pane(pane);
        true
    }

    fn remove_pane(&mut self, pane: PaneId) {
        let root = self.root.take().expect("the root is always set");
        let (root, heir) = root.without(pane);
        self.root = Some(root.expect("a pane is removed only while another one is left"));
        if self.active == pane {
            self.active = heir.unwrap_or_else(|| self.root().first_pane());
        }
    }

    /// Takes tab `tab` out of `pane`, removing the pane when it was its last
    /// tab and another pane is left. `None` for the window's last tab.
    fn take(&mut self, pane: PaneId, tab: u64) -> Option<Tab<V>> {
        let only_pane = self.pane_count() == 1;
        let tabs = self.tabs_mut(pane)?;
        let index = tabs.index_of(tab)?;
        if tabs.len() == 1 && only_pane {
            return None;
        }
        let taken = tabs.remove(index)?;
        if tabs.is_empty() {
            self.remove_pane(pane);
        }
        Some(taken)
    }

    /// Whether dropping tab `tab` of pane `from` on `zone` of pane `to`
    /// changes anything: a pane cannot be split by its only tab, and a tab
    /// dropped in the middle of its own pane stays where it is.
    pub fn can_drop(&self, from: PaneId, tab: u64, to: PaneId, zone: Zone) -> bool {
        let Some(source) = self.pane(from) else {
            return false;
        };
        if source.tabs.index_of(tab).is_none() || self.pane(to).is_none() {
            return false;
        }
        from != to || (zone != Zone::Center && source.tabs.len() > 1)
    }

    /// Moves tab `tab` of pane `from` to `index` of pane `to` (its end when
    /// `None`), where it becomes the active tab of the now active pane. In its
    /// own pane it takes the place of the tab at `index`. False when nothing
    /// moved.
    pub fn move_tab(&mut self, from: PaneId, tab: u64, to: PaneId, index: Option<usize>) -> bool {
        if self.pane(to).is_none() {
            return false;
        }
        if from == to {
            let Some(tabs) = self.tabs_mut(to) else {
                return false;
            };
            let Some(at) = tabs.index_of(tab) else {
                return false;
            };
            let target = index.unwrap_or(tabs.len() - 1).min(tabs.len() - 1);
            let moved = tabs.remove(at).expect("the tab is in the pane");
            tabs.insert(target, moved);
            self.active = to;
            return at != target;
        }
        let Some(moved) = self.take(from, tab) else {
            return false;
        };
        let tabs = self
            .tabs_mut(to)
            .expect("the target pane is a different one");
        tabs.insert(index.unwrap_or(usize::MAX), moved);
        self.active = to;
        true
    }

    /// Drops tab `tab` of pane `from` on `zone` of pane `to`: into `to` for
    /// the middle, else into a new pane on that side of `to`, which becomes
    /// the active one. False when [`Self::can_drop`] says nothing changes.
    pub fn drop_tab(&mut self, from: PaneId, tab: u64, to: PaneId, zone: Zone) -> bool {
        if !self.can_drop(from, tab, to, zone) {
            return false;
        }
        if zone == Zone::Center {
            return self.move_tab(from, tab, to, None);
        }
        let Some(moved) = self.take(from, tab) else {
            return false;
        };
        self.split(to, zone, moved);
        true
    }

    /// Splits pane `target` on `zone`'s side with a new pane holding `tab`,
    /// and makes the new pane active. A `Center` zone opens `tab` in the
    /// pane instead.
    pub fn split(&mut self, target: PaneId, zone: Zone, tab: Tab<V>) {
        if zone == Zone::Center {
            if let Some(tabs) = self.tabs_mut(target) {
                tabs.insert(usize::MAX, tab);
                self.active = target;
            }
            return;
        }
        let pane = Pane {
            id: self.node_id(),
            tabs: Tabs::new(vec![tab], 0),
        };
        let added = pane.id;
        let split_id = self.node_id();
        let root = self.root.take().expect("the root is always set");
        self.root = Some(root.split_at(target, zone, pane, split_id));
        if self.pane(added).is_some() {
            self.active = added;
        }
    }

    /// Sets split `id`'s share for its first side. False when it did not
    /// change.
    pub fn set_ratio(&mut self, id: u64, ratio: f32) -> bool {
        let ratio = clamp_ratio(ratio);
        match self.root_mut().split_mut(id) {
            Some(split) if split.ratio != ratio => {
                split.ratio = ratio;
                true
            }
            _ => false,
        }
    }

    /// The layout to save, and the active pane's place in reading order.
    pub fn layout(&self) -> (PaneLayout, usize) {
        let active = self
            .panes()
            .iter()
            .position(|pane| pane.id == self.active)
            .unwrap_or(0);
        (self.root().layout(), active)
    }
}

fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(MIN_RATIO, 1. - MIN_RATIO)
    } else {
        0.5
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_RATIO, Node, Panes, Zone};
    use crate::route::Route;
    use crate::store::{PaneLayout, SplitAxis};

    fn issue(id: &str) -> Route {
        Route::LinearIssue {
            identifier: id.into(),
        }
    }

    fn pane(tabs: &[Route], active: usize) -> PaneLayout {
        PaneLayout::Pane {
            tabs: tabs.to_vec(),
            active,
        }
    }

    fn restore(layout: &PaneLayout, active: usize) -> Panes<()> {
        Panes::restore(layout, active, Route::Inbox, |_| ())
    }

    /// A pull request and a ticket open in one pane, the ticket active.
    fn pr_and_ticket() -> Panes<()> {
        restore(&pane(&[Route::Inbox, issue("ENG-1")], 1), 0)
    }

    fn ticket_tab(panes: &Panes<()>) -> (u64, u64) {
        let (pane, index) = panes.find(&issue("ENG-1")).expect("the ticket is open");
        let tab = panes.pane(pane).unwrap().tabs.iter().nth(index).unwrap().id;
        (pane, tab)
    }

    #[test]
    fn dropping_a_tab_on_a_side_puts_the_two_side_by_side_or_stacked() {
        for (zone, axis, ticket_first) in [
            (Zone::Right, SplitAxis::Horizontal, false),
            (Zone::Left, SplitAxis::Horizontal, true),
            (Zone::Down, SplitAxis::Vertical, false),
            (Zone::Up, SplitAxis::Vertical, true),
        ] {
            let mut panes = pr_and_ticket();
            let (from, tab) = ticket_tab(&panes);
            assert!(panes.drop_tab(from, tab, from, zone), "{zone:?} splits");
            let (first, second) = (pane(&[Route::Inbox], 0), pane(&[issue("ENG-1")], 0));
            let (first, second) = if ticket_first {
                (second, first)
            } else {
                (first, second)
            };
            assert_eq!(
                panes.layout(),
                (
                    PaneLayout::Split {
                        axis,
                        ratio: 0.5,
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                    if ticket_first { 0 } else { 1 },
                ),
                "{zone:?}: the ticket leaves the pane for a new one on that side, which is active"
            );
        }
    }

    #[test]
    fn a_pane_cannot_be_split_by_its_only_tab_nor_a_tab_dropped_on_its_own_middle() {
        let mut panes = restore(&pane(&[Route::Inbox], 0), 0);
        let (from, tab) = (panes.active_pane(), panes.active().active().id);
        for zone in [Zone::Center, Zone::Right, Zone::Down] {
            assert!(!panes.can_drop(from, tab, from, zone));
            assert!(!panes.drop_tab(from, tab, from, zone));
        }
        assert_eq!(panes.pane_count(), 1);

        let mut panes = pr_and_ticket();
        let (from, tab) = ticket_tab(&panes);
        assert!(!panes.drop_tab(from, tab, from, Zone::Center));
        assert_eq!(panes.layout().0, pane(&[Route::Inbox, issue("ENG-1")], 1));
    }

    #[test]
    fn closing_the_last_tab_of_a_pane_closes_the_pane() {
        let mut panes = pr_and_ticket();
        let (from, tab) = ticket_tab(&panes);
        panes.drop_tab(from, tab, from, Zone::Right);
        let ticket_pane = panes.active_pane();
        assert!(panes.close(ticket_pane, 0));
        assert_eq!(
            panes.layout(),
            (pane(&[Route::Inbox], 0), 0),
            "the sibling takes the space and becomes active"
        );
        assert!(
            !panes.close(panes.active_pane(), 0),
            "the window's last tab stays"
        );
    }

    #[test]
    fn a_tab_moves_between_panes_and_an_emptied_pane_goes_away() {
        let mut panes = restore(
            &PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.4,
                first: Box::new(pane(&[Route::Inbox, Route::Notifications], 0)),
                second: Box::new(pane(&[issue("ENG-1")], 0)),
            },
            0,
        );
        let left = panes.panes()[0].id;
        let (right, ticket) = ticket_tab(&panes);
        assert!(panes.move_tab(right, ticket, left, Some(1)));
        assert_eq!(
            panes.layout(),
            (
                pane(&[Route::Inbox, issue("ENG-1"), Route::Notifications], 1),
                0
            ),
            "the ticket lands where it was dropped, active, and its empty pane closes"
        );
    }

    #[test]
    fn dragging_a_tab_along_its_row_reorders_it() {
        let mut panes = restore(&pane(&[issue("A"), issue("B"), issue("C")], 0), 0);
        let id = |panes: &Panes<()>, name: &str| {
            let (_, index) = panes.find(&issue(name)).unwrap();
            panes.active().iter().nth(index).unwrap().id
        };
        let pane_id = panes.active_pane();
        let a = id(&panes, "A");
        assert!(panes.move_tab(pane_id, a, pane_id, Some(2)));
        assert_eq!(
            panes.layout().0,
            pane(&[issue("B"), issue("C"), issue("A")], 2),
            "dropped on C, A takes C's place"
        );
        let a = id(&panes, "A");
        assert!(panes.move_tab(pane_id, a, pane_id, Some(0)));
        assert_eq!(
            panes.layout().0,
            pane(&[issue("A"), issue("B"), issue("C")], 0)
        );
        assert!(
            !panes.move_tab(pane_id, a, pane_id, Some(0)),
            "dropped on itself, nothing moves"
        );
    }

    #[test]
    fn the_layout_comes_back_as_saved() {
        let saved = PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.3,
            first: Box::new(pane(&[Route::Inbox], 0)),
            second: Box::new(PaneLayout::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.6,
                first: Box::new(pane(&[issue("A"), issue("B")], 1)),
                second: Box::new(pane(&[Route::Settings], 0)),
            }),
        };
        let panes = restore(&saved, 2);
        assert_eq!(panes.layout(), (saved, 2));
        assert_eq!(
            panes.active().routes(),
            [Route::Settings],
            "the saved active pane is active again"
        );
    }

    #[test]
    fn a_saved_empty_pane_is_dropped_and_ratios_stay_in_bounds() {
        let panes = restore(
            &PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(pane(&[], 0)),
                second: Box::new(pane(&[issue("A")], 0)),
            },
            7,
        );
        assert_eq!(panes.layout(), (pane(&[issue("A")], 0), 0));

        let mut panes = pr_and_ticket();
        let (from, tab) = ticket_tab(&panes);
        panes.drop_tab(from, tab, from, Zone::Right);
        let Node::Split(split) = panes.root() else {
            panic!("the drop splits the pane")
        };
        let split = split.id;
        assert!(panes.set_ratio(split, 0.99));
        let PaneLayout::Split { ratio, .. } = panes.layout().0 else {
            unreachable!()
        };
        assert_eq!(ratio, 1. - MIN_RATIO);
    }

    #[test]
    fn a_drop_zone_is_the_nearest_edge_within_a_quarter() {
        assert_eq!(Zone::at(0.5, 0.5), Zone::Center);
        assert_eq!(Zone::at(0.9, 0.5), Zone::Right);
        assert_eq!(Zone::at(0.1, 0.5), Zone::Left);
        assert_eq!(Zone::at(0.5, 0.95), Zone::Down);
        assert_eq!(Zone::at(0.5, 0.05), Zone::Up);
        assert_eq!(Zone::at(0.9, 0.95), Zone::Down, "the nearer edge wins");
        assert_eq!(Zone::at(0.3, 0.5), Zone::Center);
    }
}
