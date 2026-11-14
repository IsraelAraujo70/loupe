//! The pull requests picked on the Reviews screen with their checkboxes.

use loupe_github::PrRef;

/// More than this many pull requests asks before opening them all.
pub(super) const CONFIRM_ABOVE: usize = 10;

/// The picked pull requests and the anchor a Shift+click extends from.
/// Kept by reference, so a refresh or a new sort keeps them; the ones that
/// leave the list stop counting ([`Selection::in_order`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Selection {
    picked: Vec<PrRef>,
    anchor: Option<PrRef>,
}

impl Selection {
    pub(super) fn contains(&self, pr: &PrRef) -> bool {
        self.picked.contains(pr)
    }

    /// Picks `pr`, or drops it if it was picked; it becomes the anchor.
    pub(super) fn toggle(&mut self, pr: &PrRef) {
        match self.picked.iter().position(|picked| picked == pr) {
            Some(at) => {
                self.picked.remove(at);
            }
            None => self.picked.push(pr.clone()),
        }
        self.anchor = Some(pr.clone());
    }

    /// Picks every row of `rows` (the list in its order on screen) from the
    /// anchor to `pr`, both included, and makes `pr` the anchor. With no
    /// anchor on the list, it toggles `pr` alone.
    pub(super) fn extend_to(&mut self, rows: &[PrRef], pr: &PrRef) {
        let from = self
            .anchor
            .as_ref()
            .and_then(|anchor| rows.iter().position(|row| row == anchor));
        let to = rows.iter().position(|row| row == pr);
        let (Some(from), Some(to)) = (from, to) else {
            self.toggle(pr);
            return;
        };
        for row in &rows[from.min(to)..=from.max(to)] {
            if !self.picked.contains(row) {
                self.picked.push(row.clone());
            }
        }
        self.anchor = Some(pr.clone());
    }

    /// Picks every row of `rows`, one group's or the whole tab's.
    pub(super) fn select_all(&mut self, rows: &[PrRef]) {
        for row in rows {
            if !self.picked.contains(row) {
                self.picked.push(row.clone());
            }
        }
    }

    /// Drops every row of `rows` from the selection.
    pub(super) fn deselect_all(&mut self, rows: &[PrRef]) {
        self.picked.retain(|picked| !rows.contains(picked));
    }

    pub(super) fn clear(&mut self) {
        *self = Selection::default();
    }

    /// The picked pull requests that `rows` (the list on screen, top to
    /// bottom) shows, in that order and each once.
    pub(super) fn in_order(&self, rows: &[PrRef]) -> Vec<PrRef> {
        let mut ordered: Vec<PrRef> = Vec::new();
        for row in rows {
            if self.picked.contains(row) && !ordered.contains(row) {
                ordered.push(row.clone());
            }
        }
        ordered
    }
}

#[cfg(test)]
mod tests {
    use loupe_github::{PrRef, RepoRef};

    use super::Selection;

    fn rows(numbers: &[u64]) -> Vec<PrRef> {
        numbers
            .iter()
            .map(|&number| PrRef::new(RepoRef::new("a", "repo"), number))
            .collect()
    }

    fn numbers(prs: &[PrRef]) -> Vec<u64> {
        prs.iter().map(|pr| pr.number).collect()
    }

    #[test]
    fn a_click_toggles_one_row_and_the_list_order_wins() {
        let list = rows(&[1, 2, 3, 4]);
        let mut selection = Selection::default();
        selection.toggle(&list[2]);
        selection.toggle(&list[0]);
        assert_eq!(numbers(&selection.in_order(&list)), [1, 3]);
        selection.toggle(&list[2]);
        assert_eq!(numbers(&selection.in_order(&list)), [1]);
        // A row that left the list no longer counts.
        assert!(selection.in_order(&rows(&[2, 3])).is_empty());
    }

    #[test]
    fn shift_click_picks_the_range_from_the_last_click_either_way() {
        let list = rows(&[1, 2, 3, 4, 5]);
        let mut selection = Selection::default();
        selection.toggle(&list[1]);
        selection.extend_to(&list, &list[3]);
        assert_eq!(numbers(&selection.in_order(&list)), [2, 3, 4]);
        // Upward from the new anchor (4) to 1.
        selection.extend_to(&list, &list[0]);
        assert_eq!(numbers(&selection.in_order(&list)), [1, 2, 3, 4]);
        // The range follows the order on screen, not GitHub's.
        let sorted = rows(&[5, 1, 4, 2, 3]);
        let mut selection = Selection::default();
        selection.toggle(&sorted[0]);
        selection.extend_to(&sorted, &sorted[2]);
        assert_eq!(numbers(&selection.in_order(&list)), [1, 4, 5]);
        // With no anchor, a Shift+click picks the one row.
        let mut selection = Selection::default();
        selection.extend_to(&list, &list[4]);
        assert_eq!(numbers(&selection.in_order(&list)), [5]);
    }

    #[test]
    fn a_group_selects_and_deselects_as_a_whole() {
        let (first, second) = (rows(&[1, 2]), rows(&[3, 4]));
        let all: Vec<PrRef> = first.iter().chain(&second).cloned().collect();
        let mut selection = Selection::default();
        selection.toggle(&first[0]);
        selection.select_all(&second);
        assert_eq!(numbers(&selection.in_order(&all)), [1, 3, 4]);
        selection.deselect_all(&second);
        assert_eq!(numbers(&selection.in_order(&all)), [1]);
        selection.clear();
        assert!(selection.in_order(&all).is_empty());
        assert_eq!(selection, Selection::default(), "the anchor goes too");
    }
}
