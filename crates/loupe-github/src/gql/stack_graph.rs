//! GraphQL queries for building and tracking a pull request's full stack
//! graph (linear chains and container trees) from GitHub's open pull requests.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::stack_merge::{StackGraph, StackNode};
use crate::{
    ChecksState, Error, GitHub, MergeMethod, MergeStateStatus, Mergeable, NodeId, PrRef, PrState,
    RepoRef, Result, ReviewDecision, StackMember, StackSummary,
};

/// Maximum open pull requests collected into one stack graph. Guards against
/// runaway BFS on a shared integration branch with hundreds of PRs.
const MAX_STACK_NODES: usize = 50;
/// Maximum downward hops when walking from a child PR to its root PR.
const MAX_ANCESTOR_HOPS: usize = 50;
/// Base branches queried in one batched children request.
const BRANCHES_PER_REQUEST: usize = 25;

macro_rules! stack_node_fragment {
    () => {
        r#"
fragment StackNodeFields on PullRequest {
  id
  number
  title
  state
  isDraft
  baseRefName
  headRefName
  headRefOid
  isCrossRepository
  mergeable
  mergeStateStatus
  reviewDecision
  viewerCanUpdate
  latestReviews(first: 20) {
    nodes {
      state
      author { __typename login }
    }
  }
  commits(last: 1) {
    nodes {
      commit {
        statusCheckRollup { state }
      }
    }
  }
  stack {
    number
    entries(first: 50) {
      nodes {
        position
        pullRequest { number title state }
      }
    }
  }
}
"#
    };
}

const STACK_NEIGHBORS_QUERY: &str = concat!(
    r#"
query PullRequestStackNeighbors(
  $owner: String!
  $name: String!
  $number: Int!
  $baseRef: String!
  $headRef: String!
) {
  repository(owner: $owner, name: $name) {
    defaultBranchRef { name }
    mergeCommitAllowed
    squashMergeAllowed
    rebaseMergeAllowed
    pullRequest(number: $number) {
      ...StackNodeFields
      requiredCheckProbe: commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              contexts(first: 100) {
                nodes {
                  ... on CheckRun { isRequired(pullRequestNumber: $number) }
                  ... on StatusContext { isRequired(pullRequestNumber: $number) }
                }
              }
            }
          }
        }
      }
    }
    parents: pullRequests(headRefName: $baseRef, states: OPEN, first: 10) {
      nodes { ...StackNodeFields }
    }
    children: pullRequests(baseRefName: $headRef, states: OPEN, first: 50) {
      pageInfo { hasNextPage }
      nodes { ...StackNodeFields }
    }
  }
}
"#,
    stack_node_fragment!()
);

const STACK_PARENTS_QUERY: &str = concat!(
    r#"
query PullRequestStackParents($owner: String!, $name: String!, $headRef: String!) {
  repository(owner: $owner, name: $name) {
    pullRequests(headRefName: $headRef, states: OPEN, first: 10) {
      nodes { ...StackNodeFields }
    }
  }
}
"#,
    stack_node_fragment!()
);

const STACK_NODES_BY_IDS_QUERY: &str = concat!(
    r#"
query StackNodesByIds($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on PullRequest { ...StackNodeFields }
  }
}
"#,
    stack_node_fragment!()
);

const STACK_NODE_STATUS_QUERY: &str = concat!(
    r#"
query StackNodeStatus($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) { ...StackNodeFields }
  }
}
"#,
    stack_node_fragment!()
);

/// Batched children lookup for `count` base branch names on one repository.
pub(crate) fn stack_children_query(count: usize) -> String {
    let vars: Vec<String> = (0..count).map(|at| format!("$b{at}: String!")).collect();
    let fields: Vec<String> = (0..count)
        .map(|at| {
            format!(
                "    c{at}: pullRequests(baseRefName: $b{at}, states: OPEN, first: 50) {{ \
                 pageInfo {{ hasNextPage }} nodes {{ ...StackNodeFields }} }}"
            )
        })
        .collect();
    format!(
        "query PullRequestStackChildren($owner: String!, $name: String!, {}) {{\n\
         \x20 repository(owner: $owner, name: $name) {{\n\
         {}\n\
         \x20 }}\n\
         }}\n{}",
        vars.join(", "),
        fields.join("\n"),
        stack_node_fragment!()
    )
}

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[
    ("pull_request_stack_neighbors", STACK_NEIGHBORS_QUERY),
    ("pull_request_stack_parents", STACK_PARENTS_QUERY),
    ("stack_nodes_by_ids", STACK_NODES_BY_IDS_QUERY),
    ("stack_node_status", STACK_NODE_STATUS_QUERY),
];

impl GitHub {
    /// Builds the full open stack graph (chain or container tree) containing
    /// `pr`, rooted at the bottom open pull request of its branch hierarchy.
    /// Returns `Ok(None)` when `pr` is not open, is alone (no open parent or
    /// children), or exceeds the safety cap.
    pub fn pull_request_stack(
        &self,
        pr: &PrRef,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<Option<StackGraph>> {
        let data: NeighborsData = self.graphql(
            STACK_NEIGHBORS_QUERY,
            json!({
                "owner": pr.repo.owner,
                "name": pr.repo.name,
                "number": pr.number,
                "baseRef": base_ref,
                "headRef": head_ref,
            }),
        )?;
        let Some(repo) = data.repository else {
            return Ok(None);
        };
        let Some(current) = repo.pull_request else {
            return Ok(None);
        };
        let current = current.into_node(&pr.repo);
        if current.state != PrState::Open {
            return Ok(None);
        }
        let default_branch = repo.default_branch_ref.map(|r| r.name);
        let merge_methods = allowed_methods(
            repo.merge_commit_allowed,
            repo.squash_merge_allowed,
            repo.rebase_merge_allowed,
        );
        let mut nodes_by_num: HashMap<u64, StackNode> = HashMap::new();
        nodes_by_num.insert(current.pr.number, current.clone());

        let initial_parents: Vec<StackNode> = repo
            .parents
            .into_vec()
            .into_iter()
            .map(|w| w.into_node(&pr.repo))
            .collect();
        let initial_children_truncated = repo
            .children
            .page_info
            .as_ref()
            .is_some_and(|p| p.has_next_page);
        let initial_children: Vec<StackNode> = repo
            .children
            .into_vec()
            .into_iter()
            .map(|w| w.into_node(&pr.repo))
            .collect();

        // Walk downward to find the root PR (if `current` is stacked on another open PR).
        let mut root = current.clone();
        let mut visited_bases: HashSet<String> = HashSet::new();
        visited_bases.insert(current.head_ref.clone());
        let mut parents = initial_parents;
        for _ in 0..MAX_ANCESTOR_HOPS {
            let candidates: Vec<StackNode> = parents
                .into_iter()
                .filter(|p| {
                    p.state == PrState::Open
                        && !p.cross_repository
                        && p.pr.number != root.pr.number
                        && p.head_ref == root.base_ref
                })
                .collect();
            if candidates.len() != 1 {
                break;
            }
            let parent = candidates.into_iter().next().unwrap();
            if !visited_bases.insert(parent.head_ref.clone()) {
                // Cycle detected; stop walking.
                return Ok(None);
            }
            nodes_by_num.insert(parent.pr.number, parent.clone());
            root = parent;
            parents = self.stack_parents(&pr.repo, &root.base_ref)?;
        }

        // Fast path: if `current` was already the root and has no children, it's not a stack.
        if root.pr.number == current.pr.number {
            if initial_children_truncated {
                return Ok(None);
            }
            if initial_children.is_empty() {
                return Ok(None);
            }
        }

        // Walk upward (BFS) from `root.head_ref` to collect every descendant.
        let mut queried_branches: HashSet<String> = HashSet::new();
        let mut frontier: VecDeque<String> = VecDeque::new();
        if root.pr.number == current.pr.number {
            queried_branches.insert(root.head_ref.clone());
            for child in initial_children {
                if child.state == PrState::Open && !nodes_by_num.contains_key(&child.pr.number) {
                    if !child.cross_repository && !child.head_ref.is_empty() {
                        frontier.push_back(child.head_ref.clone());
                    }
                    nodes_by_num.insert(child.pr.number, child);
                }
            }
        } else if !root.cross_repository && !root.head_ref.is_empty() {
            frontier.push_back(root.head_ref.clone());
        }

        while !frontier.is_empty() {
            if nodes_by_num.len() > MAX_STACK_NODES {
                return Ok(None);
            }
            let mut batch: Vec<String> = Vec::new();
            while batch.len() < BRANCHES_PER_REQUEST {
                let Some(branch) = frontier.pop_front() else {
                    break;
                };
                if queried_branches.insert(branch.clone()) {
                    batch.push(branch);
                }
            }
            if batch.is_empty() {
                break;
            }
            let (found, truncated) = self.stack_children(&pr.repo, &batch)?;
            if truncated {
                return Ok(None);
            }
            for child in found {
                if child.state != PrState::Open {
                    continue;
                }
                // The pull request on screen is already known when it sits in
                // the middle of the stack, but the ones built on it are not.
                if !child.cross_repository && !child.head_ref.is_empty() {
                    frontier.push_back(child.head_ref.clone());
                }
                if nodes_by_num.contains_key(&child.pr.number) {
                    continue;
                }
                nodes_by_num.insert(child.pr.number, child);
                if nodes_by_num.len() > MAX_STACK_NODES {
                    return Ok(None);
                }
            }
        }

        let nodes: Vec<StackNode> = nodes_by_num.into_values().collect();
        Ok(StackGraph::build(
            root.pr.number,
            nodes,
            default_branch,
            merge_methods,
        ))
    }

    /// Reads fresh merge-critical fields for the given pull request node ids in
    /// one batched GraphQL call (for pre-flight checks and retry refresh).
    pub fn stack_nodes_by_ids(&self, repo: &RepoRef, ids: &[NodeId]) -> Result<Vec<StackNode>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        #[derive(Deserialize)]
        struct Data {
            nodes: Vec<Value>,
        }
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(100) {
            let data: Data = self.graphql(STACK_NODES_BY_IDS_QUERY, json!({ "ids": chunk }))?;
            for value in data.nodes {
                if value.as_object().is_some_and(|m| !m.is_empty()) {
                    let wire: WireStackNode = serde_json::from_value(value)
                        .map_err(|error| Error::Decode(error.to_string()))?;
                    out.push(wire.into_node(repo));
                }
            }
        }
        Ok(out)
    }

    /// Reads fresh merge-critical fields for a single pull request.
    pub fn stack_node_status(&self, pr: &PrRef) -> Result<StackNode> {
        #[derive(Deserialize)]
        struct Data {
            repository: Option<RepoPr>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RepoPr {
            pull_request: Option<WireStackNode>,
        }
        let data: Data = self.graphql(
            STACK_NODE_STATUS_QUERY,
            json!({
                "owner": pr.repo.owner,
                "name": pr.repo.name,
                "number": pr.number,
            }),
        )?;
        data.repository
            .and_then(|r| r.pull_request)
            .map(|w| w.into_node(&pr.repo))
            .ok_or_else(|| {
                Error::GraphQl(vec![format!(
                    "Pull request #{number} not found",
                    number = pr.number
                )])
            })
    }

    fn stack_parents(&self, repo: &RepoRef, head_ref: &str) -> Result<Vec<StackNode>> {
        #[derive(Deserialize)]
        struct Data {
            repository: Option<RepoParents>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RepoParents {
            pull_requests: WireConnection<WireStackNode>,
        }
        let data: Data = self.graphql(
            STACK_PARENTS_QUERY,
            json!({
                "owner": repo.owner,
                "name": repo.name,
                "headRef": head_ref,
            }),
        )?;
        Ok(data
            .repository
            .map(|r| {
                r.pull_requests
                    .into_vec()
                    .into_iter()
                    .map(|w| w.into_node(repo))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn stack_children(&self, repo: &RepoRef, bases: &[String]) -> Result<(Vec<StackNode>, bool)> {
        let mut vars = serde_json::Map::new();
        vars.insert("owner".into(), json!(repo.owner));
        vars.insert("name".into(), json!(repo.name));
        for (at, base) in bases.iter().enumerate() {
            vars.insert(format!("b{at}"), json!(base));
        }
        #[derive(Deserialize)]
        struct Data {
            repository: Option<HashMap<String, WireConnection<WireStackNode>>>,
        }
        let data: Data = self.graphql(&stack_children_query(bases.len()), Value::Object(vars))?;
        let Some(map) = data.repository else {
            return Ok((Vec::new(), false));
        };
        let mut nodes = Vec::new();
        let mut truncated = false;
        for at in 0..bases.len() {
            if let Some(conn) = map.get(&format!("c{at}")) {
                if conn.page_info.as_ref().is_some_and(|p| p.has_next_page) {
                    truncated = true;
                }
                if let Some(raw) = &conn.nodes {
                    for item in raw.iter().flatten() {
                        nodes.push(item.clone().into_node(repo));
                    }
                }
            }
        }
        Ok((nodes, truncated))
    }
}

fn allowed_methods(merge: bool, squash: bool, rebase: bool) -> Vec<MergeMethod> {
    let mut out = Vec::with_capacity(3);
    if merge {
        out.push(MergeMethod::Merge);
    }
    if squash {
        out.push(MergeMethod::Squash);
    }
    if rebase {
        out.push(MergeMethod::Rebase);
    }
    out
}

#[derive(Deserialize)]
struct NeighborsData {
    repository: Option<WireNeighborsRepo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireNeighborsRepo {
    default_branch_ref: Option<WireRefName>,
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    pull_request: Option<WireStackNode>,
    parents: WireConnection<WireStackNode>,
    children: WireConnection<WireStackNode>,
}

#[derive(Deserialize)]
struct WireRefName {
    name: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePageInfo {
    has_next_page: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireConnection<T> {
    page_info: Option<WirePageInfo>,
    nodes: Option<Vec<Option<T>>>,
}

impl<T> WireConnection<T> {
    fn into_vec(self) -> Vec<T> {
        self.nodes
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect()
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRequiredCheckProbe {
    commit: WireRequiredCommit,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRequiredCommit {
    status_check_rollup: Option<WireRequiredRollup>,
}

#[derive(Clone, Deserialize)]
struct WireRequiredRollup {
    contexts: WireConnection<WireRequiredContext>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRequiredContext {
    #[serde(default)]
    is_required: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStackNode {
    id: String,
    number: u64,
    title: String,
    state: String,
    is_draft: bool,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    is_cross_repository: bool,
    mergeable: String,
    merge_state_status: String,
    review_decision: Option<String>,
    viewer_can_update: bool,
    latest_reviews: WireConnection<WireReview>,
    commits: WireConnection<WireCommitNode>,
    #[serde(default)]
    required_check_probe: Option<WireConnection<WireRequiredCheckProbe>>,
    #[serde(default)]
    stack: Option<WireNativeStack>,
}

#[derive(Clone, Deserialize)]
struct WireReview {
    state: Option<String>,
    author: Option<WireReviewAuthor>,
}

#[derive(Clone, Deserialize)]
struct WireReviewAuthor {
    #[serde(rename = "__typename", default)]
    typename: String,
}

#[derive(Clone, Deserialize)]
struct WireCommitNode {
    commit: WireCommit,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCommit {
    status_check_rollup: Option<WireRollup>,
}

#[derive(Clone, Deserialize)]
struct WireRollup {
    state: String,
}

#[derive(Clone, Deserialize)]
struct WireNativeStack {
    number: u64,
    entries: WireConnection<WireNativeStackEntry>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireNativeStackEntry {
    position: u32,
    pull_request: Option<WireNativeStackPr>,
}

#[derive(Clone, Deserialize)]
struct WireNativeStackPr {
    number: u64,
    title: String,
    state: String,
}

impl WireStackNode {
    fn into_node(self, repo: &RepoRef) -> StackNode {
        let checks = self
            .commits
            .into_vec()
            .into_iter()
            .next()
            .and_then(|c| c.commit.status_check_rollup)
            .and_then(|r| match r.state.as_str() {
                "SUCCESS" => Some(ChecksState::Passing),
                "FAILURE" | "ERROR" => Some(ChecksState::Failing),
                "PENDING" | "EXPECTED" => Some(ChecksState::Pending),
                _ => None,
            });
        let base_requires_review = self.review_decision.is_some();
        let base_requires_checks = self
            .required_check_probe
            .map(WireConnection::into_vec)
            .into_iter()
            .flatten()
            .filter_map(|c| c.commit.status_check_rollup)
            .flat_map(|r| r.contexts.into_vec())
            .any(|ctx| ctx.is_required);
        let latest = self.latest_reviews.into_vec();
        let review_decision = self
            .review_decision
            .as_deref()
            .and_then(|d| match d {
                "APPROVED" => Some(ReviewDecision::Approved),
                "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
                "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequired),
                _ => None,
            })
            .or_else(|| decision_from_reviews(&latest));
        let stack = self.stack.map(|s| {
            let mut open: Vec<StackMember> = s
                .entries
                .into_vec()
                .into_iter()
                .filter_map(|e| {
                    let pr = e.pull_request?;
                    (pr.state == "OPEN").then_some(StackMember {
                        position: e.position,
                        number: pr.number,
                        title: pr.title,
                    })
                })
                .collect();
            open.sort_by_key(|m| m.position);
            StackSummary {
                number: s.number,
                open,
            }
        });
        StackNode {
            id: self.id,
            pr: PrRef::new(repo.clone(), self.number),
            title: self.title,
            state: match self.state.as_str() {
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ => PrState::Open,
            },
            is_draft: self.is_draft,
            base_ref: self.base_ref_name,
            head_ref: self.head_ref_name,
            head_oid: self.head_ref_oid,
            cross_repository: self.is_cross_repository,
            mergeable: match self.mergeable.as_str() {
                "MERGEABLE" => Mergeable::Mergeable,
                "CONFLICTING" => Mergeable::Conflicting,
                _ => Mergeable::Unknown,
            },
            merge_state: match self.merge_state_status.as_str() {
                "BEHIND" => MergeStateStatus::Behind,
                "BLOCKED" => MergeStateStatus::Blocked,
                "CLEAN" => MergeStateStatus::Clean,
                "DIRTY" => MergeStateStatus::Dirty,
                "DRAFT" => MergeStateStatus::Draft,
                "HAS_HOOKS" => MergeStateStatus::HasHooks,
                "UNSTABLE" => MergeStateStatus::Unstable,
                _ => MergeStateStatus::Unknown,
            },
            checks,
            review_decision,
            base_requires_review,
            base_requires_checks,
            viewer_can_update: self.viewer_can_update,
            stack,
        }
    }
}

fn decision_from_reviews(latest: &[WireReview]) -> Option<ReviewDecision> {
    let mut approved = false;
    for review in latest {
        if review.author.as_ref().is_some_and(|a| a.typename == "Bot") {
            continue;
        }
        match review.state.as_deref() {
            Some("CHANGES_REQUESTED") => return Some(ReviewDecision::ChangesRequested),
            Some("APPROVED") => approved = true,
            _ => {}
        }
    }
    approved.then_some(ReviewDecision::Approved)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::WireStackNode;
    use crate::{ChecksState, MergeStateStatus, Mergeable, PrState, RepoRef, ReviewDecision};

    #[test]
    fn parses_wire_stack_node_with_review_fallback_ignoring_bots() {
        let wire: WireStackNode = serde_json::from_value(json!({
            "id": "PR_10",
            "number": 10,
            "title": "Root feature",
            "state": "OPEN",
            "isDraft": false,
            "baseRefName": "main",
            "headRefName": "feat/root",
            "headRefOid": "abc123",
            "isCrossRepository": false,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "reviewDecision": null,
            "viewerCanUpdate": true,
            "latestReviews": { "nodes": [
                { "state": "CHANGES_REQUESTED", "author": { "__typename": "Bot", "login": "bot" } },
                { "state": "APPROVED", "author": { "__typename": "User", "login": "alice" } }
            ] },
            "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "state": "SUCCESS" } } } ] },
            "stack": null
        }))
        .expect("valid wire node");
        let node = wire.into_node(&RepoRef::new("acme", "app"));
        assert_eq!(node.pr.number, 10);
        assert_eq!(node.state, PrState::Open);
        assert_eq!(node.mergeable, Mergeable::Mergeable);
        assert_eq!(node.merge_state, MergeStateStatus::Clean);
        assert_eq!(node.checks, Some(ChecksState::Passing));
        assert_eq!(node.review_decision, Some(ReviewDecision::Approved));
        assert_eq!(node.head_oid, "abc123");
    }
}
