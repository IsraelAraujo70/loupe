//! Merging a pull request through GitHub's asynchronous merge endpoint:
//! <https://docs.github.com/rest/pulls/pulls#merge-a-pull-request-asynchronously>.
//!
//! It is the only API that merges a pull request that is part of a stack (the
//! GraphQL `mergePullRequest` mutation rejects those), and GitHub recommends
//! it over the synchronous REST merge for every pull request, so Loupe uses it
//! for all of them. GitHub answers with a request UUID; the result is read from
//! <https://docs.github.com/rest/pulls/pulls#get-the-result-of-an-asynchronous-merge>
//! until it is no longer `pending`.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::rest::{RestMethod, http_error};
use crate::{Error, GitHub, MergeMethod, PrRef, Result};

/// How long to wait between two reads of a pending merge's result.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How many times to read a pending result before handing back
/// [`MergeOutcome::Pending`]: about two minutes.
const MAX_POLLS: u32 = 120;

/// How an asynchronous merge ended, as far as Loupe waited for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The pull request is merged (for a stacked one, with the open pull
    /// requests below it).
    Merged,
    /// The pull request was already in a merge queue; it merges when the
    /// queue gets to it.
    Enqueued,
    /// GitHub was still working on the merge when Loupe stopped asking.
    Pending,
}

/// One state of an asynchronous merge request, parsed from GitHub's
/// `{status, details}` answer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AsyncMerge {
    Pending { uuid: String },
    Merged,
    Enqueued,
    Failed { message: String },
}

impl GitHub {
    /// Merges `pr` with `method`, only if its head is still
    /// `expected_head_oid`, and waits for GitHub to finish. Blocks for up to
    /// about two minutes while the merge is pending. A merge GitHub refuses or
    /// that fails in the background is [`Error::MergeFailed`] with GitHub's
    /// reason.
    pub fn merge(
        &self,
        pr: &PrRef,
        method: MergeMethod,
        expected_head_oid: &str,
    ) -> Result<MergeOutcome> {
        run_merge(
            pr,
            method,
            expected_head_oid,
            |path, body| self.rest_with_status(RestMethod::Put(Some(body)), path),
            |path| self.rest(RestMethod::Get, path).map(String::into_bytes),
            std::thread::sleep,
        )
    }
}

/// The whole merge with its HTTP calls passed in: `put` sends the merge
/// request, `get` reads a result, and `wait` sleeps between reads.
fn run_merge(
    pr: &PrRef,
    method: MergeMethod,
    expected_head_oid: &str,
    put: impl FnOnce(&str, &Value) -> Result<(u16, Vec<u8>)>,
    mut get: impl FnMut(&str) -> Result<Vec<u8>>,
    wait: impl FnMut(Duration),
) -> Result<MergeOutcome> {
    let path = format!(
        "/repos/{}/{}/pulls/{}/merge-async",
        pr.repo.owner, pr.repo.name, pr.number
    );
    let (status, answer) = put(&path, &request_body(method, expected_head_oid))?;
    let first = parse_request_answer(status, &answer, method)?;
    settle(
        first,
        |uuid| parse_async_merge(&get(&format!("{path}/{uuid}"))?),
        wait,
    )
}

/// The merge request: GitHub refuses it when the head is no longer
/// `expected_head_oid`.
fn request_body(method: MergeMethod, expected_head_oid: &str) -> Value {
    json!({
        "sha": expected_head_oid,
        "merge_method": rest_method_name(method),
        // `default` would add the pull request to a merge queue when the base
        // branch has one; the method picker means a direct merge.
        "merge_action": "direct_merge",
    })
}

fn rest_method_name(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

/// Reads the answer to the merge request. 200 (already merged or queued) and
/// 202 (accepted) carry the request's state, 400 (cannot be merged) a failed
/// one, and 409 the merge request already pending for this pull request.
/// Loupe waits for that one only when it uses the `method` the user picked;
/// otherwise it would report someone else's merge as this one.
fn parse_request_answer(status: u16, body: &[u8], method: MergeMethod) -> Result<AsyncMerge> {
    match status {
        200 | 202 => parse_async_merge(body),
        409 => {
            let pending = parse_async_merge(body)?;
            let theirs = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|answer| {
                    answer["details"]["merge_method"]
                        .as_str()
                        .map(str::to_owned)
                });
            match theirs {
                Some(theirs) if theirs != rest_method_name(method) => Ok(AsyncMerge::Failed {
                    message: format!(
                        "a merge of this pull request with method {theirs} is already running. \
                         Wait for it to finish."
                    ),
                }),
                _ => Ok(pending),
            }
        }
        400 => parse_async_merge(body).map_err(|_| http_error(status, body)),
        _ => Err(http_error(status, body)),
    }
}

fn parse_async_merge(body: &[u8]) -> Result<AsyncMerge> {
    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Status {
        Pending,
        Merged,
        Enqueued,
        Failed,
    }
    #[derive(Deserialize)]
    struct Details {
        message: Option<String>,
        uuid: Option<String>,
    }
    #[derive(Deserialize)]
    struct Wire {
        status: Status,
        details: Details,
    }
    let wire: Wire =
        serde_json::from_slice(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(match wire.status {
        Status::Pending => {
            let uuid = wire
                .details
                .uuid
                .filter(|uuid| is_request_uuid(uuid))
                .ok_or_else(|| Error::Decode("pending merge without a valid uuid".into()))?;
            AsyncMerge::Pending { uuid }
        }
        Status::Merged => AsyncMerge::Merged,
        Status::Enqueued => AsyncMerge::Enqueued,
        Status::Failed => AsyncMerge::Failed {
            message: wire
                .details
                .message
                .unwrap_or_else(|| "GitHub did not say why".into()),
        },
    })
}

/// The UUID goes into a URL path, so it may only hold hex digits and dashes.
fn is_request_uuid(uuid: &str) -> bool {
    !uuid.is_empty()
        && uuid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

/// Follows a merge request from its first state to a final one: reads the
/// result with `fetch` after each `wait` while it is pending, at most
/// [`MAX_POLLS`] times.
fn settle(
    mut state: AsyncMerge,
    mut fetch: impl FnMut(&str) -> Result<AsyncMerge>,
    mut wait: impl FnMut(Duration),
) -> Result<MergeOutcome> {
    let mut polls = 0;
    loop {
        match state {
            AsyncMerge::Pending { uuid } if polls < MAX_POLLS => {
                wait(POLL_INTERVAL);
                polls += 1;
                state = fetch(&uuid)?;
            }
            AsyncMerge::Pending { .. } => return Ok(MergeOutcome::Pending),
            AsyncMerge::Merged => return Ok(MergeOutcome::Merged),
            AsyncMerge::Enqueued => return Ok(MergeOutcome::Enqueued),
            AsyncMerge::Failed { message } => return Err(Error::MergeFailed(message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::{
        AsyncMerge, MAX_POLLS, MergeOutcome, POLL_INTERVAL, parse_async_merge,
        parse_request_answer, request_body, rest_method_name, run_merge, settle,
    };
    use crate::{Error, MergeMethod, PrRef, RepoRef};

    // Answers recorded from api.github.com on this repository's pull
    // requests, except ENQUEUED and BACKGROUND_FAILURE, which follow the
    // response schema in the REST reference.
    const ACCEPTED: &str = r#"{"status":"pending","details":{"message":"Merge request enqueued.","uuid":"11bee280-61dd-46b9-9b18-c9f9576bc67b","merge_method":"squash","merge_action":"direct_merge","expected_head_sha":"6b468bc965fc953b423d4c8519e6f88266f8c82a","bypass_rules":false}}"#;
    const IN_PROGRESS: &str = r#"{"status":"pending","details":{"message":"Merge request is in progress.","uuid":"11bee280-61dd-46b9-9b18-c9f9576bc67b","merge_method":"squash","merge_action":"direct_merge","expected_head_sha":"6b468bc965fc953b423d4c8519e6f88266f8c82a","bypass_rules":false}}"#;
    const MERGED: &str = r#"{"status":"merged","details":{"message":"Pull request was merged.","sha":"ca95d3affd12a93d6a60e8d84ac161fd448bc42a"}}"#;
    const ALREADY_MERGED: &str = r#"{"status":"merged","details":{"message":"Pull request is already merged.","sha":"4a1ad49272b9be272d1c41af8e612511c25dc9b5"}}"#;
    const CLOSED: &str = r#"{"status":"failed","details":{"message":"Pull request is closed."}}"#;
    const HEAD_MOVED: &str =
        r#"{"status":"failed","details":{"message":"Pull request head branch was modified."}}"#;
    const NOT_FOUND: &str = r#"{"message":"Not Found","documentation_url":"https://docs.github.com/rest/pulls/pulls#merge-a-pull-request-asynchronously","status":"404"}"#;
    const ENQUEUED: &str = r#"{"status":"enqueued","details":{"message":"Pull request was added to the merge queue."}}"#;
    const BACKGROUND_FAILURE: &str = r#"{"status":"failed","details":{"message":"Required status check \"check\" is expected."}}"#;

    const PATH: &str = "/repos/IsraelAraujo70/loupe/pulls/43/merge-async";
    const HEAD: &str = "6b468bc965fc953b423d4c8519e6f88266f8c82a";

    /// Runs a squash merge of #43 against `put_answer` and then `results`,
    /// returning the outcome, the paths read, and how often it waited.
    fn merge_against(
        put_answer: (u16, &str),
        results: &[&str],
    ) -> (crate::Result<MergeOutcome>, Vec<String>, u32) {
        let pr = PrRef::new(RepoRef::new("IsraelAraujo70", "loupe"), 43);
        let mut results = results.iter().rev().copied().collect::<Vec<_>>();
        let mut read = Vec::new();
        let mut waits = 0;
        let outcome = run_merge(
            &pr,
            MergeMethod::Squash,
            HEAD,
            |path, body| {
                assert_eq!(path, PATH);
                assert_eq!(body, &request_body(MergeMethod::Squash, HEAD));
                Ok((put_answer.0, put_answer.1.as_bytes().to_vec()))
            },
            |path| {
                read.push(path.to_owned());
                let answer = results.pop().expect("an answer per read");
                Ok(answer.as_bytes().to_vec())
            },
            |delay| {
                assert_eq!(delay, POLL_INTERVAL);
                waits += 1;
            },
        );
        (outcome, read, waits)
    }

    #[test]
    fn an_accepted_merge_is_read_by_its_uuid_until_merged() {
        let (outcome, read, waits) =
            merge_against((202, ACCEPTED), &[IN_PROGRESS, IN_PROGRESS, MERGED]);
        assert_eq!(outcome.ok(), Some(MergeOutcome::Merged));
        assert_eq!(
            read,
            vec![format!("{PATH}/11bee280-61dd-46b9-9b18-c9f9576bc67b"); 3]
        );
        assert_eq!(waits, 3);
    }

    #[test]
    fn a_merge_already_running_with_the_same_method_is_followed() {
        let (outcome, read, _) = merge_against((409, ACCEPTED), &[MERGED]);
        assert_eq!(outcome.ok(), Some(MergeOutcome::Merged));
        assert_eq!(read.len(), 1);
    }

    #[test]
    fn a_merge_already_running_with_another_method_is_not_reported_as_this_one() {
        let rebase = ACCEPTED.replace(r#""merge_method":"squash""#, r#""merge_method":"rebase""#);
        let (outcome, read, waits) = merge_against((409, &rebase), &[]);
        assert!(matches!(
            outcome,
            Err(Error::MergeFailed(message)) if message.contains("rebase")
        ));
        assert!(read.is_empty());
        assert_eq!(waits, 0);
    }

    #[test]
    fn a_refused_merge_is_an_error_without_polling() {
        let (outcome, read, waits) = merge_against((400, HEAD_MOVED), &[]);
        assert!(matches!(
            outcome,
            Err(Error::MergeFailed(message)) if message == "Pull request head branch was modified."
        ));
        assert!(read.is_empty());
        assert_eq!(waits, 0);
    }

    fn pending() -> AsyncMerge {
        AsyncMerge::Pending {
            uuid: "11bee280-61dd-46b9-9b18-c9f9576bc67b".into(),
        }
    }

    #[test]
    fn asks_for_a_direct_merge_of_the_expected_head() {
        assert_eq!(
            request_body(
                MergeMethod::Squash,
                "6b468bc965fc953b423d4c8519e6f88266f8c82a"
            ),
            json!({
                "sha": "6b468bc965fc953b423d4c8519e6f88266f8c82a",
                "merge_method": "squash",
                "merge_action": "direct_merge",
            })
        );
    }

    #[test]
    fn sends_rest_method_names() {
        assert_eq!(rest_method_name(MergeMethod::Merge), "merge");
        assert_eq!(rest_method_name(MergeMethod::Squash), "squash");
        assert_eq!(rest_method_name(MergeMethod::Rebase), "rebase");
    }

    #[test]
    fn reads_each_answer_to_the_merge_request() {
        assert_eq!(
            parse_request_answer(200, ALREADY_MERGED.as_bytes(), MergeMethod::Squash).ok(),
            Some(AsyncMerge::Merged)
        );
        assert_eq!(
            parse_request_answer(202, ACCEPTED.as_bytes(), MergeMethod::Squash).ok(),
            Some(pending())
        );
        // A merge already pending for the pull request is followed, not an
        // error.
        assert_eq!(
            parse_request_answer(409, ACCEPTED.as_bytes(), MergeMethod::Squash).ok(),
            Some(pending())
        );
        assert_eq!(
            parse_request_answer(400, CLOSED.as_bytes(), MergeMethod::Squash).ok(),
            Some(AsyncMerge::Failed {
                message: "Pull request is closed.".into()
            })
        );
        assert_eq!(
            parse_request_answer(400, HEAD_MOVED.as_bytes(), MergeMethod::Squash).ok(),
            Some(AsyncMerge::Failed {
                message: "Pull request head branch was modified.".into()
            })
        );
        assert!(matches!(
            parse_request_answer(404, NOT_FOUND.as_bytes(), MergeMethod::Squash),
            Err(Error::Http { status: 404, message }) if message == "Not Found"
        ));
        assert!(matches!(
            parse_request_answer(400, b"{\"message\":\"Problems parsing JSON\"}", MergeMethod::Squash),
            Err(Error::Http { status: 400, message }) if message == "Problems parsing JSON"
        ));
    }

    #[test]
    fn rejects_a_pending_answer_without_a_safe_uuid() {
        let traversal = ACCEPTED.replace("11bee280-61dd-46b9-9b18-c9f9576bc67b", "../../user");
        assert!(matches!(
            parse_async_merge(traversal.as_bytes()),
            Err(Error::Decode(_))
        ));
        assert!(matches!(
            parse_async_merge(br#"{"status":"pending","details":{"message":"x"}}"#),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn a_background_failure_is_an_error_with_githubs_reason() {
        let outcome = settle(
            pending(),
            |_| parse_async_merge(BACKGROUND_FAILURE.as_bytes()),
            |_| {},
        );
        assert!(matches!(
            outcome,
            Err(Error::MergeFailed(message)) if message.starts_with("Required status check")
        ));
    }

    #[test]
    fn final_answers_need_no_polling() {
        let never = |_: &str| -> crate::Result<AsyncMerge> { panic!("polled a final state") };
        let no_wait = |_: Duration| panic!("waited on a final state");
        assert_eq!(
            settle(AsyncMerge::Merged, never, no_wait).ok(),
            Some(MergeOutcome::Merged)
        );
        assert_eq!(
            settle(
                parse_async_merge(ENQUEUED.as_bytes()).expect("enqueued parses"),
                never,
                no_wait
            )
            .ok(),
            Some(MergeOutcome::Enqueued)
        );
    }

    #[test]
    fn stops_asking_after_the_poll_limit() {
        let mut polls = 0;
        let outcome = settle(
            pending(),
            |_| {
                polls += 1;
                parse_async_merge(IN_PROGRESS.as_bytes())
            },
            |_| {},
        );
        assert_eq!(outcome.ok(), Some(MergeOutcome::Pending));
        assert_eq!(polls, MAX_POLLS);
    }
}
