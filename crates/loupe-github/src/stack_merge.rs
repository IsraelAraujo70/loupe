//! Stack graph construction, readiness checks, merge planning, and sequential
//! stack merge execution for linear chains and container trees.
//!
//! Two stack shapes are supported:
//! - **Chain** (`A → base`, `B → A`, `C → B`): every PR has at most one child.
//!   Merges bottom-up (`A`, then `B`, then `C`), updating each next PR's base
//!   to `base` when GitHub did not already retarget it.
//! - **Tree with container** (`R → base`, `C1 → R`, `C2 → R`, `G1 → C1`):
//!   at least one PR has two or more children. Merges deepest children first,
//!   each into its own base branch, advancing the parent's expected head OID
//!   after each child merge, and merges the root PR last into `base`.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::merge::MergeResult;
use crate::{
    ChecksState, GitHub, MergeMethod, MergeOutcome, MergeStateStatus, Mergeable, NodeId, PrRef,
    PrState, RepoRef, Result, ReviewDecision, StackSummary,
};

/// How long to wait between polls while GitHub settles a retargeted base branch
/// or synchronizes a parent PR's head after a child merges into it.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Maximum polls before giving up on a base retarget or parent sync (~30s).
const MAX_SETTLE_POLLS: u32 = 30;

/// One open pull request in a repository's stack graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackNode {
    pub id: NodeId,
    pub pr: PrRef,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
    pub base_ref: String,
    pub head_ref: String,
    pub head_oid: String,
    pub cross_repository: bool,
    pub mergeable: Mergeable,
    pub merge_state: MergeStateStatus,
    pub checks: Option<ChecksState>,
    pub review_decision: Option<ReviewDecision>,
    /// True when GitHub reported a non-null `reviewDecision` for this PR's
    /// base branch (i.e. its base branch protection requires approving reviews).
    #[serde(default)]
    pub base_requires_review: bool,
    /// True when at least one status check on this PR is marked `isRequired`
    /// by its base branch protection.
    #[serde(default)]
    pub base_requires_checks: bool,
    pub viewer_can_update: bool,
    /// GitHub native stack summary, if this PR belongs to one.
    #[serde(default)]
    pub stack: Option<StackSummary>,
}

/// The shape of a stack rooted at one PR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackShape {
    /// Every node has at most one child (A → base, B → A, C → B).
    Chain,
    /// At least one node has two or more children (container tree).
    Tree,
}

impl StackShape {
    pub fn label(self) -> &'static str {
        match self {
            StackShape::Chain => "Chain",
            StackShape::Tree => "Tree",
        }
    }
}

/// One PR placed inside a [`StackGraph`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackGraphNode {
    pub node: StackNode,
    /// 0 for the root PR, 1 for its direct children, and so on.
    pub depth: usize,
    /// `None` for the root PR; `Some(parent_number)` for children.
    pub parent: Option<u64>,
}

/// A validated stack of two or more open pull requests rooted at `root`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackGraph {
    /// Target branch of the root PR (e.g. `main` or a release branch).
    pub base_ref: String,
    /// Repository default branch, if reported.
    pub default_branch: Option<String>,
    /// Merge methods allowed by the repository.
    pub merge_methods: Vec<MergeMethod>,
    /// Root PR number (depth 0).
    pub root: u64,
    pub shape: StackShape,
    /// All open PRs in the stack, root first, then descendants in deterministic order.
    pub nodes: Vec<StackGraphNode>,
}

/// One row for the Graphite-style stack panel, ordered top-down to the base branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackDisplayEntry<'a> {
    pub node: &'a StackNode,
    /// 1 at the root (closest to base branch), increasing upward.
    pub position: usize,
    pub total: usize,
    pub depth: usize,
    pub parent: Option<u64>,
}

/// Why a PR in a stack is not ready to merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StackBlocker {
    Draft,
    Conflicts,
    CheckingMergeability,
    ChangesRequested,
    ReviewRequired,
    ChecksFailing,
    ChecksPending,
    ChecksMissing,
    BehindBase,
    BlockedByProtection,
    NoPermission,
    GitHubStackConflict,
}

impl StackBlocker {
    pub fn label(self) -> &'static str {
        match self {
            StackBlocker::Draft => "Draft",
            StackBlocker::Conflicts => "Conflicts",
            StackBlocker::CheckingMergeability => "Checking mergeability",
            StackBlocker::ChangesRequested => "Changes requested",
            StackBlocker::ReviewRequired => "Review required",
            StackBlocker::ChecksFailing => "Checks failing",
            StackBlocker::ChecksPending => "Checks running",
            StackBlocker::ChecksMissing => "No checks",
            StackBlocker::BehindBase => "Behind base",
            StackBlocker::BlockedByProtection => "Blocked",
            StackBlocker::NoPermission => "No permission",
            StackBlocker::GitHubStackConflict => "GitHub stack order",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            StackBlocker::Draft => "pull request is still a draft",
            StackBlocker::Conflicts => "has merge conflicts",
            StackBlocker::CheckingMergeability => "GitHub is still checking mergeability",
            StackBlocker::ChangesRequested => "changes were requested",
            StackBlocker::ReviewRequired => "required review is missing",
            StackBlocker::ChecksFailing => "checks are failing",
            StackBlocker::ChecksPending => "checks are still running",
            StackBlocker::ChecksMissing => "no checks reported on this branch yet",
            StackBlocker::BehindBase => "branch is behind its base",
            StackBlocker::BlockedByProtection => "blocked by branch protection",
            StackBlocker::NoPermission => "you do not have permission to merge it",
            StackBlocker::GitHubStackConflict => {
                "belongs to a GitHub stack with an open base PR that would merge out of order"
            }
        }
    }
}

/// One PR step in a [`StackMergePlan`], in the exact order it will merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackMergeStep {
    pub node: StackNode,
    pub depth: usize,
    pub parent: Option<u64>,
    /// The branch this PR will merge into.
    pub target_base: String,
    /// True for non-root PRs in a Chain: their base branch must be updated to
    /// `target_base` after the previous PR merges if GitHub did not auto-retarget.
    pub needs_retarget: bool,
    /// For Tree children: merging this PR advances the parent PR's head branch,
    /// so the parent's `expected_head_oid` must advance to this merge's commit SHA.
    pub updates_parent_head: Option<u64>,
    /// Reasons this PR cannot merge right now; empty means ready.
    pub blockers: Vec<StackBlocker>,
}

impl StackMergeStep {
    pub fn is_ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// The complete merge plan for a [`StackGraph`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackMergePlan {
    pub repo: RepoRef,
    pub root: u64,
    pub base_ref: String,
    pub shape: StackShape,
    /// Whether the root PR's base branch requires approving reviews (propagated
    /// to child PRs whose feature base branches lack those protection rules).
    pub base_requires_review: bool,
    /// Whether the root PR's base branch requires status checks.
    pub base_requires_checks: bool,
    /// Steps in the exact order they will merge.
    pub steps: Vec<StackMergeStep>,
}

/// Branch protection expectations read from the root PR's target base branch,
/// applied to child PRs before they are retargeted or merged into the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootRequirements<'a> {
    pub base_ref: &'a str,
    pub requires_review: bool,
    pub requires_checks: bool,
}

impl StackGraph {
    /// Builds a validated stack graph rooted at `root_number` from `raw_nodes`.
    /// Returns `None` if `root_number` is missing/not open, or has no open
    /// descendants (a stack has at least 2 open pull requests).
    pub fn build(
        root_number: u64,
        raw_nodes: Vec<StackNode>,
        default_branch: Option<String>,
        merge_methods: Vec<MergeMethod>,
    ) -> Option<Self> {
        let mut by_number: HashMap<u64, StackNode> = HashMap::new();
        for node in raw_nodes {
            if node.state == PrState::Open {
                by_number.insert(node.pr.number, node);
            }
        }
        let root_node = by_number.get(&root_number)?.clone();
        let repo = root_node.pr.repo.clone();
        let base_ref = root_node.base_ref.clone();

        // Keep only open PRs from the same repository.
        by_number.retain(|_, node| node.pr.repo == repo);

        // Count how many open same-repo PRs claim each head branch; an
        // ambiguous head branch shared by two PRs cannot parent children.
        let mut head_owners: HashMap<&str, Vec<u64>> = HashMap::new();
        for node in by_number.values() {
            if !node.cross_repository && !node.head_ref.is_empty() {
                head_owners
                    .entry(node.head_ref.as_str())
                    .or_default()
                    .push(node.pr.number);
            }
        }
        let unique_parent_by_branch: HashMap<String, u64> = head_owners
            .into_iter()
            .filter_map(|(branch, owners)| {
                (owners.len() == 1).then(|| (branch.to_owned(), owners[0]))
            })
            .collect();

        // Map parent PR number -> direct children PR numbers (sorted ascending).
        let mut children_map: HashMap<u64, Vec<u64>> = HashMap::new();
        for node in by_number.values() {
            if node.pr.number == root_number {
                continue;
            }
            if node.base_ref == node.head_ref {
                continue;
            }
            if let Some(&parent) = unique_parent_by_branch.get(&node.base_ref)
                && parent != node.pr.number
            {
                children_map.entry(parent).or_default().push(node.pr.number);
            }
        }
        for kids in children_map.values_mut() {
            kids.sort_unstable();
        }

        // Traverse from root with cycle protection.
        struct Walk<'a> {
            by_number: &'a HashMap<u64, StackNode>,
            children_map: &'a HashMap<u64, Vec<u64>>,
            visited: HashSet<u64>,
            placed: Vec<StackGraphNode>,
            max_children: usize,
        }

        impl Walk<'_> {
            fn visit(&mut self, number: u64, depth: usize, parent: Option<u64>) {
                if !self.visited.insert(number) {
                    return;
                }
                let Some(node) = self.by_number.get(&number) else {
                    return;
                };
                self.placed.push(StackGraphNode {
                    node: node.clone(),
                    depth,
                    parent,
                });
                if let Some(kids) = self.children_map.get(&number) {
                    let unvisited_kids: Vec<u64> = kids
                        .iter()
                        .copied()
                        .filter(|kid| !self.visited.contains(kid))
                        .collect();
                    self.max_children = self.max_children.max(unvisited_kids.len());
                    for kid in unvisited_kids {
                        self.visit(kid, depth + 1, Some(number));
                    }
                }
            }
        }

        let mut walk = Walk {
            by_number: &by_number,
            children_map: &children_map,
            visited: HashSet::new(),
            placed: Vec::new(),
            max_children: 0,
        };
        walk.visit(root_number, 0, None);
        let placed = walk.placed;
        let max_children = walk.max_children;

        if placed.len() < 2 {
            return None;
        }
        let shape = if max_children <= 1 {
            StackShape::Chain
        } else {
            StackShape::Tree
        };
        Some(StackGraph {
            base_ref,
            default_branch,
            merge_methods,
            root: root_number,
            shape,
            nodes: placed,
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn is_root(&self, number: u64) -> bool {
        self.root == number
    }

    pub fn contains(&self, number: u64) -> bool {
        self.nodes.iter().any(|n| n.node.pr.number == number)
    }

    /// Entries for the Graphite-style stack panel, ordered from the top of the
    /// stack down to the root PR (closest to `base_ref`).
    ///
    /// For a chain (`A → base`, `B → A`, `C → B`), returns `[C (3/3), B (2/3), A (1/3)]`.
    /// For a tree, returns post-order DFS so each child sits directly above its
    /// parent and the root sits at the bottom right above `base_ref`.
    pub fn display_entries(&self) -> Vec<StackDisplayEntry<'_>> {
        let total = self.nodes.len();
        let mut by_num: HashMap<u64, &StackGraphNode> = HashMap::new();
        let mut kids: HashMap<u64, Vec<u64>> = HashMap::new();
        for item in &self.nodes {
            by_num.insert(item.node.pr.number, item);
            if let Some(parent) = item.parent {
                kids.entry(parent).or_default().push(item.node.pr.number);
            }
        }
        for list in kids.values_mut() {
            list.sort_unstable();
        }
        // Assign 1-based positions root-first (same order as `self.nodes`).
        let position_of: HashMap<u64, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(ix, item)| (item.node.pr.number, ix + 1))
            .collect();

        let mut out = Vec::with_capacity(total);
        fn post_order<'a>(
            number: u64,
            by_num: &HashMap<u64, &'a StackGraphNode>,
            kids: &HashMap<u64, Vec<u64>>,
            position_of: &HashMap<u64, usize>,
            total: usize,
            out: &mut Vec<StackDisplayEntry<'a>>,
        ) {
            if let Some(children) = kids.get(&number) {
                for &child in children {
                    post_order(child, by_num, kids, position_of, total, out);
                }
            }
            if let Some(item) = by_num.get(&number) {
                out.push(StackDisplayEntry {
                    node: &item.node,
                    position: position_of[&number],
                    total,
                    depth: item.depth,
                    parent: item.parent,
                });
            }
        }
        post_order(self.root, &by_num, &kids, &position_of, total, &mut out);
        out
    }

    /// Computes the exact merge plan (order, target branch per PR, retargeting
    /// flags, parent head updates, and readiness blockers).
    pub fn merge_plan(&self) -> StackMergePlan {
        let root_node = self
            .nodes
            .iter()
            .find(|n| n.node.pr.number == self.root)
            .map(|n| &n.node)
            .unwrap_or(&self.nodes[0].node);
        let repo = root_node.pr.repo.clone();
        let base_requires_review = root_node.base_requires_review;
        let base_requires_checks = root_node.base_requires_checks;
        let root_reqs = RootRequirements {
            base_ref: &self.base_ref,
            requires_review: base_requires_review,
            requires_checks: base_requires_checks,
        };
        let member_numbers: HashSet<u64> = self.nodes.iter().map(|n| n.node.pr.number).collect();

        let mut ordered: Vec<&StackGraphNode> = self.nodes.iter().collect();
        match self.shape {
            StackShape::Chain => {
                // Closest to base branch first (depth 0, 1, 2, ...).
                ordered.sort_by_key(|item| (item.depth, item.node.pr.number));
            }
            StackShape::Tree => {
                // Deepest children first, siblings by PR number ascending, root last.
                ordered.sort_by_key(|item| (Reverse(item.depth), item.node.pr.number));
            }
        }

        let steps = ordered
            .into_iter()
            .map(|item| {
                let (target_base, needs_retarget, updates_parent_head) = match self.shape {
                    StackShape::Chain => (self.base_ref.clone(), item.depth > 0, None),
                    StackShape::Tree => (item.node.base_ref.clone(), false, item.parent),
                };
                let blockers = node_blockers(&item.node, root_reqs, self.shape, &member_numbers);
                StackMergeStep {
                    node: item.node.clone(),
                    depth: item.depth,
                    parent: item.parent,
                    target_base,
                    needs_retarget,
                    updates_parent_head,
                    blockers,
                }
            })
            .collect();

        StackMergePlan {
            repo,
            root: self.root,
            base_ref: self.base_ref.clone(),
            shape: self.shape,
            base_requires_review,
            base_requires_checks,
            steps,
        }
    }
}

/// Evaluates every merge readiness requirement on `node`.
pub fn node_blockers(
    node: &StackNode,
    root_reqs: RootRequirements<'_>,
    shape: StackShape,
    stack_members: &HashSet<u64>,
) -> Vec<StackBlocker> {
    let mut blockers = Vec::new();
    if node.is_draft || node.merge_state == MergeStateStatus::Draft {
        blockers.push(StackBlocker::Draft);
    }
    if node.mergeable == Mergeable::Conflicting || node.merge_state == MergeStateStatus::Dirty {
        blockers.push(StackBlocker::Conflicts);
    } else if node.mergeable == Mergeable::Unknown || node.merge_state == MergeStateStatus::Unknown
    {
        blockers.push(StackBlocker::CheckingMergeability);
    }
    match node.review_decision {
        Some(ReviewDecision::ChangesRequested) => blockers.push(StackBlocker::ChangesRequested),
        Some(ReviewDecision::ReviewRequired) => blockers.push(StackBlocker::ReviewRequired),
        _ => {}
    }
    if node.merge_state == MergeStateStatus::Behind {
        blockers.push(StackBlocker::BehindBase);
    }
    if node.merge_state == MergeStateStatus::Blocked {
        match node.checks {
            Some(ChecksState::Failing) => blockers.push(StackBlocker::ChecksFailing),
            Some(ChecksState::Pending) => blockers.push(StackBlocker::ChecksPending),
            _ => {
                if !blockers.contains(&StackBlocker::ReviewRequired)
                    && !blockers.contains(&StackBlocker::ChangesRequested)
                {
                    blockers.push(StackBlocker::BlockedByProtection);
                }
            }
        }
    } else if node.base_ref != root_reqs.base_ref {
        // Child PRs target another PR's feature branch where the root target's
        // branch protection rules are not evaluated yet: enforce the root
        // base's review and check requirements fail-closed before mutating.
        if root_reqs.requires_review
            && node.review_decision != Some(ReviewDecision::Approved)
            && !blockers.contains(&StackBlocker::ChangesRequested)
            && !blockers.contains(&StackBlocker::ReviewRequired)
        {
            blockers.push(StackBlocker::ReviewRequired);
        }
        match node.checks {
            Some(ChecksState::Failing) => blockers.push(StackBlocker::ChecksFailing),
            Some(ChecksState::Pending) => blockers.push(StackBlocker::ChecksPending),
            None if root_reqs.requires_checks => blockers.push(StackBlocker::ChecksMissing),
            _ => {}
        }
    }
    if !node.viewer_can_update {
        blockers.push(StackBlocker::NoPermission);
    }
    // In a container tree, merging a child that GitHub considers part of a
    // native GitHub stack with an open downstack member in this tree would
    // cause GitHub's async merge to land that ancestor prematurely.
    if shape == StackShape::Tree
        && let Some(native) = &node.stack
        && let Some(pos) = native
            .open
            .iter()
            .find(|m| m.number == node.pr.number)
            .map(|m| m.position)
        && native
            .open
            .iter()
            .any(|m| m.position < pos && stack_members.contains(&m.number))
    {
        blockers.push(StackBlocker::GitHubStackConflict);
    }
    blockers
}

impl StackMergePlan {
    pub fn root_requirements(&self) -> RootRequirements<'_> {
        RootRequirements {
            base_ref: &self.base_ref,
            requires_review: self.base_requires_review,
            requires_checks: self.base_requires_checks,
        }
    }

    pub fn all_ready(&self) -> bool {
        self.steps.iter().all(StackMergeStep::is_ready)
    }

    pub fn blocking_steps(&self) -> Vec<&StackMergeStep> {
        self.steps.iter().filter(|step| !step.is_ready()).collect()
    }

    /// The subset of PR numbers that can safely merge right now without
    /// including any unready PR:
    /// - For a **Chain**: longest contiguous ready prefix starting from root.
    /// - For a **Tree**: every PR whose entire subtree (self + all descendants)
    ///   is ready, kept in the plan's deepest-first merge order.
    pub fn ready_subset(&self) -> Vec<u64> {
        match self.shape {
            StackShape::Chain => self
                .steps
                .iter()
                .take_while(|step| step.is_ready())
                .map(|step| step.node.pr.number)
                .collect(),
            StackShape::Tree => {
                let mut children_of: HashMap<u64, Vec<u64>> = HashMap::new();
                for step in &self.steps {
                    if let Some(parent) = step.parent {
                        children_of
                            .entry(parent)
                            .or_default()
                            .push(step.node.pr.number);
                    }
                }
                // `self.steps` is already deepest-first, so every child is
                // evaluated before its parent.
                let mut eligible: HashMap<u64, bool> = HashMap::new();
                let mut out = Vec::new();
                for step in &self.steps {
                    let num = step.node.pr.number;
                    let all_kids_ready = children_of
                        .get(&num)
                        .is_none_or(|kids| kids.iter().all(|k| eligible.get(k) == Some(&true)));
                    let ok = step.is_ready() && all_kids_ready;
                    eligible.insert(num, ok);
                    if ok {
                        out.push(num);
                    }
                }
                out
            }
        }
    }

    /// The steps matching `selected` PR numbers, in the plan's merge order.
    pub fn selected_steps(&self, selected: &[u64]) -> Vec<StackMergeStep> {
        let wanted: HashSet<u64> = selected.iter().copied().collect();
        self.steps
            .iter()
            .filter(|step| wanted.contains(&step.node.pr.number))
            .cloned()
            .collect()
    }

    /// For a partial Chain merge, the immediately-next unmerged PR above the
    /// last selected PR whose base branch was that last PR's head branch.
    pub fn next_unmerged_chain_step(&self, selected: &[u64]) -> Option<&StackMergeStep> {
        if self.shape != StackShape::Chain {
            return None;
        }
        let wanted: HashSet<u64> = selected.iter().copied().collect();
        let last_selected_ix = self
            .steps
            .iter()
            .rposition(|step| wanted.contains(&step.node.pr.number))?;
        let next = self.steps.get(last_selected_ix + 1)?;
        (!wanted.contains(&next.node.pr.number)).then_some(next)
    }
}

/// Live status of one PR during a stack merge run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Waiting,
    Retargeting,
    WaitingForChecks,
    Merging,
    Merged,
    Failed(String),
}

/// How a failed stack merge run may be retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryPolicy {
    /// Retrying must keep every PR's confirmed (or child-merged) head SHA.
    SameHead,
    /// Only the specific failed non-container PR `number` may adopt a newly
    /// pushed head commit after the user fixes its branch (behind base,
    /// conflicts, failing checks, etc.). Stored inside `StackMergeState` so
    /// transient fetch errors or UI state never transfer it to another PR.
    AllowFailedBranchUpdate { number: u64 },
    /// Head integrity or stack structure failed: retry is forbidden until the
    /// user dismisses, refreshes, and confirms the stack again.
    Abort,
}

/// Reason a stack merge stopped, and whether retrying in place is safe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackMergeFailure {
    pub number: u64,
    pub reason: String,
    pub can_retry: bool,
}

/// Mutable state carried across steps (and across retries) of one stack merge.
#[derive(Clone, Debug)]
pub struct StackMergeState {
    /// Expected head commit SHA for each PR. In a Tree, when a child merges
    /// into its parent's head branch, the parent's expected head advances to
    /// the merge commit SHA GitHub returned.
    pub expected_heads: HashMap<u64, String>,
    /// Valid head SHAs each PR has legitimately held during this run (its
    /// initial SHA plus any merge SHAs we produced by merging its children).
    /// Used while polling for GitHub to synchronize a parent PR's head so we
    /// distinguish sync lag from an external push.
    pub seen_heads: HashMap<u64, HashSet<String>>,
    /// Parents whose child merged without returning a commit SHA, so their
    /// new head cannot be verified safely.
    pub unverified_parents: HashSet<u64>,
    /// PRs already merged in this run (skipped on retry).
    pub merged: HashSet<u64>,
    /// Base branches that have already received at least one merge during this
    /// run (so subsequent PRs targeting them wait for GitHub to recompute
    /// mergeability).
    pub touched_bases: HashSet<String>,
    /// Retry rule set by the most recent failure (if any).
    pub retry_policy: RetryPolicy,
}

impl StackMergeState {
    pub fn from_steps(steps: &[StackMergeStep]) -> Self {
        let mut expected_heads = HashMap::new();
        let mut seen_heads = HashMap::new();
        for step in steps {
            let num = step.node.pr.number;
            expected_heads.insert(num, step.node.head_oid.clone());
            seen_heads.insert(num, HashSet::from([step.node.head_oid.clone()]));
        }
        Self {
            expected_heads,
            seen_heads,
            unverified_parents: HashSet::new(),
            merged: HashSet::new(),
            touched_bases: HashSet::new(),
            retry_policy: RetryPolicy::SameHead,
        }
    }

    /// Updates a PR's expected head SHA after a fresh retry snapshot.
    pub fn reset_head(&mut self, number: u64, head_oid: String) {
        self.expected_heads.insert(number, head_oid.clone());
        self.seen_heads.insert(number, HashSet::from([head_oid]));
        self.unverified_parents.remove(&number);
    }

    /// Records that `step` merged cleanly and updates parent/base tracking.
    pub fn record_merged(&mut self, step: &StackMergeStep, merge_sha: Option<String>) {
        self.merged.insert(step.node.pr.number);
        self.touched_bases.insert(step.target_base.clone());
        self.retry_policy = RetryPolicy::SameHead;
        if let Some(parent) = step.updates_parent_head {
            match merge_sha {
                Some(sha) if !sha.is_empty() => {
                    self.seen_heads
                        .entry(parent)
                        .or_default()
                        .insert(sha.clone());
                    self.expected_heads.insert(parent, sha);
                }
                _ => {
                    self.unverified_parents.insert(parent);
                }
            }
        }
    }

    fn head_advanced(&self, number: u64) -> bool {
        self.seen_heads
            .get(&number)
            .is_some_and(|seen| seen.len() > 1)
    }
}

/// Progress event emitted by [`run_stack_merge`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackMergeEvent {
    Retargeting(u64),
    WaitingForChecks(u64),
    Merging(u64),
    Merged(u64),
    Failed {
        number: u64,
        reason: String,
        can_retry: bool,
    },
}

/// Side-effecting operations needed to execute a stack merge, abstracted so
/// unit tests can drive every success, retarget, race, and failure path.
pub trait StackMergeBackend {
    fn fetch_nodes_by_ids(&self, repo: &RepoRef, ids: &[NodeId]) -> Result<Vec<StackNode>>;
    fn fetch_pr_status(&self, pr: &PrRef) -> Result<StackNode>;
    fn update_base(&self, pull_request_id: &str, base_ref: &str) -> Result<()>;
    fn merge_pr(
        &self,
        pr: &PrRef,
        method: MergeMethod,
        expected_head_oid: &str,
    ) -> Result<MergeResult>;
    fn sleep(&self, duration: Duration);
}

impl StackMergeBackend for GitHub {
    fn fetch_nodes_by_ids(&self, repo: &RepoRef, ids: &[NodeId]) -> Result<Vec<StackNode>> {
        self.stack_nodes_by_ids(repo, ids)
    }

    fn fetch_pr_status(&self, pr: &PrRef) -> Result<StackNode> {
        self.stack_node_status(pr)
    }

    fn update_base(&self, pull_request_id: &str, base_ref: &str) -> Result<()> {
        self.update_pull_request_base(pull_request_id, base_ref)
    }

    fn merge_pr(
        &self,
        pr: &PrRef,
        method: MergeMethod,
        expected_head_oid: &str,
    ) -> Result<MergeResult> {
        self.merge_with_sha(pr, method, expected_head_oid)
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

fn blocker_retry_policy(
    blockers: &[StackBlocker],
    head_advanced: bool,
    number: u64,
) -> RetryPolicy {
    if head_advanced {
        return RetryPolicy::SameHead;
    }
    let needs_branch_change = blockers.iter().any(|b| {
        matches!(
            b,
            StackBlocker::BehindBase
                | StackBlocker::Conflicts
                | StackBlocker::ChecksFailing
                | StackBlocker::ChangesRequested
                | StackBlocker::Draft
        )
    });
    if needs_branch_change {
        RetryPolicy::AllowFailedBranchUpdate { number }
    } else {
        RetryPolicy::SameHead
    }
}

/// One-query pre-flight verification before mutating anything on GitHub:
/// ensures every selected unmerged PR is still open, matches the confirmed
/// `head_oid`, and has no new blockers.
pub fn preflight_stack_merge(
    backend: &dyn StackMergeBackend,
    plan: &StackMergePlan,
    steps: &[StackMergeStep],
    state: &mut StackMergeState,
) -> std::result::Result<(), StackMergeFailure> {
    let remaining: Vec<&StackMergeStep> = steps
        .iter()
        .filter(|s| !state.merged.contains(&s.node.pr.number))
        .collect();
    if remaining.is_empty() {
        return Ok(());
    }
    let ids: Vec<NodeId> = remaining.iter().map(|s| s.node.id.clone()).collect();
    let fresh = backend.fetch_nodes_by_ids(&plan.repo, &ids).map_err(|e| {
        state.retry_policy = RetryPolicy::SameHead;
        StackMergeFailure {
            number: remaining[0].node.pr.number,
            reason: e.to_string(),
            can_retry: true,
        }
    })?;
    let by_num: HashMap<u64, StackNode> = fresh.into_iter().map(|n| (n.pr.number, n)).collect();
    let remaining_members: HashSet<u64> = remaining.iter().map(|s| s.node.pr.number).collect();

    let root_reqs = plan.root_requirements();
    for step in remaining {
        let num = step.node.pr.number;
        let Some(current) = by_num.get(&num) else {
            state.retry_policy = RetryPolicy::Abort;
            return Err(StackMergeFailure {
                number: num,
                reason: format!("Pull request #{num} could not be loaded."),
                can_retry: false,
            });
        };
        if current.state != PrState::Open {
            state.retry_policy = RetryPolicy::Abort;
            return Err(StackMergeFailure {
                number: num,
                reason: format!("Pull request #{num} is no longer open."),
                can_retry: false,
            });
        }
        if let Some(expected) = state.expected_heads.get(&num)
            && &current.head_oid != expected
        {
            state.retry_policy = RetryPolicy::Abort;
            return Err(StackMergeFailure {
                number: num,
                reason: format!(
                    "Pull request #{num} received new commits since confirmation. Dismiss and confirm again."
                ),
                can_retry: false,
            });
        }
        let blockers = node_blockers(current, root_reqs, plan.shape, &remaining_members);
        if !blockers.is_empty() {
            state.retry_policy = blocker_retry_policy(&blockers, state.head_advanced(num), num);
            let reasons: Vec<&str> = blockers.iter().map(|b| b.detail()).collect();
            return Err(StackMergeFailure {
                number: num,
                reason: format!("Pull request #{num} is not ready: {}.", reasons.join(", ")),
                can_retry: true,
            });
        }
    }
    Ok(())
}

/// Refreshes remaining unmerged steps from GitHub on Retry.
///
/// Retrying is forbidden after any head-integrity or structural abort
/// (`RetryPolicy::Abort`). Even on a retryable failure, a new head SHA may ONLY
/// be adopted on the exact PR authorized inside `state.retry_policy`
/// (`RetryPolicy::AllowFailedBranchUpdate { number }`) AND when that PR has not
/// already received child merges during this run (`seen_heads[n].len() <= 1`).
pub fn refresh_for_retry(
    backend: &dyn StackMergeBackend,
    plan: &StackMergePlan,
    steps: &[StackMergeStep],
    state: &mut StackMergeState,
    failed_pr: Option<u64>,
) -> std::result::Result<(), StackMergeFailure> {
    let remaining: Vec<&StackMergeStep> = steps
        .iter()
        .filter(|s| !state.merged.contains(&s.node.pr.number))
        .collect();
    if remaining.is_empty() {
        return Ok(());
    }
    if state.retry_policy == RetryPolicy::Abort {
        return Err(StackMergeFailure {
            number: failed_pr.unwrap_or(remaining[0].node.pr.number),
            reason: "Stack commits changed since confirmation. Dismiss and confirm again.".into(),
            can_retry: false,
        });
    }
    let ids: Vec<NodeId> = remaining.iter().map(|s| s.node.id.clone()).collect();
    let fresh = backend
        .fetch_nodes_by_ids(&plan.repo, &ids)
        .map_err(|e| StackMergeFailure {
            number: remaining[0].node.pr.number,
            reason: e.to_string(),
            can_retry: true,
        })?;
    let by_num: HashMap<u64, StackNode> = fresh.into_iter().map(|n| (n.pr.number, n)).collect();
    let remaining_members: HashSet<u64> = remaining.iter().map(|s| s.node.pr.number).collect();
    let root_reqs = plan.root_requirements();
    let prior_policy = state.retry_policy;

    // In a Tree, if a parent PR was merged externally while one of its child
    // PRs is still unmerged, continuing would merge the child into a dead
    // branch that will never reach the root's base. Abort immediately.
    for step in &remaining {
        let num = step.node.pr.number;
        if by_num.get(&num).is_some_and(|n| n.state == PrState::Merged) {
            let has_unmerged_child = remaining.iter().any(|child| {
                child.updates_parent_head == Some(num)
                    && by_num
                        .get(&child.node.pr.number)
                        .is_none_or(|c| c.state != PrState::Merged)
            });
            if has_unmerged_child {
                state.retry_policy = RetryPolicy::Abort;
                return Err(StackMergeFailure {
                    number: num,
                    reason: format!(
                        "Pull request #{num} was already merged outside this run while child pull requests still target its branch. Dismiss and confirm again."
                    ),
                    can_retry: false,
                });
            }
        }
    }

    for (ix, step) in remaining.iter().enumerate() {
        let num = step.node.pr.number;
        let Some(current) = by_num.get(&num) else {
            state.retry_policy = RetryPolicy::Abort;
            return Err(StackMergeFailure {
                number: num,
                reason: format!("Pull request #{num} could not be loaded."),
                can_retry: false,
            });
        };
        if current.state == PrState::Merged {
            state.merged.insert(num);
            state.touched_bases.insert(step.target_base.clone());
            continue;
        }
        if current.state != PrState::Open {
            state.retry_policy = RetryPolicy::Abort;
            return Err(StackMergeFailure {
                number: num,
                reason: format!("Pull request #{num} is closed."),
                can_retry: false,
            });
        }
        let head_advanced = state.head_advanced(num);
        let allow_new_head =
            prior_policy == RetryPolicy::AllowFailedBranchUpdate { number: num } && !head_advanced;
        if allow_new_head {
            state.reset_head(num, current.head_oid.clone());
        } else if let Some(expected) = state.expected_heads.get(&num)
            && &current.head_oid != expected
        {
            let lagging_known = head_advanced
                && state
                    .seen_heads
                    .get(&num)
                    .is_some_and(|seen| seen.contains(&current.head_oid));
            if !lagging_known {
                state.retry_policy = RetryPolicy::Abort;
                return Err(StackMergeFailure {
                    number: num,
                    reason: format!(
                        "Pull request #{num} received new commits since confirmation. Dismiss and confirm again."
                    ),
                    can_retry: false,
                });
            }
        }
        let check_now = ix == 0 || !step.needs_retarget;
        if check_now {
            let blockers = node_blockers(current, root_reqs, plan.shape, &remaining_members);
            if !blockers.is_empty() {
                state.retry_policy = blocker_retry_policy(&blockers, head_advanced, num);
                let reasons: Vec<&str> = blockers.iter().map(|b| b.detail()).collect();
                return Err(StackMergeFailure {
                    number: num,
                    reason: format!(
                        "Pull request #{num} is still not ready: {}.",
                        reasons.join(", ")
                    ),
                    can_retry: true,
                });
            }
        }
    }
    Ok(())
}

/// Prepares one step right before merging:
/// - In a **Chain** (`step.needs_retarget`): verifies the PR is still open at
///   `expected_head_oid`, updates `baseRefName` to `step.target_base` if GitHub
///   did not auto-retarget it, and waits for GitHub to settle mergeability.
/// - In a **Tree** (when children merged into this PR's head or an earlier
///   sibling merged into this PR's base): verifies no external commit landed,
///   waits for GitHub to sync `head_oid == expected_head` and recompute
///   mergeability, and waits while required checks start/run on the new head.
pub fn prepare_step(
    backend: &dyn StackMergeBackend,
    step: &StackMergeStep,
    state: &StackMergeState,
    on_event: &mut impl FnMut(StackMergeEvent),
) -> std::result::Result<(), (String, RetryPolicy)> {
    let num = step.node.pr.number;
    if state.unverified_parents.contains(&num) {
        return Err((
            format!(
                "Could not verify the new head commit of #{num} after merging its child pull requests."
            ),
            RetryPolicy::Abort,
        ));
    }
    let expected_head = state.expected_heads.get(&num).ok_or_else(|| {
        (
            format!("Missing expected head commit for #{num}."),
            RetryPolicy::Abort,
        )
    })?;

    if step.needs_retarget {
        let mut fresh = backend
            .fetch_pr_status(&step.node.pr)
            .map_err(|e| (e.to_string(), RetryPolicy::SameHead))?;
        check_open_and_head(&fresh, expected_head, state.seen_heads.get(&num), false)?;
        if fresh.base_ref != step.target_base {
            on_event(StackMergeEvent::Retargeting(num));
            backend
                .update_base(&fresh.id, &step.target_base)
                .map_err(|e| {
                    (
                        format!(
                            "Could not update base of #{num} to {}: {e}",
                            step.target_base
                        ),
                        RetryPolicy::SameHead,
                    )
                })?;
            // Give GitHub a beat to begin recalculating mergeability against
            // the new base instead of reading a stale MERGEABLE right away.
            backend.sleep(SETTLE_POLL_INTERVAL);
            fresh = backend
                .fetch_pr_status(&step.node.pr)
                .map_err(|e| (e.to_string(), RetryPolicy::SameHead))?;
        }
        for _ in 0..MAX_SETTLE_POLLS {
            check_open_and_head(&fresh, expected_head, state.seen_heads.get(&num), false)?;
            if fresh.base_ref == step.target_base && fresh.mergeable != Mergeable::Unknown {
                break;
            }
            backend.sleep(SETTLE_POLL_INTERVAL);
            fresh = backend
                .fetch_pr_status(&step.node.pr)
                .map_err(|e| (e.to_string(), RetryPolicy::SameHead))?;
        }
        check_open_and_head(&fresh, expected_head, state.seen_heads.get(&num), false)?;
        if fresh.base_ref != step.target_base {
            return Err((
                format!(
                    "Base branch of #{num} did not update to {} (still {}).",
                    step.target_base, fresh.base_ref
                ),
                RetryPolicy::SameHead,
            ));
        }
        return check_settled_mergeable(&fresh, &step.target_base, false);
    }

    // Tree step (or root of a chain): settle if children merged into this PR's
    // head or an earlier PR merged into this PR's target base.
    let head_advanced = state.head_advanced(num);
    let base_touched = state.touched_bases.contains(&step.target_base);
    if !head_advanced && !base_touched {
        return Ok(());
    }

    let mut fresh = backend
        .fetch_pr_status(&step.node.pr)
        .map_err(|e| (e.to_string(), RetryPolicy::SameHead))?;
    let mut notified_waiting_checks = false;
    for _ in 0..MAX_SETTLE_POLLS {
        check_open_and_head(
            &fresh,
            expected_head,
            state.seen_heads.get(&num),
            head_advanced,
        )?;
        if fresh.base_ref != step.target_base {
            return Err((
                format!(
                    "Base branch of #{num} changed to {} (expected {}).",
                    fresh.base_ref, step.target_base
                ),
                RetryPolicy::Abort,
            ));
        }
        if &fresh.head_oid == expected_head && fresh.mergeable != Mergeable::Unknown {
            // After child merges advance a protected parent/root PR to a new
            // commit, wait while required checks start or run on that commit
            // before either merging or stopping cleanly with retry.
            let waiting_on_new_checks = head_advanced
                && fresh.merge_state == MergeStateStatus::Blocked
                && fresh.review_decision != Some(ReviewDecision::ReviewRequired)
                && fresh.review_decision != Some(ReviewDecision::ChangesRequested)
                && matches!(fresh.checks, Some(ChecksState::Pending) | None);
            if !waiting_on_new_checks {
                break;
            }
            if !notified_waiting_checks {
                notified_waiting_checks = true;
                on_event(StackMergeEvent::WaitingForChecks(num));
            }
        }
        backend.sleep(SETTLE_POLL_INTERVAL);
        fresh = backend
            .fetch_pr_status(&step.node.pr)
            .map_err(|e| (e.to_string(), RetryPolicy::SameHead))?;
    }
    check_open_and_head(
        &fresh,
        expected_head,
        state.seen_heads.get(&num),
        head_advanced,
    )?;
    if fresh.base_ref != step.target_base {
        return Err((
            format!(
                "Base branch of #{num} changed to {} (expected {}).",
                fresh.base_ref, step.target_base
            ),
            RetryPolicy::Abort,
        ));
    }
    if &fresh.head_oid != expected_head {
        return Err((
            format!(
                "Timed out waiting for #{num} to synchronize its head commit after child merges."
            ),
            RetryPolicy::SameHead,
        ));
    }
    check_settled_mergeable(&fresh, &step.target_base, head_advanced)
}

fn check_open_and_head(
    fresh: &StackNode,
    expected_head: &str,
    seen_heads: Option<&HashSet<String>>,
    allow_seen_lag: bool,
) -> std::result::Result<(), (String, RetryPolicy)> {
    let num = fresh.pr.number;
    match fresh.state {
        PrState::Open => {}
        PrState::Merged => {
            return Err((
                format!("Pull request #{num} was already merged outside this run."),
                RetryPolicy::Abort,
            ));
        }
        PrState::Closed => {
            return Err((
                format!("Pull request #{num} was closed."),
                RetryPolicy::Abort,
            ));
        }
    }
    if fresh.head_oid != expected_head {
        let lagging_known =
            allow_seen_lag && seen_heads.is_some_and(|seen| seen.contains(&fresh.head_oid));
        if !lagging_known {
            return Err((
                format!(
                    "Pull request #{num} head branch was modified by another commit. Dismiss and confirm again."
                ),
                RetryPolicy::Abort,
            ));
        }
    }
    Ok(())
}

fn check_settled_mergeable(
    fresh: &StackNode,
    target_base: &str,
    head_advanced: bool,
) -> std::result::Result<(), (String, RetryPolicy)> {
    let num = fresh.pr.number;
    let branch_fix_policy = if head_advanced {
        RetryPolicy::SameHead
    } else {
        RetryPolicy::AllowFailedBranchUpdate { number: num }
    };
    if fresh.is_draft || fresh.merge_state == MergeStateStatus::Draft {
        return Err((
            format!("Pull request #{num} was converted to draft."),
            branch_fix_policy,
        ));
    }
    if fresh.mergeable == Mergeable::Conflicting || fresh.merge_state == MergeStateStatus::Dirty {
        return Err((
            format!("Pull request #{num} has merge conflicts with {target_base}."),
            branch_fix_policy,
        ));
    }
    if fresh.review_decision == Some(ReviewDecision::ChangesRequested) {
        return Err((
            format!("Changes were requested on #{num}."),
            branch_fix_policy,
        ));
    }
    if fresh.merge_state == MergeStateStatus::Behind {
        return Err((
            format!(
                "Pull request #{num} is behind {target_base}. Update the branch, then retry from #{num}."
            ),
            branch_fix_policy,
        ));
    }
    if fresh.merge_state == MergeStateStatus::Blocked {
        if fresh.review_decision == Some(ReviewDecision::ReviewRequired) {
            return Err((
                format!("Review is required on #{num} before merging into {target_base}."),
                RetryPolicy::SameHead,
            ));
        }
        return match fresh.checks {
            Some(ChecksState::Failing) => Err((
                format!("Required checks are failing on #{num}."),
                branch_fix_policy,
            )),
            Some(ChecksState::Pending) | None => Err((
                format!(
                    "Required checks have not passed on #{num} yet. Wait for checks to finish, then retry from #{num}."
                ),
                RetryPolicy::SameHead,
            )),
            Some(ChecksState::Passing) => Err((
                format!("Pull request #{num} is blocked by branch protection on {target_base}."),
                RetryPolicy::SameHead,
            )),
        };
    }
    Ok(())
}

/// Executes the asynchronous merge for one step and updates `state`.
pub fn merge_step(
    backend: &dyn StackMergeBackend,
    step: &StackMergeStep,
    method: MergeMethod,
    state: &mut StackMergeState,
) -> std::result::Result<Option<String>, (String, RetryPolicy)> {
    let num = step.node.pr.number;
    let head_advanced = state.head_advanced(num);
    let expected_head = state.expected_heads.get(&num).cloned().ok_or_else(|| {
        (
            format!("Missing expected head commit for #{num}."),
            RetryPolicy::Abort,
        )
    })?;
    let result = backend
        .merge_pr(&step.node.pr, method, &expected_head)
        .map_err(|e| {
            let policy = if head_advanced {
                RetryPolicy::SameHead
            } else {
                RetryPolicy::AllowFailedBranchUpdate { number: num }
            };
            (e.to_string(), policy)
        })?;
    match result.outcome {
        MergeOutcome::Merged => {
            state.record_merged(step, result.sha.clone());
            Ok(result.sha)
        }
        MergeOutcome::Pending => Err((
            format!("GitHub is still merging #{num}. Wait for it to finish, then retry."),
            RetryPolicy::SameHead,
        )),
        MergeOutcome::Enqueued => Err((
            format!(
                "Pull request #{num} was added to a merge queue instead of merging immediately. Wait for it to land, then retry."
            ),
            RetryPolicy::SameHead,
        )),
    }
}

/// After a partial Chain merge finishes, best-effort retargets the next
/// unmerged PR above the merged prefix to `plan.base_ref` so it does not point
/// at an already-merged branch. Errors are ignored because the selected PRs
/// already merged cleanly.
pub fn retarget_after_partial_chain(
    backend: &dyn StackMergeBackend,
    plan: &StackMergePlan,
    selected: &[u64],
) {
    let Some(next) = plan.next_unmerged_chain_step(selected) else {
        return;
    };
    let Ok(fresh) = backend.fetch_pr_status(&next.node.pr) else {
        return;
    };
    if fresh.state == PrState::Open && fresh.base_ref != plan.base_ref {
        let _ = backend.update_base(&fresh.id, &plan.base_ref);
    }
}

/// Runs a complete (or resumed) stack merge synchronously, emitting events for
/// each step and stopping on the first failure without touching later PRs.
///
/// When `retry_from` is `Some(pr_number)`, refreshes remaining PRs from GitHub
/// honoring [`StackMergeState::retry_policy`]: head-integrity aborts cannot be
/// retried in place, container parents with child merges never adopt unverified
/// external commits, and other remaining PRs stay pinned to their confirmed SHA.
pub fn run_stack_merge(
    backend: &dyn StackMergeBackend,
    plan: &StackMergePlan,
    selected: &[u64],
    method: MergeMethod,
    state: &mut StackMergeState,
    retry_from: Option<u64>,
    mut on_event: impl FnMut(StackMergeEvent),
) -> std::result::Result<(), StackMergeFailure> {
    let steps = plan.selected_steps(selected);
    if steps.is_empty() {
        return Ok(());
    }
    if retry_from.is_some() {
        if let Err(failure) = refresh_for_retry(backend, plan, &steps, state, retry_from) {
            on_event(StackMergeEvent::Failed {
                number: failure.number,
                reason: failure.reason.clone(),
                can_retry: failure.can_retry,
            });
            return Err(failure);
        }
        // Emit Merged for any step that was externally merged before retry.
        for step in &steps {
            let num = step.node.pr.number;
            if state.merged.contains(&num) {
                on_event(StackMergeEvent::Merged(num));
            }
        }
    } else if let Err(failure) = preflight_stack_merge(backend, plan, &steps, state) {
        on_event(StackMergeEvent::Failed {
            number: failure.number,
            reason: failure.reason.clone(),
            can_retry: failure.can_retry,
        });
        return Err(failure);
    }

    for step in &steps {
        let num = step.node.pr.number;
        if state.merged.contains(&num) {
            continue;
        }
        if let Err((reason, policy)) = prepare_step(backend, step, state, &mut on_event) {
            state.retry_policy = policy;
            let can_retry = policy != RetryPolicy::Abort;
            on_event(StackMergeEvent::Failed {
                number: num,
                reason: reason.clone(),
                can_retry,
            });
            return Err(StackMergeFailure {
                number: num,
                reason,
                can_retry,
            });
        }
        on_event(StackMergeEvent::Merging(num));
        match merge_step(backend, step, method, state) {
            Ok(_) => on_event(StackMergeEvent::Merged(num)),
            Err((reason, policy)) => {
                state.retry_policy = policy;
                let can_retry = policy != RetryPolicy::Abort;
                on_event(StackMergeEvent::Failed {
                    number: num,
                    reason: reason.clone(),
                    can_retry,
                });
                return Err(StackMergeFailure {
                    number: num,
                    reason,
                    can_retry,
                });
            }
        }
    }

    retarget_after_partial_chain(backend, plan, selected);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::Duration;

    use super::{
        StackBlocker, StackGraph, StackMergeBackend, StackMergeEvent, StackMergeState, StackNode,
        StackShape, run_stack_merge,
    };
    use crate::merge::MergeResult;
    use crate::{
        ChecksState, Error, MergeMethod, MergeOutcome, MergeStateStatus, Mergeable, NodeId, PrRef,
        PrState, RepoRef, Result, ReviewDecision, StackMember, StackSummary,
    };

    fn repo() -> RepoRef {
        RepoRef::new("acme", "app")
    }

    fn node(number: u64, base: &str, head: &str) -> StackNode {
        StackNode {
            id: format!("PR_{number}"),
            pr: PrRef::new(repo(), number),
            title: format!("PR {number}"),
            state: PrState::Open,
            is_draft: false,
            base_ref: base.into(),
            head_ref: head.into(),
            head_oid: format!("sha_{number}_0"),
            cross_repository: false,
            mergeable: Mergeable::Mergeable,
            merge_state: MergeStateStatus::Clean,
            checks: Some(ChecksState::Passing),
            review_decision: Some(ReviewDecision::Approved),
            base_requires_review: false,
            base_requires_checks: false,
            viewer_can_update: true,
            stack: None,
        }
    }

    type AfterMergeHook = Box<dyn FnMut(u64, &mut HashMap<u64, StackNode>)>;

    #[derive(Default)]
    struct FakeBackend {
        nodes: RefCell<HashMap<u64, StackNode>>,
        calls: RefCell<Vec<String>>,
        /// Optional failure on merge for a PR number.
        fail_merge: RefCell<HashMap<u64, String>>,
        /// Optional one-shot failure on `fetch_nodes_by_ids` (for transient network errors).
        fail_fetch_once: RefCell<Option<String>>,
        /// Optional custom merge outcome (e.g. Pending / Enqueued).
        custom_outcome: RefCell<HashMap<u64, MergeOutcome>>,
        /// If true, merging a chain PR auto-retargets any PR whose base was its head.
        auto_retarget_on_merge: bool,
        /// Optional callback invoked right after a PR merges (to inject races/conflicts).
        after_merge: RefCell<Option<AfterMergeHook>>,
    }

    impl FakeBackend {
        fn new(nodes: Vec<StackNode>) -> Self {
            Self {
                nodes: RefCell::new(nodes.into_iter().map(|n| (n.pr.number, n)).collect()),
                ..Self::default()
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl StackMergeBackend for FakeBackend {
        fn fetch_nodes_by_ids(&self, _repo: &RepoRef, ids: &[NodeId]) -> Result<Vec<StackNode>> {
            if let Some(msg) = self.fail_fetch_once.borrow_mut().take() {
                return Err(Error::GraphQl(vec![msg]));
            }
            let map = self.nodes.borrow();
            Ok(ids
                .iter()
                .filter_map(|id| map.values().find(|n| &n.id == id).cloned())
                .collect())
        }

        fn fetch_pr_status(&self, pr: &PrRef) -> Result<StackNode> {
            self.nodes
                .borrow()
                .get(&pr.number)
                .cloned()
                .ok_or_else(|| Error::GraphQl(vec!["not found".into()]))
        }

        fn update_base(&self, pull_request_id: &str, base_ref: &str) -> Result<()> {
            let mut map = self.nodes.borrow_mut();
            let node = map
                .values_mut()
                .find(|n| n.id == pull_request_id)
                .expect("pr exists");
            self.calls
                .borrow_mut()
                .push(format!("retarget:#{}->{base_ref}", node.pr.number));
            node.base_ref = base_ref.to_owned();
            Ok(())
        }

        fn merge_pr(
            &self,
            pr: &PrRef,
            method: MergeMethod,
            expected_head_oid: &str,
        ) -> Result<MergeResult> {
            self.calls.borrow_mut().push(format!(
                "merge:#{}@{expected_head_oid}:{method:?}",
                pr.number
            ));
            if let Some(reason) = self.fail_merge.borrow().get(&pr.number).cloned() {
                return Err(Error::MergeFailed(reason));
            }
            if let Some(outcome) = self.custom_outcome.borrow().get(&pr.number).copied() {
                return Ok(MergeResult { outcome, sha: None });
            }
            let new_sha = format!("merged_{}", pr.number);
            let (merged_head, merged_base) = {
                let mut map = self.nodes.borrow_mut();
                let current = map.get_mut(&pr.number).expect("pr exists");
                if current.head_oid != expected_head_oid {
                    return Err(Error::MergeFailed(format!(
                        "Pull request head branch was modified (expected {expected_head_oid}, got {}).",
                        current.head_oid
                    )));
                }
                current.state = PrState::Merged;
                (current.head_ref.clone(), current.base_ref.clone())
            };
            {
                let mut map = self.nodes.borrow_mut();
                // Simulate container branch advancing: any open PR whose head_ref == merged_base gets new_sha.
                for other in map.values_mut() {
                    if other.state == PrState::Open && other.head_ref == merged_base {
                        other.head_oid = new_sha.clone();
                    }
                    if self.auto_retarget_on_merge
                        && other.state == PrState::Open
                        && other.base_ref == merged_head
                    {
                        other.base_ref = merged_base.clone();
                    }
                }
                if let Some(hook) = self.after_merge.borrow_mut().as_mut() {
                    hook(pr.number, &mut map);
                }
            }
            Ok(MergeResult {
                outcome: MergeOutcome::Merged,
                sha: Some(new_sha),
            })
        }

        fn sleep(&self, _duration: Duration) {}
    }

    #[test]
    fn chain_merges_bottom_up_retargeting_each_next_pr_to_root_base() {
        let nodes = vec![
            node(10, "release", "a"),
            node(11, "a", "b"),
            node(12, "b", "c"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain builds");
        assert_eq!(graph.shape, StackShape::Chain);
        // Display is top-down: C (3/3), B (2/3), A (1/3).
        let display: Vec<(u64, usize)> = graph
            .display_entries()
            .into_iter()
            .map(|e| (e.node.pr.number, e.position))
            .collect();
        assert_eq!(display, vec![(12, 3), (11, 2), (10, 1)]);

        let plan = graph.merge_plan();
        assert!(plan.all_ready());
        let order: Vec<(u64, &str, bool)> = plan
            .steps
            .iter()
            .map(|s| (s.node.pr.number, s.target_base.as_str(), s.needs_retarget))
            .collect();
        assert_eq!(
            order,
            vec![
                (10, "release", false),
                (11, "release", true),
                (12, "release", true),
            ]
        );

        let backend = FakeBackend::new(nodes);
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();
        let mut state = StackMergeState::from_steps(&plan.steps);
        let mut events = Vec::new();
        run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |e| events.push(e),
        )
        .expect("chain merges cleanly");

        assert_eq!(
            backend.calls(),
            vec![
                "merge:#10@sha_10_0:Squash",
                "retarget:#11->release",
                "merge:#11@sha_11_0:Squash",
                "retarget:#12->release",
                "merge:#12@sha_12_0:Squash",
            ]
        );
        assert_eq!(
            events,
            vec![
                StackMergeEvent::Merging(10),
                StackMergeEvent::Merged(10),
                StackMergeEvent::Retargeting(11),
                StackMergeEvent::Merging(11),
                StackMergeEvent::Merged(11),
                StackMergeEvent::Retargeting(12),
                StackMergeEvent::Merging(12),
                StackMergeEvent::Merged(12),
            ]
        );
    }

    #[test]
    fn tree_with_container_merges_deepest_children_first_and_advances_parent_head_oid() {
        // Container root #10 (epic -> release), children #11, #12, #13 -> epic,
        // and grandchild #14 -> c2 (depth 2).
        let nodes = vec![
            node(10, "release", "epic"),
            node(11, "epic", "c1"),
            node(12, "epic", "c2"),
            node(13, "epic", "c3"),
            node(14, "c2", "g1"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree builds");
        assert_eq!(graph.shape, StackShape::Tree);

        let plan = graph.merge_plan();
        assert!(plan.all_ready());
        let order: Vec<(u64, &str, Option<u64>)> = plan
            .steps
            .iter()
            .map(|s| {
                (
                    s.node.pr.number,
                    s.target_base.as_str(),
                    s.updates_parent_head,
                )
            })
            .collect();
        assert_eq!(
            order,
            vec![
                (14, "c2", Some(12)),
                (11, "epic", Some(10)),
                (12, "epic", Some(10)),
                (13, "epic", Some(10)),
                (10, "release", None),
            ]
        );

        let backend = FakeBackend::new(nodes);
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();
        let mut state = StackMergeState::from_steps(&plan.steps);
        run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect("tree merges cleanly");

        // Notice #12 receives expected_head_oid = merged_14 (produced by #14 -> c2),
        // and #10 receives expected_head_oid = merged_13 (produced by last child #13 -> epic)!
        assert_eq!(
            backend.calls(),
            vec![
                "merge:#14@sha_14_0:Squash",
                "merge:#11@sha_11_0:Squash",
                "merge:#12@merged_14:Squash",
                "merge:#13@sha_13_0:Squash",
                "merge:#10@merged_13:Squash",
            ]
        );
    }

    #[test]
    fn stops_at_first_failure_leaves_earlier_merged_and_retries_from_failed_pr() {
        let nodes = vec![
            node(10, "release", "a"),
            node(11, "a", "b"),
            node(12, "b", "c"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        backend
            .fail_merge
            .borrow_mut()
            .insert(11, "Required status check \"check\" is expected.".into());

        let mut state = StackMergeState::from_steps(&plan.steps);
        let mut events = Vec::new();
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |e| events.push(e),
        )
        .expect_err("must stop at #11");
        assert_eq!(err.number, 11);
        assert!(err.reason.contains("Required status check"));
        assert!(err.can_retry);
        assert!(state.merged.contains(&10));
        assert!(!state.merged.contains(&11));
        assert!(!state.merged.contains(&12));
        // #12 was NEVER touched!
        assert!(backend.calls().iter().all(|c| !c.contains("#12")));

        // User pushes a fix to #11 (new head SHA) and clears the failure, then retries.
        backend.fail_merge.borrow_mut().remove(&11);
        backend.nodes.borrow_mut().get_mut(&11).unwrap().head_oid = "sha_11_fixed".into();
        backend.calls.borrow_mut().clear();

        run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(11),
            |_| {},
        )
        .expect("retry succeeds from #11");

        // #10 was NOT merged again; #11 used the refreshed head SHA; #12 retargeted and merged.
        assert_eq!(
            backend.calls(),
            vec![
                "merge:#11@sha_11_fixed:Squash",
                "retarget:#12->release",
                "merge:#12@sha_12_0:Squash",
            ]
        );
    }

    #[test]
    fn detects_external_commit_on_container_parent_and_refuses_to_merge_it() {
        let nodes = vec![
            node(10, "release", "epic"),
            node(11, "epic", "c1"),
            node(12, "epic", "c2"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        *backend.after_merge.borrow_mut() = Some(Box::new(|merged_pr, map| {
            if merged_pr == 12 {
                // External unreviewed commit pushed onto `epic` right after #12 merged!
                map.get_mut(&10).unwrap().head_oid = "unreviewed_external_sha".into();
            }
        }));
        let mut state = StackMergeState::from_steps(&plan.steps);
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect_err("must reject external commit on #10");
        assert_eq!(err.number, 10);
        assert!(!err.can_retry);
        assert!(err.reason.contains("modified by another commit"));
        assert!(backend.calls().iter().all(|c| !c.starts_with("merge:#10")));

        // Retrying after a head-integrity abort must stay blocked and never merge #10.
        backend.calls.borrow_mut().clear();
        let retry_err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(10),
            |_| {},
        )
        .expect_err("retry after head-integrity abort must fail");
        assert!(!retry_err.can_retry);
        assert!(backend.calls().iter().all(|c| !c.starts_with("merge:#10")));
    }

    #[test]
    fn preflight_aborts_before_merging_anything_when_a_later_pr_changed() {
        let nodes = vec![node(10, "release", "a"), node(11, "a", "b")];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        // Someone pushes a commit to #11 between confirmation and click!
        backend.nodes.borrow_mut().get_mut(&11).unwrap().head_oid = "sneaky_new_sha".into();
        let mut state = StackMergeState::from_steps(&plan.steps);
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect_err("preflight must abort");
        assert_eq!(err.number, 11);
        assert!(!err.can_retry);
        assert!(
            backend.calls().is_empty(),
            "zero PRs merged on preflight failure"
        );

        // Retrying after preflight head mismatch must stay blocked.
        let retry_err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(11),
            |_| {},
        )
        .expect_err("retry after preflight head change must fail");
        assert!(!retry_err.can_retry);
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn pending_or_enqueued_merge_stops_immediately_without_continuing() {
        let nodes = vec![node(10, "release", "a"), node(11, "a", "b")];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        for outcome in [MergeOutcome::Pending, MergeOutcome::Enqueued] {
            let backend = FakeBackend::new(nodes.clone());
            backend.custom_outcome.borrow_mut().insert(10, outcome);
            let mut state = StackMergeState::from_steps(&plan.steps);
            let err = run_stack_merge(
                &backend,
                &plan,
                &selected,
                MergeMethod::Squash,
                &mut state,
                None,
                |_| {},
            )
            .expect_err("must stop on non-merged outcome");
            assert_eq!(err.number, 10);
            assert!(err.can_retry);
            assert!(backend.calls().iter().all(|c| !c.contains("#11")));
        }
    }

    #[test]
    fn readiness_blockers_and_ready_subset_for_chain_and_tree() {
        // Chain: #10 ready, #11 ready, #12 draft, #13 ready -> ready subset is [10, 11].
        let mut n12 = node(12, "b", "c");
        n12.is_draft = true;
        let chain = StackGraph::build(
            10,
            vec![
                node(10, "main", "a"),
                node(11, "a", "b"),
                n12,
                node(13, "c", "d"),
            ],
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = chain.merge_plan();
        assert!(!plan.all_ready());
        assert_eq!(
            plan.blocking_steps()
                .iter()
                .map(|s| (s.node.pr.number, s.blockers.clone()))
                .collect::<Vec<_>>(),
            vec![(12, vec![StackBlocker::Draft])]
        );
        assert_eq!(plan.ready_subset(), vec![10, 11]);

        // Tree: #10 ready, #11 ready (with #14 failing checks), #12 ready, #13 conflicts.
        // Only #12's subtree is completely ready! #11 cannot merge without #14; #10 cannot merge without #11 & #13.
        let mut n13 = node(13, "epic", "c3");
        n13.mergeable = Mergeable::Conflicting;
        let mut n14 = node(14, "c1", "g1");
        n14.checks = Some(ChecksState::Failing);
        let tree = StackGraph::build(
            10,
            vec![
                node(10, "main", "epic"),
                node(11, "epic", "c1"),
                node(12, "epic", "c2"),
                n13,
                n14,
            ],
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree");
        let tree_plan = tree.merge_plan();
        assert!(!tree_plan.all_ready());
        assert_eq!(tree_plan.ready_subset(), vec![12]);
        let blockers: HashMap<u64, Vec<StackBlocker>> = tree_plan
            .blocking_steps()
            .into_iter()
            .map(|s| (s.node.pr.number, s.blockers.clone()))
            .collect();
        assert_eq!(blockers.get(&13), Some(&vec![StackBlocker::Conflicts]));
        assert_eq!(blockers.get(&14), Some(&vec![StackBlocker::ChecksFailing]));
    }

    #[test]
    fn tree_blocks_child_that_shares_github_native_stack_with_open_root() {
        let mut c1 = node(11, "epic", "c1");
        c1.stack = Some(StackSummary {
            number: 1,
            open: vec![
                StackMember {
                    position: 1,
                    number: 10,
                    title: "Root".into(),
                },
                StackMember {
                    position: 2,
                    number: 11,
                    title: "Child 1".into(),
                },
            ],
        });
        let tree = StackGraph::build(
            10,
            vec![node(10, "main", "epic"), c1, node(12, "epic", "c2")],
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree");
        let plan = tree.merge_plan();
        assert!(!plan.all_ready());
        let step11 = plan.steps.iter().find(|s| s.node.pr.number == 11).unwrap();
        assert!(step11.blockers.contains(&StackBlocker::GitHubStackConflict));
    }

    #[test]
    fn partial_chain_merge_best_effort_retargets_next_unmerged_pr() {
        let mut n12 = node(12, "b", "c");
        n12.is_draft = true;
        let nodes = vec![node(10, "release", "a"), node(11, "a", "b"), n12];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        let subset = plan.ready_subset();
        assert_eq!(subset, vec![10, 11]);

        let backend = FakeBackend::new(nodes);
        let mut state = StackMergeState::from_steps(&plan.selected_steps(&subset));
        run_stack_merge(
            &backend,
            &plan,
            &subset,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect("ready bottom merges");
        assert_eq!(
            backend.calls(),
            vec![
                "merge:#10@sha_10_0:Squash",
                "retarget:#11->release",
                "merge:#11@sha_11_0:Squash",
                "retarget:#12->release",
            ]
        );
        assert_eq!(backend.nodes.borrow().get(&12).unwrap().base_ref, "release");
    }

    #[test]
    fn protected_root_propagates_review_and_check_requirements_to_unprotected_child_branches() {
        let mut root = node(10, "main", "a");
        root.base_requires_review = true;
        root.base_requires_checks = true;
        // #11 targets unprotected feature branch `a`, has no review and passing checks.
        let mut child_no_review = node(11, "a", "b");
        child_no_review.review_decision = None;
        // #12 targets unprotected feature branch `b`, is approved but has no checks yet.
        let mut child_no_checks = node(12, "b", "c");
        child_no_checks.checks = None;

        let graph = StackGraph::build(
            10,
            vec![root, child_no_review, child_no_checks],
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        assert!(!plan.all_ready());
        let step11 = plan.steps.iter().find(|s| s.node.pr.number == 11).unwrap();
        assert!(step11.blockers.contains(&StackBlocker::ReviewRequired));
        let step12 = plan.steps.iter().find(|s| s.node.pr.number == 12).unwrap();
        assert!(step12.blockers.contains(&StackBlocker::ChecksMissing));
        assert_eq!(plan.ready_subset(), vec![10]);
    }

    #[test]
    fn retry_rejects_unreviewed_commit_pushed_to_a_different_remaining_pr() {
        let nodes = vec![
            node(10, "release", "a"),
            node(11, "a", "b"),
            node(12, "b", "c"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        backend
            .fail_merge
            .borrow_mut()
            .insert(11, "transient failure".into());
        let mut state = StackMergeState::from_steps(&plan.steps);
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect_err("stops at #11");
        assert_eq!(err.number, 11);
        assert!(err.can_retry);

        // User fixes #11, but someone else pushes an unreviewed commit to #12!
        backend.fail_merge.borrow_mut().remove(&11);
        backend.nodes.borrow_mut().get_mut(&11).unwrap().head_oid = "sha_11_fixed".into();
        backend.nodes.borrow_mut().get_mut(&12).unwrap().head_oid = "sha_12_sneaky".into();
        backend.calls.borrow_mut().clear();

        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(11),
            |_| {},
        )
        .expect_err("retry must reject sneaky commit on #12");
        assert_eq!(err.number, 12);
        assert!(!err.can_retry);
        assert!(err.reason.contains("new commits since confirmation"));
        assert!(
            backend.calls().is_empty(),
            "must not merge #11 when #12 changed externally"
        );

        // A second retry (even pointing at #12) must remain blocked and merge nothing!
        let err2 = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(12),
            |_| {},
        )
        .expect_err("second retry after head-integrity abort must also fail");
        assert!(!err2.can_retry);
        assert!(
            backend.calls().is_empty(),
            "second retry must never accept sha_12_sneaky or merge #11/#12"
        );
    }

    #[test]
    fn tree_root_blocked_by_new_checks_or_behind_stops_before_put_merge() {
        let nodes = vec![
            node(10, "main", "epic"),
            node(11, "epic", "c1"),
            node(12, "epic", "c2"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        *backend.after_merge.borrow_mut() = Some(Box::new(|merged_pr, map| {
            if merged_pr == 12 {
                // New head commit on `epic` triggers pending required checks on `main`.
                let root = map.get_mut(&10).unwrap();
                root.merge_state = MergeStateStatus::Blocked;
                root.checks = Some(ChecksState::Pending);
            }
        }));
        let mut state = StackMergeState::from_steps(&plan.steps);
        let mut events = Vec::new();
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |e| events.push(e),
        )
        .expect_err("must stop cleanly before PUT when root checks stay pending");
        assert_eq!(err.number, 10);
        assert!(err.can_retry);
        assert!(err.reason.contains("Required checks have not passed"));
        assert!(events.contains(&StackMergeEvent::WaitingForChecks(10)));
        assert!(
            backend.calls().iter().all(|c| !c.starts_with("merge:#10")),
            "must not send doomed PUT /merge-async when root is Blocked"
        );

        // If an unreviewed commit X is pushed to the root while checks were
        // pending, retrying from #10 must reject X and refuse to merge #10.
        backend.calls.borrow_mut().clear();
        {
            let mut map = backend.nodes.borrow_mut();
            let root = map.get_mut(&10).unwrap();
            root.head_oid = "unreviewed_commit_x".into();
            root.merge_state = MergeStateStatus::Clean;
            root.checks = Some(ChecksState::Passing);
        }
        let retry_err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(10),
            |_| {},
        )
        .expect_err("retry on container root with external commit X must abort");
        assert_eq!(retry_err.number, 10);
        assert!(!retry_err.can_retry);
        assert!(retry_err.reason.contains("new commits since confirmation"));
        assert!(backend.calls().is_empty());

        // Second retry stays aborted and merges nothing.
        let retry_err2 = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(10),
            |_| {},
        )
        .expect_err("second retry must stay aborted");
        assert!(!retry_err2.can_retry);
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn tree_root_checks_timeout_retries_cleanly_once_checks_pass_on_same_child_merge_sha() {
        let nodes = vec![
            node(10, "main", "epic"),
            node(11, "epic", "c1"),
            node(12, "epic", "c2"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        *backend.after_merge.borrow_mut() = Some(Box::new(|merged_pr, map| {
            if merged_pr == 12 {
                let root = map.get_mut(&10).unwrap();
                root.merge_state = MergeStateStatus::Blocked;
                root.checks = Some(ChecksState::Pending);
            }
        }));
        let mut state = StackMergeState::from_steps(&plan.steps);
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect_err("pauses waiting for checks on #10");
        assert_eq!(err.number, 10);
        assert!(err.can_retry);

        // Checks pass on the exact child-merge commit (`merged_12`): Retry lands #10!
        backend.calls.borrow_mut().clear();
        {
            let mut map = backend.nodes.borrow_mut();
            let root = map.get_mut(&10).unwrap();
            root.merge_state = MergeStateStatus::Clean;
            root.checks = Some(ChecksState::Passing);
        }
        run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(10),
            |_| {},
        )
        .expect("retry succeeds on verified child-merge head");
        assert_eq!(backend.calls(), vec!["merge:#10@merged_12:Squash"]);
    }

    #[test]
    fn transient_retry_fetch_error_never_transfers_branch_update_permission_to_another_pr() {
        let nodes = vec![
            node(10, "main", "a"),
            node(11, "a", "b"),
            node(12, "b", "c"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("chain");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        // Preflight fails on #12 (ChecksFailing) -> authorizes branch updates ONLY on #12.
        backend.nodes.borrow_mut().get_mut(&12).unwrap().checks = Some(ChecksState::Failing);
        let mut state = StackMergeState::from_steps(&plan.steps);
        let err1 = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect_err("preflight stops on #12");
        assert_eq!(err1.number, 12);
        assert!(err1.can_retry);

        // User fixes #12, clicks Retry, and `fetch_nodes_by_ids` hits a transient error
        // (returning `number: 10`, the first remaining PR).
        backend.nodes.borrow_mut().get_mut(&12).unwrap().checks = Some(ChecksState::Passing);
        *backend.fail_fetch_once.borrow_mut() = Some("transient network timeout".into());
        let err2 = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(12),
            |_| {},
        )
        .expect_err("transient fetch error");
        assert_eq!(err2.number, 10);
        assert!(err2.can_retry);

        // Meanwhile someone pushes an unreviewed commit X to #10.
        backend.nodes.borrow_mut().get_mut(&10).unwrap().head_oid = "unreviewed_on_10".into();

        // Even though the UI passes `retry_from = Some(10)`, #10 is NOT authorized
        // to adopt a new head commit (only #12 was); retry must abort with zero merges.
        let err3 = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(10),
            |_| {},
        )
        .expect_err("must reject unreviewed commit on #10");
        assert_eq!(err3.number, 10);
        assert!(!err3.can_retry);
        assert!(err3.reason.contains("new commits since confirmation"));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn retry_aborts_tree_when_parent_pr_was_merged_externally() {
        let nodes = vec![
            node(10, "main", "epic"),
            node(11, "epic", "c1"),
            node(12, "epic", "c2"),
        ];
        let graph = StackGraph::build(
            10,
            nodes.clone(),
            Some("main".into()),
            vec![MergeMethod::Squash],
        )
        .expect("tree");
        let plan = graph.merge_plan();
        let selected: Vec<u64> = plan.steps.iter().map(|s| s.node.pr.number).collect();

        let backend = FakeBackend::new(nodes);
        backend
            .fail_merge
            .borrow_mut()
            .insert(12, "transient failure".into());
        let mut state = StackMergeState::from_steps(&plan.steps);
        let err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            None,
            |_| {},
        )
        .expect_err("stops at #12 after #11 merged");
        assert_eq!(err.number, 12);
        assert!(state.merged.contains(&11));

        // Someone merges root #10 externally on GitHub while #12 still targets `epic`!
        backend.fail_merge.borrow_mut().remove(&12);
        backend.nodes.borrow_mut().get_mut(&10).unwrap().state = PrState::Merged;
        backend.calls.borrow_mut().clear();

        let retry_err = run_stack_merge(
            &backend,
            &plan,
            &selected,
            MergeMethod::Squash,
            &mut state,
            Some(12),
            |_| {},
        )
        .expect_err("must abort instead of merging #12 into already-merged #10");
        assert_eq!(retry_err.number, 10);
        assert!(!retry_err.can_retry);
        assert!(retry_err.reason.contains("already merged outside this run"));
        assert!(
            backend.calls().is_empty(),
            "must never merge #12 into dead branch `epic`"
        );
    }
}
