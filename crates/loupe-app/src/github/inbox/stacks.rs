//! Pull requests that build on each other, read into stacks so Reviews can
//! show each stack under one row that folds.
//!
//! Membership and order come from GitHub, never from a title: GitHub's own
//! stack (`PullRequest.stack`) where it has one, else the chain of branches,
//! where a pull request is stacked on the one whose head branch is its base
//! when GitHub says it is the one open pull request on that branch.

use std::collections::HashMap;

use loupe_github::{PrRef, PrSummary, RepoRef};

/// Which stack a row belongs to; it outlives a refresh, so a stack opened
/// stays open.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum StackKey {
    /// GitHub's stack `number` in `repo`.
    GitHub { repo: RepoRef, number: u64 },
    /// A chain of branches, by the pull request at its base.
    Branches { base: PrRef },
}

impl StackKey {
    /// A name for the stack's elements, unique on the page and stable
    /// across renders.
    pub fn element_id(&self) -> String {
        match self {
            StackKey::GitHub { repo, number } => {
                format!("stack-{}-{}-{number}", repo.owner, repo.name)
            }
            StackKey::Branches { base } => {
                format!(
                    "stack-{}-{}-pr{}",
                    base.repo.owner, base.repo.name, base.number
                )
            }
        }
    }
}

/// One stack: its open pull requests base first, including those no tab
/// loaded (a GitHub stack lists every one the viewer can see).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stack {
    pub key: StackKey,
    /// The title of the pull request at its base.
    pub name: String,
    pub members: Vec<PrRef>,
}

impl Stack {
    /// Where `pr` sits in the stack, 1 at the base.
    pub fn place(&self, pr: &PrRef) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member == pr)
            .map(|at| at + 1)
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }
}

/// How many open pull requests target each `(repository, branch)`, as
/// GitHub counted them.
pub(super) type OpenByBase = HashMap<(RepoRef, String), u32>;

/// Every stack of two or more open pull requests among the loaded ones.
#[derive(Debug, Default)]
pub(super) struct Stacks {
    stacks: Vec<Stack>,
    of: HashMap<PrRef, usize>,
}

impl Stacks {
    /// The stacks among `prs`, every pull request the page loaded whatever
    /// the filters and the tab, so a stack keeps its order and its size when
    /// a filter hides part of it. A pull request listed twice counts once.
    ///
    /// Branch chains: a pull request is stacked on the open one of its
    /// repository whose head branch is its base (a fork's head is never
    /// one), when `open_by_base` says it is the only open pull request on
    /// that branch: a long-lived branch (`staging`, with a promotion pull
    /// request into `main`) has others, loaded or not, and a branch not
    /// counted yet joins nothing. A branch that two loaded pull requests
    /// build on ends the chain under both, since a stack is one line; so
    /// does a base two pull requests share as their head, and a loop of
    /// branches. A missing link splits the chain: a pull request joins the
    /// one under it only through the one in between. A pull request in a
    /// GitHub stack is never in a branch chain.
    pub fn build<'a>(
        prs: impl IntoIterator<Item = &'a PrSummary>,
        open_by_base: &OpenByBase,
    ) -> Self {
        let mut unique: Vec<&PrSummary> = Vec::new();
        for pr in prs {
            if unique.iter().all(|known| known.pr != pr.pr) {
                unique.push(pr);
            }
        }
        let mut stacks = Stacks::default();
        for pr in &unique {
            let Some(native) = &pr.stack else {
                continue;
            };
            let key = StackKey::GitHub {
                repo: pr.pr.repo.clone(),
                number: native.number,
            };
            if let Some(at) = stacks.stacks.iter().position(|stack| stack.key == key) {
                if stacks.stacks[at].place(&pr.pr).is_some() {
                    stacks.of.insert(pr.pr.clone(), at);
                }
                continue;
            }
            let members: Vec<PrRef> = native
                .open
                .iter()
                .map(|member| PrRef::new(pr.pr.repo.clone(), member.number))
                .collect();
            if members.len() < 2 || !members.contains(&pr.pr) {
                continue;
            }
            let name = native
                .open
                .first()
                .map_or_else(|| pr.title.clone(), |base| base.title.clone());
            stacks.stacks.push(Stack { key, name, members });
            stacks.of.insert(pr.pr.clone(), stacks.stacks.len() - 1);
        }
        let loose: Vec<&PrSummary> = unique.into_iter().filter(|pr| pr.stack.is_none()).collect();
        for chain in branch_chains(&loose, open_by_base) {
            let base = chain[0];
            stacks.stacks.push(Stack {
                key: StackKey::Branches {
                    base: base.pr.clone(),
                },
                name: base.title.clone(),
                members: chain.iter().map(|pr| pr.pr.clone()).collect(),
            });
            let at = stacks.stacks.len() - 1;
            for pr in chain {
                stacks.of.insert(pr.pr.clone(), at);
            }
        }
        stacks
    }

    /// The stack `pr` is in, if any.
    pub fn of(&self, pr: &PrRef) -> Option<&Stack> {
        self.of.get(pr).map(|&at| &self.stacks[at])
    }
}

/// The head branches among `prs` that another of them is based on: those
/// GitHub must count open pull requests on before a chain can use them.
pub(super) fn bases_to_count<'a>(
    prs: impl IntoIterator<Item = &'a PrSummary>,
) -> Vec<(RepoRef, String)> {
    let prs: Vec<&PrSummary> = prs.into_iter().filter(|pr| pr.stack.is_none()).collect();
    let mut bases: Vec<(RepoRef, String)> = Vec::new();
    for head in &prs {
        if head.cross_repository || head.head_ref.is_empty() {
            continue;
        }
        let built_on = prs.iter().any(|pr| {
            pr.pr != head.pr && pr.pr.repo == head.pr.repo && pr.base_ref == head.head_ref
        });
        let base = (head.pr.repo.clone(), head.head_ref.clone());
        if built_on && !bases.contains(&base) {
            bases.push(base);
        }
    }
    bases
}

/// The chains of two or more pull requests among `prs`, each base first,
/// in the order their bases come in `prs`.
fn branch_chains<'a>(prs: &[&'a PrSummary], open_by_base: &OpenByBase) -> Vec<Vec<&'a PrSummary>> {
    let mut heads: HashMap<(&RepoRef, &str), Vec<usize>> = HashMap::new();
    for (at, pr) in prs.iter().enumerate() {
        if !pr.cross_repository && !pr.head_ref.is_empty() {
            heads
                .entry((&pr.pr.repo, pr.head_ref.as_str()))
                .or_default()
                .push(at);
        }
    }
    let alone_on =
        |pr: &PrSummary| open_by_base.get(&(pr.pr.repo.clone(), pr.base_ref.clone())) == Some(&1);
    // The one pull request each one is based on, if exactly one has its
    // base as head.
    let under: Vec<Option<usize>> = prs
        .iter()
        .enumerate()
        .map(
            |(at, pr)| match heads.get(&(&pr.pr.repo, pr.base_ref.as_str())) {
                Some(under) if under.len() == 1 && under[0] != at => Some(under[0]),
                _ => None,
            },
        )
        .collect();
    // Following the bases from a pull request on a loop of branches, or
    // above one, never reaches a bottom: it has no place in a chain.
    let looped: Vec<bool> = (0..prs.len())
        .map(|start| {
            let mut at = start;
            for _ in 0..prs.len() {
                match under[at] {
                    Some(next) => at = next,
                    None => return false,
                }
            }
            true
        })
        .collect();
    // A link holds when GitHub counts no other open pull request on the
    // branch.
    let parent: Vec<Option<usize>> = under
        .iter()
        .enumerate()
        .map(|(at, under)| under.filter(|_| !looped[at] && alone_on(prs[at])))
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); prs.len()];
    for (at, under) in parent.iter().enumerate() {
        if let Some(under) = under {
            children[*under].push(at);
        }
    }
    let linked_up = |at: usize| parent[at].is_some_and(|under| children[under].len() == 1);
    let mut placed = vec![false; prs.len()];
    let mut chains = Vec::new();
    for start in (0..prs.len()).filter(|&at| !looped[at] && !linked_up(at)) {
        let mut chain = vec![prs[start]];
        placed[start] = true;
        let mut at = start;
        while let [only] = children[at][..] {
            if placed[only] {
                break;
            }
            placed[only] = true;
            chain.push(prs[only]);
            at = only;
        }
        if chain.len() > 1 {
            chains.push(chain);
        }
    }
    chains
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use loupe_github::{Mergeable, PrRef, PrState, PrSummary, RepoRef, StackMember, StackSummary};

    use super::{OpenByBase, StackKey, Stacks, bases_to_count};

    /// A pull request from branch `head` into `base` in acme/web.
    fn pr(number: u64, base: &str, head: &str) -> PrSummary {
        let at = Utc::now();
        PrSummary {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new("acme", "web"), number),
            title: format!("Title {number}"),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            author: None,
            created_at: at,
            updated_at: at,
            comment_count: 0,
            additions: 0,
            deletions: 0,
            checks: None,
            review_decision: None,
            head_ref: head.into(),
            base_ref: base.into(),
            cross_repository: false,
            stack: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: Mergeable::Unknown,
        }
    }

    /// What GitHub would count if `prs` were every open pull request.
    fn counted(prs: &[PrSummary]) -> OpenByBase {
        let mut counts = OpenByBase::new();
        let mut seen: Vec<&PrRef> = Vec::new();
        for pr in prs.iter().filter(|pr| {
            let new = !seen.contains(&&pr.pr);
            seen.push(&pr.pr);
            new
        }) {
            *counts
                .entry((pr.pr.repo.clone(), pr.base_ref.clone()))
                .or_default() += 1;
        }
        counts
    }

    /// Each stack's members as numbers, base first, in the order of the
    /// first pull request of `prs` that is in it.
    fn chains(prs: &[PrSummary]) -> Vec<Vec<u64>> {
        chains_with(prs, &counted(prs))
    }

    fn chains_with(prs: &[PrSummary], open_by_base: &OpenByBase) -> Vec<Vec<u64>> {
        let stacks = Stacks::build(prs, open_by_base);
        let mut seen: Vec<StackKey> = Vec::new();
        let mut out = Vec::new();
        for pr in prs {
            if let Some(stack) = stacks.of(&pr.pr)
                && !seen.contains(&stack.key)
            {
                seen.push(stack.key.clone());
                out.push(stack.members.iter().map(|member| member.number).collect());
            }
        }
        out
    }

    #[test]
    fn a_chain_of_branches_reads_base_first_whatever_the_order_it_came_in() {
        let prs = [
            pr(12, "b", "c"),
            pr(30, "main", "loose"),
            pr(10, "main", "a"),
            pr(11, "a", "b"),
        ];
        assert_eq!(chains(&prs), vec![vec![10, 11, 12]]);
        let stacks = Stacks::build(&prs, &counted(&prs));
        let stack = stacks.of(&prs[0].pr).expect("12 is stacked");
        assert_eq!(stack.name, "Title 10", "named after its base");
        assert_eq!(stack.place(&prs[0].pr), Some(3));
        assert_eq!(stack.len(), 3);
        assert!(
            stacks.of(&prs[1].pr).is_none(),
            "a loose pull request stays out"
        );
    }

    #[test]
    fn a_missing_link_does_not_join_the_pull_requests_around_it() {
        // 11 (a → b) is not loaded: 10 and 12 are not one stack.
        let prs = [pr(10, "main", "a"), pr(12, "b", "c"), pr(13, "c", "d")];
        assert_eq!(chains(&prs), vec![vec![12, 13]]);
        assert!(Stacks::build(&prs, &counted(&prs)).of(&prs[0].pr).is_none());
    }

    #[test]
    fn a_fork_in_the_chain_or_a_fork_repository_ends_it() {
        // Two pull requests build on `a`: neither joins 10.
        let forked = [pr(10, "main", "a"), pr(11, "a", "b"), pr(12, "a", "c")];
        assert!(chains(&forked).is_empty());
        // Past the fork, a branch with one pull request on it is a stack.
        let forked = [
            pr(10, "main", "a"),
            pr(11, "a", "b"),
            pr(12, "a", "c"),
            pr(13, "b", "d"),
        ];
        assert_eq!(chains(&forked), vec![vec![11, 13]]);
        // Counts read before the second pull request opened on `a` still
        // say one: the loaded pull requests end the chain all the same.
        let mut stale = counted(&forked);
        stale.insert((RepoRef::new("acme", "web"), "a".into()), 1);
        assert_eq!(chains_with(&forked, &stale), vec![vec![11, 13]]);
        // A fork's head named like a branch of the repository is not it.
        let mut fork = pr(20, "main", "x");
        fork.cross_repository = true;
        let prs = [fork, pr(21, "x", "y")];
        assert!(chains(&prs).is_empty());
        // Another repository's branch of the same name is not it either.
        let mut other = pr(31, "a", "b");
        other.pr = PrRef::new(RepoRef::new("acme", "api"), 31);
        assert!(chains(&[pr(30, "main", "a"), other]).is_empty());
    }

    #[test]
    fn a_cycle_of_branches_is_no_stack_and_a_pull_request_counts_once() {
        let cycle = [pr(1, "b", "a"), pr(2, "a", "b")];
        assert!(chains(&cycle).is_empty());
        let twice = [pr(10, "main", "a"), pr(11, "a", "b"), pr(11, "a", "b")];
        assert_eq!(chains(&twice), vec![vec![10, 11]]);
    }

    fn in_github_stack(mut pr: PrSummary, number: u64, open: &[u64]) -> PrSummary {
        pr.stack = Some(StackSummary {
            number,
            open: open
                .iter()
                .enumerate()
                .map(|(at, &number)| StackMember {
                    position: at as u32 + 2,
                    number,
                    title: format!("Layer {number}"),
                })
                .collect(),
        });
        pr
    }

    #[test]
    fn github_stacks_win_over_branches_and_count_members_no_tab_loaded() {
        // GitHub says 7, 5, 9 from the base, whatever the branches say.
        let prs = [
            in_github_stack(pr(9, "main", "z"), 3, &[7, 5, 9]),
            in_github_stack(pr(7, "z", "q"), 3, &[7, 5, 9]),
            pr(40, "main", "a"),
            pr(41, "a", "b"),
        ];
        assert_eq!(chains(&prs), vec![vec![7, 5, 9], vec![40, 41]]);
        let stacks = Stacks::build(&prs, &counted(&prs));
        let stack = stacks.of(&prs[0].pr).expect("9 is stacked");
        assert_eq!(stack.name, "Layer 7");
        assert_eq!(
            stack.key,
            StackKey::GitHub {
                repo: RepoRef::new("acme", "web"),
                number: 3
            }
        );
        assert_eq!(stack.len(), 3, "5 counts though no tab loaded it");
        // A GitHub stack with one open pull request left is no stack.
        let alone = [in_github_stack(pr(8, "main", "y"), 4, &[8])];
        assert!(chains(&alone).is_empty());
    }

    #[test]
    fn a_branch_with_other_open_pull_requests_on_it_stacks_nothing() {
        // A promotion pull request (develop into main) and a feature into
        // develop, while GitHub counts 9 open pull requests on develop that
        // no tab loaded: not a stack.
        let prs = [pr(5, "main", "develop"), pr(6, "develop", "feat")];
        let mut counts = counted(&prs);
        counts.insert((RepoRef::new("acme", "web"), "develop".into()), 9);
        assert!(chains_with(&prs, &counts).is_empty());
        // Not counted yet: not a stack either.
        assert!(chains_with(&prs, &OpenByBase::new()).is_empty());
        // The only one on it: a stack.
        assert_eq!(chains(&prs), vec![vec![5, 6]]);
        assert_eq!(
            bases_to_count(&prs),
            vec![(RepoRef::new("acme", "web"), "develop".to_owned())]
        );
    }

    #[test]
    fn a_loop_of_branches_stacks_nothing_above_it_either() {
        let prs = [pr(1, "b", "a"), pr(2, "a", "b"), pr(3, "a", "c")];
        assert!(chains(&prs).is_empty());
    }

    #[test]
    fn a_pull_request_in_a_github_stack_is_in_no_branch_chain() {
        // GitHub's stack keeps 7 though only one of its layers is open; 8 is
        // based on its head.
        let prs = [
            in_github_stack(pr(7, "main", "a"), 3, &[7]),
            pr(8, "a", "b"),
        ];
        assert!(chains(&prs).is_empty());
        assert!(bases_to_count(&prs).is_empty());
    }
}
