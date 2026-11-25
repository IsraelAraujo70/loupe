//! GraphQL queries for building and tracking a pull request's full stack
//! graph (linear chains and container trees) from GitHub's open pull requests.

use std::collections::{HashSet, VecDeque};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::stack_merge::{StackGraph, StackNode};
use crate::{
    ChecksState, Error, GitHub, MergeMethod, MergeStateStatus, Mergeable, NodeId, PrRef, PrState,
    RepoRef, Result, ReviewDecision, StackMember, StackSummary,
};

/// Maximum open pull requests collected into one stack graph.
const MAX_STACK_NODES: usize = 500;
/// Open pull requests listed per request while looking for a stack's
/// members (GitHub's page maximum).
const OPEN_BRANCHES_PAGE_SIZE: u32 = 100;
/// Pages of open pull requests read at most: a repository with more open
/// pull requests than that shows no stack graph.
const MAX_OPEN_BRANCHES_PAGES: usize = 20;

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

/// The branches of every open pull request of a repository, a page at a
/// time: all a stack walk needs, so a stack of any depth costs one request
/// per hundred open pull requests instead of one per level.
const OPEN_BRANCHES_QUERY: &str = r#"
query OpenPullRequestBranches($owner: String!, $name: String!, $first: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequests(states: OPEN, first: $first, after: $cursor) {
      pageInfo { hasNextPage endCursor }
      nodes { id number baseRefName headRefName isCrossRepository }
    }
  }
}
"#;

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

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[
    ("pull_request_stack_neighbors", STACK_NEIGHBORS_QUERY),
    ("open_pull_request_branches", OPEN_BRANCHES_QUERY),
    ("stack_nodes_by_ids", STACK_NODES_BY_IDS_QUERY),
    ("stack_node_status", STACK_NODE_STATUS_QUERY),
];

impl GitHub {
    /// Builds the full open stack graph (chain or container tree) containing
    /// `pr`, rooted at the bottom open pull request of its branch hierarchy.
    /// Returns `Ok(None)` when `pr` is not open, is alone (no open parent or
    /// children), sits on a loop of branches, or the stack or the
    /// repository's open pull requests exceed the safety caps.
    ///
    /// A pull request alone costs one request. One in a stack also lists the
    /// branches of the repository's open pull requests (one request per
    /// hundred), walks the stack from them, and reads the members (one
    /// request per hundred), whatever the stack's depth.
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
        let parents = repo
            .parents
            .into_vec()
            .into_iter()
            .map(|w| w.into_node(&pr.repo))
            .filter(|p| {
                p.state == PrState::Open
                    && !p.cross_repository
                    && p.pr.number != current.pr.number
                    && p.head_ref == current.base_ref
            })
            .count();
        let has_children = repo
            .children
            .into_vec()
            .into_iter()
            .any(|w| w.into_node(&pr.repo).state == PrState::Open);
        if parents != 1 && !has_children {
            return Ok(None);
        }

        let Some(mut open) = open_branches(|cursor| self.open_branches_page(&pr.repo, cursor))?
        else {
            return Ok(None);
        };
        if open.iter().all(|branch| branch.number != current.pr.number) {
            open.push(OpenBranch {
                id: current.id.clone(),
                number: current.pr.number,
                base_ref_name: current.base_ref.clone(),
                head_ref_name: current.head_ref.clone(),
                is_cross_repository: current.cross_repository,
            });
        }
        let Some((root, members)) = stack_members(current.pr.number, &open) else {
            return Ok(None);
        };
        let ids: Vec<NodeId> = members
            .iter()
            .filter(|member| member.number != current.pr.number)
            .map(|member| member.id.clone())
            .collect();
        let mut nodes = self.stack_nodes_by_ids(&pr.repo, &ids)?;
        nodes.push(current);
        Ok(StackGraph::build(
            root,
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

    fn open_branches_page(
        &self,
        repo: &RepoRef,
        cursor: Option<&str>,
    ) -> Result<WireConnection<OpenBranch>> {
        #[derive(Deserialize)]
        struct Data {
            repository: Option<Repo>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Repo {
            pull_requests: WireConnection<OpenBranch>,
        }
        let data: Data = self.graphql(
            OPEN_BRANCHES_QUERY,
            json!({
                "owner": repo.owner,
                "name": repo.name,
                "first": OPEN_BRANCHES_PAGE_SIZE,
                "cursor": cursor,
            }),
        )?;
        data.repository
            .map(|r| r.pull_requests)
            .ok_or_else(|| Error::Decode(format!("repository {repo} not found")))
    }
}

/// The branches of one open pull request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenBranch {
    id: NodeId,
    number: u64,
    base_ref_name: String,
    head_ref_name: String,
    is_cross_repository: bool,
}

/// Every open pull request's branches, `fetch` returning the page after a
/// cursor (`None` for the first). `None` when the repository has more open
/// pull requests than [`MAX_OPEN_BRANCHES_PAGES`] pages hold, since a stack
/// read from part of them could miss members.
fn open_branches(
    mut fetch: impl FnMut(Option<&str>) -> Result<WireConnection<OpenBranch>>,
) -> Result<Option<Vec<OpenBranch>>> {
    let mut open = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_OPEN_BRANCHES_PAGES {
        let page = fetch(cursor.as_deref())?;
        let next = page
            .page_info
            .as_ref()
            .filter(|info| info.has_next_page)
            .and_then(|info| info.end_cursor.clone());
        open.extend(page.into_vec());
        match next {
            Some(next) => cursor = Some(next),
            None => return Ok(Some(open)),
        }
    }
    Ok(None)
}

/// The stack holding pull request `current` among `open`: its root and its
/// members, root first. The root is reached by following each base to the
/// one open pull request of the repository whose head it is; the members are
/// every open pull request built on the root, level by level (one from a fork
/// joins, but nothing builds on its head). `None` when `current` is alone,
/// sits on a loop of branches, or the stack passes [`MAX_STACK_NODES`].
fn stack_members(current: u64, open: &[OpenBranch]) -> Option<(u64, Vec<&OpenBranch>)> {
    let mut root = open.iter().find(|branch| branch.number == current)?;
    let mut heads: HashSet<&str> = HashSet::from([root.head_ref_name.as_str()]);
    loop {
        let mut parents = open.iter().filter(|p| {
            !p.is_cross_repository
                && p.number != root.number
                && p.head_ref_name == root.base_ref_name
        });
        let (Some(parent), None) = (parents.next(), parents.next()) else {
            break;
        };
        if !heads.insert(parent.head_ref_name.as_str()) {
            return None;
        }
        root = parent;
    }

    let mut members = vec![root];
    let mut known: HashSet<u64> = HashSet::from([root.number]);
    let mut walked: HashSet<&str> = HashSet::new();
    let mut frontier: VecDeque<&str> = VecDeque::new();
    if !root.is_cross_repository && !root.head_ref_name.is_empty() {
        frontier.push_back(&root.head_ref_name);
    }
    while let Some(branch) = frontier.pop_front() {
        if !walked.insert(branch) {
            continue;
        }
        for child in open.iter().filter(|child| child.base_ref_name == branch) {
            if !child.is_cross_repository && !child.head_ref_name.is_empty() {
                frontier.push_back(&child.head_ref_name);
            }
            if known.insert(child.number) {
                members.push(child);
                if members.len() > MAX_STACK_NODES {
                    return None;
                }
            }
        }
    }
    (members.len() > 1).then_some((root.number, members))
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
    #[serde(default)]
    end_cursor: Option<String>,
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
    use serde_json::{Value, json};

    use super::{
        MAX_OPEN_BRANCHES_PAGES, MAX_STACK_NODES, OpenBranch, WireConnection, WireStackNode,
        open_branches, stack_members,
    };
    use crate::{
        ChecksState, Error, MergeStateStatus, Mergeable, PrState, RepoRef, ReviewDecision,
    };

    /// Open pull request `number` from branch `head` into `base`.
    fn branch(number: u64, base: &str, head: &str) -> OpenBranch {
        OpenBranch {
            id: format!("PR_{number}"),
            number,
            base_ref_name: base.into(),
            head_ref_name: head.into(),
            is_cross_repository: false,
        }
    }

    /// A chain of `len` pull requests numbered from 1: #1 into `main`, each
    /// next one on the head of the one before.
    fn chain(len: u64) -> Vec<OpenBranch> {
        (1..=len)
            .map(|n| {
                let base = if n == 1 {
                    "main".to_owned()
                } else {
                    format!("layer-{}", n - 1)
                };
                branch(n, &base, &format!("layer-{n}"))
            })
            .collect()
    }

    /// One page of `OpenPullRequestBranches` holding `branches`.
    fn page(branches: &[OpenBranch], next: Option<&str>) -> WireConnection<OpenBranch> {
        let nodes: Vec<Value> = branches
            .iter()
            .map(|b| {
                json!({
                    "id": b.id,
                    "number": b.number,
                    "baseRefName": b.base_ref_name,
                    "headRefName": b.head_ref_name,
                    "isCrossRepository": b.is_cross_repository,
                })
            })
            .collect();
        serde_json::from_value(json!({
            "pageInfo": { "hasNextPage": next.is_some(), "endCursor": next },
            "nodes": nodes,
        }))
        .unwrap()
    }

    fn numbers(members: &[&OpenBranch]) -> Vec<u64> {
        members.iter().map(|m| m.number).collect()
    }

    #[test]
    fn a_stack_of_120_pull_requests_takes_one_request_per_hundred_open_ones() {
        // 120 stacked pull requests and 30 loose ones into main: 150 open,
        // two pages. Walking the chain level by level took a request per
        // level and gave up at 50.
        let mut all = chain(120);
        all.extend((121..=150).map(|n| branch(n, "main", &format!("loose-{n}"))));
        let mut asked: Vec<Option<String>> = Vec::new();
        let open = open_branches(|cursor| {
            asked.push(cursor.map(str::to_owned));
            Ok(match cursor {
                None => page(&all[..100], Some("after-100")),
                Some("after-100") => page(&all[100..], None),
                Some(other) => panic!("unexpected cursor {other}"),
            })
        })
        .unwrap()
        .expect("every open pull request listed");
        assert_eq!(asked, [None, Some("after-100".to_owned())]);
        assert_eq!(open.len(), 150);

        for current in [1, 60, 120] {
            let (root, members) = stack_members(current, &open).expect("one stack");
            assert_eq!(root, 1, "from #{current}");
            assert_eq!(numbers(&members), (1..=120).collect::<Vec<_>>());
        }
        assert!(stack_members(130, &open).is_none(), "a loose one is alone");
    }

    #[test]
    fn listing_open_pull_requests_stops_at_the_page_cap_and_surfaces_errors() {
        let mut calls = 0;
        let open = open_branches(|_| {
            calls += 1;
            Ok(page(&[branch(calls as u64, "main", "a")], Some("more")))
        })
        .unwrap();
        assert_eq!(calls, MAX_OPEN_BRANCHES_PAGES);
        assert_eq!(
            open, None,
            "a stack read from part of them could miss members"
        );

        let failure = open_branches(|_| {
            Err(Error::Http {
                status: 502,
                message: String::new(),
            })
        });
        assert!(matches!(failure, Err(Error::Http { status: 502, .. })));
    }

    #[test]
    fn a_container_tree_joins_every_branch_built_on_its_root() {
        // 10 into main; 11 and 12 on 10; 13 on 11; a fork's 14 on 10 joins,
        // but 15 on the fork's head (same name as a branch here) does not.
        let mut fork = branch(14, "a", "x");
        fork.is_cross_repository = true;
        let open = [
            branch(10, "main", "a"),
            branch(11, "a", "b"),
            branch(12, "a", "c"),
            branch(13, "b", "d"),
            fork,
            branch(15, "x", "y"),
            branch(20, "main", "z"),
        ];
        let (root, members) = stack_members(13, &open).expect("one stack");
        assert_eq!(root, 10);
        let mut got = numbers(&members);
        got.sort_unstable();
        assert_eq!(got, [10, 11, 12, 13, 14]);
        assert!(stack_members(20, &open).is_none());
    }

    #[test]
    fn an_ambiguous_parent_a_loop_or_a_runaway_stack_is_no_stack_above_it() {
        // Two open pull requests have `a` as head: 12 roots its own stack.
        let shared = [
            branch(10, "main", "a"),
            branch(11, "dev", "a"),
            branch(12, "a", "b"),
            branch(13, "b", "c"),
        ];
        let (root, members) = stack_members(13, &shared).expect("12 and 13");
        assert_eq!((root, numbers(&members)), (12, vec![12, 13]));

        let looped = [
            branch(1, "b", "a"),
            branch(2, "a", "b"),
            branch(3, "a", "c"),
        ];
        assert!(stack_members(3, &looped).is_none());

        let long = chain(MAX_STACK_NODES as u64 + 1);
        assert!(stack_members(1, &long).is_none());
        let fits = chain(MAX_STACK_NODES as u64);
        assert_eq!(stack_members(1, &fits).unwrap().1.len(), MAX_STACK_NODES);
    }

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
