//! The pull request detail query.

use std::collections::HashSet;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    ActionsJob, Actor, BodyEdit, CheckRun, CheckStatus, Checks, ChecksState, Commit, Error, GitHub,
    IssueComment, Label, MergeMethod, MergeStateStatus, Mergeable, PendingReview, PrRef, PrState,
    PullRequestDetail, RepoRef, Result, Review, ReviewComment, ReviewDecision, ReviewState,
    ReviewThread, Reviewer, ReviewerKind, ReviewerState, Side, TimelineItem, Timestamp,
};

/// Timeline and thread pages fetched per pull request, the first one included.
const MAX_PAGES: usize = 10;

macro_rules! timeline_page_fragment {
    () => {
        r#"
fragment TimelinePage on PullRequestTimelineItemsConnection {
  pageInfo { hasNextPage endCursor }
  nodes {
    __typename
    ... on IssueComment {
      id
      author { __typename login avatarUrl }
      body
      createdAt
      url
      viewerCanUpdate
      viewerDidAuthor
      isMinimized
      minimizedReason
    }
    ... on PullRequestReview {
      id
      author { __typename login avatarUrl }
      state
      body
      isMinimized
      minimizedReason
      submittedAt
      url
      comments { totalCount }
      viewerCanUpdate
    }
  }
}
"#
    };
}

macro_rules! threads_page_fragment {
    () => {
        r#"
fragment ThreadsPage on PullRequestReviewThreadConnection {
  pageInfo { hasNextPage endCursor }
  nodes {
    id
    path
    line
    startLine
    originalLine
    originalStartLine
    diffSide
    startDiffSide
    isOutdated
    isResolved
    resolvedBy { login avatarUrl }
    viewerCanReply
    viewerCanResolve
    viewerCanUnresolve
    comments(first: 100) {
      nodes {
        id
        author { login avatarUrl }
        body
        createdAt
        url
        diffHunk
        state
        viewerCanUpdate
        viewerDidAuthor
      }
    }
  }
}
"#
    };
}

const DETAIL_QUERY: &str = concat!(
    r#"
query PullRequestDetail($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    mergeCommitAllowed
    squashMergeAllowed
    rebaseMergeAllowed
    pullRequest(number: $number) {
      id
      url
      title
      body
      state
      isDraft
      mergeable
      mergeStateStatus
      baseRefName
      baseRefOid
      headRefName
      headRefOid
      headRepository { owner { login } name }
      author { login avatarUrl }
      createdAt
      updatedAt
      includesCreatedEdit
      userContentEdits(first: 20) {
        nodes { editedAt editor { login avatarUrl } }
      }
      additions
      deletions
      changedFiles
      reviewDecision
      viewerCanUpdate
      reviewRequests(first: 50) {
        nodes {
          requestedReviewer {
            ... on User { login avatarUrl }
            ... on Bot { login avatarUrl }
            ... on Mannequin { login avatarUrl }
            ... on Team { name slug }
          }
        }
      }
      latestReviews(first: 50) {
        nodes { author { login avatarUrl } state }
      }
      labels(first: 50) { nodes { id name color } }
      assignees(first: 50) { nodes { login avatarUrl } }
      commits(last: 100) {
        nodes {
          commit {
            oid
            messageHeadline
            messageBody
            committedDate
            author { name user { login avatarUrl } }
            statusCheckRollup { state }
          }
        }
      }
      headCommit: commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              state
              contexts(first: 100) {
                nodes {
                  __typename
                  ... on CheckRun {
                    databaseId
                    name
                    status
                    conclusion
                    startedAt
                    completedAt
                    detailsUrl
                    isRequired(pullRequestNumber: $number)
                    checkSuite { workflowRun { databaseId workflow { name } } }
                  }
                  ... on StatusContext {
                    context
                    state
                    targetUrl
                    createdAt
                    isRequired(pullRequestNumber: $number)
                  }
                }
              }
            }
          }
        }
      }
      timelineItems(first: 50, itemTypes: [ISSUE_COMMENT, PULL_REQUEST_REVIEW]) {
        ...TimelinePage
      }
      reviewThreads(first: 30) {
        ...ThreadsPage
      }
      pendingReviews: reviews(states: PENDING, first: 5) {
        nodes { id viewerDidAuthor comments { totalCount } }
      }
    }
  }
}
"#,
    timeline_page_fragment!(),
    threads_page_fragment!()
);

const TIMELINE_PAGE_QUERY: &str = concat!(
    r#"
query PullRequestTimelinePage($owner: String!, $name: String!, $number: Int!, $cursor: String!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      timelineItems(first: 100, after: $cursor, itemTypes: [ISSUE_COMMENT, PULL_REQUEST_REVIEW]) {
        ...TimelinePage
      }
    }
  }
}
"#,
    timeline_page_fragment!()
);

const THREADS_PAGE_QUERY: &str = concat!(
    r#"
query PullRequestThreadsPage($owner: String!, $name: String!, $number: Int!, $cursor: String!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 50, after: $cursor) {
        ...ThreadsPage
      }
    }
  }
}
"#,
    threads_page_fragment!()
);

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[
    ("pull_request_detail", DETAIL_QUERY),
    ("pull_request_timeline_page", TIMELINE_PAGE_QUERY),
    ("pull_request_threads_page", THREADS_PAGE_QUERY),
];

impl GitHub {
    /// Header, body, reviewers, labels, assignees, commits, checks, timeline,
    /// review threads, and the viewer's pending review of one pull request.
    ///
    /// The timeline and the review threads are paged (up to ten pages each)
    /// with small first pages, because GitHub times out (HTTP 502) on one
    /// large query for busy pull requests. A thread keeps its first 30
    /// comments, and the head commit's checks are capped at 100.
    pub fn pull_request(&self, pr: &PrRef) -> Result<PullRequestDetail> {
        let data: Data = self.graphql(
            DETAIL_QUERY,
            json!({
                "owner": pr.repo.owner,
                "name": pr.repo.name,
                "number": pr.number,
            }),
        )?;
        let FirstPage {
            mut detail,
            mut timeline_cursor,
            mut threads_cursor,
        } = parse_detail(pr, data)?;

        for _ in 1..MAX_PAGES {
            let Some(cursor) = timeline_cursor.take() else {
                break;
            };
            let connection =
                self.connection_page(TIMELINE_PAGE_QUERY, pr, &cursor, "timelineItems")?;
            let (items, next) = parse_timeline(connection);
            detail.timeline.extend(items);
            timeline_cursor = next;
        }
        for _ in 1..MAX_PAGES {
            let Some(cursor) = threads_cursor.take() else {
                break;
            };
            let connection =
                self.connection_page(THREADS_PAGE_QUERY, pr, &cursor, "reviewThreads")?;
            let (threads, next) = parse_threads(connection);
            detail.threads.extend(threads);
            threads_cursor = next;
        }
        Ok(detail)
    }

    fn connection_page<T: DeserializeOwned>(
        &self,
        query: &str,
        pr: &PrRef,
        cursor: &str,
        field: &str,
    ) -> Result<Connection<T>> {
        let data: Value = self.graphql(
            query,
            json!({
                "owner": pr.repo.owner,
                "name": pr.repo.name,
                "number": pr.number,
                "cursor": cursor,
            }),
        )?;
        let node = data
            .pointer(&format!("/repository/pullRequest/{field}"))
            .filter(|node| !node.is_null())
            .ok_or_else(not_found)?;
        Connection::deserialize(node).map_err(|error| Error::Decode(error.to_string()))
    }
}

fn not_found() -> Error {
    Error::GraphQl(vec!["Pull request not found".to_owned()])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    nodes: Option<Vec<Option<T>>>,
    page_info: Option<PageInfo>,
}

impl<T> Connection<T> {
    fn into_page(self) -> (Vec<T>, Option<String>) {
        let nodes = self
            .nodes
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        let cursor = self
            .page_info
            .filter(|info| info.has_next_page)
            .and_then(|info| info.end_cursor);
        (nodes, cursor)
    }

    fn into_nodes(self) -> Vec<T> {
        self.into_page().0
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireActor {
    login: String,
    avatar_url: String,
    /// Asked for only where Loupe tells bots from people.
    #[serde(rename = "__typename", default)]
    typename: Option<String>,
}

impl WireActor {
    fn is_bot(author: &Option<WireActor>) -> bool {
        author
            .as_ref()
            .is_some_and(|author| author.typename.as_deref() == Some("Bot"))
    }
}

impl From<WireActor> for Actor {
    fn from(actor: WireActor) -> Self {
        Actor {
            login: actor.login,
            avatar_url: actor.avatar_url,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WirePrState {
    Open,
    Closed,
    Merged,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireMergeable {
    Mergeable,
    Conflicting,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireMergeState {
    Behind,
    Blocked,
    Clean,
    Dirty,
    Draft,
    HasHooks,
    Unstable,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireReviewState {
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireRollupState {
    Success,
    Failure,
    Error,
    Pending,
    Expected,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireCheckRunStatus {
    Queued,
    Waiting,
    Pending,
    Requested,
    InProgress,
    Completed,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireConclusion {
    Success,
    Failure,
    TimedOut,
    StartupFailure,
    Neutral,
    Skipped,
    Stale,
    Cancelled,
    ActionRequired,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireCommentState {
    Pending,
    Submitted,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireSide {
    Left,
    Right,
    #[serde(other)]
    Unknown,
}

impl From<WireSide> for Side {
    fn from(side: WireSide) -> Self {
        match side {
            WireSide::Left => Side::Left,
            WireSide::Right | WireSide::Unknown => Side::Right,
        }
    }
}

#[derive(Deserialize)]
struct Data {
    viewer: WireViewer,
    repository: Option<WireRepository>,
}

#[derive(Deserialize)]
struct WireViewer {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRepository {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    pull_request: Option<WirePullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePullRequest {
    id: String,
    url: String,
    title: String,
    body: String,
    state: WirePrState,
    is_draft: bool,
    mergeable: WireMergeable,
    merge_state_status: WireMergeState,
    base_ref_name: String,
    base_ref_oid: String,
    head_ref_name: String,
    head_ref_oid: String,
    head_repository: Option<WireHeadRepository>,
    author: Option<WireActor>,
    created_at: Timestamp,
    updated_at: Timestamp,
    includes_created_edit: bool,
    user_content_edits: Option<Connection<WireContentEdit>>,
    additions: u32,
    deletions: u32,
    changed_files: u32,
    review_decision: Option<WireReviewDecision>,
    viewer_can_update: bool,
    review_requests: Connection<WireReviewRequest>,
    latest_reviews: Connection<WireLatestReview>,
    labels: Connection<WireLabel>,
    assignees: Connection<WireActor>,
    commits: Connection<WireCommitNode>,
    head_commit: Connection<WireHeadCommitNode>,
    timeline_items: Connection<WireTimelineNode>,
    review_threads: Connection<WireThread>,
    pending_reviews: Connection<WirePendingReview>,
}

/// The description's edits, newest first. GitHub lists the original text as
/// one more edit once the description has been edited; it is marked
/// `created` so it reads as the creation.
fn body_edits(edits: Vec<WireContentEdit>, includes_created: bool) -> Vec<BodyEdit> {
    let mut edits: Vec<BodyEdit> = edits
        .into_iter()
        .map(|edit| BodyEdit {
            editor: edit.editor.map(Actor::from),
            edited_at: edit.edited_at,
            created: false,
        })
        .collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.edited_at));
    if includes_created && let Some(oldest) = edits.last_mut() {
        oldest.created = true;
    }
    edits
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireContentEdit {
    edited_at: Timestamp,
    editor: Option<WireActor>,
}

#[derive(Deserialize)]
struct WireHeadRepository {
    owner: WireOwner,
    name: String,
}

#[derive(Deserialize)]
struct WireOwner {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireReviewRequest {
    requested_reviewer: Option<WireRequestedReviewer>,
}

/// A union of user-like actors (`login`, `avatarUrl`) and teams (`name`,
/// `slug`); the fields a variant does not select are absent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRequestedReviewer {
    login: Option<String>,
    avatar_url: Option<String>,
    name: Option<String>,
    slug: Option<String>,
}

#[derive(Deserialize)]
struct WireLatestReview {
    author: Option<WireActor>,
    state: WireReviewState,
}

#[derive(Deserialize)]
struct WireLabel {
    id: String,
    name: String,
    color: String,
}

#[derive(Deserialize)]
struct WireCommitNode {
    commit: WireCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCommit {
    oid: String,
    message_headline: String,
    message_body: String,
    committed_date: Timestamp,
    author: Option<WireGitActor>,
    status_check_rollup: Option<WireCommitRollup>,
}

#[derive(Deserialize)]
struct WireGitActor {
    name: Option<String>,
    user: Option<WireActor>,
}

#[derive(Deserialize)]
struct WireCommitRollup {
    state: WireRollupState,
}

#[derive(Deserialize)]
struct WireHeadCommitNode {
    commit: WireHeadCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireHeadCommit {
    status_check_rollup: Option<WireRollup>,
}

#[derive(Deserialize)]
struct WireRollup {
    state: WireRollupState,
    contexts: Connection<WireContext>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum WireContext {
    CheckRun(WireCheckRun),
    StatusContext(WireStatusContext),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCheckRun {
    database_id: Option<u64>,
    name: String,
    status: WireCheckRunStatus,
    conclusion: Option<WireConclusion>,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
    details_url: Option<String>,
    is_required: bool,
    check_suite: Option<WireCheckSuite>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCheckSuite {
    workflow_run: Option<WireWorkflowRun>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireWorkflowRun {
    database_id: Option<u64>,
    workflow: Option<WireWorkflow>,
}

#[derive(Deserialize)]
struct WireWorkflow {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStatusContext {
    context: String,
    state: WireRollupState,
    target_url: Option<String>,
    created_at: Option<Timestamp>,
    is_required: bool,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum WireTimelineNode {
    IssueComment(WireIssueComment),
    PullRequestReview(WireReview),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIssueComment {
    id: String,
    author: Option<WireActor>,
    body: String,
    created_at: Timestamp,
    url: String,
    viewer_can_update: bool,
    viewer_did_author: bool,
    #[serde(default)]
    is_minimized: bool,
    minimized_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireReview {
    id: String,
    author: Option<WireActor>,
    state: WireReviewState,
    body: String,
    submitted_at: Option<Timestamp>,
    url: String,
    comments: Count,
    viewer_can_update: bool,
    #[serde(default)]
    is_minimized: bool,
    minimized_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireThread {
    id: String,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    original_start_line: Option<u32>,
    diff_side: WireSide,
    start_diff_side: Option<WireSide>,
    is_outdated: bool,
    is_resolved: bool,
    resolved_by: Option<WireActor>,
    viewer_can_reply: bool,
    viewer_can_resolve: bool,
    viewer_can_unresolve: bool,
    comments: Connection<WireThreadComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireThreadComment {
    id: String,
    author: Option<WireActor>,
    body: String,
    created_at: Timestamp,
    url: String,
    diff_hunk: String,
    state: WireCommentState,
    viewer_can_update: bool,
    viewer_did_author: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePendingReview {
    id: String,
    viewer_did_author: bool,
    comments: Count,
}

struct FirstPage {
    detail: PullRequestDetail,
    timeline_cursor: Option<String>,
    threads_cursor: Option<String>,
}

fn parse_detail(pr: &PrRef, data: Data) -> Result<FirstPage> {
    let repository = data.repository.ok_or_else(not_found)?;
    let wire = repository.pull_request.ok_or_else(not_found)?;

    let mut merge_methods = Vec::new();
    if repository.merge_commit_allowed {
        merge_methods.push(MergeMethod::Merge);
    }
    if repository.squash_merge_allowed {
        merge_methods.push(MergeMethod::Squash);
    }
    if repository.rebase_merge_allowed {
        merge_methods.push(MergeMethod::Rebase);
    }

    let head_repo = wire
        .head_repository
        .map(|head| RepoRef::new(head.owner.login, head.name))
        .filter(|head| {
            !(head.owner.eq_ignore_ascii_case(&pr.repo.owner)
                && head.name.eq_ignore_ascii_case(&pr.repo.name))
        });

    let author: Option<Actor> = wire.author.map(Actor::from);
    let reviewers = collect_reviewers(
        wire.review_requests.into_nodes(),
        wire.latest_reviews.into_nodes(),
        author.as_ref().map(|author| author.login.as_str()),
    );

    let checks = wire
        .head_commit
        .into_nodes()
        .into_iter()
        .next()
        .and_then(|node| node.commit.status_check_rollup)
        .map(|rollup| Checks {
            state: Some(checks_state(rollup.state)),
            runs: rollup
                .contexts
                .into_nodes()
                .into_iter()
                .filter_map(check_run)
                .collect(),
        })
        .unwrap_or(Checks {
            state: None,
            runs: Vec::new(),
        });

    let (timeline, timeline_cursor) = parse_timeline(wire.timeline_items);
    let (threads, threads_cursor) = parse_threads(wire.review_threads);

    let pending_review = wire
        .pending_reviews
        .into_nodes()
        .into_iter()
        .find(|review| review.viewer_did_author)
        .map(|review| PendingReview {
            id: review.id,
            comment_count: review.comments.total_count,
        });

    let body_edits = body_edits(
        wire.user_content_edits
            .map(Connection::into_nodes)
            .unwrap_or_default(),
        wire.includes_created_edit,
    );
    let detail = PullRequestDetail {
        id: wire.id,
        pr: pr.clone(),
        url: wire.url,
        title: wire.title,
        body: wire.body,
        state: match wire.state {
            WirePrState::Open | WirePrState::Unknown => PrState::Open,
            WirePrState::Closed => PrState::Closed,
            WirePrState::Merged => PrState::Merged,
        },
        is_draft: wire.is_draft,
        mergeable: match wire.mergeable {
            WireMergeable::Mergeable => Mergeable::Mergeable,
            WireMergeable::Conflicting => Mergeable::Conflicting,
            WireMergeable::Unknown => Mergeable::Unknown,
        },
        merge_state: match wire.merge_state_status {
            WireMergeState::Behind => MergeStateStatus::Behind,
            WireMergeState::Blocked => MergeStateStatus::Blocked,
            WireMergeState::Clean => MergeStateStatus::Clean,
            WireMergeState::Dirty => MergeStateStatus::Dirty,
            WireMergeState::Draft => MergeStateStatus::Draft,
            WireMergeState::HasHooks => MergeStateStatus::HasHooks,
            WireMergeState::Unstable => MergeStateStatus::Unstable,
            WireMergeState::Unknown => MergeStateStatus::Unknown,
        },
        merge_methods,
        base_ref: wire.base_ref_name,
        base_oid: wire.base_ref_oid,
        head_ref: wire.head_ref_name,
        head_repo,
        head_oid: wire.head_ref_oid,
        author,
        created_at: wire.created_at,
        updated_at: wire.updated_at,
        body_edits,
        additions: wire.additions,
        deletions: wire.deletions,
        changed_files: wire.changed_files,
        review_decision: wire.review_decision.and_then(|decision| match decision {
            WireReviewDecision::Approved => Some(ReviewDecision::Approved),
            WireReviewDecision::ChangesRequested => Some(ReviewDecision::ChangesRequested),
            WireReviewDecision::ReviewRequired => Some(ReviewDecision::ReviewRequired),
            WireReviewDecision::Unknown => None,
        }),
        reviewers,
        labels: wire
            .labels
            .into_nodes()
            .into_iter()
            .map(|label| Label {
                id: label.id,
                name: label.name,
                color: label.color,
            })
            .collect(),
        assignees: wire
            .assignees
            .into_nodes()
            .into_iter()
            .map(Actor::from)
            .collect(),
        commits: wire
            .commits
            .into_nodes()
            .into_iter()
            .map(|node| commit(node.commit))
            .collect(),
        checks,
        timeline,
        threads,
        pending_review,
        viewer_login: data.viewer.login,
        viewer_can_update: wire.viewer_can_update,
    };
    Ok(FirstPage {
        detail,
        timeline_cursor,
        threads_cursor,
    })
}

/// Requested reviewers first, then people who reviewed and were not asked
/// again. A user who both reviewed and was re-requested appears once, as
/// `Requested`. The pull request author is never a reviewer.
fn collect_reviewers(
    requests: Vec<WireReviewRequest>,
    latest: Vec<WireLatestReview>,
    author_login: Option<&str>,
) -> Vec<Reviewer> {
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(login) = author_login {
        seen.insert(login.to_ascii_lowercase());
    }
    let mut reviewers = Vec::new();

    for request in requests {
        let Some(requested) = request.requested_reviewer else {
            continue;
        };
        let kind = match requested {
            WireRequestedReviewer {
                login: Some(login),
                avatar_url: Some(avatar_url),
                ..
            } => {
                if !seen.insert(login.to_ascii_lowercase()) {
                    continue;
                }
                ReviewerKind::User(Actor { login, avatar_url })
            }
            WireRequestedReviewer {
                name: Some(name),
                slug: Some(slug),
                ..
            } => ReviewerKind::Team { name, slug },
            _ => continue,
        };
        reviewers.push(Reviewer {
            kind,
            state: ReviewerState::Requested,
        });
    }

    for review in latest {
        let Some(author) = review.author else {
            continue;
        };
        let state = match review.state {
            WireReviewState::Pending => continue,
            WireReviewState::Approved => ReviewerState::Approved,
            WireReviewState::ChangesRequested => ReviewerState::ChangesRequested,
            WireReviewState::Dismissed => ReviewerState::Dismissed,
            WireReviewState::Commented | WireReviewState::Unknown => ReviewerState::Commented,
        };
        if !seen.insert(author.login.to_ascii_lowercase()) {
            continue;
        }
        reviewers.push(Reviewer {
            kind: ReviewerKind::User(author.into()),
            state,
        });
    }
    reviewers
}

fn checks_state(state: WireRollupState) -> ChecksState {
    match state {
        WireRollupState::Success => ChecksState::Passing,
        WireRollupState::Failure | WireRollupState::Error => ChecksState::Failing,
        WireRollupState::Pending | WireRollupState::Expected | WireRollupState::Unknown => {
            ChecksState::Pending
        }
    }
}

fn commit(wire: WireCommit) -> Commit {
    let (author_name, author) = match wire.author {
        Some(git_actor) => (
            git_actor.name.unwrap_or_default(),
            git_actor.user.map(Actor::from),
        ),
        None => (String::new(), None),
    };
    Commit {
        oid: wire.oid,
        headline: wire.message_headline,
        body: wire.message_body,
        author_name,
        author,
        committed_at: wire.committed_date,
        checks: wire
            .status_check_rollup
            .map(|rollup| checks_state(rollup.state)),
    }
}

fn check_run(context: WireContext) -> Option<CheckRun> {
    match context {
        WireContext::CheckRun(run) => {
            let workflow_run = run.check_suite.and_then(|suite| suite.workflow_run);
            let actions = run
                .database_id
                .zip(
                    workflow_run
                        .as_ref()
                        .and_then(|workflow_run| workflow_run.database_id),
                )
                .map(|(job_id, run_id)| ActionsJob { job_id, run_id });
            Some(CheckRun {
                name: run.name,
                workflow: workflow_run
                    .and_then(|workflow_run| workflow_run.workflow)
                    .map(|workflow| workflow.name),
                status: check_run_status(run.status, run.conclusion),
                started_at: run.started_at,
                completed_at: run.completed_at,
                details_url: run.details_url,
                is_required: run.is_required,
                actions,
            })
        }
        WireContext::StatusContext(status) => Some(CheckRun {
            name: status.context,
            workflow: None,
            status: match status.state {
                WireRollupState::Success => CheckStatus::Passed,
                WireRollupState::Failure | WireRollupState::Error => CheckStatus::Failed,
                WireRollupState::Pending | WireRollupState::Expected => CheckStatus::Running,
                WireRollupState::Unknown => CheckStatus::Neutral,
            },
            started_at: status.created_at,
            completed_at: None,
            details_url: status.target_url,
            is_required: status.is_required,
            actions: None,
        }),
        WireContext::Other => None,
    }
}

fn check_run_status(status: WireCheckRunStatus, conclusion: Option<WireConclusion>) -> CheckStatus {
    match status {
        WireCheckRunStatus::Queued
        | WireCheckRunStatus::Waiting
        | WireCheckRunStatus::Pending
        | WireCheckRunStatus::Requested => CheckStatus::Queued,
        WireCheckRunStatus::InProgress => CheckStatus::Running,
        WireCheckRunStatus::Unknown => CheckStatus::Neutral,
        WireCheckRunStatus::Completed => match conclusion {
            Some(WireConclusion::Success) => CheckStatus::Passed,
            Some(
                WireConclusion::Failure | WireConclusion::TimedOut | WireConclusion::StartupFailure,
            ) => CheckStatus::Failed,
            Some(WireConclusion::Skipped | WireConclusion::Stale) => CheckStatus::Skipped,
            Some(WireConclusion::Cancelled) => CheckStatus::Cancelled,
            Some(WireConclusion::ActionRequired) => CheckStatus::ActionRequired,
            Some(WireConclusion::Neutral | WireConclusion::Unknown) | None => CheckStatus::Neutral,
        },
    }
}

/// Converts one timeline page. Pending reviews are the viewer's draft and
/// belong to `PullRequestDetail::pending_review`, not the conversation.
fn parse_timeline(connection: Connection<WireTimelineNode>) -> (Vec<TimelineItem>, Option<String>) {
    let (nodes, cursor) = connection.into_page();
    let items = nodes
        .into_iter()
        .filter_map(|node| match node {
            WireTimelineNode::IssueComment(comment) => Some(TimelineItem::Comment(IssueComment {
                id: comment.id,
                author_is_bot: WireActor::is_bot(&comment.author),
                hidden: comment.is_minimized,
                hidden_reason: comment.minimized_reason,
                author: comment.author.map(Actor::from),
                body: comment.body,
                created_at: comment.created_at,
                url: comment.url,
                viewer_can_update: comment.viewer_can_update,
                viewer_did_author: comment.viewer_did_author,
            })),
            WireTimelineNode::PullRequestReview(review) => {
                let state = match review.state {
                    WireReviewState::Pending => return None,
                    WireReviewState::Approved => ReviewState::Approved,
                    WireReviewState::ChangesRequested => ReviewState::ChangesRequested,
                    WireReviewState::Dismissed => ReviewState::Dismissed,
                    WireReviewState::Commented | WireReviewState::Unknown => ReviewState::Commented,
                };
                Some(TimelineItem::Review(Review {
                    id: review.id,
                    author_is_bot: WireActor::is_bot(&review.author),
                    hidden: review.is_minimized,
                    hidden_reason: review.minimized_reason,
                    author: review.author.map(Actor::from),
                    state,
                    body: review.body,
                    submitted_at: review.submitted_at,
                    url: review.url,
                    comment_count: review.comments.total_count,
                    viewer_can_update: review.viewer_can_update,
                }))
            }
            WireTimelineNode::Other => None,
        })
        .collect();
    (items, cursor)
}

/// GitHub reports `startLine == line` for single-line threads; the domain
/// keeps a start only for a real range.
fn range_start(start: Option<u32>, end: Option<u32>) -> Option<u32> {
    start.filter(|start| Some(*start) != end)
}

fn parse_threads(connection: Connection<WireThread>) -> (Vec<ReviewThread>, Option<String>) {
    let (nodes, cursor) = connection.into_page();
    let threads = nodes
        .into_iter()
        .map(|thread| ReviewThread {
            id: thread.id,
            path: thread.path,
            line: thread.line,
            start_line: range_start(thread.start_line, thread.line),
            original_line: thread.original_line,
            original_start_line: range_start(thread.original_start_line, thread.original_line),
            side: thread.diff_side.into(),
            start_side: thread.start_diff_side.map(Side::from),
            is_outdated: thread.is_outdated,
            is_resolved: thread.is_resolved,
            resolved_by: thread.resolved_by.map(Actor::from),
            viewer_can_reply: thread.viewer_can_reply,
            viewer_can_resolve: thread.viewer_can_resolve,
            viewer_can_unresolve: thread.viewer_can_unresolve,
            comments: thread
                .comments
                .into_nodes()
                .into_iter()
                .map(|comment| ReviewComment {
                    id: comment.id,
                    author: comment.author.map(Actor::from),
                    body: comment.body,
                    created_at: comment.created_at,
                    url: comment.url,
                    diff_hunk: comment.diff_hunk,
                    is_pending: matches!(comment.state, WireCommentState::Pending),
                    viewer_can_update: comment.viewer_can_update,
                    viewer_did_author: comment.viewer_did_author,
                })
                .collect(),
        })
        .collect();
    (threads, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(login: &str) -> Value {
        json!({ "login": login, "avatarUrl": format!("https://avatars.example/{login}") })
    }

    fn pull_request_json() -> Value {
        json!({
            "id": "PR_1",
            "url": "https://github.com/acme/web/pull/7",
            "title": "Add search",
            "body": "Body **markdown**",
            "state": "OPEN",
            "isDraft": false,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "baseRefName": "main",
            "baseRefOid": "base123",
            "headRefName": "feature/search",
            "headRefOid": "abc123",
            "headRepository": { "owner": { "login": "Acme" }, "name": "Web" },
            "author": actor("pr-author"),
            "createdAt": "2026-01-02T03:04:05Z",
            "updatedAt": "2026-01-03T03:04:05Z",
            "includesCreatedEdit": false,
            "userContentEdits": { "nodes": [] },
            "additions": 10,
            "deletions": 4,
            "changedFiles": 3,
            "reviewDecision": "REVIEW_REQUIRED",
            "viewerCanUpdate": true,
            "reviewRequests": { "nodes": [
                { "requestedReviewer": actor("alice") },
                { "requestedReviewer": { "name": "Core team", "slug": "core" } },
                { "requestedReviewer": {} },
                { "requestedReviewer": null }
            ] },
            "latestReviews": { "nodes": [
                { "author": actor("alice"), "state": "APPROVED" },
                { "author": actor("bob"), "state": "CHANGES_REQUESTED" },
                { "author": actor("pr-author"), "state": "COMMENTED" },
                { "author": actor("carol"), "state": "SOMETHING_NEW" },
                { "author": null, "state": "APPROVED" }
            ] },
            "labels": { "nodes": [{ "id": "LA_bug", "name": "bug", "color": "d73a4a" }] },
            "assignees": { "nodes": [actor("dave")] },
            "commits": { "nodes": [
                { "commit": {
                    "oid": "c1",
                    "messageHeadline": "first",
                    "messageBody": "",
                    "committedDate": "2026-01-02T04:00:00Z",
                    "author": { "name": "Eve", "user": actor("eve") },
                    "statusCheckRollup": { "state": "FAILURE" }
                } },
                { "commit": {
                    "oid": "c2",
                    "messageHeadline": "second",
                    "messageBody": "details",
                    "committedDate": "2026-01-02T05:00:00Z",
                    "author": { "name": "Frank", "user": null },
                    "statusCheckRollup": null
                } }
            ] },
            "headCommit": { "nodes": [ { "commit": { "statusCheckRollup": {
                "state": "PENDING",
                "contexts": { "nodes": [
                    {
                        "__typename": "CheckRun",
                        "databaseId": 501,
                        "name": "build",
                        "status": "COMPLETED",
                        "conclusion": "SUCCESS",
                        "startedAt": "2026-01-02T05:01:00Z",
                        "completedAt": "2026-01-02T05:03:00Z",
                        "detailsUrl": "https://github.com/acme/web/actions/runs/1",
                        "isRequired": true,
                        "checkSuite": { "workflowRun": { "databaseId": 77, "workflow": { "name": "CI" } } }
                    },
                    {
                        "__typename": "CheckRun",
                        "name": "lint",
                        "status": "IN_PROGRESS",
                        "conclusion": null,
                        "startedAt": null,
                        "completedAt": null,
                        "detailsUrl": null,
                        "isRequired": false,
                        "checkSuite": { "workflowRun": null }
                    },
                    {
                        "__typename": "StatusContext",
                        "context": "ci/legacy",
                        "state": "PENDING",
                        "targetUrl": "https://ci.example/1",
                        "createdAt": "2026-01-02T05:02:00Z",
                        "isRequired": true
                    },
                    { "__typename": "FutureContext" }
                ] }
            } } } ] },
            "timelineItems": {
                "pageInfo": { "hasNextPage": true, "endCursor": "cursor-t" },
                "nodes": [
                    {
                        "__typename": "IssueComment",
                        "id": "IC_1",
                        "author": null,
                        "body": "ghost comment",
                        "createdAt": "2026-01-02T06:00:00Z",
                        "url": "https://github.com/acme/web/pull/7#issuecomment-1",
                        "viewerCanUpdate": false,
                        "viewerDidAuthor": false
                    },
                    {
                        "__typename": "PullRequestReview",
                        "id": "PRR_1",
                        "author": actor("bob"),
                        "state": "CHANGES_REQUESTED",
                        "body": "needs work",
                        "submittedAt": "2026-01-02T07:00:00Z",
                        "url": "https://github.com/acme/web/pull/7#pullrequestreview-1",
                        "comments": { "totalCount": 2 },
                        "viewerCanUpdate": false,
                        "isMinimized": true,
                        "minimizedReason": "outdated"
                    },
                    {
                        "__typename": "PullRequestReview",
                        "id": "PRR_draft",
                        "author": actor("me"),
                        "state": "PENDING",
                        "body": "",
                        "submittedAt": null,
                        "url": "https://github.com/acme/web/pull/7#pullrequestreview-2",
                        "comments": { "totalCount": 1 },
                        "viewerCanUpdate": true
                    },
                    { "__typename": "ReactionEvent" },
                    {
                        "__typename": "IssueComment",
                        "id": "IC_bot",
                        "author": { "__typename": "Bot", "login": "github-actions", "avatarUrl": "" },
                        "body": "plan output",
                        "createdAt": "2026-01-02T09:00:00Z",
                        "url": "https://github.com/acme/web/pull/7#issuecomment-2",
                        "viewerCanUpdate": false,
                        "viewerDidAuthor": false,
                        "isMinimized": false,
                        "minimizedReason": null
                    }
                ]
            },
            "reviewThreads": {
                "pageInfo": { "hasNextPage": false, "endCursor": "cursor-r" },
                "nodes": [
                    {
                        "id": "PRRT_outdated",
                        "path": "src/lib.rs",
                        "line": null,
                        "startLine": null,
                        "originalLine": 12,
                        "originalStartLine": 10,
                        "diffSide": "LEFT",
                        "startDiffSide": "LEFT",
                        "isOutdated": true,
                        "isResolved": true,
                        "resolvedBy": actor("bob"),
                        "viewerCanReply": true,
                        "viewerCanResolve": false,
                        "viewerCanUnresolve": true,
                        "comments": { "nodes": [
                            {
                                "id": "PRRC_1",
                                "author": null,
                                "body": "old comment",
                                "createdAt": "2026-01-02T08:00:00Z",
                                "url": "https://github.com/acme/web/pull/7#discussion_r1",
                                "diffHunk": "@@ -1 +1 @@",
                                "state": "SUBMITTED",
                                "viewerCanUpdate": false,
                                "viewerDidAuthor": false
                            }
                        ] }
                    },
                    {
                        "id": "PRRT_draft",
                        "path": "src/main.rs",
                        "line": 5,
                        "startLine": 5,
                        "originalLine": 5,
                        "originalStartLine": 5,
                        "diffSide": "SIDEWAYS",
                        "startDiffSide": null,
                        "isOutdated": false,
                        "isResolved": false,
                        "resolvedBy": null,
                        "viewerCanReply": true,
                        "viewerCanResolve": true,
                        "viewerCanUnresolve": false,
                        "comments": { "nodes": [
                            {
                                "id": "PRRC_2",
                                "author": actor("me"),
                                "body": "draft comment",
                                "createdAt": "2026-01-03T08:00:00Z",
                                "url": "https://github.com/acme/web/pull/7#discussion_r2",
                                "diffHunk": "@@ -5 +5 @@",
                                "state": "PENDING",
                                "viewerCanUpdate": true,
                                "viewerDidAuthor": true
                            }
                        ] }
                    }
                ]
            },
            "pendingReviews": { "nodes": [
                { "id": "PRR_other", "viewerDidAuthor": false, "comments": { "totalCount": 9 } },
                { "id": "PRR_draft", "viewerDidAuthor": true, "comments": { "totalCount": 1 } }
            ] }
        })
    }

    fn data(pull_request: Value) -> Data {
        let value = json!({
            "viewer": { "login": "me" },
            "repository": {
                "mergeCommitAllowed": true,
                "squashMergeAllowed": true,
                "rebaseMergeAllowed": false,
                "pullRequest": pull_request,
            }
        });
        serde_json::from_value(value).unwrap()
    }

    fn pr_ref() -> PrRef {
        PrRef::new(RepoRef::new("acme", "web"), 7)
    }

    fn parsed() -> FirstPage {
        parse_detail(&pr_ref(), data(pull_request_json())).unwrap()
    }

    #[test]
    fn header_fields_and_merge_options() {
        let detail = parsed().detail;
        assert_eq!(detail.state, PrState::Open);
        assert_eq!(detail.mergeable, Mergeable::Mergeable);
        assert_eq!(detail.merge_state, MergeStateStatus::Clean);
        assert_eq!(
            detail.merge_methods,
            vec![MergeMethod::Merge, MergeMethod::Squash]
        );
        assert_eq!(detail.head_oid, "abc123");
        assert_eq!(detail.review_decision, Some(ReviewDecision::ReviewRequired));
        assert_eq!(detail.viewer_login, "me");
        assert!(detail.viewer_can_update);
        assert_eq!(detail.head_repo, None, "same repo, case-insensitively");
        assert_eq!(detail.labels[0].color, "d73a4a");
        assert_eq!(detail.assignees[0].login, "dave");
    }

    #[test]
    fn description_edits_newest_first_with_the_original_marked() {
        let detail = parsed().detail;
        assert!(detail.body_edits.is_empty(), "never edited");

        let mut json = pull_request_json();
        json["includesCreatedEdit"] = json!(true);
        json["userContentEdits"] = json!({ "nodes": [
            { "editedAt": "2026-01-02T03:04:05Z", "editor": actor("pr-author") },
            { "editedAt": "2026-01-04T00:00:00Z", "editor": null },
            { "editedAt": "2026-01-03T00:00:00Z", "editor": actor("alice") },
        ] });
        let edits = parse_detail(&pr_ref(), data(json))
            .unwrap()
            .detail
            .body_edits;
        let summary: Vec<(Option<&str>, bool)> = edits
            .iter()
            .map(|edit| (edit.editor.as_ref().map(|a| a.login.as_str()), edit.created))
            .collect();
        assert_eq!(
            summary,
            vec![
                (None, false),
                (Some("alice"), false),
                (Some("pr-author"), true)
            ]
        );
    }

    #[test]
    fn fork_sets_head_repo() {
        let mut json = pull_request_json();
        json["headRepository"] = json!({ "owner": { "login": "someone" }, "name": "web" });
        let detail = parse_detail(&pr_ref(), data(json)).unwrap().detail;
        assert_eq!(detail.head_repo, Some(RepoRef::new("someone", "web")));

        let mut json = pull_request_json();
        json["headRepository"] = Value::Null;
        let detail = parse_detail(&pr_ref(), data(json)).unwrap().detail;
        assert_eq!(detail.head_repo, None);
    }

    #[test]
    fn reviewers_merge_requests_and_latest_reviews() {
        let reviewers = parsed().detail.reviewers;
        let summary: Vec<(String, ReviewerState)> = reviewers
            .iter()
            .map(|reviewer| {
                let name = match &reviewer.kind {
                    ReviewerKind::User(actor) => actor.login.clone(),
                    ReviewerKind::Team { slug, .. } => format!("team:{slug}"),
                };
                (name, reviewer.state)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("alice".to_owned(), ReviewerState::Requested),
                ("team:core".to_owned(), ReviewerState::Requested),
                ("bob".to_owned(), ReviewerState::ChangesRequested),
                ("carol".to_owned(), ReviewerState::Commented),
            ],
            "alice once as Requested, the author skipped, an unknown state is Commented, a null author skipped"
        );
    }

    #[test]
    fn commits_keep_order_and_missing_authors() {
        let commits = parsed().detail.commits;
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid, "c1");
        assert_eq!(commits[0].author.as_ref().unwrap().login, "eve");
        assert_eq!(commits[0].checks, Some(ChecksState::Failing));
        assert_eq!(commits[1].author_name, "Frank");
        assert_eq!(commits[1].author, None);
        assert_eq!(commits[1].checks, None);
    }

    #[test]
    fn head_commit_checks_cover_check_runs_and_statuses() {
        let checks = parsed().detail.checks;
        assert_eq!(checks.state, Some(ChecksState::Pending));
        assert_eq!(checks.runs.len(), 3, "the unknown context type is dropped");

        let build = &checks.runs[0];
        assert_eq!(build.name, "build");
        assert_eq!(build.workflow.as_deref(), Some("CI"));
        assert_eq!(build.status, CheckStatus::Passed);
        assert!(build.is_required);
        assert!(build.started_at.is_some() && build.completed_at.is_some());
        assert_eq!(
            build.actions,
            Some(ActionsJob {
                job_id: 501,
                run_id: 77
            })
        );

        let lint = &checks.runs[1];
        assert_eq!(lint.status, CheckStatus::Running);
        assert_eq!(lint.workflow, None);
        assert_eq!(lint.details_url, None);
        assert_eq!(lint.actions, None, "not an Actions job");

        let legacy = &checks.runs[2];
        assert_eq!(legacy.name, "ci/legacy");
        assert_eq!(legacy.workflow, None);
        assert_eq!(legacy.status, CheckStatus::Running);
        assert_eq!(legacy.details_url.as_deref(), Some("https://ci.example/1"));
        assert!(legacy.completed_at.is_none());
        assert_eq!(legacy.actions, None);
    }

    #[test]
    fn no_rollup_means_no_checks() {
        let mut json = pull_request_json();
        json["headCommit"] = json!({ "nodes": [ { "commit": { "statusCheckRollup": null } } ] });
        let checks = parse_detail(&pr_ref(), data(json)).unwrap().detail.checks;
        assert_eq!(checks.state, None);
        assert!(checks.runs.is_empty());
    }

    #[test]
    fn check_run_status_table() {
        use WireCheckRunStatus as S;
        use WireConclusion as C;
        let cases = [
            (S::Queued, None, CheckStatus::Queued),
            (S::Waiting, None, CheckStatus::Queued),
            (S::Pending, None, CheckStatus::Queued),
            (S::Requested, None, CheckStatus::Queued),
            (S::InProgress, None, CheckStatus::Running),
            (S::Completed, Some(C::Success), CheckStatus::Passed),
            (S::Completed, Some(C::Failure), CheckStatus::Failed),
            (S::Completed, Some(C::TimedOut), CheckStatus::Failed),
            (S::Completed, Some(C::StartupFailure), CheckStatus::Failed),
            (S::Completed, Some(C::Neutral), CheckStatus::Neutral),
            (S::Completed, Some(C::Skipped), CheckStatus::Skipped),
            (S::Completed, Some(C::Stale), CheckStatus::Skipped),
            (S::Completed, Some(C::Cancelled), CheckStatus::Cancelled),
            (
                S::Completed,
                Some(C::ActionRequired),
                CheckStatus::ActionRequired,
            ),
            (S::Completed, Some(C::Unknown), CheckStatus::Neutral),
            (S::Completed, None, CheckStatus::Neutral),
            (S::Unknown, None, CheckStatus::Neutral),
        ];
        for (status, conclusion, expected) in cases {
            assert_eq!(check_run_status(status, conclusion), expected);
        }
    }

    #[test]
    fn timeline_drops_pending_reviews_and_reports_the_next_cursor() {
        let first = parsed();
        assert_eq!(first.timeline_cursor.as_deref(), Some("cursor-t"));
        assert_eq!(first.threads_cursor, None);
        let timeline = first.detail.timeline;
        assert_eq!(timeline.len(), 3);
        match &timeline[0] {
            TimelineItem::Comment(comment) => {
                assert_eq!(comment.author, None);
                assert_eq!(comment.body, "ghost comment");
                assert!(!comment.author_is_bot && !comment.hidden);
            }
            other => panic!("expected a comment, got {other:?}"),
        }
        match &timeline[1] {
            TimelineItem::Review(review) => {
                assert_eq!(review.state, ReviewState::ChangesRequested);
                assert_eq!(review.comment_count, 2);
                assert!(review.submitted_at.is_some());
                assert!(review.hidden, "a minimized review is hidden");
                assert_eq!(review.hidden_reason.as_deref(), Some("outdated"));
                assert!(!review.author_is_bot);
            }
            other => panic!("expected a review, got {other:?}"),
        }
        match &timeline[2] {
            TimelineItem::Comment(comment) => {
                assert!(comment.author_is_bot, "a Bot author is a bot");
                assert!(!comment.hidden);
                assert_eq!(
                    comment.author.as_ref().map(|a| a.login.as_str()),
                    Some("github-actions")
                );
            }
            other => panic!("expected a comment, got {other:?}"),
        }
    }

    #[test]
    fn threads_cover_outdated_pending_and_unknown_sides() {
        let threads = parsed().detail.threads;
        assert_eq!(threads.len(), 2);

        let outdated = &threads[0];
        assert_eq!(outdated.line, None);
        assert_eq!(outdated.original_line, Some(12));
        assert_eq!(outdated.original_start_line, Some(10));
        assert_eq!(outdated.side, Side::Left);
        assert_eq!(outdated.start_side, Some(Side::Left));
        assert!(outdated.is_outdated && outdated.is_resolved);
        assert_eq!(outdated.resolved_by.as_ref().unwrap().login, "bob");
        assert_eq!(outdated.comments[0].author, None);
        assert!(!outdated.comments[0].is_pending);

        let draft = &threads[1];
        assert_eq!(draft.line, Some(5));
        assert_eq!(
            draft.side,
            Side::Right,
            "an unknown side falls back to Right"
        );
        assert_eq!(draft.start_side, None);
        assert_eq!(
            draft.start_line, None,
            "startLine equal to line is not a range"
        );
        assert!(draft.comments[0].is_pending);
        assert!(draft.comments[0].viewer_did_author);
    }

    #[test]
    fn pending_review_is_the_one_the_viewer_authored() {
        let pending = parsed().detail.pending_review.unwrap();
        assert_eq!(pending.id, "PRR_draft");
        assert_eq!(pending.comment_count, 1);

        let mut json = pull_request_json();
        json["pendingReviews"] = json!({ "nodes": [
            { "id": "PRR_other", "viewerDidAuthor": false, "comments": { "totalCount": 3 } }
        ] });
        let detail = parse_detail(&pr_ref(), data(json)).unwrap().detail;
        assert_eq!(detail.pending_review, None);
    }

    #[test]
    fn unknown_enum_values_do_not_fail_the_decode() {
        let mut json = pull_request_json();
        json["state"] = json!("QUANTUM");
        json["mergeable"] = json!("MAYBE");
        json["mergeStateStatus"] = json!("FUTURE_VALUE");
        json["reviewDecision"] = json!("NEW_DECISION");
        let detail = parse_detail(&pr_ref(), data(json)).unwrap().detail;
        assert_eq!(detail.mergeable, Mergeable::Unknown);
        assert_eq!(detail.merge_state, MergeStateStatus::Unknown);
        assert_eq!(detail.review_decision, None);
    }

    #[test]
    fn null_author_is_none() {
        let mut json = pull_request_json();
        json["author"] = Value::Null;
        let detail = parse_detail(&pr_ref(), data(json)).unwrap().detail;
        assert_eq!(detail.author, None);
        assert!(
            detail.reviewers.iter().any(
                |reviewer| matches!(&reviewer.kind, ReviewerKind::User(a) if a.login == "pr-author")
            ),
            "without an author nobody is skipped as the author"
        );
    }

    #[test]
    fn missing_pull_request_is_not_found() {
        for value in [
            json!({ "viewer": { "login": "me" }, "repository": null }),
            json!({
                "viewer": { "login": "me" },
                "repository": {
                    "mergeCommitAllowed": true,
                    "squashMergeAllowed": true,
                    "rebaseMergeAllowed": true,
                    "pullRequest": null
                }
            }),
        ] {
            let data: Data = serde_json::from_value(value).unwrap();
            match parse_detail(&pr_ref(), data) {
                Err(Error::GraphQl(messages)) => {
                    assert_eq!(messages, vec!["Pull request not found".to_owned()]);
                }
                Err(other) => panic!("unexpected error: {other}"),
                Ok(_) => panic!("expected an error"),
            }
        }
    }

    #[test]
    fn page_cursor_stops_when_there_is_no_next_page() {
        let connection: Connection<WireTimelineNode> = serde_json::from_value(json!({
            "pageInfo": { "hasNextPage": false, "endCursor": "ignored" },
            "nodes": [null, { "__typename": "ReactionEvent" }]
        }))
        .unwrap();
        let (items, cursor) = parse_timeline(connection);
        assert!(items.is_empty());
        assert_eq!(cursor, None);
    }
}
