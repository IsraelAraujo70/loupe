//! Every GraphQL write the app makes (merging goes through REST, in
//! `merge.rs`). Each returns once GitHub accepted it; the app
//! reloads the pull request afterwards.

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Value, json};

use crate::{Error, GitHub, NewThread, Result, ReviewEvent, Side};

const ADD_COMMENT: &str = "mutation AddComment($subjectId: ID!, $body: String!) { \
    addComment(input: {subjectId: $subjectId, body: $body}) { clientMutationId } }";

const EDIT_ISSUE_COMMENT: &str = "mutation EditIssueComment($id: ID!, $body: String!) { \
    updateIssueComment(input: {id: $id, body: $body}) { clientMutationId } }";

const EDIT_REVIEW_COMMENT: &str = "mutation EditReviewComment($id: ID!, $body: String!) { \
    updatePullRequestReviewComment(input: {pullRequestReviewCommentId: $id, body: $body}) { \
    clientMutationId } }";

const ADD_REVIEW_THREAD: &str = "mutation AddReviewThread($pullRequestId: ID!, $path: String!, \
    $body: String!, $line: Int!, $side: DiffSide!, $startLine: Int, $startSide: DiffSide) { \
    addPullRequestReviewThread(input: {pullRequestId: $pullRequestId, path: $path, body: $body, \
    line: $line, side: $side, startLine: $startLine, startSide: $startSide, subjectType: LINE}) { \
    thread { id } } }";

const REPLY_TO_THREAD: &str = "mutation ReplyToThread($threadId: ID!, $body: String!) { \
    addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $threadId, body: $body}) { \
    clientMutationId } }";

const RESOLVE_THREAD: &str = "mutation ResolveThread($threadId: ID!) { \
    resolveReviewThread(input: {threadId: $threadId}) { clientMutationId } }";

const UNRESOLVE_THREAD: &str = "mutation UnresolveThread($threadId: ID!) { \
    unresolveReviewThread(input: {threadId: $threadId}) { clientMutationId } }";

const SUBMIT_REVIEW: &str = "mutation SubmitReview($reviewId: ID!, $event: PullRequestReviewEvent!, \
    $body: String) { submitPullRequestReview(input: {pullRequestReviewId: $reviewId, \
    event: $event, body: $body}) { clientMutationId } }";

const CREATE_REVIEW: &str = "mutation CreateReview($pullRequestId: ID!, \
    $event: PullRequestReviewEvent!, $body: String) { \
    addPullRequestReview(input: {pullRequestId: $pullRequestId, event: $event, body: $body}) { \
    clientMutationId } }";

const DISCARD_REVIEW: &str = "mutation DiscardReview($reviewId: ID!) { \
    deletePullRequestReview(input: {pullRequestReviewId: $reviewId}) { clientMutationId } }";

const MARK_FILE_VIEWED: &str = "mutation MarkFileViewed($pullRequestId: ID!, $path: String!) { \
    markFileAsViewed(input: {pullRequestId: $pullRequestId, path: $path}) { clientMutationId } }";

const UNMARK_FILE_VIEWED: &str = "mutation UnmarkFileViewed($pullRequestId: ID!, $path: String!) { \
    unmarkFileAsViewed(input: {pullRequestId: $pullRequestId, path: $path}) { clientMutationId } }";

const MARK_READY: &str = "mutation MarkReady($pullRequestId: ID!) { \
    markPullRequestReadyForReview(input: {pullRequestId: $pullRequestId}) { clientMutationId } }";

const CONVERT_TO_DRAFT: &str = "mutation ConvertToDraft($pullRequestId: ID!) { \
    convertPullRequestToDraft(input: {pullRequestId: $pullRequestId}) { clientMutationId } }";

const CLOSE_PULL_REQUEST: &str = "mutation ClosePullRequest($pullRequestId: ID!) { \
    closePullRequest(input: {pullRequestId: $pullRequestId}) { clientMutationId } }";

const REOPEN_PULL_REQUEST: &str = "mutation ReopenPullRequest($pullRequestId: ID!) { \
    reopenPullRequest(input: {pullRequestId: $pullRequestId}) { clientMutationId } }";

const REQUEST_REVIEWERS: &str = "mutation RequestReviewers($pullRequestId: ID!, \
    $userIds: [ID!]) { \
    requestReviews(input: {pullRequestId: $pullRequestId, userIds: $userIds, union: true}) { \
    clientMutationId } }";

const UPDATE_PULL_REQUEST: &str = "mutation UpdatePullRequest($pullRequestId: ID!, \
    $title: String, $body: String) { \
    updatePullRequest(input: {pullRequestId: $pullRequestId, title: $title, body: $body}) { \
    clientMutationId } }";

const ADD_LABELS: &str = "mutation AddLabels($labelableId: ID!, $labelIds: [ID!]!) { \
    addLabelsToLabelable(input: {labelableId: $labelableId, labelIds: $labelIds}) { \
    clientMutationId } }";

const REMOVE_LABELS: &str = "mutation RemoveLabels($labelableId: ID!, $labelIds: [ID!]!) { \
    removeLabelsFromLabelable(input: {labelableId: $labelableId, labelIds: $labelIds}) { \
    clientMutationId } }";

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[
    ("add_comment", ADD_COMMENT),
    ("edit_issue_comment", EDIT_ISSUE_COMMENT),
    ("edit_review_comment", EDIT_REVIEW_COMMENT),
    ("add_review_thread", ADD_REVIEW_THREAD),
    ("reply_to_thread", REPLY_TO_THREAD),
    ("resolve_thread", RESOLVE_THREAD),
    ("unresolve_thread", UNRESOLVE_THREAD),
    ("submit_review", SUBMIT_REVIEW),
    ("create_review", CREATE_REVIEW),
    ("discard_review", DISCARD_REVIEW),
    ("mark_file_viewed", MARK_FILE_VIEWED),
    ("unmark_file_viewed", UNMARK_FILE_VIEWED),
    ("mark_ready_for_review", MARK_READY),
    ("convert_to_draft", CONVERT_TO_DRAFT),
    ("close_pull_request", CLOSE_PULL_REQUEST),
    ("reopen_pull_request", REOPEN_PULL_REQUEST),
    ("request_reviewers", REQUEST_REVIEWERS),
    ("update_pull_request", UPDATE_PULL_REQUEST),
    ("add_labels", ADD_LABELS),
    ("remove_labels", REMOVE_LABELS),
];

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Left => "LEFT",
        Side::Right => "RIGHT",
    }
}

fn review_event_name(event: ReviewEvent) -> &'static str {
    match event {
        ReviewEvent::Comment => "COMMENT",
        ReviewEvent::Approve => "APPROVE",
        ReviewEvent::RequestChanges => "REQUEST_CHANGES",
    }
}

/// Variables for `ADD_REVIEW_THREAD`. A single-line anchor leaves
/// `startLine`/`startSide` unset, which GitHub treats as "no range".
fn review_thread_variables(thread: &NewThread) -> Value {
    let anchor = &thread.anchor;
    let mut variables = json!({
        "pullRequestId": thread.pull_request_id,
        "path": thread.path,
        "body": thread.body,
        "line": anchor.line,
        "side": side_name(anchor.side),
    });
    if let (Some((start_line, start_side)), Some(map)) = (anchor.start, variables.as_object_mut()) {
        map.insert("startLine".into(), json!(start_line));
        map.insert("startSide".into(), json!(side_name(start_side)));
    }
    variables
}

impl GitHub {
    /// Sends one mutation and discards the payload: callers reload the pull
    /// request afterwards instead of reading the mutation result.
    fn mutate(&self, query: &str, variables: Value) -> Result<()> {
        self.graphql::<IgnoredAny>(query, variables).map(|_| ())
    }

    /// Adds a conversation comment to a pull request or issue.
    pub fn add_comment(&self, subject_id: &str, body: &str) -> Result<()> {
        self.mutate(
            ADD_COMMENT,
            json!({ "subjectId": subject_id, "body": body }),
        )
    }

    pub fn edit_issue_comment(&self, comment_id: &str, body: &str) -> Result<()> {
        self.mutate(
            EDIT_ISSUE_COMMENT,
            json!({ "id": comment_id, "body": body }),
        )
    }

    pub fn edit_review_comment(&self, comment_id: &str, body: &str) -> Result<()> {
        self.mutate(
            EDIT_REVIEW_COMMENT,
            json!({ "id": comment_id, "body": body }),
        )
    }

    /// Adds an inline thread to the viewer's pending review, starting one
    /// when none exists.
    pub fn add_review_thread(&self, thread: &NewThread) -> Result<()> {
        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "addPullRequestReviewThread")]
            payload: Payload,
        }
        #[derive(Deserialize)]
        struct Payload {
            thread: Option<IgnoredAny>,
        }
        let data: Data = self.graphql(ADD_REVIEW_THREAD, review_thread_variables(thread))?;
        match data.payload.thread {
            Some(_) => Ok(()),
            None => Err(Error::GraphQl(vec![
                "GitHub did not create the thread".into(),
            ])),
        }
    }

    /// Replies to a thread. The reply is posted at once, not added to a
    /// pending review.
    pub fn reply_to_thread(&self, thread_id: &str, body: &str) -> Result<()> {
        self.mutate(
            REPLY_TO_THREAD,
            json!({ "threadId": thread_id, "body": body }),
        )
    }

    pub fn resolve_thread(&self, thread_id: &str) -> Result<()> {
        self.mutate(RESOLVE_THREAD, json!({ "threadId": thread_id }))
    }

    pub fn unresolve_thread(&self, thread_id: &str) -> Result<()> {
        self.mutate(UNRESOLVE_THREAD, json!({ "threadId": thread_id }))
    }

    /// Submits the pending review `review_id`, or creates and submits a new
    /// review when there is none.
    pub fn submit_review(
        &self,
        pull_request_id: &str,
        review_id: Option<&str>,
        event: ReviewEvent,
        body: &str,
    ) -> Result<()> {
        let event = review_event_name(event);
        match review_id {
            Some(review_id) => self.mutate(
                SUBMIT_REVIEW,
                json!({ "reviewId": review_id, "event": event, "body": body }),
            ),
            None => self.mutate(
                CREATE_REVIEW,
                json!({ "pullRequestId": pull_request_id, "event": event, "body": body }),
            ),
        }
    }

    /// Deletes a pending review and its comments.
    pub fn discard_review(&self, review_id: &str) -> Result<()> {
        self.mutate(DISCARD_REVIEW, json!({ "reviewId": review_id }))
    }

    pub fn set_file_viewed(&self, pull_request_id: &str, path: &str, viewed: bool) -> Result<()> {
        let query = if viewed {
            MARK_FILE_VIEWED
        } else {
            UNMARK_FILE_VIEWED
        };
        self.mutate(
            query,
            json!({ "pullRequestId": pull_request_id, "path": path }),
        )
    }

    pub fn mark_ready_for_review(&self, pull_request_id: &str) -> Result<()> {
        self.mutate(MARK_READY, json!({ "pullRequestId": pull_request_id }))
    }

    pub fn convert_to_draft(&self, pull_request_id: &str) -> Result<()> {
        self.mutate(
            CONVERT_TO_DRAFT,
            json!({ "pullRequestId": pull_request_id }),
        )
    }

    pub fn close_pull_request(&self, pull_request_id: &str) -> Result<()> {
        self.mutate(
            CLOSE_PULL_REQUEST,
            json!({ "pullRequestId": pull_request_id }),
        )
    }

    pub fn reopen_pull_request(&self, pull_request_id: &str) -> Result<()> {
        self.mutate(
            REOPEN_PULL_REQUEST,
            json!({ "pullRequestId": pull_request_id }),
        )
    }

    /// Sets the title, the body, or both; `None` leaves that field as it is.
    pub fn update_pull_request(
        &self,
        pull_request_id: &str,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        self.mutate(
            UPDATE_PULL_REQUEST,
            json!({ "pullRequestId": pull_request_id, "title": title, "body": body }),
        )
    }

    /// Adds labels (label node ids) to a pull request or issue.
    pub fn add_labels(&self, labelable_id: &str, label_ids: &[String]) -> Result<()> {
        self.mutate(
            ADD_LABELS,
            json!({ "labelableId": labelable_id, "labelIds": label_ids }),
        )
    }

    /// Removes labels (label node ids) from a pull request or issue.
    pub fn remove_labels(&self, labelable_id: &str, label_ids: &[String]) -> Result<()> {
        self.mutate(
            REMOVE_LABELS,
            json!({ "labelableId": labelable_id, "labelIds": label_ids }),
        )
    }

    /// Adds reviewers (user node ids) without removing existing requests.
    pub fn request_reviewers(&self, pull_request_id: &str, user_ids: &[String]) -> Result<()> {
        self.mutate(
            REQUEST_REVIEWERS,
            json!({ "pullRequestId": pull_request_id, "userIds": user_ids }),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{review_event_name, review_thread_variables, side_name};
    use crate::{LineAnchor, NewThread, ReviewEvent, Side};

    fn thread(anchor: LineAnchor) -> NewThread {
        NewThread {
            pull_request_id: "PR_x".into(),
            path: "src/a.rs".into(),
            body: "hi".into(),
            anchor,
        }
    }

    #[test]
    fn maps_sides() {
        assert_eq!(side_name(Side::Left), "LEFT");
        assert_eq!(side_name(Side::Right), "RIGHT");
    }

    #[test]
    fn maps_review_events() {
        assert_eq!(review_event_name(ReviewEvent::Comment), "COMMENT");
        assert_eq!(review_event_name(ReviewEvent::Approve), "APPROVE");
        assert_eq!(
            review_event_name(ReviewEvent::RequestChanges),
            "REQUEST_CHANGES"
        );
    }

    #[test]
    fn single_line_anchor_omits_range() {
        let variables = review_thread_variables(&thread(LineAnchor {
            line: 7,
            side: Side::Left,
            start: None,
        }));
        assert_eq!(
            variables,
            json!({
                "pullRequestId": "PR_x",
                "path": "src/a.rs",
                "body": "hi",
                "line": 7,
                "side": "LEFT",
            })
        );
    }

    #[test]
    fn range_anchor_carries_start() {
        let variables = review_thread_variables(&thread(LineAnchor {
            line: 9,
            side: Side::Right,
            start: Some((4, Side::Left)),
        }));
        assert_eq!(variables["line"], 9);
        assert_eq!(variables["side"], "RIGHT");
        assert_eq!(variables["startLine"], 4);
        assert_eq!(variables["startSide"], "LEFT");
    }
}
