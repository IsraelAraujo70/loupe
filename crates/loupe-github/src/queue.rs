//! A repository's open pull requests as a review queue: what each one asks
//! of the viewer, how big it is, and which ones build on each other.

use std::collections::HashMap;

use crate::{PrSummary, RepoRef, ReviewStanding};

/// How big a pull request is, by the lines it changes (additions plus
/// deletions): S under 100, M under 500, L under 2,000, XL from 2,000.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SizeClass {
    S,
    M,
    L,
    Xl,
}

impl SizeClass {
    /// Every class, smallest first.
    pub const ALL: [SizeClass; 4] = [SizeClass::S, SizeClass::M, SizeClass::L, SizeClass::Xl];

    pub fn of(additions: u32, deletions: u32) -> Self {
        match additions.saturating_add(deletions) {
            0..100 => SizeClass::S,
            100..500 => SizeClass::M,
            500..2000 => SizeClass::L,
            _ => SizeClass::Xl,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SizeClass::S => "S",
            SizeClass::M => "M",
            SizeClass::L => "L",
            SizeClass::Xl => "XL",
        }
    }

    /// The lines changed the class covers, in words.
    pub fn range(self) -> &'static str {
        match self {
            SizeClass::S => "under 100 lines changed",
            SizeClass::M => "100 to 499 lines changed",
            SizeClass::L => "500 to 1,999 lines changed",
            SizeClass::Xl => "2,000 lines changed or more",
        }
    }

    /// 1 for S up to 4 for XL.
    pub fn level(self) -> usize {
        self as usize + 1
    }
}

/// Where an open pull request waits in the viewer's review queue. The
/// order of the variants is the queue's: what needs the viewer first, then
/// what may, then what does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueueGroup {
    /// A review is asked of the viewer.
    ReviewRequested,
    /// The viewer reviewed it and the head moved since.
    ChangesSinceReview,
    /// Somebody else's, which the viewer was not asked to review and has
    /// not reviewed.
    NotReviewed,
    /// The viewer wrote it.
    Yours,
    /// The viewer's latest review is on its head.
    Reviewed,
    /// Not ready for review, whoever it is from.
    Draft,
}

impl QueueGroup {
    pub fn of(pr: &PrSummary) -> Self {
        if pr.is_draft {
            return QueueGroup::Draft;
        }
        match pr.viewer_review {
            Some(ReviewStanding::Requested) => QueueGroup::ReviewRequested,
            Some(ReviewStanding::ChangesSinceReview) => QueueGroup::ChangesSinceReview,
            Some(ReviewStanding::Reviewed) => QueueGroup::Reviewed,
            Some(ReviewStanding::Authored) => QueueGroup::Yours,
            None => QueueGroup::NotReviewed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QueueGroup::ReviewRequested => "Review requested",
            QueueGroup::ChangesSinceReview => "Changes since your review",
            QueueGroup::NotReviewed => "Not reviewed",
            QueueGroup::Yours => "Yours",
            QueueGroup::Reviewed => "Reviewed",
            QueueGroup::Draft => "Draft",
        }
    }
}

/// The stacks among `prs`, the open pull requests a repository's list
/// loaded: each one base first (merge order), as indices into `prs`, in the
/// order their first member comes in `prs`. Only stacks of two or more.
///
/// GitHub's own stack where it has one, in its order. Otherwise the chain
/// of branches: a pull request sits on the one whose head branch is its
/// base, when exactly one loaded pull request has that head and only one
/// builds on it, so a stack is one line. A fork's head is never a base.
pub fn stacks(prs: &[&PrSummary]) -> Vec<Vec<usize>> {
    let mut found: Vec<Vec<usize>> = Vec::new();
    let mut native: HashMap<u64, usize> = HashMap::new();
    for (at, pr) in prs.iter().enumerate() {
        let Some(stack) = &pr.stack else {
            continue;
        };
        let slot = *native.entry(stack.number).or_insert_with(|| {
            found.push(Vec::new());
            found.len() - 1
        });
        found[slot].push(at);
    }
    for members in &mut found {
        members.sort_by_key(|&at| {
            prs[at].stack.as_ref().and_then(|stack| {
                stack
                    .open
                    .iter()
                    .find(|member| member.number == prs[at].pr.number)
                    .map(|member| member.position)
            })
        });
    }

    let loose: Vec<usize> = (0..prs.len())
        .filter(|&at| prs[at].stack.is_none())
        .collect();
    let mut heads: HashMap<(&RepoRef, &str), Vec<usize>> = HashMap::new();
    for &at in &loose {
        let pr = prs[at];
        if !pr.cross_repository && !pr.head_ref.is_empty() {
            heads
                .entry((&pr.pr.repo, pr.head_ref.as_str()))
                .or_default()
                .push(at);
        }
    }
    let under = |at: usize| -> Option<usize> {
        let pr = prs[at];
        match heads.get(&(&pr.pr.repo, pr.base_ref.as_str())) {
            Some(under) if under.len() == 1 && under[0] != at => Some(under[0]),
            _ => None,
        }
    };
    let mut built_on: HashMap<usize, Vec<usize>> = HashMap::new();
    for &at in &loose {
        if let Some(base) = under(at) {
            built_on.entry(base).or_default().push(at);
        }
    }
    let next = |at: usize| match built_on.get(&at).map(Vec::as_slice) {
        Some([only]) => Some(*only),
        _ => None,
    };
    let linked_below = |at: usize| under(at).is_some_and(|base| next(base) == Some(at));
    for &root in &loose {
        if linked_below(root) {
            continue;
        }
        let mut chain = vec![root];
        while let Some(up) = next(*chain.last().unwrap_or(&root)) {
            if chain.contains(&up) {
                break;
            }
            chain.push(up);
        }
        if chain.len() > 1 {
            found.push(chain);
        }
    }

    found.retain(|members| members.len() > 1);
    found.sort_by_key(|members| members.iter().min().copied());
    found
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{QueueGroup, SizeClass, stacks};
    use crate::{
        Mergeable, PrRef, PrState, PrSummary, RepoRef, ReviewStanding, StackMember, StackSummary,
    };

    fn pr(number: u64, base: &str, head: &str) -> PrSummary {
        let at = Utc::now();
        PrSummary {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new("a", "repo"), number),
            title: format!("pr {number}"),
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
            review_requests: Vec::new(),
            reviewed_by: Vec::new(),
            viewer_review: None,
        }
    }

    #[test]
    fn size_classes_follow_the_documented_limits() {
        let class = |lines| SizeClass::of(lines, 0);
        assert_eq!(class(0), SizeClass::S);
        assert_eq!(class(99), SizeClass::S);
        assert_eq!(class(100), SizeClass::M);
        assert_eq!(class(499), SizeClass::M);
        assert_eq!(class(500), SizeClass::L);
        assert_eq!(class(1999), SizeClass::L);
        assert_eq!(class(2000), SizeClass::Xl);
        assert_eq!(SizeClass::of(78029, 0).label(), "XL", "the large fixture");
        assert_eq!(SizeClass::of(60, 50), SizeClass::M, "deletions count too");
        assert_eq!(SizeClass::of(u32::MAX, 1), SizeClass::Xl);
        let levels: Vec<usize> = SizeClass::ALL.iter().map(|class| class.level()).collect();
        assert_eq!(levels, [1, 2, 3, 4]);
    }

    #[test]
    fn the_queue_puts_what_needs_the_viewer_first_and_drafts_last() {
        let with = |review: Option<ReviewStanding>, draft: bool| PrSummary {
            viewer_review: review,
            is_draft: draft,
            ..pr(1, "main", "x")
        };
        assert_eq!(
            QueueGroup::of(&with(Some(ReviewStanding::Requested), false)),
            QueueGroup::ReviewRequested
        );
        assert_eq!(
            QueueGroup::of(&with(Some(ReviewStanding::ChangesSinceReview), false)),
            QueueGroup::ChangesSinceReview
        );
        assert_eq!(QueueGroup::of(&with(None, false)), QueueGroup::NotReviewed);
        assert_eq!(
            QueueGroup::of(&with(Some(ReviewStanding::Authored), false)),
            QueueGroup::Yours
        );
        assert_eq!(
            QueueGroup::of(&with(Some(ReviewStanding::Reviewed), false)),
            QueueGroup::Reviewed
        );
        assert_eq!(
            QueueGroup::of(&with(Some(ReviewStanding::Requested), true)),
            QueueGroup::Draft,
            "a draft is not ready, even when a review is asked"
        );
        let mut order = [
            QueueGroup::Draft,
            QueueGroup::Reviewed,
            QueueGroup::Yours,
            QueueGroup::NotReviewed,
            QueueGroup::ChangesSinceReview,
            QueueGroup::ReviewRequested,
        ];
        order.sort();
        assert_eq!(order[0], QueueGroup::ReviewRequested);
        assert_eq!(order[5], QueueGroup::Draft);
    }

    /// The fixture's stack as GitHub lists it, newest first (3, 1, 2):
    /// the chain of branches puts it back in merge order.
    #[test]
    fn a_chain_of_branches_reads_base_first() {
        let prs = [
            pr(196, "main", "refactor"),
            pr(186, "base", "stack-1"),
            pr(188, "stack-2", "stack-3"),
            pr(187, "stack-1", "stack-2"),
            pr(189, "base", "large"),
        ];
        let refs: Vec<&PrSummary> = prs.iter().collect();
        assert_eq!(stacks(&refs), [vec![1, 3, 2]]);
    }

    #[test]
    fn a_branch_two_pull_requests_build_on_ends_the_chain() {
        let prs = [
            pr(1, "main", "a"),
            pr(2, "a", "b"),
            pr(3, "a", "c"),
            pr(4, "b", "d"),
        ];
        let refs: Vec<&PrSummary> = prs.iter().collect();
        assert_eq!(stacks(&refs), [vec![1, 3]]);
    }

    #[test]
    fn a_fork_head_and_a_loop_make_no_stack() {
        let mut fork = pr(1, "main", "a");
        fork.cross_repository = true;
        let prs = [fork, pr(2, "a", "b")];
        let refs: Vec<&PrSummary> = prs.iter().collect();
        assert!(stacks(&refs).is_empty());

        let prs = [pr(1, "b", "a"), pr(2, "a", "b")];
        let refs: Vec<&PrSummary> = prs.iter().collect();
        assert!(stacks(&refs).is_empty(), "a loop has no base");
    }

    #[test]
    fn a_github_stack_keeps_its_own_order() {
        let native = StackSummary {
            number: 7,
            open: [(1, 30), (2, 10), (3, 20)]
                .map(|(position, number)| StackMember {
                    position,
                    number,
                    title: String::new(),
                })
                .to_vec(),
        };
        let member = |number| PrSummary {
            stack: Some(native.clone()),
            ..pr(number, "x", &format!("h{number}"))
        };
        let prs = [member(20), pr(5, "main", "z"), member(10), member(30)];
        let refs: Vec<&PrSummary> = prs.iter().collect();
        assert_eq!(stacks(&refs), [vec![3, 2, 0]]);
    }
}
