//! The tab model: each tab has its own history with back and forward, and
//! each pane keeps its tabs in order with one active. Generic over the view
//! type so the rules are tested without GPUI.
//!
//! Tabs can be grouped, as in Chrome: a group has a name and a color, its
//! tabs sit next to each other, and it can collapse to hide them. A group
//! with no tab left is gone, and the open tab is never in a collapsed group.
//!
//! Tabs can be pinned, as in Chrome: the pinned tabs come first in their
//! pane, are never in a group, and a page they open goes to a new tab after
//! them (the shell's `push`). Every insertion goes through `place`, which
//! keeps a pinned tab among the pinned ones and any other tab after them.

use crate::route::Route;
use crate::store::{ClosedTabGroup, SavedTabGroup, TabGroupColor};

/// A named, colored group of tabs in one pane.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TabGroup {
    pub id: u64,
    pub name: String,
    pub color: TabGroupColor,
    pub collapsed: bool,
}

pub(super) struct Page<V> {
    pub route: Route,
    pub view: V,
}

/// One tab. `index` is the page on screen; pages after it are the forward
/// history, which a new navigation drops, as in a browser.
pub(super) struct Tab<V> {
    pub id: u64,
    /// The [`TabGroup`] of its pane the tab is in, if any. Always `None`
    /// while the tab is pinned.
    pub group: Option<u64>,
    /// Pinned tabs sit before the others and keep showing their page: a
    /// link followed in one opens in a new tab.
    pub pinned: bool,
    history: Vec<Page<V>>,
    index: usize,
}

impl<V> Tab<V> {
    pub fn new(id: u64, page: Page<V>) -> Self {
        Self {
            id,
            group: None,
            pinned: false,
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
    groups: Vec<TabGroup>,
    next_group: u64,
}

impl<V> Tabs<V> {
    /// The tabs `tabs` with the one at `active` (clamped) on screen, in no
    /// group. Pinned tabs move ahead of the others, each side keeping its
    /// order. `tabs` is not empty.
    pub fn new(mut tabs: Vec<Tab<V>>, active: usize) -> Self {
        debug_assert!(!tabs.is_empty(), "a pane has at least one tab");
        let active = active.min(tabs.len().saturating_sub(1));
        let active_id = tabs[active].id;
        tabs.sort_by_key(|tab| !tab.pinned);
        let active = tabs.iter().position(|tab| tab.id == active_id).unwrap_or(0);
        let mut tabs = Self {
            tabs,
            active,
            last_background: None,
            groups: Vec::new(),
            next_group: 0,
        };
        tabs.tidy();
        tabs
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

    /// Opens `tab` right after the active one, in its group, and activates
    /// it. From a pinned tab it lands right after the pinned tabs.
    pub fn open(&mut self, tab: Tab<V>) {
        let at = self.active + 1;
        let group = self.active().group;
        self.active = self.place(at, tab, group);
        self.tidy();
    }

    /// Opens `tab` without leaving the active one, as a browser's middle
    /// click does. Tabs opened this way from the same tab stay in the order
    /// they were opened, right after it.
    pub fn open_in_background(&mut self, tab: Tab<V>) {
        let from = self.active().id;
        // The last one counts only while it still follows the active tab in
        // the same group; once moved elsewhere, a new run starts.
        let group = self.active().group;
        let after = match self.last_background {
            Some((active, last)) if active == from => self
                .index_of(last)
                .filter(|&ix| ix > self.active && self.tabs[ix].group == group),
            _ => None,
        };
        let at = after.unwrap_or(self.active) + 1;
        self.last_background = Some((from, tab.id));
        let active = self.active().id;
        self.place(at, tab, group);
        self.active = self.index_of(active).unwrap_or(0);
        self.tidy();
    }

    /// Opens `tab` the way grouping by repository does: a tab whose page
    /// belongs to a GitHub repository ([`Route::repo`]) joins the group named
    /// after the repository, after its last tab, and the group is made on
    /// the spot when the pane has none by that name; any other tab opens
    /// outside every group, after the active tab, or after the group the
    /// active tab is in. `activate` puts it on screen; otherwise the active
    /// tab stays and, as with [`Self::open_in_background`], tabs opened this
    /// way from the same tab follow one another.
    pub fn open_in_repo_group(&mut self, tab: Tab<V>, activate: bool) {
        let from = self.active().id;
        let id = tab.id;
        let repo = tab.current().route.repo().map(|repo| repo.name.clone());
        let existing = repo.as_deref().and_then(|name| {
            let group = self.groups.iter().find(|group| group.name == name)?;
            Some(group.id)
        });
        match existing {
            Some(group) => {
                let last = self.end_of_group(group);
                self.place(last + 1, tab, Some(group));
            }
            None => {
                let last_background = match self.last_background {
                    Some((active, last)) if !activate && active == from => {
                        self.index_of(last).filter(|&ix| ix > self.active)
                    }
                    _ => None,
                };
                let after = last_background.unwrap_or(self.active);
                let at = match self.tabs[after].group {
                    Some(group) => self.end_of_group(group),
                    None => after,
                };
                self.place(at + 1, tab, None);
                self.active = self.index_of(from).unwrap_or(0);
                if let Some(name) = repo
                    && let Some(index) = self.index_of(id)
                {
                    self.new_group(index, &name);
                }
            }
        }
        if !activate {
            self.last_background = Some((from, id));
        }
        let shown = if activate { id } else { from };
        self.active = self.index_of(shown).unwrap_or(0);
        self.tidy();
    }

    /// The index of group `id`'s last tab; 0 when it has none.
    fn end_of_group(&self, id: u64) -> usize {
        self.tabs
            .iter()
            .rposition(|tab| tab.group == Some(id))
            .unwrap_or(0)
    }

    /// Puts `tab` at `index` (clamped to the end, and to its side of the
    /// pinned tabs), in `group` when it lands next to one of that group's
    /// tabs, and activates it.
    pub fn insert(&mut self, index: usize, tab: Tab<V>, group: Option<u64>) {
        let at = index.min(self.tabs.len());
        self.active = self.place(at, tab, group);
        self.tidy();
    }

    /// Moves the tab at `from` to `to` (clamped), in `group` as [`Self::insert`]
    /// decides, and opens it. Nothing else changes on the way: a group that
    /// loses the tab only for the move, or a collapsed group next to it,
    /// stays as it is. False when the tab stays where it was, in its group.
    pub fn move_within(&mut self, from: usize, to: usize, group: Option<u64>) -> bool {
        if from >= self.tabs.len() {
            return false;
        }
        let to = to.min(self.tabs.len() - 1);
        let before = self.tabs[from].group;
        if from == to && before == group {
            self.select(from);
            return false;
        }
        let tab = self.tabs.remove(from);
        let to = self.place(to, tab, group);
        self.active = to;
        self.tidy();
        from != to || self.tabs[to].group != before
    }

    /// How many tabs are pinned: the first ones of the pane.
    pub fn pinned_len(&self) -> usize {
        self.tabs.iter().take_while(|tab| tab.pinned).count()
    }

    /// Inserts `tab` at `at` and gives it a group: the one it lands inside,
    /// whatever `wanted` says, so a group's tabs stay next to each other;
    /// else `wanted` when a neighbor is in it; else none. A pinned tab
    /// lands among the pinned tabs, in no group; any other tab after them.
    /// Returns where the tab landed.
    fn place(&mut self, at: usize, mut tab: Tab<V>, wanted: Option<u64>) -> usize {
        let pinned = self.pinned_len();
        let at = if tab.pinned {
            at.min(pinned)
        } else {
            at.clamp(pinned, self.tabs.len())
        };
        let before = at.checked_sub(1).and_then(|ix| self.tabs.get(ix));
        let before = before.and_then(|tab| tab.group);
        let after = self.tabs.get(at).and_then(|tab| tab.group);
        tab.group = match (before, after) {
            _ if tab.pinned => None,
            (Some(left), Some(right)) if left == right => Some(left),
            _ if wanted.is_some() && (before == wanted || after == wanted) => wanted,
            _ => None,
        };
        self.tabs.insert(at, tab);
        at
    }

    /// Pins the tab at `index`: it leaves its group and goes after the
    /// pinned tabs. The active tab stays active.
    pub fn pin(&mut self, index: usize) -> bool {
        self.set_pinned(index, true)
    }

    /// Unpins the tab at `index`: it goes right after the pinned tabs, in
    /// no group. The active tab stays active.
    pub fn unpin(&mut self, index: usize) -> bool {
        self.set_pinned(index, false)
    }

    fn set_pinned(&mut self, index: usize, pinned: bool) -> bool {
        if self.tabs.get(index).is_none_or(|tab| tab.pinned == pinned) {
            return false;
        }
        let active = self.active().id;
        let mut tab = self.tabs.remove(index);
        tab.pinned = pinned;
        let at = self.pinned_len();
        self.place(at, tab, None);
        self.active = self.index_of(active).unwrap_or(0);
        self.tidy();
        true
    }

    /// Takes the tab at `index` out, even the last one, which leaves the
    /// pane empty for its owner to drop. The tab that takes its place is
    /// active, or the one before when it was the last.
    pub fn remove(&mut self, index: usize) -> Option<Tab<V>> {
        if index >= self.tabs.len() {
            return None;
        }
        let was_open = index == self.active;
        let tab = self.tabs.remove(index);
        if (index < self.active || self.active >= self.tabs.len()) && self.active > 0 {
            self.active -= 1;
        }
        if was_open {
            self.leave_hidden();
        }
        self.tidy();
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

    /// Selects the tab at `index`, expanding its group if it is collapsed.
    pub fn select(&mut self, index: usize) {
        self.active = index.min(self.tabs.len() - 1);
        self.tidy();
    }

    pub fn select_last(&mut self) {
        self.select(self.tabs.len() - 1);
    }

    /// The next tab that is not hidden in a collapsed group, wrapping
    /// around; `step` is 1 or -1.
    pub fn cycle(&mut self, step: isize) {
        let len = self.tabs.len() as isize;
        let mut index = self.active as isize;
        for _ in 0..len {
            index = (index + step).rem_euclid(len);
            if !self.is_hidden(index as usize) {
                break;
            }
        }
        self.active = index as usize;
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

    pub fn groups(&self) -> &[TabGroup] {
        &self.groups
    }

    pub fn group(&self, id: u64) -> Option<&TabGroup> {
        self.groups.iter().find(|group| group.id == id)
    }

    /// The group of the tab at `index`, if any.
    pub fn group_at(&self, index: usize) -> Option<u64> {
        self.tabs.get(index).and_then(|tab| tab.group)
    }

    /// Whether the tab at `index` is hidden in a collapsed group.
    pub fn is_hidden(&self, index: usize) -> bool {
        self.group_at(index)
            .and_then(|id| self.group(id))
            .is_some_and(|group| group.collapsed)
    }

    /// How many tabs group `id` has.
    pub fn group_len(&self, id: u64) -> usize {
        self.tabs.iter().filter(|tab| tab.group == Some(id)).count()
    }

    /// Puts the tab at `index` in a new group named `name`, in the first
    /// color no other group of the pane has. A tab taken from the middle of
    /// another group moves to just after it. Returns the new group's id.
    pub fn new_group(&mut self, index: usize, name: &str) -> Option<u64> {
        if index >= self.tabs.len() {
            return None;
        }
        let color = self.free_color();
        let id = self.next_group;
        self.next_group += 1;
        self.groups.push(TabGroup {
            id,
            name: name.to_owned(),
            color,
            collapsed: false,
        });
        self.regroup(index, Some(id));
        Some(id)
    }

    /// The color a new group of this pane takes: the first no other group
    /// has.
    pub fn free_color(&self) -> TabGroupColor {
        let used: Vec<TabGroupColor> = self.groups.iter().map(|group| group.color).collect();
        TabGroupColor::ALL
            .into_iter()
            .find(|color| !used.contains(color))
            .unwrap_or(TabGroupColor::ALL[self.groups.len() % TabGroupColor::ALL.len()])
    }

    /// Opens `tab` after the last tab of group `group`, in it, and leaves
    /// the open tab on screen. False, and `tab` is dropped, when the pane
    /// has no such group.
    pub fn append_to_group(&mut self, mut tab: Tab<V>, group: u64) -> bool {
        if self.group(group).is_none() {
            return false;
        }
        tab.pinned = false;
        let active = self.active().id;
        let at = self.end_of_group(group) + 1;
        self.place(at, tab, Some(group));
        self.active = self.index_of(active).unwrap_or(0);
        self.tidy();
        true
    }

    /// Opens `tab` in a new group named `name`, in `color` or else the
    /// [free one](Self::free_color), right after the open tab (after its
    /// group, when it is in one; after the pinned tabs, when it is pinned),
    /// and leaves the open tab on screen. Returns the new group's id.
    pub fn open_in_new_group(
        &mut self,
        mut tab: Tab<V>,
        name: &str,
        color: Option<TabGroupColor>,
    ) -> Option<u64> {
        tab.pinned = false;
        let active = self.active().id;
        let at = match self.active().group {
            Some(group) => self.end_of_group(group),
            None => self.active,
        } + 1;
        let at = self.place(at, tab, None);
        self.active = self.index_of(active).unwrap_or(0);
        let id = self.new_group(at, name)?;
        if let Some(color) = color {
            self.set_group_color(id, color);
        }
        Some(id)
    }

    /// Moves the tab at `index` to the end of group `group`.
    pub fn add_to_group(&mut self, index: usize, group: u64) -> bool {
        if self.group(group).is_none() || self.group_at(index) == Some(group) {
            return false;
        }
        self.regroup(index, Some(group))
    }

    /// Takes the tab at `index` out of its group, to just after the group.
    pub fn remove_from_group(&mut self, index: usize) -> bool {
        if self.group_at(index).is_none() {
            return false;
        }
        self.regroup(index, None)
    }

    /// Moves the tab at `index` into `group`, after the group's last tab, or
    /// out of every group, after the one it leaves. A tab that joins a new
    /// group from no group stays where it is; a pinned one is unpinned and
    /// goes right after the pinned tabs. The active tab stays active.
    fn regroup(&mut self, index: usize, group: Option<u64>) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        let active = self.active().id;
        let mut tab = self.tabs.remove(index);
        let left = tab.group;
        tab.group = None;
        if group.is_some() {
            tab.pinned = false;
        }
        let last_of = |tabs: &[Tab<V>], id: Option<u64>| {
            let id = id?;
            tabs.iter().rposition(|tab| tab.group == Some(id))
        };
        let at = last_of(&self.tabs, group)
            .or_else(|| last_of(&self.tabs, left))
            .map_or(index, |last| last + 1);
        let pinned = self.pinned_len();
        let at = if tab.pinned {
            at.min(pinned)
        } else {
            at.max(pinned)
        };
        self.tabs.insert(at, tab);
        self.tabs[at].group = group;
        self.active = self.index_of(active).unwrap_or(0);
        self.tidy();
        true
    }

    /// Ungroups the tabs of group `id`, which stay where they are.
    pub fn ungroup(&mut self, id: u64) -> bool {
        if self.group(id).is_none() {
            return false;
        }
        for tab in &mut self.tabs {
            if tab.group == Some(id) {
                tab.group = None;
            }
        }
        self.tidy();
        true
    }

    /// The index of group `id`'s first tab and of the tab after its last.
    fn group_range(&self, id: u64) -> Option<(usize, usize)> {
        let first = self.tabs.iter().position(|tab| tab.group == Some(id))?;
        Some((first, first + self.group_len(id)))
    }

    /// What moves aside when a group is dropped on the tab at `index`: its
    /// whole group, or only the tab outside every group. As a range.
    fn unit_at(&self, index: usize) -> (usize, usize) {
        self.group_at(index)
            .and_then(|id| self.group_range(id))
            .unwrap_or((index, index + 1))
    }

    /// The nearest place to `at` (clamped to the end) where a whole group
    /// can go: never among the pinned tabs, which it follows, nor inside
    /// another group, whose end it moves to.
    fn group_slot(&self, at: usize) -> usize {
        let at = at.clamp(self.pinned_len(), self.tabs.len());
        match at.checked_sub(1).and_then(|before| self.group_at(before)) {
            Some(left) if self.group_at(at) == Some(left) => self.end_of_group(left) + 1,
            _ => at,
        }
    }

    /// Where group `id` lands when dropped on the tab at `onto`: it takes
    /// the place of what that tab belongs to (the tab, or its whole group),
    /// after it when the group comes from its left, else before it; `None`,
    /// or an index past the end, is after the last tab. Dropped on a pinned
    /// tab, it goes right after the pinned tabs. The index, among the tabs
    /// as they are now, that the group goes before; `None` when it would not
    /// move.
    pub fn group_drop_at(&self, id: u64, onto: Option<usize>) -> Option<usize> {
        let (first, end) = self.group_range(id)?;
        let to = match onto.filter(|&index| index < self.tabs.len()) {
            None => self.tabs.len(),
            Some(index) if (first..end).contains(&index) => return None,
            Some(index) => {
                let (start, stop) = self.unit_at(index);
                let to = if start >= end { stop } else { start };
                to.max(self.pinned_len())
            }
        };
        (to != first && to != end).then_some(to)
    }

    /// Moves group `id`, every tab of it in order, where
    /// [`Self::group_drop_at`] says. The other tabs keep their order and the
    /// open tab stays open. False when nothing moved.
    pub fn move_group(&mut self, id: u64, onto: Option<usize>) -> bool {
        let (Some(to), Some((first, end))) = (self.group_drop_at(id, onto), self.group_range(id))
        else {
            return false;
        };
        let active = self.active().id;
        let moved: Vec<Tab<V>> = self.tabs.drain(first..end).collect();
        let at = if to > first { to - moved.len() } else { to };
        let at = self.group_slot(at);
        self.tabs.splice(at..at, moved);
        self.active = self.index_of(active).unwrap_or(0);
        self.tidy();
        at != first
    }

    /// Closes every tab of group `id` and returns the group as it was, for
    /// reopening. When the open tab was in it, the tab after the group
    /// opens, or else the one before it. A group holding every tab of the
    /// pane leaves it empty, for its owner to fill or drop.
    pub fn close_group(&mut self, id: u64) -> Option<ClosedTabGroup> {
        let group = self.group(id)?.clone();
        let (first, end) = self.group_range(id)?;
        let active = self.active().id;
        let closed: Vec<Tab<V>> = self.tabs.drain(first..end).collect();
        self.active = match self.index_of(active) {
            Some(index) => index,
            None if self.tabs.is_empty() => 0,
            None => {
                let next = first.min(self.tabs.len() - 1);
                if self.is_hidden(next) {
                    self.nearest_shown(next).unwrap_or(next)
                } else {
                    next
                }
            }
        };
        self.tidy();
        Some(ClosedTabGroup {
            name: group.name,
            color: group.color,
            tabs: closed
                .iter()
                .map(|tab| tab.current().route.clone())
                .collect(),
            position: first,
        })
    }

    /// Puts `tabs` back, in order, as an expanded group named `name` in
    /// `color`, from `at` (the end when past it; a group it would fall
    /// inside keeps it after its end), and opens its first tab. Returns the
    /// group's id; `None` when `tabs` is empty.
    pub fn insert_group(
        &mut self,
        at: usize,
        name: &str,
        color: TabGroupColor,
        mut tabs: Vec<Tab<V>>,
    ) -> Option<u64> {
        if tabs.is_empty() {
            return None;
        }
        let id = self.next_group;
        self.next_group += 1;
        self.groups.push(TabGroup {
            id,
            name: name.to_owned(),
            color,
            collapsed: false,
        });
        for tab in &mut tabs {
            tab.group = Some(id);
        }
        let at = self.group_slot(at);
        self.tabs.splice(at..at, tabs);
        self.active = at;
        self.tidy();
        Some(id)
    }

    pub fn rename_group(&mut self, id: u64, name: &str) -> bool {
        match self.groups.iter_mut().find(|group| group.id == id) {
            Some(group) if group.name != name => {
                group.name = name.to_owned();
                true
            }
            _ => false,
        }
    }

    pub fn set_group_color(&mut self, id: u64, color: TabGroupColor) -> bool {
        match self.groups.iter_mut().find(|group| group.id == id) {
            Some(group) if group.color != color => {
                group.color = color;
                true
            }
            _ => false,
        }
    }

    /// Collapses group `id` or expands it. Collapsing the group of the open
    /// tab opens the nearest tab outside it; when every other tab is hidden
    /// too, the group stays open and this returns false.
    pub fn toggle_group(&mut self, id: u64) -> bool {
        let Some(at) = self.groups.iter().position(|group| group.id == id) else {
            return false;
        };
        if self.groups[at].collapsed {
            self.groups[at].collapsed = false;
            return true;
        }
        self.groups[at].collapsed = true;
        if self.active().group == Some(id) {
            match self.nearest_shown(self.active) {
                Some(index) => self.active = index,
                None => {
                    self.groups[at].collapsed = false;
                    return false;
                }
            }
        }
        true
    }

    /// The tab nearest `index` that is not hidden, after it first.
    fn nearest_shown(&self, index: usize) -> Option<usize> {
        let after = (index + 1..self.tabs.len()).find(|&ix| !self.is_hidden(ix));
        after.or_else(|| (0..index).rev().find(|&ix| !self.is_hidden(ix)))
    }

    /// When the active tab is hidden after a close, opens the nearest shown
    /// tab instead; [`Self::tidy`] expands the group if there is none.
    fn leave_hidden(&mut self) {
        if self.active < self.tabs.len()
            && self.is_hidden(self.active)
            && let Some(index) = self.nearest_shown(self.active)
        {
            self.active = index;
        }
    }

    /// Restores the groups `saved` for these tabs. A group whose tabs are out
    /// of range, pinned, or already in another group is dropped.
    pub fn restore_groups(&mut self, saved: &[SavedTabGroup]) {
        for group in saved {
            let range = group.first..group.first.saturating_add(group.len);
            if group.len == 0
                || range.end > self.tabs.len()
                || self.tabs[range.clone()]
                    .iter()
                    .any(|tab| tab.group.is_some() || tab.pinned)
            {
                continue;
            }
            let id = self.next_group;
            self.next_group += 1;
            self.groups.push(TabGroup {
                id,
                name: group.name.clone(),
                color: group.color,
                collapsed: group.collapsed,
            });
            for tab in &mut self.tabs[range] {
                tab.group = Some(id);
            }
        }
        self.tidy();
    }

    /// The groups to save, in the order they appear.
    pub fn saved_groups(&self) -> Vec<SavedTabGroup> {
        let mut saved: Vec<SavedTabGroup> = self
            .groups
            .iter()
            .filter_map(|group| {
                let first = self
                    .tabs
                    .iter()
                    .position(|tab| tab.group == Some(group.id))?;
                Some(SavedTabGroup {
                    name: group.name.clone(),
                    color: group.color,
                    collapsed: group.collapsed,
                    first,
                    len: self.group_len(group.id),
                })
            })
            .collect();
        saved.sort_by_key(|group| group.first);
        saved
    }

    /// Drops groups no tab is in and tabs' links to groups that are gone,
    /// and expands the open tab's group.
    fn tidy(&mut self) {
        let groups = &self.groups;
        for tab in &mut self.tabs {
            if tab
                .group
                .is_some_and(|id| !groups.iter().any(|group| group.id == id))
            {
                tab.group = None;
            }
        }
        let tabs = &self.tabs;
        self.groups
            .retain(|group| tabs.iter().any(|tab| tab.group == Some(group.id)));
        if let Some(open) = self.tabs.get(self.active).and_then(|tab| tab.group)
            && let Some(group) = self.groups.iter_mut().find(|group| group.id == open)
        {
            group.collapsed = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{Page, Tab, Tabs};
    use crate::route::Route;
    use crate::store::{ClosedTabGroup, SavedTabGroup, TabGroupColor};
    use loupe_github::{PrRef, RepoRef};

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

    /// The tabs of a pane as names, each with its group's name after a `:`,
    /// checking on the way that every group's tabs sit next to each other.
    fn names(tabs: &Tabs<()>) -> Vec<String> {
        let mut seen: Vec<u64> = Vec::new();
        let mut previous = None;
        tabs.iter()
            .map(|tab| {
                if let Some(group) = tab.group
                    && previous != Some(group)
                {
                    assert!(!seen.contains(&group), "group {group} is split in two");
                    seen.push(group);
                }
                previous = tab.group;
                let name = match &tab.current().route {
                    Route::LinearIssue { identifier } => identifier.clone(),
                    route => route.label().to_string(),
                };
                match tab.group.and_then(|id| tabs.group(id)) {
                    Some(group) => format!("{name}:{}", group.name),
                    None => name,
                }
            })
            .collect()
    }

    fn five_tabs() -> Tabs<()> {
        Tabs::new(
            ["A", "B", "C", "D", "E"]
                .into_iter()
                .map(|name| tab(issue(name)))
                .collect(),
            0,
        )
    }

    #[test]
    fn a_group_keeps_its_tabs_together() {
        let mut tabs = five_tabs();
        let review = tabs.new_group(1, "Review").expect("B is a tab");
        assert_eq!(tabs.group(review).unwrap().color, TabGroupColor::Grey);
        assert_eq!(names(&tabs), ["A", "B:Review", "C", "D", "E"]);

        assert!(tabs.add_to_group(3, review));
        assert_eq!(
            names(&tabs),
            ["A", "B:Review", "D:Review", "C", "E"],
            "a tab added to a group moves to its end"
        );
        assert_eq!(tabs.active_index(), 0, "the open tab stays open");

        let later = tabs.new_group(1, "Later").unwrap();
        assert_eq!(
            names(&tabs),
            ["A", "D:Review", "B:Later", "C", "E"],
            "a tab taken out of the middle of a group goes after it"
        );
        assert_eq!(
            tabs.group(later).unwrap().color,
            TabGroupColor::Blue,
            "a new group takes a color no other group has"
        );

        tabs.select(1);
        tabs.open(tab(issue("F")));
        assert_eq!(
            names(&tabs),
            ["A", "D:Review", "F:Review", "B:Later", "C", "E"],
            "a tab opened from a grouped tab joins its group"
        );
        tabs.open_in_background(tab(issue("G")));
        assert_eq!(names(&tabs)[3], "G:Review");

        let f = tabs.index_of(tabs.active().id).unwrap();
        assert!(tabs.remove_from_group(f));
        assert_eq!(
            names(&tabs),
            ["A", "D:Review", "G:Review", "F", "B:Later", "C", "E"],
            "a tab taken out of a group goes after it"
        );
        assert_eq!(names(&tabs)[tabs.active_index()], "F");

        tabs.insert(2, tab(issue("H")), None);
        assert_eq!(
            names(&tabs)[2],
            "H:Review",
            "a tab put inside a group joins it"
        );
        tabs.insert(1, tab(issue("I")), None);
        assert_eq!(
            names(&tabs)[1],
            "I",
            "a tab put at a group's edge needs to ask"
        );
    }

    #[test]
    fn a_group_with_no_tab_left_is_gone() {
        let mut tabs = five_tabs();
        let review = tabs.new_group(4, "Review").unwrap();
        assert!(tabs.close(4));
        assert!(tabs.group(review).is_none());
        assert!(tabs.groups().is_empty());

        let review = tabs.new_group(0, "Review").unwrap();
        tabs.add_to_group(1, review);
        assert!(tabs.ungroup(review));
        assert_eq!(names(&tabs), ["A", "B", "C", "D"]);
        assert!(tabs.groups().is_empty());

        let review = tabs.new_group(2, "Review").unwrap();
        assert!(tabs.remove_from_group(2));
        assert!(
            tabs.group(review).is_none(),
            "nor when its last tab leaves it"
        );
    }

    #[test]
    fn a_collapsed_group_hides_its_tabs_but_never_the_open_one() {
        let mut tabs = five_tabs();
        let review = tabs.new_group(1, "Review").unwrap();
        tabs.add_to_group(2, review);
        assert!(tabs.toggle_group(review));
        assert!(tabs.group(review).unwrap().collapsed);
        assert!(tabs.is_hidden(1) && tabs.is_hidden(2) && !tabs.is_hidden(0));

        tabs.cycle(1);
        assert_eq!(tabs.active_index(), 3, "Ctrl+Tab skips the hidden tabs");
        tabs.cycle(-1);
        assert_eq!(tabs.active_index(), 0);

        tabs.select(2);
        assert!(
            !tabs.group(review).unwrap().collapsed,
            "opening a hidden tab expands its group"
        );

        assert!(tabs.toggle_group(review));
        assert_eq!(
            tabs.active_index(),
            3,
            "collapsing the open tab's group opens the next tab outside it"
        );

        tabs.toggle_group(review);
        tabs.select(0);
        tabs.toggle_group(review);
        assert!(tabs.close(0));
        assert_eq!(
            names(&tabs)[tabs.active_index()],
            "D",
            "closing the open tab skips the hidden ones"
        );

        let mut only = Tabs::new(vec![tab(issue("A")), tab(issue("B"))], 0);
        let all = only.new_group(0, "All").unwrap();
        only.add_to_group(1, all);
        assert!(
            !only.toggle_group(all),
            "a group with every tab of the pane stays open"
        );
        assert!(!only.group(all).unwrap().collapsed);
    }

    #[test]
    fn a_tab_dropped_on_itself_changes_nothing() {
        let mut tabs = five_tabs();
        let solo = tabs.new_group(1, "Solo").unwrap();
        assert!(!tabs.move_within(1, 1, Some(solo)));
        assert_eq!(
            names(&tabs),
            ["A", "B:Solo", "C", "D", "E"],
            "a group of one tab keeps it"
        );
        assert_eq!(tabs.active_index(), 1);

        let mut tabs = five_tabs();
        let hidden = tabs.new_group(1, "Hidden").unwrap();
        for _ in 0..3 {
            tabs.add_to_group(2, hidden);
        }
        tabs.toggle_group(hidden);
        tabs.select(0);
        assert!(!tabs.move_within(0, 0, None));
        assert!(
            tabs.group(hidden).unwrap().collapsed,
            "the only tab on show dropped on itself leaves the group collapsed"
        );
    }

    #[test]
    fn a_background_tab_never_lands_in_a_group_it_was_not_opened_from() {
        let mut tabs = Tabs::new(vec![tab(issue("A")), tab(issue("B"))], 0);
        tabs.open_in_background(tab(issue("X")));
        let other = tabs.new_group(1, "Other").unwrap();
        tabs.add_to_group(2, other);
        tabs.toggle_group(other);
        tabs.open_in_background(tab(issue("Y")));
        assert_eq!(
            names(&tabs),
            ["A", "Y", "X:Other", "B:Other"],
            "once X moved to another group, Y starts a new run after A"
        );
        assert!(!tabs.is_hidden(1));
    }

    /// The tabs as names, a pinned one with a `*` after it, checking on the
    /// way that the pinned tabs come first and are in no group.
    fn pinned_names(tabs: &Tabs<()>) -> Vec<String> {
        let pinned = tabs.pinned_len();
        assert!(
            tabs.iter().skip(pinned).all(|tab| !tab.pinned),
            "the pinned tabs come first"
        );
        assert!(
            tabs.iter().take(pinned).all(|tab| tab.group.is_none()),
            "a pinned tab is in no group"
        );
        names(tabs)
            .into_iter()
            .zip(tabs.iter())
            .map(|(name, tab)| if tab.pinned { format!("{name}*") } else { name })
            .collect()
    }

    #[test]
    fn pinning_moves_a_tab_after_the_pinned_ones_and_unpinning_right_after_them() {
        let mut tabs = five_tabs();
        tabs.select(3);
        assert!(tabs.pin(2));
        assert_eq!(pinned_names(&tabs), ["C*", "A", "B", "D", "E"]);
        assert!(tabs.pin(4));
        assert_eq!(
            pinned_names(&tabs),
            ["C*", "E*", "A", "B", "D"],
            "pinned tabs keep the order they were pinned in"
        );
        assert_eq!(
            names(&tabs)[tabs.active_index()],
            "D",
            "the open tab stays open"
        );
        assert!(!tabs.pin(1), "a pinned tab pins once");

        assert!(tabs.unpin(0));
        assert_eq!(
            pinned_names(&tabs),
            ["E*", "C", "A", "B", "D"],
            "an unpinned tab goes right after the pinned ones"
        );
        assert!(!tabs.unpin(1));
    }

    #[test]
    fn a_page_opened_from_a_pinned_tab_lands_after_the_pinned_tabs() {
        let mut tabs = five_tabs();
        tabs.pin(0);
        tabs.pin(1);
        tabs.select(0);
        tabs.open(tab(issue("F")));
        assert_eq!(pinned_names(&tabs), ["A*", "B*", "F", "C", "D", "E"]);
        assert_eq!(tabs.active_index(), 2, "the new tab is open");
        assert_eq!(
            tabs.iter().next().unwrap().current().route,
            issue("A"),
            "the pinned tab keeps its page"
        );

        tabs.select(1);
        tabs.open(tab(issue("G")));
        assert_eq!(
            pinned_names(&tabs)[2],
            "G",
            "each one lands right after the pinned tabs"
        );

        tabs.select(0);
        tabs.open_in_background(tab(issue("H")));
        tabs.open_in_background(tab(issue("I")));
        assert_eq!(
            tabs.active_index(),
            0,
            "a background tab leaves the pinned tab open"
        );
        assert_eq!(
            pinned_names(&tabs),
            ["A*", "B*", "H", "I", "G", "F", "C", "D", "E"]
        );
    }

    #[test]
    fn pinned_tabs_and_groups_stay_apart() {
        let mut tabs = five_tabs();
        let review = tabs.new_group(0, "Review").unwrap();
        tabs.add_to_group(1, review);
        assert!(tabs.pin(1));
        assert_eq!(
            pinned_names(&tabs),
            ["B*", "A:Review", "C", "D", "E"],
            "a pinned tab leaves its group"
        );

        tabs.select(0);
        tabs.open(tab(issue("F")));
        assert_eq!(
            pinned_names(&tabs)[1],
            "F",
            "a tab opened from a pinned tab joins no group, even next to one"
        );

        assert!(tabs.add_to_group(0, review));
        assert_eq!(
            pinned_names(&tabs),
            ["F", "A:Review", "B:Review", "C", "D", "E"],
            "adding a pinned tab to a group unpins it"
        );
        tabs.pin(5);
        tabs.new_group(0, "Later").unwrap();
        assert_eq!(
            pinned_names(&tabs),
            ["E:Later", "F", "A:Review", "B:Review", "C", "D"],
            "so does a new group"
        );

        tabs.pin(5);
        tabs.pin(5);
        assert_eq!(
            pinned_names(&tabs),
            ["D*", "C*", "E:Later", "F", "A:Review", "B:Review"]
        );
        assert!(tabs.move_within(5, 0, None));
        assert_eq!(
            pinned_names(&tabs)[2],
            "B",
            "a tab dragged onto the pinned tabs stops after them"
        );
        assert!(tabs.move_within(0, 5, tabs.group_at(5)));
        assert_eq!(
            pinned_names(&tabs)[..2],
            ["C*", "D*"],
            "a pinned tab dragged past them stays pinned, at their end"
        );
        tabs.insert(0, tab(issue("G")), None);
        assert_eq!(pinned_names(&tabs)[2], "G");
    }

    #[test]
    fn grouping_by_repository_keeps_the_pinned_tabs_first() {
        use loupe_github::{PrRef, RepoRef};

        let pr = |number| Route::PullRequest(PrRef::new(RepoRef::new("octo", "app"), number));
        let mut tabs = five_tabs();
        tabs.pin(0);
        tabs.select(0);
        tabs.open_in_repo_group(tab(pr(1)), true);
        assert_eq!(
            pinned_names(&tabs)[..3],
            ["A*", "app#1:app", "B"],
            "a tab opened from a pinned tab lands after it, in its repository's group"
        );
        tabs.select(0);
        tabs.open_in_repo_group(tab(pr(2)), false);
        assert_eq!(pinned_names(&tabs)[..3], ["A*", "app#1:app", "app#2:app"]);
        assert_eq!(tabs.active_index(), 0, "the pinned tab stays open");
    }

    #[test]
    fn pinned_tabs_come_back_first_and_no_group_takes_them() {
        let mut saved: Vec<Tab<()>> = ["A", "B", "C"]
            .into_iter()
            .map(|name| tab(issue(name)))
            .collect();
        saved[2].pinned = true;
        let mut tabs = Tabs::new(saved, 2);
        assert_eq!(pinned_names(&tabs), ["C*", "A", "B"]);
        assert_eq!(tabs.active_index(), 0, "the active tab is the same tab");

        tabs.restore_groups(&[SavedTabGroup {
            name: "Over".into(),
            color: TabGroupColor::Grey,
            collapsed: false,
            first: 0,
            len: 2,
        }]);
        assert!(
            tabs.groups().is_empty(),
            "a saved group over a pinned tab is dropped"
        );
    }

    fn six_tabs_two_groups() -> (Tabs<()>, u64, u64) {
        let mut tabs = Tabs::new(
            ["A", "B", "C", "D", "E", "F"]
                .into_iter()
                .map(|name| tab(issue(name)))
                .collect(),
            0,
        );
        let review = tabs.new_group(1, "Review").unwrap();
        tabs.add_to_group(2, review);
        let later = tabs.new_group(4, "Later").unwrap();
        tabs.add_to_group(5, later);
        assert_eq!(
            names(&tabs),
            ["A", "B:Review", "C:Review", "D", "E:Later", "F:Later"]
        );
        (tabs, review, later)
    }

    #[test]
    fn dragging_a_group_moves_all_its_tabs_and_keeps_the_others_in_order() {
        let (mut tabs, review, later) = six_tabs_two_groups();
        tabs.select(3);
        assert!(tabs.move_group(review, Some(3)));
        assert_eq!(
            names(&tabs),
            ["A", "D", "B:Review", "C:Review", "E:Later", "F:Later"],
            "dropped on a tab to its right, the group goes after it"
        );
        assert!(tabs.move_group(review, Some(4)));
        assert_eq!(
            names(&tabs),
            ["A", "D", "E:Later", "F:Later", "B:Review", "C:Review"],
            "dropped on another group's tab, it goes past that whole group"
        );
        assert!(tabs.move_group(review, Some(0)));
        assert_eq!(
            names(&tabs),
            ["B:Review", "C:Review", "A", "D", "E:Later", "F:Later"],
            "dropped on a tab to its left, it goes before it"
        );
        assert_eq!(
            names(&tabs)[tabs.active_index()],
            "D",
            "the open tab stays open"
        );
        assert!(!tabs.move_group(review, Some(1)), "onto itself, it stays");
        assert!(!tabs.move_group(later, None), "the last group stays last");
        assert!(tabs.move_group(review, None));
        assert_eq!(
            names(&tabs),
            ["A", "D", "E:Later", "F:Later", "B:Review", "C:Review"],
            "dropped after the last tab, it goes to the end"
        );

        tabs.toggle_group(later);
        assert!(tabs.move_group(later, Some(0)));
        assert_eq!(
            names(&tabs),
            ["E:Later", "F:Later", "A", "D", "B:Review", "C:Review"]
        );
        assert!(
            tabs.group(later).unwrap().collapsed,
            "a collapsed group moves collapsed"
        );
    }

    #[test]
    fn a_group_lands_before_or_after_what_it_is_dropped_on() {
        let (tabs, review, later) = six_tabs_two_groups();
        assert_eq!(tabs.group_drop_at(review, Some(3)), Some(4), "after D");
        assert_eq!(tabs.group_drop_at(review, Some(5)), Some(6), "after Later");
        assert_eq!(tabs.group_drop_at(later, Some(1)), Some(1), "before Review");
        assert_eq!(tabs.group_drop_at(later, Some(0)), Some(0), "before A");
        assert_eq!(tabs.group_drop_at(review, Some(2)), None, "on itself");
        assert_eq!(tabs.group_drop_at(later, None), None, "already last");
    }

    #[test]
    fn closing_and_reopening_a_group_gives_back_its_tabs_name_and_color_in_place() {
        let (mut tabs, review, _) = six_tabs_two_groups();
        tabs.set_group_color(review, TabGroupColor::Purple);
        tabs.select(2);
        let closed = tabs.close_group(review).expect("Review is a group");
        assert_eq!(
            closed,
            ClosedTabGroup {
                name: "Review".into(),
                color: TabGroupColor::Purple,
                tabs: vec![issue("B"), issue("C")],
                position: 1,
            }
        );
        assert_eq!(names(&tabs), ["A", "D", "E:Later", "F:Later"]);
        assert_eq!(
            names(&tabs)[tabs.active_index()],
            "D",
            "closing the open tab's group opens the tab after it"
        );

        let reopened: Vec<Tab<()>> = closed.tabs.iter().cloned().map(tab).collect();
        let id = tabs
            .insert_group(closed.position, &closed.name, closed.color, reopened)
            .unwrap();
        assert_eq!(
            names(&tabs),
            ["A", "B:Review", "C:Review", "D", "E:Later", "F:Later"],
            "the same tabs, in the same order, in the same place"
        );
        assert_eq!(tabs.group(id).unwrap().color, TabGroupColor::Purple);
        assert_eq!(names(&tabs)[tabs.active_index()], "B:Review");

        let (mut tabs, review, later) = six_tabs_two_groups();
        let closed = tabs.close_group(review).unwrap();
        tabs.move_group(later, Some(0));
        assert_eq!(names(&tabs), ["E:Later", "F:Later", "A", "D"]);
        let reopened: Vec<Tab<()>> = closed.tabs.iter().cloned().map(tab).collect();
        tabs.insert_group(closed.position, &closed.name, closed.color, reopened);
        assert_eq!(
            names(&tabs),
            ["E:Later", "F:Later", "B:Review", "C:Review", "A", "D"],
            "a place now inside another group moves to that group's end"
        );
        let reopened = vec![tab(issue("G"))];
        tabs.insert_group(99, "Gone", TabGroupColor::Grey, reopened);
        assert_eq!(
            names(&tabs).last().unwrap(),
            "G:Gone",
            "a place past the end is the end"
        );
    }

    #[test]
    fn a_group_never_moves_or_reopens_among_the_pinned_tabs() {
        let (mut tabs, review, later) = six_tabs_two_groups();
        tabs.pin(0);
        tabs.pin(3);
        assert_eq!(
            pinned_names(&tabs),
            ["A*", "D*", "B:Review", "C:Review", "E:Later", "F:Later"]
        );
        assert_eq!(
            tabs.group_drop_at(review, Some(0)),
            None,
            "a group right after the pinned tabs stays there"
        );
        assert!(tabs.move_group(later, Some(0)));
        assert_eq!(
            pinned_names(&tabs),
            ["A*", "D*", "E:Later", "F:Later", "B:Review", "C:Review"],
            "dropped on a pinned tab, a group goes right after the pinned tabs"
        );

        tabs.insert_group(1, "Early", TabGroupColor::Grey, vec![tab(issue("G"))]);
        assert_eq!(
            pinned_names(&tabs)[..3],
            ["A*", "D*", "G:Early"],
            "a group reopened where the pinned tabs now are goes after them"
        );
    }

    #[test]
    fn groups_come_back_as_saved() {
        let mut tabs = five_tabs();
        // Made right to left, saved left to right.
        let later = tabs.new_group(4, "Later").unwrap();
        tabs.toggle_group(later);
        let review = tabs.new_group(1, "Review").unwrap();
        tabs.add_to_group(2, review);
        tabs.set_group_color(review, TabGroupColor::Purple);
        let saved = tabs.saved_groups();
        assert_eq!(
            saved,
            [
                SavedTabGroup {
                    name: "Review".into(),
                    color: TabGroupColor::Purple,
                    collapsed: false,
                    first: 1,
                    len: 2,
                },
                SavedTabGroup {
                    name: "Later".into(),
                    color: TabGroupColor::Grey,
                    collapsed: true,
                    first: 4,
                    len: 1,
                },
            ]
        );

        let mut restored = five_tabs();
        let mut broken = saved.clone();
        broken.push(SavedTabGroup {
            first: 2,
            ..saved[1].clone()
        });
        broken.push(SavedTabGroup {
            first: 9,
            ..saved[1].clone()
        });
        restored.restore_groups(&broken);
        assert_eq!(
            restored.saved_groups(),
            saved,
            "a saved group over another one or past the end is dropped"
        );
        assert_eq!(
            names(&restored),
            ["A", "B:Review", "C:Review", "D", "E:Later"]
        );
    }

    fn pr(owner: &str, repo: &str, number: u64) -> Route {
        Route::PullRequest(PrRef::new(RepoRef::new(owner, repo), number))
    }

    #[test]
    fn grouping_by_repo_puts_each_new_tab_in_its_repository_s_group() {
        let mut tabs = one_tab();
        tabs.open_in_repo_group(tab(pr("octo", "loupe", 1)), true);
        tabs.open_in_repo_group(tab(pr("octo", "site", 2)), true);
        tabs.open_in_repo_group(tab(Route::Repo(RepoRef::new("octo", "loupe"))), true);
        assert_eq!(
            names(&tabs),
            [
                "Reviews",
                "loupe#1:loupe",
                "octo/loupe:loupe",
                "site#2:site"
            ],
            "a group is made on the spot, and a later tab joins it at its end"
        );
        assert_eq!(tabs.active_index(), 2, "the new tab is on screen");
        assert_ne!(
            tabs.groups()[0].color,
            tabs.groups()[1].color,
            "each repository's group gets its own color"
        );

        tabs.open_in_repo_group(tab(issue("ENG-1")), true);
        tabs.open_in_repo_group(tab(Route::Notifications), false);
        assert_eq!(
            names(&tabs),
            [
                "Reviews",
                "loupe#1:loupe",
                "octo/loupe:loupe",
                "ENG-1",
                "Inbox",
                "site#2:site"
            ],
            "a tab without a repository opens outside every group, even from a grouped tab"
        );
        assert_eq!(
            tabs.active_index(),
            3,
            "a background tab leaves the active one"
        );
    }

    #[test]
    fn grouping_by_repo_keeps_background_tabs_in_order_and_hand_made_groups() {
        let mut tabs = one_tab();
        tabs.new_group(0, "Mine");
        tabs.open_in_repo_group(tab(pr("octo", "loupe", 1)), false);
        tabs.open_in_repo_group(tab(issue("ENG-1")), false);
        tabs.open_in_repo_group(tab(issue("ENG-2")), false);
        tabs.open_in_repo_group(tab(pr("octo", "loupe", 2)), false);
        assert_eq!(
            names(&tabs),
            [
                "Reviews:Mine",
                "loupe#1:loupe",
                "loupe#2:loupe",
                "ENG-1",
                "ENG-2"
            ],
            "the hand-made group keeps only its own tab, and background tabs keep their order"
        );
        assert_eq!(tabs.active_index(), 0);

        assert!(tabs.remove_from_group(1), "take loupe#1 out by hand");
        tabs.open_in_repo_group(tab(pr("octo", "loupe", 3)), true);
        assert_eq!(
            names(&tabs),
            [
                "Reviews:Mine",
                "loupe#2:loupe",
                "loupe#3:loupe",
                "loupe#1",
                "ENG-1",
                "ENG-2"
            ],
            "a tab taken out of its repository's group by hand stays out"
        );
    }
}
