//! Domain types the API returns. Wire shapes stay private to the modules that
//! parse them; everything here is already validated and normalized.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{PrRef, RepoRef};

pub type Timestamp = DateTime<Utc>;

/// A GraphQL global node id (`PR_kwDO...`, `PRRT_...`, ...).
pub type NodeId = String;

/// A user, bot, or app. Deleted accounts arrive as GitHub's `ghost`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// Rolled-up state of a commit's checks and statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChecksState {
    Pending,
    Passing,
    Failing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// A review someone asked for and that has not come in since. GitHub drops
/// the request once the reviewer submits a review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewRequest {
    /// A user, by login.
    User(String),
    /// A team, by `org/slug`.
    Team(String),
}

/// One pull request as a list row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrSummary {
    pub id: NodeId,
    pub pr: PrRef,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub is_draft: bool,
    pub author: Option<Actor>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Conversation comments (issue comments), as GitHub's list shows.
    pub comment_count: u32,
    pub additions: u32,
    pub deletions: u32,
    /// `None` when the head commit has no checks or statuses.
    pub checks: Option<ChecksState>,
    pub review_decision: Option<ReviewDecision>,
    pub head_ref: String,
    /// The branch it merges into. Empty in a copy stored before Loupe read it.
    #[serde(default)]
    pub base_ref: String,
    /// The head branch is in a fork, so no other pull request of the
    /// repository can be based on it.
    #[serde(default)]
    pub cross_repository: bool,
    /// The GitHub stack it is part of; `None` outside one.
    #[serde(default)]
    pub stack: Option<crate::StackSummary>,
    pub labels: Vec<Label>,
    pub assignees: Vec<Actor>,
    /// Users asked to review, then users who reviewed, each once. Team
    /// requests are left out.
    pub reviewers: Vec<Actor>,
    /// `Unknown` while GitHub has not computed it yet.
    pub mergeable: Mergeable,
    /// Reviews still asked for, from users and teams. Requests to bots are
    /// left out.
    #[serde(default)]
    pub review_requests: Vec<ReviewRequest>,
    /// Logins of the users whose latest review is on the pull request, the
    /// author included when they replied in a review. Bots are left out.
    #[serde(default)]
    pub reviewed_by: Vec<String>,
}

/// One page of a connection. `next_cursor` is `Some` while more pages exist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total_count: u32,
    pub next_cursor: Option<String>,
}

/// The dashboard: three searches run in one request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    /// `is:open is:pr review-requested:@me`
    pub review_requested: Page<PrSummary>,
    /// `is:open is:pr author:@me`
    pub authored: Page<PrSummary>,
    /// `is:open is:pr involves:@me -author:@me -review-requested:@me`
    pub involved: Page<PrSummary>,
    /// The signed-in user's login; empty in a copy stored before it was read.
    #[serde(default)]
    pub viewer_login: String,
}

/// Which pull requests a repository list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrListFilter {
    #[default]
    Open,
    Closed,
    Merged,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSummary {
    pub repo: RepoRef,
    pub description: Option<String>,
    pub is_private: bool,
    pub pushed_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    Unknown,
}

/// GitHub's `mergeStateStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MergeStateStatus {
    Behind,
    Blocked,
    Clean,
    Dirty,
    Draft,
    HasHooks,
    Unknown,
    Unstable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// A side of a diff in GitHub's terms: `Left` is the base (deleted and
/// original lines), `Right` is the head (added and context lines).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub id: NodeId,
    pub name: String,
    /// Six hex digits without `#`, as GitHub sends it.
    pub color: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewerKind {
    User(Actor),
    Team { name: String, slug: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewerState {
    /// Review requested and not yet given.
    Requested,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

/// A requested or past reviewer with their latest state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewer {
    pub kind: ReviewerKind,
    pub state: ReviewerState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub oid: String,
    pub headline: String,
    pub body: String,
    /// Git author name, always present.
    pub author_name: String,
    /// The GitHub account behind the author, when GitHub could match one.
    pub author: Option<Actor>,
    pub committed_at: Timestamp,
    pub checks: Option<ChecksState>,
}

/// One check run or commit status, normalized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckStatus {
    Queued,
    Running,
    Passed,
    Failed,
    Neutral,
    Skipped,
    Cancelled,
    ActionRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    /// Workflow name for Actions check runs; `None` for commit statuses.
    pub workflow: Option<String>,
    pub status: CheckStatus,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub details_url: Option<String>,
    pub is_required: bool,
    /// The Actions job behind the check; `None` for commit statuses and for
    /// check runs from other apps.
    pub actions: Option<ActionsJob>,
}

/// The GitHub Actions job behind a check run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionsJob {
    /// The job's id, which is also the check run's database id.
    pub job_id: u64,
    /// The workflow run the job belongs to.
    pub run_id: u64,
}

/// The end of a failed Actions job's log, from [`GitHub::failed_step_log`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepLog {
    /// The failed step's name; `None` when the job names no failed step and
    /// the lines are the end of the whole log.
    pub step: Option<String>,
    /// The last lines, without timestamps or terminal color codes.
    pub lines: Vec<String>,
    /// Whether earlier lines were left out.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub state: Option<ChecksState>,
    pub runs: Vec<CheckRun>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: NodeId,
    pub author: Option<Actor>,
    /// Posted by a GitHub App or bot account rather than a person.
    #[serde(default)]
    pub author_is_bot: bool,
    /// Hidden (minimized) on GitHub by a maintainer.
    #[serde(default)]
    pub hidden: bool,
    /// GitHub's reason for hiding it (`outdated`, `resolved`, `off-topic`, ...).
    #[serde(default)]
    pub hidden_reason: Option<String>,
    pub body: String,
    pub created_at: Timestamp,
    pub url: String,
    pub viewer_can_update: bool,
    pub viewer_did_author: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewState {
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: NodeId,
    pub author: Option<Actor>,
    /// Posted by a GitHub App or bot account rather than a person.
    #[serde(default)]
    pub author_is_bot: bool,
    /// Hidden (minimized) on GitHub by a maintainer.
    #[serde(default)]
    pub hidden: bool,
    /// GitHub's reason for hiding it (`outdated`, `resolved`, `off-topic`, ...).
    #[serde(default)]
    pub hidden_reason: Option<String>,
    pub state: ReviewState,
    pub body: String,
    /// `None` while pending.
    pub submitted_at: Option<Timestamp>,
    pub url: String,
    /// Inline comments that belong to this review.
    pub comment_count: u32,
    pub viewer_can_update: bool,
}

/// The conversation tab's items, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineItem {
    Comment(IssueComment),
    Review(Review),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: NodeId,
    pub author: Option<Actor>,
    pub body: String,
    pub created_at: Timestamp,
    pub url: String,
    /// The hunk GitHub stored with the comment, ending at the commented line.
    pub diff_hunk: String,
    /// Part of the viewer's pending review, not yet visible to others.
    pub is_pending: bool,
    pub viewer_can_update: bool,
    pub viewer_did_author: bool,
}

/// An inline review thread. `line`/`side` are where the thread ends;
/// `start_line`/`start_side` are set for a multi-line thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub id: NodeId,
    pub path: String,
    /// `None` when the thread is outdated and no longer maps to the diff.
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub original_line: Option<u32>,
    pub original_start_line: Option<u32>,
    pub side: Side,
    pub start_side: Option<Side>,
    pub is_outdated: bool,
    pub is_resolved: bool,
    pub resolved_by: Option<Actor>,
    pub viewer_can_reply: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
    pub comments: Vec<ReviewComment>,
}

/// The viewer's review in progress on this pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingReview {
    pub id: NodeId,
    pub comment_count: u32,
}

/// Everything the pull request view shows except changed files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDetail {
    pub id: NodeId,
    pub pr: PrRef,
    pub url: String,
    pub title: String,
    /// Markdown source.
    pub body: String,
    pub state: PrState,
    pub is_draft: bool,
    pub mergeable: Mergeable,
    pub merge_state: MergeStateStatus,
    /// Merge methods the repository allows, in GitHub's order.
    pub merge_methods: Vec<MergeMethod>,
    pub base_ref: String,
    pub base_oid: String,
    pub head_ref: String,
    /// The head repository when it differs from the base (a fork).
    pub head_repo: Option<RepoRef>,
    pub head_oid: String,
    pub author: Option<Actor>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Edits to the description, newest first; empty when never edited.
    /// Defaulted so details cached before this field still load.
    #[serde(default)]
    pub body_edits: Vec<BodyEdit>,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    pub review_decision: Option<ReviewDecision>,
    pub reviewers: Vec<Reviewer>,
    pub labels: Vec<Label>,
    pub assignees: Vec<Actor>,
    pub commits: Vec<Commit>,
    pub checks: Checks,
    pub timeline: Vec<TimelineItem>,
    pub threads: Vec<ReviewThread>,
    pub pending_review: Option<PendingReview>,
    pub viewer_login: String,
    pub viewer_can_update: bool,
    /// The stack this pull request is part of; `None` outside a stack.
    /// Defaulted so details cached before this field still load.
    #[serde(default)]
    pub stack: Option<crate::PrStack>,
}

/// One edit of a pull request's description.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyEdit {
    pub editor: Option<Actor>,
    pub edited_at: Timestamp,
    /// This entry is the original text, not a later edit.
    pub created: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileStatus {
    Added,
    Removed,
    Modified,
    Renamed,
    Copied,
    Changed,
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewedState {
    Viewed,
    Unviewed,
    /// Viewed, then changed by a later push.
    Dismissed,
}

/// One changed file with its unified-diff hunks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// The old path of a renamed or copied file.
    pub previous_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    /// Hunks only (starting at `@@`). `None` for binary files and patches
    /// GitHub omits because they are too large.
    pub patch: Option<String>,
    pub viewed: ViewedState,
}

/// What GitHub knows about a branch whose workflow run failed: the pull
/// request the branch belongs to, and its failed runs. A notification about
/// a workflow run names only the workflow and the branch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchCi {
    /// The pull request with this head branch that was updated last, open or
    /// not; `None` for a branch with no pull request.
    pub pull_request: Option<BranchPullRequest>,
    /// The failed runs of the branch, newest first.
    pub failed_runs: Vec<FailedRun>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPullRequest {
    pub pr: PrRef,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
}

/// One failed run of a GitHub Actions workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedRun {
    /// The workflow's name, such as `CI`.
    pub workflow: String,
    /// The run's number in its workflow, the `#57` GitHub shows.
    pub run_number: u64,
    pub finished_at: Timestamp,
    /// The run's page on github.com.
    pub url: String,
    /// The jobs of the run's last attempt that failed, in the run's order.
    /// Empty when the jobs were not read.
    pub failed_jobs: Vec<String>,
}

/// What a notification is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationSubject {
    PullRequest(PrRef),
    Issue { repo: RepoRef, number: u64 },
    Other { kind: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// The notification thread id used to mark it read.
    pub id: String,
    pub repo: RepoRef,
    pub title: String,
    pub subject: NotificationSubject,
    /// GitHub's reason string, such as `review_requested` or `mention`.
    pub reason: String,
    pub unread: bool,
    pub updated_at: Timestamp,
}

/// Where a new inline thread goes. `line`/`side` is the last line of the
/// range; `start` is the first line for a multi-line comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineAnchor {
    pub line: u32,
    pub side: Side,
    pub start: Option<(u32, Side)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewThread {
    pub pull_request_id: NodeId,
    pub path: String,
    pub body: String,
    pub anchor: LineAnchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}
