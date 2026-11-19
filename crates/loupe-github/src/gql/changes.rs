//! The cheap question a list asks before loading everything again: which
//! pull requests changed since a time, and what the checks of the pending
//! ones say now. One request costs one rate-limit point, against 18 for the
//! inbox it saves.

use serde::Deserialize;
use serde_json::{Value, json};

use super::reads::checks_state;
use crate::{ChecksState, GitHub, NodeId, Result, Timestamp};

/// Pull requests read per search; more than that changed is "everything".
const STAMPS_PER_SEARCH: u32 = 50;
/// GitHub's limit on `nodes(ids:)`.
const MAX_PENDING: usize = 100;

/// What to ask about: the searches whose results changed since `since`, and
/// the pull requests (node ids) whose checks were pending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangesQuery {
    pub since: Timestamp,
    /// GitHub search text, without `updated:` or `sort:`.
    pub searches: Vec<String>,
    pub pending: Vec<NodeId>,
}

/// A pull request and when it last changed, as search reports it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrStamp {
    pub id: NodeId,
    pub updated_at: Timestamp,
}

/// What [`GitHub::changes_since`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// What each search of the query found updated since the time, in the
    /// query's order.
    pub searches: Vec<Vec<PrStamp>>,
    /// Whether `searches` holds all of them: a search with more than a page
    /// of results leaves some out.
    pub complete: bool,
    /// The checks of each pending pull request that GitHub still knows.
    pub checks: Vec<(NodeId, Option<ChecksState>)>,
}

/// The query for `searches` searches, aliased `s0`, `s1`, …, and the
/// pending pull requests; the search text is a variable, never spliced in.
pub(crate) fn changes_document(searches: usize) -> String {
    let variables: String = (0..searches)
        .map(|at| format!(", $s{at}: String!"))
        .collect();
    let fields: String = (0..searches)
        .map(|at| {
            format!(
                "  s{at}: search(type: ISSUE, first: {STAMPS_PER_SEARCH}, query: $s{at}) {{ \
                 issueCount nodes {{ ... on PullRequest {{ id updatedAt }} }} }}\n"
            )
        })
        .collect();
    format!(
        "query ChangesSince($pending: [ID!]!{variables}) {{\n{fields}  \
         pending: nodes(ids: $pending) {{ ... on PullRequest {{ id \
         commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }} }} }}\n}}"
    )
}

impl Changes {
    /// Every pull request any search found, once, in the order found.
    pub fn updated(&self) -> Vec<&PrStamp> {
        let mut all: Vec<&PrStamp> = Vec::new();
        for stamp in self.searches.iter().flatten() {
            if !all.iter().any(|known| known.id == stamp.id) {
                all.push(stamp);
            }
        }
        all
    }

    /// Whether search `at` of the query found the pull request `id`.
    pub fn found(&self, at: usize, id: &str) -> bool {
        self.searches
            .get(at)
            .is_some_and(|stamps| stamps.iter().any(|stamp| stamp.id == id))
    }
}

impl GitHub {
    /// Which pull requests the searches of `query` found updated since its
    /// time, and the checks of its pending pull requests now.
    pub fn changes_since(&self, query: &ChangesQuery) -> Result<Changes> {
        let since = query.since.format("%Y-%m-%dT%H:%M:%SZ");
        let mut variables = serde_json::Map::new();
        for (at, search) in query.searches.iter().enumerate() {
            variables.insert(
                format!("s{at}"),
                json!(format!("{search} updated:>={since} sort:updated-desc")),
            );
        }
        let pending: Vec<&NodeId> = query.pending.iter().take(MAX_PENDING).collect();
        variables.insert("pending".into(), json!(pending));
        let data: Value = self.graphql(
            &changes_document(query.searches.len()),
            Value::Object(variables),
        )?;
        decode(&data, query.searches.len())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSearch {
    issue_count: u32,
    nodes: Vec<WireStamp>,
}

/// A search node: an empty object for a result that is not a pull request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStamp {
    id: Option<NodeId>,
    updated_at: Option<Timestamp>,
}

#[derive(Deserialize)]
struct WirePending {
    id: Option<NodeId>,
    commits: Option<WireCommits>,
}

#[derive(Deserialize)]
struct WireCommits {
    nodes: Vec<WireCommitNode>,
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

fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    T::deserialize(value).map_err(|error| crate::Error::Decode(error.to_string()))
}

fn decode(data: &Value, searches: usize) -> Result<Changes> {
    let mut changes = Changes {
        complete: true,
        ..Changes::default()
    };
    for at in 0..searches {
        let search: WireSearch = parse(&data[format!("s{at}")])?;
        changes.complete &= search.issue_count as usize <= search.nodes.len();
        changes.searches.push(
            search
                .nodes
                .into_iter()
                .filter_map(|node| {
                    Some(PrStamp {
                        id: node.id?,
                        updated_at: node.updated_at?,
                    })
                })
                .collect(),
        );
    }
    let pending: Vec<Option<WirePending>> = parse(&data["pending"])?;
    changes.checks = pending
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let state = node
                .commits?
                .nodes
                .into_iter()
                .last()
                .and_then(|node| node.commit.status_check_rollup)
                .and_then(|rollup| checks_state(&rollup.state));
            Some((node.id?, state))
        })
        .collect();
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use serde_json::json;

    use super::{PrStamp, decode};
    use crate::ChecksState;

    #[test]
    fn reads_what_each_search_found_and_the_pending_checks() {
        let data = json!({
            "s0": { "issueCount": 2, "nodes": [
                { "id": "PR_1", "updatedAt": "2026-11-19T12:00:00Z" },
                { "id": "PR_2", "updatedAt": "2026-11-19T12:01:00Z" }
            ] },
            "s1": { "issueCount": 2, "nodes": [
                { "id": "PR_2", "updatedAt": "2026-11-19T12:01:00Z" },
                {}
            ] },
            "pending": [
                { "id": "PR_3", "commits": { "nodes": [
                    { "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }
                ] } },
                { "id": "PR_4", "commits": { "nodes": [ { "commit": { "statusCheckRollup": null } } ] } },
                null
            ]
        });
        let changes = decode(&data, 2).unwrap();
        let at = |text: &str| DateTime::parse_from_rfc3339(text).unwrap().to_utc();
        let one = PrStamp {
            id: "PR_1".into(),
            updated_at: at("2026-11-19T12:00:00Z"),
        };
        let two = PrStamp {
            id: "PR_2".into(),
            updated_at: at("2026-11-19T12:01:00Z"),
        };
        assert_eq!(changes.updated(), [&one, &two], "each once");
        assert!(changes.found(1, "PR_2") && !changes.found(1, "PR_1"));
        assert!(!changes.found(2, "PR_1"), "no third search");
        assert!(changes.complete);
        assert_eq!(
            changes.checks,
            [
                ("PR_3".to_owned(), Some(ChecksState::Passing)),
                ("PR_4".to_owned(), None)
            ]
        );
    }

    #[test]
    fn a_search_with_more_than_a_page_is_incomplete() {
        let data = json!({
            "s0": { "issueCount": 51, "nodes": [ { "id": "PR_1", "updatedAt": "2026-11-19T12:00:00Z" } ] },
            "pending": []
        });
        assert!(!decode(&data, 1).unwrap().complete);
    }
}
