//! The GraphQL selection of a pull request's stack and its parsing into
//! [`PrStack`]. A query asks for `stack { ...StackFields }` on a pull request
//! and concatenates [`stack_fragment!`].

use serde::Deserialize;

use crate::{ChecksState, Mergeable, PrStack, PrState, ReviewDecision, StackEntry};

/// Entries read per stack. GitHub has no documented cap on a stack's size;
/// a stack taller than this loses its top entries, never the ones under a
/// pull request near the base.
macro_rules! stack_fragment {
    () => {
        r#"
fragment StackFields on PullRequestStack {
  number
  baseRefName
  entries(first: 50) {
    nodes {
      position
      pullRequest {
        number
        title
        url
        state
        isDraft
        mergeable
        reviewDecision
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
      }
    }
  }
}
"#
    };
}
pub(super) use stack_fragment;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireStack {
    number: u64,
    base_ref_name: String,
    entries: WireNodes<WireEntry>,
}

#[derive(Deserialize)]
struct WireNodes<T> {
    nodes: Option<Vec<Option<T>>>,
}

impl<T> WireNodes<T> {
    fn into_vec(self) -> Vec<T> {
        self.nodes
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEntry {
    position: u32,
    /// Null when the viewer cannot see the pull request.
    pull_request: Option<WirePullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePullRequest {
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    mergeable: String,
    review_decision: Option<String>,
    commits: WireNodes<WireCommitNode>,
}

#[derive(Deserialize)]
struct WireCommitNode {
    commit: WireCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCommit {
    status_check_rollup: Option<WireRollup>,
}

#[derive(Deserialize)]
struct WireRollup {
    state: String,
}

impl WireStack {
    pub(super) fn into_stack(self) -> PrStack {
        let entries = self
            .entries
            .into_vec()
            .into_iter()
            .filter_map(|entry| {
                let pr = entry.pull_request?;
                let checks = pr
                    .commits
                    .into_vec()
                    .into_iter()
                    .next()
                    .and_then(|node| node.commit.status_check_rollup)
                    .map(|rollup| checks_state(&rollup.state));
                Some(StackEntry {
                    position: entry.position,
                    number: pr.number,
                    title: pr.title,
                    url: pr.url,
                    state: match pr.state.as_str() {
                        "MERGED" => PrState::Merged,
                        "CLOSED" => PrState::Closed,
                        _ => PrState::Open,
                    },
                    is_draft: pr.is_draft,
                    checks,
                    mergeable: match pr.mergeable.as_str() {
                        "MERGEABLE" => Mergeable::Mergeable,
                        "CONFLICTING" => Mergeable::Conflicting,
                        _ => Mergeable::Unknown,
                    },
                    review_decision: match pr.review_decision.as_deref() {
                        Some("APPROVED") => Some(ReviewDecision::Approved),
                        Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
                        Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
                        _ => None,
                    },
                })
            })
            .collect();
        PrStack::new(self.number, self.base_ref_name, entries)
    }
}

fn checks_state(state: &str) -> ChecksState {
    match state {
        "SUCCESS" => ChecksState::Passing,
        "FAILURE" | "ERROR" => ChecksState::Failing,
        _ => ChecksState::Pending,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::WireStack;
    use crate::{ChecksState, Mergeable, PrState, ReviewDecision};

    #[test]
    fn a_stack_parses_into_entries_sorted_base_first() {
        let wire: WireStack = serde_json::from_value(json!({
            "number": 37,
            "baseRefName": "release",
            "entries": { "nodes": [
                { "position": 2, "pullRequest": {
                    "number": 35, "title": "Layer B", "url": "https://github.com/acme/web/pull/35",
                    "state": "OPEN", "isDraft": true, "mergeable": "CONFLICTING",
                    "reviewDecision": "CHANGES_REQUESTED",
                    "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "state": "ERROR" } } } ] }
                } },
                { "position": 1, "pullRequest": {
                    "number": 34, "title": "Layer A", "url": "https://github.com/acme/web/pull/34",
                    "state": "MERGED", "isDraft": false, "mergeable": "UNKNOWN",
                    "reviewDecision": null,
                    "commits": { "nodes": [ { "commit": { "statusCheckRollup": null } } ] }
                } },
                { "position": 3, "pullRequest": null },
                null
            ] }
        }))
        .expect("stack JSON");
        let stack = wire.into_stack();

        assert_eq!((stack.number, stack.base_ref.as_str()), (37, "release"));
        let numbers: Vec<u64> = stack.entries.iter().map(|entry| entry.number).collect();
        assert_eq!(numbers, vec![34, 35]);
        let a = &stack.entries[0];
        assert_eq!(
            (a.state, a.checks, a.mergeable, a.review_decision),
            (PrState::Merged, None, Mergeable::Unknown, None)
        );
        let b = &stack.entries[1];
        assert_eq!(b.title, "Layer B");
        assert!(b.is_draft);
        assert_eq!(
            (b.state, b.checks, b.mergeable, b.review_decision),
            (
                PrState::Open,
                Some(ChecksState::Failing),
                Mergeable::Conflicting,
                Some(ReviewDecision::ChangesRequested)
            )
        );
    }
}
