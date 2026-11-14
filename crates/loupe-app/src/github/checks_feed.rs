//! Tells the open Reviews lists when a pull request view loads newer checks,
//! so the Checks column changes with the pull request's page.

use gpui_kit::*;
use loupe_github::{ChecksState, Inbox, PrRef};

/// The checks a pull request view just loaded for `pr`.
#[derive(Clone, Debug)]
pub struct ChecksLoaded {
    pub pr: PrRef,
    pub checks: Option<ChecksState>,
}

pub struct ChecksFeed;

impl EventEmitter<ChecksLoaded> for ChecksFeed {}

struct FeedHandle(Entity<ChecksFeed>);

impl Global for FeedHandle {}

/// The one feed of the app, created on first use.
pub fn feed(cx: &mut App) -> Entity<ChecksFeed> {
    if let Some(handle) = cx.try_global::<FeedHandle>() {
        return handle.0.clone();
    }
    let feed = cx.new(|_| ChecksFeed);
    cx.set_global(FeedHandle(feed.clone()));
    feed
}

impl ChecksLoaded {
    /// Puts the checks on every row of `pr` in `inbox`; whether a row changed.
    pub fn apply(&self, inbox: &mut Inbox) -> bool {
        let mut changed = false;
        for page in [
            &mut inbox.review_requested,
            &mut inbox.authored,
            &mut inbox.involved,
        ] {
            for item in page.items.iter_mut().filter(|item| item.pr == self.pr) {
                changed |= item.checks != self.checks;
                item.checks = self.checks;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_github::{ChecksState, Inbox, Mergeable, Page, PrRef, PrState, PrSummary, RepoRef};

    use super::ChecksLoaded;

    fn summary(number: u64, checks: Option<ChecksState>) -> PrSummary {
        PrSummary {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new("acme", "app"), number),
            title: String::new(),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            author: None,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
            updated_at: DateTime::from_timestamp(0, 0).unwrap(),
            comment_count: 0,
            additions: 0,
            deletions: 0,
            checks,
            review_decision: None,
            head_ref: String::new(),
            base_ref: String::new(),
            cross_repository: false,
            stack: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: Mergeable::Unknown,
        }
    }

    fn page(items: Vec<PrSummary>) -> Page<PrSummary> {
        Page {
            total_count: items.len() as u32,
            items,
            next_cursor: None,
        }
    }

    #[test]
    fn loaded_checks_reach_every_row_of_the_pull_request_and_no_other() {
        let pending = Some(ChecksState::Pending);
        let mut inbox = Inbox {
            review_requested: page(vec![summary(7, pending), summary(8, pending)]),
            authored: page(Vec::new()),
            involved: page(vec![summary(7, pending)]),
        };
        let loaded = ChecksLoaded {
            pr: PrRef::new(RepoRef::new("acme", "app"), 7),
            checks: Some(ChecksState::Passing),
        };
        assert!(loaded.apply(&mut inbox));
        assert_eq!(
            inbox.review_requested.items[0].checks,
            Some(ChecksState::Passing)
        );
        assert_eq!(inbox.involved.items[0].checks, Some(ChecksState::Passing));
        assert_eq!(inbox.review_requested.items[1].checks, pending);
        assert!(!loaded.apply(&mut inbox), "nothing left to change");
    }
}
