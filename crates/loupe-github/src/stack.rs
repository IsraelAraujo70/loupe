//! A pull request stack, as GitHub keeps it (`PullRequest.stack` in GraphQL,
//! <https://docs.github.com/rest/pulls/stacks> in REST): pull requests that
//! build on each other, numbered from the one closest to the stack's base.
//!
//! The asynchronous merge merges a stacked pull request "with all open
//! downstack pull requests", so [`PrStack::open_below`] is the set of pull
//! requests a merge takes along.

use serde::{Deserialize, Serialize};

use crate::{ChecksState, Mergeable, PrState, ReviewDecision};

/// One stack and every pull request in it, merged and closed ones included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrStack {
    /// The stack's number in its repository.
    pub number: u64,
    /// The branch the whole stack merges into: the base of its bottom pull
    /// request.
    pub base_ref: String,
    /// Sorted by position, base first.
    pub entries: Vec<StackEntry>,
}

/// One pull request of a stack, with what a merge confirmation needs to warn
/// about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackEntry {
    /// 1 is the pull request closest to the stack's base.
    pub position: u32,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub is_draft: bool,
    /// The head commit's rolled-up checks; `None` when it has none.
    pub checks: Option<ChecksState>,
    pub mergeable: Mergeable,
    pub review_decision: Option<ReviewDecision>,
}

impl StackEntry {
    /// The head conflicts with its base.
    pub fn has_conflicts(&self) -> bool {
        self.mergeable == Mergeable::Conflicting
    }

    /// A reviewer asked for changes.
    pub fn changes_requested(&self) -> bool {
        self.review_decision == Some(ReviewDecision::ChangesRequested)
    }

    pub fn checks_failing(&self) -> bool {
        self.checks == Some(ChecksState::Failing)
    }
}

impl PrStack {
    /// Builds a stack from its entries in any order. Two entries with one
    /// position keep the order they came in.
    pub fn new(number: u64, base_ref: String, mut entries: Vec<StackEntry>) -> Self {
        entries.sort_by_key(|entry| entry.position);
        Self {
            number,
            base_ref,
            entries,
        }
    }

    /// The entry of pull request `number`, if it is in this stack.
    pub fn entry(&self, number: u64) -> Option<&StackEntry> {
        self.entries.iter().find(|entry| entry.number == number)
    }

    /// The open pull requests under `number`, base first: the ones GitHub
    /// merges along with it. Empty when `number` is not in the stack, is its
    /// base, or has nothing open under it.
    pub fn open_below(&self, number: u64) -> Vec<&StackEntry> {
        let Some(position) = self.entry(number).map(|entry| entry.position) else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|entry| entry.position < position && entry.state == PrState::Open)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{PrStack, StackEntry};
    use crate::{ChecksState, Mergeable, PrState, ReviewDecision};

    fn entry(position: u32, number: u64, state: PrState) -> StackEntry {
        StackEntry {
            position,
            number,
            title: format!("Layer {position}"),
            url: format!("https://github.com/acme/web/pull/{number}"),
            state,
            is_draft: false,
            checks: Some(ChecksState::Passing),
            mergeable: Mergeable::Mergeable,
            review_decision: None,
        }
    }

    fn numbers(entries: Vec<&StackEntry>) -> Vec<u64> {
        entries.into_iter().map(|entry| entry.number).collect()
    }

    #[test]
    fn the_top_takes_every_open_pull_request_under_it_base_first() {
        // Entries arrive out of order, as nothing in GraphQL promises one.
        let stack = PrStack::new(
            3,
            "main".into(),
            vec![
                entry(3, 103, PrState::Open),
                entry(1, 101, PrState::Open),
                entry(4, 104, PrState::Open),
                entry(2, 102, PrState::Open),
            ],
        );
        assert_eq!(numbers(stack.open_below(103)), vec![101, 102]);
        assert_eq!(numbers(stack.open_below(104)), vec![101, 102, 103]);
    }

    #[test]
    fn merged_and_closed_pull_requests_under_it_are_left_out() {
        let stack = PrStack::new(
            3,
            "main".into(),
            vec![
                entry(1, 101, PrState::Merged),
                entry(2, 102, PrState::Closed),
                entry(3, 103, PrState::Open),
                entry(4, 104, PrState::Open),
            ],
        );
        assert_eq!(numbers(stack.open_below(104)), vec![103]);
        assert!(stack.open_below(103).is_empty());
    }

    #[test]
    fn the_base_and_a_pull_request_outside_the_stack_take_nothing() {
        let stack = PrStack::new(
            3,
            "main".into(),
            vec![entry(1, 101, PrState::Open), entry(2, 102, PrState::Open)],
        );
        assert!(stack.open_below(101).is_empty());
        assert!(stack.open_below(999).is_empty());
    }

    #[test]
    fn an_entry_reports_what_a_merge_should_warn_about() {
        let mut layer = entry(1, 101, PrState::Open);
        assert!(!layer.has_conflicts() && !layer.changes_requested() && !layer.checks_failing());
        layer.mergeable = Mergeable::Conflicting;
        layer.review_decision = Some(ReviewDecision::ChangesRequested);
        layer.checks = Some(ChecksState::Failing);
        assert!(layer.has_conflicts() && layer.changes_requested() && layer.checks_failing());
    }
}
