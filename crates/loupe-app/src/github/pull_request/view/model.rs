//! Pure view-model logic of the pull request view: tabs, which actions apply
//! to a loaded detail, labels, and review form validation.

use loupe_github::{
    CheckRun, CheckStatus, ChecksState, MergeMethod, MergeStateStatus, Mergeable, PrState,
    PullRequestDetail, ReviewDecision, ReviewEvent, ReviewerState, StackEntry, TimelineItem,
};

use crate::github::pull_request::checks::format_duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrTab {
    Conversation,
    Files,
    Commits,
    Checks,
}

impl PrTab {
    pub(super) const ALL: [PrTab; 4] = [
        PrTab::Conversation,
        PrTab::Files,
        PrTab::Commits,
        PrTab::Checks,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            PrTab::Conversation => "Conversation",
            PrTab::Files => "Files",
            PrTab::Commits => "Commits",
            PrTab::Checks => "Checks",
        }
    }

    pub(super) fn icon(self) -> gpui_kit::assets::IconName {
        use gpui_kit::assets::IconName;
        match self {
            PrTab::Conversation => IconName::MessageSquare,
            PrTab::Files => IconName::FileDiff,
            PrTab::Commits => IconName::GitCommitHorizontal,
            PrTab::Checks => IconName::ListChecks,
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }

    pub(super) fn from_index(index: usize) -> Option<PrTab> {
        Self::ALL.get(index).copied()
    }

    /// The files tab gives the whole width to the diff.
    pub(super) fn has_side_column(self) -> bool {
        self != PrTab::Files
    }

    /// The count shown next to the tab title.
    pub(super) fn count(self, detail: &PullRequestDetail) -> Option<usize> {
        match self {
            PrTab::Conversation => None,
            PrTab::Files => Some(detail.changed_files as usize),
            PrTab::Commits => Some(detail.commits.len()),
            PrTab::Checks => Some(detail.checks.runs.len()),
        }
    }
}

/// The one inline panel open under the toolbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Panel {
    #[default]
    Closed,
    Review,
    ConfirmDiscard,
    ConfirmMerge(MergeMethod),
    ConfirmClose,
}

impl Panel {
    /// Whether the panel still makes sense for a freshly loaded detail: a
    /// reload can remove the pending review or make merging unavailable.
    pub(super) fn still_valid(self, detail: &PullRequestDetail) -> bool {
        match self {
            Panel::Closed | Panel::Review => true,
            Panel::ConfirmDiscard => detail.pending_review.is_some(),
            Panel::ConfirmMerge(method) => matches!(
                available_actions(detail).merge,
                MergeAction::Available { methods, .. } if methods.contains(&method)
            ),
            Panel::ConfirmClose => available_actions(detail).state == Some(StateAction::Close),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MergeAction {
    /// Not offered: not open, or the viewer cannot update the pull request.
    Hidden,
    Available {
        methods: Vec<MergeMethod>,
        /// A caution to show next to the button.
        note: Option<&'static str>,
    },
    /// Offered but impossible now; the reason is shown as muted text.
    Unavailable(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DraftAction {
    MarkReady,
    ConvertToDraft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StateAction {
    Close,
    Reopen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PrActions {
    pub(super) merge: MergeAction,
    pub(super) draft: Option<DraftAction>,
    pub(super) state: Option<StateAction>,
}

impl PrActions {
    const NONE: PrActions = PrActions {
        merge: MergeAction::Hidden,
        draft: None,
        state: None,
    };
}

/// The write actions that apply to a loaded pull request.
pub(super) fn available_actions(detail: &PullRequestDetail) -> PrActions {
    if !detail.viewer_can_update {
        return PrActions::NONE;
    }
    match detail.state {
        PrState::Merged => PrActions::NONE,
        PrState::Closed => PrActions {
            state: Some(StateAction::Reopen),
            ..PrActions::NONE
        },
        PrState::Open => PrActions {
            merge: merge_action(detail),
            draft: Some(if detail.is_draft {
                DraftAction::MarkReady
            } else {
                DraftAction::ConvertToDraft
            }),
            state: Some(StateAction::Close),
        },
    }
}

fn merge_action(detail: &PullRequestDetail) -> MergeAction {
    if detail.is_draft {
        return MergeAction::Unavailable("Draft pull requests cannot be merged");
    }
    if detail.mergeable == Mergeable::Conflicting || detail.merge_state == MergeStateStatus::Dirty {
        return MergeAction::Unavailable("This branch has conflicts that must be resolved");
    }
    if detail.mergeable == Mergeable::Unknown {
        return MergeAction::Unavailable("GitHub is still checking whether this can be merged");
    }
    match detail.merge_state {
        MergeStateStatus::Blocked => {
            return MergeAction::Unavailable("Blocked: required reviews or checks are missing");
        }
        MergeStateStatus::Behind => {
            return MergeAction::Unavailable("This branch is behind the base branch");
        }
        _ => {}
    }
    if detail.merge_methods.is_empty() {
        return MergeAction::Unavailable("The repository allows no merge method");
    }
    MergeAction::Available {
        methods: detail.merge_methods.clone(),
        note: (detail.merge_state == MergeStateStatus::Unstable)
            .then_some("Some checks are not passing"),
    }
}

/// How the merge box reads: its icon color and weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MergeTone {
    Ready,
    Warning,
    Blocked,
    Neutral,
    Merged,
    Closed,
}

/// The merge box's headline: what stands between the pull request and a
/// merge, the most pressing reason first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MergeStatus {
    pub(super) tone: MergeTone,
    pub(super) title: &'static str,
    pub(super) detail: String,
}

pub(super) fn merge_status(detail: &PullRequestDetail) -> MergeStatus {
    let status = |tone, title, detail: String| MergeStatus {
        tone,
        title,
        detail,
    };
    let checks = check_tally(detail);
    match detail.state {
        PrState::Merged => {
            return status(
                MergeTone::Merged,
                "Merged",
                format!("This pull request was merged into {}.", detail.base_ref),
            );
        }
        PrState::Closed => {
            return status(
                MergeTone::Closed,
                "Closed",
                "This pull request was closed without merging.".into(),
            );
        }
        PrState::Open => {}
    }
    if detail.is_draft {
        return status(
            MergeTone::Neutral,
            "Draft",
            "Mark it ready for review before it can be merged.".into(),
        );
    }
    if detail.mergeable == Mergeable::Conflicting || detail.merge_state == MergeStateStatus::Dirty {
        return status(
            MergeTone::Blocked,
            "Merge conflicts",
            format!("Resolve the conflicts with {} first.", detail.base_ref),
        );
    }
    if checks.failed > 0 {
        return status(
            if detail.merge_state == MergeStateStatus::Blocked {
                MergeTone::Blocked
            } else {
                MergeTone::Warning
            },
            "Checks are failing",
            format!("{} of {} checks failed.", checks.failed, checks.total),
        );
    }
    match detail.review_decision {
        Some(ReviewDecision::ChangesRequested) => {
            return status(
                MergeTone::Blocked,
                "Changes requested",
                "A reviewer asked for changes before merging.".into(),
            );
        }
        Some(ReviewDecision::ReviewRequired) => {
            return status(
                MergeTone::Warning,
                "Review required",
                "At least one approving review is required.".into(),
            );
        }
        _ => {}
    }
    if checks.pending > 0 {
        return status(
            MergeTone::Warning,
            "Checks are running",
            format!(
                "{} of {} checks have not finished.",
                checks.pending, checks.total
            ),
        );
    }
    match (detail.mergeable, detail.merge_state) {
        (_, MergeStateStatus::Behind) => status(
            MergeTone::Warning,
            "Branch is out of date",
            format!(
                "Update it with the latest changes from {}.",
                detail.base_ref
            ),
        ),
        (_, MergeStateStatus::Blocked) => status(
            MergeTone::Blocked,
            "Merging is blocked",
            "A branch protection rule is not met yet.".into(),
        ),
        (Mergeable::Unknown, _) | (_, MergeStateStatus::Unknown) => status(
            MergeTone::Neutral,
            "Checking mergeability",
            "GitHub is still checking whether this can be merged.".into(),
        ),
        (_, MergeStateStatus::Unstable) => status(
            MergeTone::Warning,
            "Ready to merge",
            "Some checks that are not required are not passing.".into(),
        ),
        _ => status(
            MergeTone::Ready,
            "Ready to merge",
            "All requirements are met.".into(),
        ),
    }
}

/// Check runs by outcome. Neutral and skipped runs count as passed, as on
/// GitHub.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CheckTally {
    pub(super) passed: usize,
    pub(super) failed: usize,
    pub(super) pending: usize,
    pub(super) total: usize,
}

pub(super) fn check_tally(detail: &PullRequestDetail) -> CheckTally {
    let mut tally = CheckTally {
        total: detail.checks.runs.len(),
        ..CheckTally::default()
    };
    for run in &detail.checks.runs {
        match run.status {
            CheckStatus::Passed | CheckStatus::Neutral | CheckStatus::Skipped => tally.passed += 1,
            CheckStatus::Failed | CheckStatus::Cancelled | CheckStatus::ActionRequired => {
                tally.failed += 1
            }
            CheckStatus::Queued | CheckStatus::Running => tally.pending += 1,
        }
    }
    if tally.total == 0 && detail.checks.state == Some(ChecksState::Pending) {
        tally.pending = 1;
        tally.total = 1;
    }
    tally
}

/// The sections of the merge box's check list, in GitHub's order. Failing
/// holds what the tally counts as failed, so a section's count matches the
/// red run of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CheckGroup {
    Failing,
    Pending,
    Skipped,
    Neutral,
    Successful,
}

impl CheckGroup {
    const ALL: [CheckGroup; 5] = [
        CheckGroup::Failing,
        CheckGroup::Pending,
        CheckGroup::Skipped,
        CheckGroup::Neutral,
        CheckGroup::Successful,
    ];

    fn of(status: CheckStatus) -> Self {
        match status {
            CheckStatus::Failed | CheckStatus::Cancelled | CheckStatus::ActionRequired => {
                CheckGroup::Failing
            }
            CheckStatus::Queued | CheckStatus::Running => CheckGroup::Pending,
            CheckStatus::Skipped => CheckGroup::Skipped,
            CheckStatus::Neutral => CheckGroup::Neutral,
            CheckStatus::Passed => CheckGroup::Successful,
        }
    }

    pub(super) fn key(self) -> &'static str {
        match self {
            CheckGroup::Failing => "failing",
            CheckGroup::Pending => "pending",
            CheckGroup::Skipped => "skipped",
            CheckGroup::Neutral => "neutral",
            CheckGroup::Successful => "successful",
        }
    }

    /// `2 failing checks`, `1 successful check`.
    pub(super) fn heading(self, count: usize) -> String {
        let noun = if count == 1 { "check" } else { "checks" };
        format!("{count} {} {noun}", self.key())
    }
}

/// The indices of `runs` in each non-empty group, groups in GitHub's order
/// and runs in the order GitHub lists them.
pub(super) fn check_groups(runs: &[CheckRun]) -> Vec<(CheckGroup, Vec<usize>)> {
    CheckGroup::ALL
        .into_iter()
        .filter_map(|group| {
            let members: Vec<usize> = runs
                .iter()
                .enumerate()
                .filter(|(_, run)| CheckGroup::of(run.status) == group)
                .map(|(ix, _)| ix)
                .collect();
            (!members.is_empty()).then_some((group, members))
        })
        .collect()
}

/// What a check did, as GitHub words it beside the name: `Failing after
/// 3m 5s`, `Successful in 45s`, `Queued`.
pub(super) fn check_outcome(run: &CheckRun) -> String {
    let took = match (run.started_at, run.completed_at) {
        (Some(start), Some(end)) => Some(format_duration((end - start).num_seconds())),
        _ => None,
    };
    let (word, link) = match run.status {
        CheckStatus::Queued => return "Queued".into(),
        CheckStatus::Running => return "In progress".into(),
        CheckStatus::ActionRequired => return "Action required".into(),
        CheckStatus::Skipped => return "Skipped".into(),
        CheckStatus::Failed => ("Failing", "after"),
        CheckStatus::Cancelled => ("Cancelled", "after"),
        CheckStatus::Neutral => ("Neutral", "after"),
        CheckStatus::Passed => ("Successful", "in"),
    };
    match took {
        Some(took) => format!("{word} {link} {took}"),
        None => word.into(),
    }
}

/// The check list starts open while a check fails or runs, as on GitHub,
/// and closed once every check has settled well.
pub(super) fn checks_open_at_first(detail: &PullRequestDetail) -> bool {
    let tally = check_tally(detail);
    tally.failed + tally.pending > 0
}

/// `done` of `total`, for the merge box's progress bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Progress {
    pub(super) done: usize,
    pub(super) total: usize,
}

/// Review threads resolved out of all of them.
pub(super) fn resolved_threads(detail: &PullRequestDetail) -> Progress {
    Progress {
        done: detail.threads.iter().filter(|t| t.is_resolved).count(),
        total: detail.threads.len(),
    }
}

/// Reviewers whose latest review approves, out of every reviewer asked or
/// who reviewed.
pub(super) fn approvals(detail: &PullRequestDetail) -> Progress {
    Progress {
        done: detail
            .reviewers
            .iter()
            .filter(|r| r.state == ReviewerState::Approved)
            .count(),
        total: detail.reviewers.len(),
    }
}

/// Conversation comments, review summaries with a body, and inline review
/// comments: everything someone wrote on the pull request.
pub(super) fn comment_count(detail: &PullRequestDetail) -> usize {
    let timeline = detail
        .timeline
        .iter()
        .filter(|item| match item {
            TimelineItem::Comment(_) => true,
            TimelineItem::Review(review) => !review.body.trim().is_empty(),
        })
        .count();
    let inline: usize = detail.threads.iter().map(|t| t.comments.len()).sum();
    timeline + inline
}

/// The selected method when the repository still allows it, else the first
/// allowed one.
pub(super) fn resolve_method(
    methods: &[MergeMethod],
    selected: Option<MergeMethod>,
) -> Option<MergeMethod> {
    selected
        .filter(|method| methods.contains(method))
        .or_else(|| methods.first().copied())
}

pub(super) fn method_label(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge commit",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

/// The merge button's label for `method`, as GitHub words it.
pub(super) fn merge_button_label(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "Create a merge commit",
        MergeMethod::Squash => "Squash and merge",
        MergeMethod::Rebase => "Rebase and merge",
    }
}

pub(super) fn merge_confirm_text(number: u64, base_ref: &str, method: MergeMethod) -> String {
    format!(
        "Merge #{number} into {base_ref} with {}?",
        method_label(method)
    )
}

/// What merging a stacked pull request takes along: GitHub merges the open
/// pull requests under it too, into the stack's base.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct StackMerge<'a> {
    pub base_ref: &'a str,
    /// Every pull request the merge lands, base first, this one last.
    pub entries: Vec<&'a StackEntry>,
}

/// The stack merge of `detail`, or `None` when merging it lands it alone:
/// outside a stack, at its base, or with nothing open under it.
pub(super) fn stack_merge(detail: &PullRequestDetail) -> Option<StackMerge<'_>> {
    let stack = detail.stack.as_ref()?;
    let mut entries = stack.open_below(detail.pr.number);
    if entries.is_empty() {
        return None;
    }
    entries.extend(stack.entry(detail.pr.number));
    Some(StackMerge {
        base_ref: &stack.base_ref,
        entries,
    })
}

pub(super) fn stack_merge_confirm_text(
    count: usize,
    base_ref: &str,
    method: MergeMethod,
) -> String {
    format!(
        "This merges {count} pull requests into {base_ref} with {}:",
        method_label(method)
    )
}

/// What a stack entry's row warns about, in the order the row shows them.
pub(super) fn stack_entry_problems(entry: &StackEntry) -> Vec<&'static str> {
    let mut problems = Vec::new();
    if entry.has_conflicts() {
        problems.push("Conflicts");
    }
    if entry.changes_requested() {
        problems.push("Changes requested");
    }
    problems
}

pub(super) fn pending_comments_label(count: u32) -> String {
    match count {
        0 => "No pending comments".into(),
        1 => "1 pending comment".into(),
        count => format!("{count} pending comments"),
    }
}

pub(super) fn discard_confirm_text(count: u32) -> String {
    format!(
        "Discard your pending review ({})? This cannot be undone.",
        pending_comments_label(count).to_lowercase()
    )
}

/// `base` and `head`, with the fork owner in front of a head from a fork.
pub(super) fn head_label(detail: &PullRequestDetail) -> String {
    match &detail.head_repo {
        Some(repo) => format!("{}:{}", repo.owner, detail.head_ref),
        None => detail.head_ref.clone(),
    }
}

/// GitHub rejects approving or requesting changes on your own pull request.
pub(super) fn can_approve(detail: &PullRequestDetail) -> bool {
    detail
        .author
        .as_ref()
        .is_none_or(|author| !author.login.eq_ignore_ascii_case(&detail.viewer_login))
}

/// Why a review cannot be submitted as typed, before asking GitHub.
pub(super) fn review_blocker(
    event: ReviewEvent,
    body: &str,
    pending_comments: u32,
) -> Option<&'static str> {
    let empty = body.trim().is_empty();
    match event {
        ReviewEvent::RequestChanges if empty => {
            Some("Explain the requested changes in the review comment.")
        }
        ReviewEvent::Comment if empty && pending_comments == 0 => {
            Some("Write a comment, or add inline comments first.")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use loupe_github::{
        Actor, CheckRun, CheckStatus, Checks, ChecksState, MergeMethod, MergeStateStatus,
        Mergeable, PendingReview, PrRef, PrStack, PrState, PullRequestDetail, RepoRef,
        ReviewDecision, ReviewEvent, Reviewer, ReviewerKind, ReviewerState, StackEntry,
    };

    use super::{
        CheckGroup, DraftAction, MergeAction, MergeTone, Panel, PrTab, Progress, StateAction,
        approvals, available_actions, can_approve, check_groups, check_outcome, check_tally,
        checks_open_at_first, discard_confirm_text, head_label, merge_button_label,
        merge_confirm_text, merge_status, pending_comments_label, resolve_method, review_blocker,
        stack_entry_problems, stack_merge, stack_merge_confirm_text,
    };

    fn actor(login: &str) -> Actor {
        Actor {
            login: login.into(),
            avatar_url: String::new(),
        }
    }

    fn epoch() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(0, 0).unwrap()
    }

    fn detail() -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_1".into(),
            pr: PrRef::new(RepoRef::new("acme", "app"), 12),
            url: "https://github.com/acme/app/pull/12".into(),
            title: "Add thing".into(),
            body: String::new(),
            state: PrState::Open,
            is_draft: false,
            mergeable: Mergeable::Mergeable,
            merge_state: MergeStateStatus::Clean,
            merge_methods: vec![MergeMethod::Squash, MergeMethod::Rebase],
            base_ref: "main".into(),
            base_oid: "base123".into(),
            head_ref: "feature".into(),
            head_repo: None,
            head_oid: "abc".into(),
            author: Some(actor("ana")),
            created_at: epoch(),
            updated_at: epoch(),
            body_edits: Vec::new(),
            additions: 1,
            deletions: 1,
            changed_files: 2,
            review_decision: None,
            reviewers: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            commits: Vec::new(),
            checks: Checks {
                state: None,
                runs: Vec::new(),
            },
            timeline: Vec::new(),
            threads: Vec::new(),
            pending_review: None,
            viewer_login: "bruno".into(),
            viewer_can_update: true,
            stack: None,
        }
    }

    fn run(status: CheckStatus) -> CheckRun {
        CheckRun {
            name: "ci".into(),
            workflow: None,
            status,
            started_at: None,
            completed_at: None,
            details_url: None,
            is_required: true,
            actions: None,
        }
    }

    fn headline(detail: &PullRequestDetail) -> (MergeTone, &'static str) {
        let status = merge_status(detail);
        (status.tone, status.title)
    }

    #[test]
    fn merge_box_names_the_most_pressing_blocker_first() {
        let mut detail = detail();
        assert_eq!(headline(&detail), (MergeTone::Ready, "Ready to merge"));

        detail.review_decision = Some(ReviewDecision::ReviewRequired);
        assert_eq!(headline(&detail), (MergeTone::Warning, "Review required"));

        detail.checks.runs = vec![run(CheckStatus::Passed), run(CheckStatus::Failed)];
        assert_eq!(headline(&detail).1, "Checks are failing");
        assert_eq!(merge_status(&detail).detail, "1 of 2 checks failed.");

        detail.mergeable = Mergeable::Conflicting;
        assert_eq!(headline(&detail), (MergeTone::Blocked, "Merge conflicts"));

        detail.is_draft = true;
        assert_eq!(headline(&detail), (MergeTone::Neutral, "Draft"));

        detail.state = PrState::Merged;
        assert_eq!(headline(&detail), (MergeTone::Merged, "Merged"));
    }

    #[test]
    fn running_checks_and_a_stale_branch_read_as_warnings() {
        let mut detail = detail();
        detail.checks.runs = vec![run(CheckStatus::Running), run(CheckStatus::Passed)];
        assert_eq!(
            headline(&detail),
            (MergeTone::Warning, "Checks are running")
        );

        detail.checks.runs = vec![run(CheckStatus::Passed)];
        detail.merge_state = MergeStateStatus::Behind;
        assert_eq!(
            headline(&detail),
            (MergeTone::Warning, "Branch is out of date")
        );

        detail.merge_state = MergeStateStatus::Unstable;
        assert_eq!(headline(&detail), (MergeTone::Warning, "Ready to merge"));
    }

    #[test]
    fn checks_count_neutral_and_skipped_as_passed() {
        let mut detail = detail();
        detail.checks.runs = [
            CheckStatus::Passed,
            CheckStatus::Skipped,
            CheckStatus::Neutral,
            CheckStatus::Failed,
            CheckStatus::Cancelled,
            CheckStatus::Queued,
        ]
        .map(run)
        .to_vec();
        let tally = check_tally(&detail);
        assert_eq!(
            (tally.passed, tally.failed, tally.pending, tally.total),
            (3, 2, 1, 6)
        );
    }

    #[test]
    fn check_list_groups_runs_in_githubs_order_and_skips_empty_groups() {
        let runs = [
            CheckStatus::Passed,
            CheckStatus::Failed,
            CheckStatus::Skipped,
            CheckStatus::Running,
            CheckStatus::Cancelled,
            CheckStatus::Passed,
            CheckStatus::ActionRequired,
            CheckStatus::Queued,
        ]
        .map(run);
        let groups: Vec<(&str, Vec<usize>)> = check_groups(&runs)
            .into_iter()
            .map(|(group, members)| (group.key(), members))
            .collect();
        assert_eq!(
            groups,
            vec![
                ("failing", vec![1, 4, 6]),
                ("pending", vec![3, 7]),
                ("skipped", vec![2]),
                ("successful", vec![0, 5]),
            ]
        );
        assert!(check_groups(&[]).is_empty());
    }

    #[test]
    fn check_group_headings_count_like_github() {
        assert_eq!(CheckGroup::Failing.heading(2), "2 failing checks");
        assert_eq!(CheckGroup::Successful.heading(1), "1 successful check");
        assert_eq!(CheckGroup::Neutral.heading(3), "3 neutral checks");
    }

    #[test]
    fn check_outcome_says_what_happened_and_how_long_it_took() {
        let timed = |status| CheckRun {
            started_at: DateTime::<Utc>::from_timestamp(1_000, 0),
            completed_at: DateTime::<Utc>::from_timestamp(1_185, 0),
            ..run(status)
        };
        assert_eq!(
            check_outcome(&timed(CheckStatus::Failed)),
            "Failing after 3m 5s"
        );
        assert_eq!(
            check_outcome(&timed(CheckStatus::Passed)),
            "Successful in 3m 5s"
        );
        assert_eq!(
            check_outcome(&timed(CheckStatus::Cancelled)),
            "Cancelled after 3m 5s"
        );
        assert_eq!(check_outcome(&run(CheckStatus::Passed)), "Successful");
        assert_eq!(check_outcome(&run(CheckStatus::Failed)), "Failing");
        assert_eq!(check_outcome(&run(CheckStatus::Running)), "In progress");
        assert_eq!(check_outcome(&run(CheckStatus::Queued)), "Queued");
        assert_eq!(check_outcome(&timed(CheckStatus::Skipped)), "Skipped");
        assert_eq!(
            check_outcome(&run(CheckStatus::ActionRequired)),
            "Action required"
        );
    }

    #[test]
    fn check_list_starts_open_only_while_a_check_fails_or_runs() {
        let mut detail = detail();
        assert!(!checks_open_at_first(&detail));
        detail.checks.runs = vec![run(CheckStatus::Passed), run(CheckStatus::Skipped)];
        assert!(!checks_open_at_first(&detail));
        detail.checks.runs.push(run(CheckStatus::Running));
        assert!(checks_open_at_first(&detail));
        detail.checks.runs = vec![run(CheckStatus::Passed), run(CheckStatus::Cancelled)];
        assert!(checks_open_at_first(&detail));
    }

    #[test]
    fn approvals_count_reviewers_whose_latest_review_approves() {
        let mut detail = detail();
        let reviewer = |login: &str, state| Reviewer {
            kind: ReviewerKind::User(actor(login)),
            state,
        };
        detail.reviewers = vec![
            reviewer("carla", ReviewerState::Approved),
            reviewer("davi", ReviewerState::Requested),
            reviewer("eva", ReviewerState::ChangesRequested),
        ];
        assert_eq!(approvals(&detail), Progress { done: 1, total: 3 });
    }

    #[test]
    fn open_mergeable_pull_request_offers_every_action() {
        let actions = available_actions(&detail());
        assert_eq!(
            actions.merge,
            MergeAction::Available {
                methods: vec![MergeMethod::Squash, MergeMethod::Rebase],
                note: None
            }
        );
        assert_eq!(actions.draft, Some(DraftAction::ConvertToDraft));
        assert_eq!(actions.state, Some(StateAction::Close));
    }

    #[test]
    fn viewer_without_update_rights_gets_no_actions() {
        let mut detail = detail();
        detail.viewer_can_update = false;
        let actions = available_actions(&detail);
        assert_eq!(actions.merge, MergeAction::Hidden);
        assert_eq!(actions.draft, None);
        assert_eq!(actions.state, None);
    }

    #[test]
    fn draft_can_be_marked_ready_but_not_merged() {
        let mut detail = detail();
        detail.is_draft = true;
        let actions = available_actions(&detail);
        assert!(matches!(actions.merge, MergeAction::Unavailable(_)));
        assert_eq!(actions.draft, Some(DraftAction::MarkReady));
        assert_eq!(actions.state, Some(StateAction::Close));
    }

    #[test]
    fn closed_pull_request_can_only_be_reopened() {
        let mut detail = detail();
        detail.state = PrState::Closed;
        let actions = available_actions(&detail);
        assert_eq!(actions.merge, MergeAction::Hidden);
        assert_eq!(actions.draft, None);
        assert_eq!(actions.state, Some(StateAction::Reopen));
    }

    #[test]
    fn merged_pull_request_has_no_actions() {
        let mut detail = detail();
        detail.state = PrState::Merged;
        let actions = available_actions(&detail);
        assert_eq!(actions.merge, MergeAction::Hidden);
        assert_eq!(actions.draft, None);
        assert_eq!(actions.state, None);
    }

    #[test]
    fn merge_is_unavailable_for_conflicts_unknown_blocked_and_behind() {
        let cases = [
            (Mergeable::Conflicting, MergeStateStatus::Dirty),
            (Mergeable::Unknown, MergeStateStatus::Unknown),
            (Mergeable::Mergeable, MergeStateStatus::Blocked),
            (Mergeable::Mergeable, MergeStateStatus::Behind),
        ];
        for (mergeable, merge_state) in cases {
            let mut detail = detail();
            detail.mergeable = mergeable;
            detail.merge_state = merge_state;
            assert!(
                matches!(
                    available_actions(&detail).merge,
                    MergeAction::Unavailable(_)
                ),
                "{mergeable:?} {merge_state:?}"
            );
        }
    }

    #[test]
    fn unstable_merge_is_available_with_a_caution() {
        let mut detail = detail();
        detail.merge_state = MergeStateStatus::Unstable;
        assert!(matches!(
            available_actions(&detail).merge,
            MergeAction::Available { note: Some(_), .. }
        ));
    }

    #[test]
    fn merge_is_unavailable_without_a_merge_method() {
        let mut detail = detail();
        detail.merge_methods.clear();
        assert!(matches!(
            available_actions(&detail).merge,
            MergeAction::Unavailable(_)
        ));
    }

    #[test]
    fn merge_methods_are_limited_to_the_allowed_ones() {
        let allowed = [MergeMethod::Squash, MergeMethod::Rebase];
        assert_eq!(
            resolve_method(&allowed, Some(MergeMethod::Rebase)),
            Some(MergeMethod::Rebase)
        );
        assert_eq!(
            resolve_method(&allowed, Some(MergeMethod::Merge)),
            Some(MergeMethod::Squash)
        );
        assert_eq!(resolve_method(&allowed, None), Some(MergeMethod::Squash));
        assert_eq!(resolve_method(&[], Some(MergeMethod::Merge)), None);
    }

    #[test]
    fn merge_button_names_the_chosen_method() {
        assert_eq!(
            merge_button_label(MergeMethod::Merge),
            "Create a merge commit"
        );
        assert_eq!(merge_button_label(MergeMethod::Squash), "Squash and merge");
        assert_eq!(merge_button_label(MergeMethod::Rebase), "Rebase and merge");
    }

    #[test]
    fn merge_confirmation_names_pull_request_base_and_method() {
        assert_eq!(
            merge_confirm_text(12, "main", MergeMethod::Squash),
            "Merge #12 into main with squash?"
        );
    }

    fn stack_entry(position: u32, number: u64, state: PrState) -> StackEntry {
        StackEntry {
            position,
            number,
            title: format!("Layer {position}"),
            state,
            is_draft: false,
            checks: Some(ChecksState::Passing),
            mergeable: Mergeable::Mergeable,
            review_decision: None,
        }
    }

    fn stacked(number: u64, entries: Vec<StackEntry>) -> PullRequestDetail {
        let mut detail = detail();
        detail.pr.number = number;
        detail.base_ref = "layer-below".into();
        detail.stack = Some(PrStack::new(7, "stack-base".into(), entries));
        detail
    }

    fn numbers(merge: &super::StackMerge) -> Vec<u64> {
        merge.entries.iter().map(|entry| entry.number).collect()
    }

    #[test]
    fn merging_the_top_of_a_stack_lists_every_pull_request_it_lands_base_first() {
        let detail = stacked(
            12,
            vec![
                stack_entry(3, 12, PrState::Open),
                stack_entry(1, 21, PrState::Open),
                stack_entry(2, 9, PrState::Open),
            ],
        );
        let merge = stack_merge(&detail).expect("a stack merge");
        assert_eq!(numbers(&merge), vec![21, 9, 12]);
        assert_eq!(merge.base_ref, "stack-base");
        assert_eq!(
            stack_merge_confirm_text(merge.entries.len(), merge.base_ref, MergeMethod::Squash),
            "This merges 3 pull requests into stack-base with squash:"
        );
    }

    #[test]
    fn a_merged_layer_is_not_listed_and_a_lone_merge_keeps_todays_confirmation() {
        let entries = vec![
            stack_entry(1, 21, PrState::Merged),
            stack_entry(2, 9, PrState::Open),
            stack_entry(3, 12, PrState::Open),
        ];
        let top = stacked(12, entries.clone());
        assert_eq!(
            numbers(&stack_merge(&top).expect("a stack merge")),
            vec![9, 12]
        );
        // The lowest open layer merges alone, and so does a pull request
        // outside any stack.
        assert_eq!(stack_merge(&stacked(9, entries)), None);
        assert_eq!(stack_merge(&detail()), None);
    }

    #[test]
    fn a_stack_entry_warns_about_conflicts_and_requested_changes() {
        let mut entry = stack_entry(1, 10, PrState::Open);
        assert!(stack_entry_problems(&entry).is_empty());
        entry.mergeable = Mergeable::Conflicting;
        entry.review_decision = Some(ReviewDecision::ChangesRequested);
        assert_eq!(
            stack_entry_problems(&entry),
            vec!["Conflicts", "Changes requested"]
        );
    }

    #[test]
    fn panels_close_when_a_reload_removes_what_they_confirm() {
        let mut detail = detail();
        assert!(!Panel::ConfirmDiscard.still_valid(&detail));
        detail.pending_review = Some(PendingReview {
            id: "PRR_1".into(),
            comment_count: 2,
        });
        assert!(Panel::ConfirmDiscard.still_valid(&detail));
        assert!(Panel::ConfirmMerge(MergeMethod::Squash).still_valid(&detail));
        assert!(!Panel::ConfirmMerge(MergeMethod::Merge).still_valid(&detail));
        assert!(Panel::ConfirmClose.still_valid(&detail));
        detail.state = PrState::Closed;
        assert!(!Panel::ConfirmClose.still_valid(&detail));
        assert!(Panel::Review.still_valid(&detail));
    }

    #[test]
    fn head_shows_the_fork_owner_only_for_forks() {
        let mut detail = detail();
        assert_eq!(head_label(&detail), "feature");
        detail.head_repo = Some(RepoRef::new("carla", "app"));
        assert_eq!(head_label(&detail), "carla:feature");
    }

    #[test]
    fn author_cannot_approve_their_own_pull_request() {
        let mut detail = detail();
        assert!(can_approve(&detail));
        detail.viewer_login = "Ana".into();
        assert!(!can_approve(&detail));
        detail.author = None;
        assert!(can_approve(&detail));
    }

    #[test]
    fn review_needs_text_unless_it_is_an_approval_or_has_inline_comments() {
        assert!(review_blocker(ReviewEvent::Approve, "", 0).is_none());
        assert!(review_blocker(ReviewEvent::Comment, "  ", 0).is_some());
        assert!(review_blocker(ReviewEvent::Comment, "", 3).is_none());
        assert!(review_blocker(ReviewEvent::Comment, "ok", 0).is_none());
        assert!(review_blocker(ReviewEvent::RequestChanges, "", 3).is_some());
        assert!(review_blocker(ReviewEvent::RequestChanges, "fix", 0).is_none());
    }

    #[test]
    fn pending_comments_are_pluralized() {
        assert_eq!(pending_comments_label(0), "No pending comments");
        assert_eq!(pending_comments_label(1), "1 pending comment");
        assert_eq!(pending_comments_label(4), "4 pending comments");
        assert_eq!(
            discard_confirm_text(4),
            "Discard your pending review (4 pending comments)? This cannot be undone."
        );
    }

    #[test]
    fn tabs_round_trip_and_files_hides_the_side_column() {
        for tab in PrTab::ALL {
            assert_eq!(PrTab::from_index(tab.index()), Some(tab));
        }
        assert_eq!(PrTab::from_index(4), None);
        assert!(!PrTab::Files.has_side_column());
        assert!(PrTab::Conversation.has_side_column());
        assert!(PrTab::Commits.has_side_column());
        assert!(PrTab::Checks.has_side_column());
    }

    #[test]
    fn tab_counts_come_from_the_detail() {
        let detail = detail();
        assert_eq!(PrTab::Conversation.count(&detail), None);
        assert_eq!(PrTab::Files.count(&detail), Some(2));
        assert_eq!(PrTab::Commits.count(&detail), Some(0));
        assert_eq!(PrTab::Checks.count(&detail), Some(0));
    }
}
