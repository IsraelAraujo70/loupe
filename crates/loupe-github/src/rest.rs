//! REST endpoints with no GraphQL equivalent: changed files with patches,
//! notifications, and the failed workflow runs of a branch.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::RequestBuilder;

use crate::{
    BranchCi, BranchPullRequest, ChangedFile, Error, FailedRun, FileStatus, GitHub, Notification,
    NotificationSubject, PrRef, PrState, RepoRef, Result, Timestamp, ViewedState,
};

pub(crate) const REST_URL: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
/// A page of 100 files with full patches can pass ureq's default 10 MB cap.
const MAX_BODY_BYTES: u64 = 128 * 1024 * 1024;

const FILES_PER_PAGE: usize = 100;
/// GitHub lists at most 3000 files of a pull request.
const MAX_FILE_PAGES: usize = 30;
const NOTIFICATIONS_PER_PAGE: usize = 50;
const MAX_NOTIFICATION_PAGES: usize = 4;
/// The most notifications [`GitHub::notifications`] returns; a list this
/// long may have been cut.
pub const NOTIFICATIONS_LIMIT: usize = NOTIFICATIONS_PER_PAGE * MAX_NOTIFICATION_PAGES;
/// The failed runs [`GitHub::branch_ci`] reads for a branch, and for how
/// many of them it also reads the jobs: one more request each.
const FAILED_RUNS: usize = 10;
const RUNS_WITH_JOBS: usize = 5;

const FILE_VIEWED_STATES_QUERY: &str = "query PullRequestFileViewedStates($owner: String!, $name: String!, $number: Int!, $cursor: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { files(first: 100, after: $cursor) { pageInfo { hasNextPage endCursor } nodes { path viewerViewedState } } } } }";

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[("file_viewed_states", FILE_VIEWED_STATES_QUERY)];

#[derive(Clone, Copy)]
pub(crate) enum RestMethod<'a> {
    Get,
    Patch,
    Post,
    Put(Option<&'a Value>),
}

impl GitHub {
    /// Every changed file with its patch (REST, all pages), with the viewer's
    /// viewed state merged in from GraphQL.
    pub fn pull_request_files(&self, pr: &PrRef) -> Result<Vec<ChangedFile>> {
        let mut files = Vec::new();
        for page in 1..=MAX_FILE_PAGES {
            let path = format!(
                "/repos/{}/{}/pulls/{}/files?per_page={FILES_PER_PAGE}&page={page}",
                pr.repo.owner, pr.repo.name, pr.number
            );
            let body = self.rest(RestMethod::Get, &path)?;
            let batch = parse_files_page(&body)?;
            let full_page = batch.len() == FILES_PER_PAGE;
            files.extend(batch);
            if !full_page {
                break;
            }
        }
        self.refresh_viewed_states(pr, &mut files)?;
        Ok(files)
    }

    /// Sets each file's viewed state from GraphQL, without downloading the
    /// patches again: for a file list kept from an earlier load of the same
    /// head commit.
    pub fn refresh_viewed_states(&self, pr: &PrRef, files: &mut [ChangedFile]) -> Result<()> {
        let viewed = self.viewed_states(pr)?;
        merge_viewed_states(files, &viewed);
        Ok(())
    }

    /// The viewer's notifications, unread first and then newest first. `all`
    /// includes read ones.
    pub fn notifications(&self, all: bool) -> Result<Vec<Notification>> {
        let mut notifications = Vec::new();
        for page in 1..=MAX_NOTIFICATION_PAGES {
            let path =
                format!("/notifications?all={all}&per_page={NOTIFICATIONS_PER_PAGE}&page={page}");
            let body = self.rest(RestMethod::Get, &path)?;
            let batch = parse_notifications_page(&body)?;
            let full_page = batch.len() == NOTIFICATIONS_PER_PAGE;
            notifications.extend(batch);
            if !full_page {
                break;
            }
        }
        sort_notifications(&mut notifications);
        Ok(notifications)
    }

    pub fn mark_notification_read(&self, thread_id: &str) -> Result<()> {
        if thread_id.is_empty() || thread_id.contains(['/', '?', '#']) {
            return Err(Error::Decode("invalid notification id".into()));
        }
        self.rest(
            RestMethod::Patch,
            &format!("/notifications/threads/{thread_id}"),
        )
        .map(drop)
    }

    pub fn mark_all_notifications_read(&self) -> Result<()> {
        let body = json!({ "read": true });
        self.rest(RestMethod::Put(Some(&body)), "/notifications")
            .map(drop)
    }

    /// The pull request of `branch` in `repo` and the branch's failed
    /// workflow runs, newest first, with the failed jobs of the newest
    /// ones. This is what a "workflow run failed" notification leaves out.
    pub fn branch_ci(&self, repo: &RepoRef, branch: &str) -> Result<BranchCi> {
        let base = format!("/repos/{}/{}", repo.owner, repo.name);
        let branch_value = query_value(branch);
        let pulls = self.rest(
            RestMethod::Get,
            &format!(
                "{base}/pulls?head={}:{branch_value}&state=all&sort=updated&direction=desc&per_page=1",
                query_value(&repo.owner)
            ),
        )?;
        let runs = self.rest(
            RestMethod::Get,
            &format!(
                "{base}/actions/runs?branch={branch_value}&status=failure&per_page={FAILED_RUNS}"
            ),
        )?;
        let mut failed_runs = parse_failed_runs(&runs)?;
        for (id, run) in failed_runs.iter_mut().take(RUNS_WITH_JOBS) {
            let jobs = self.rest(
                RestMethod::Get,
                &format!("{base}/actions/runs/{id}/jobs?filter=latest&per_page=100"),
            )?;
            run.failed_jobs = parse_failed_jobs(&jobs)?;
        }
        Ok(BranchCi {
            pull_request: parse_branch_pull_request(repo, &pulls)?,
            failed_runs: failed_runs.into_iter().map(|(_, run)| run).collect(),
        })
    }

    fn viewed_states(&self, pr: &PrRef) -> Result<HashMap<String, ViewedState>> {
        let mut states = HashMap::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_FILE_PAGES {
            let data: ViewedStatesData = self.graphql(
                FILE_VIEWED_STATES_QUERY,
                json!({
                    "owner": pr.repo.owner,
                    "name": pr.repo.name,
                    "number": pr.number,
                    "cursor": cursor,
                }),
            )?;
            let files = data
                .repository
                .and_then(|repository| repository.pull_request)
                .ok_or_else(|| Error::Decode(format!("pull request {pr} not found")))?
                .files;
            states.extend(
                files
                    .nodes
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|node| (node.path, node.viewer_viewed_state.into())),
            );
            match files.page_info {
                PageInfo {
                    has_next_page: true,
                    end_cursor: Some(next),
                } => cursor = Some(next),
                _ => break,
            }
        }
        Ok(states)
    }

    /// Sends one REST call and returns the raw body of a 2xx response (empty
    /// for 202 and 205).
    pub(crate) fn rest(&self, method: RestMethod<'_>, path: &str) -> Result<String> {
        let body = self.rest_bytes(method, path, "application/vnd.github+json")?;
        String::from_utf8(body).map_err(|error| Error::Decode(error.to_string()))
    }

    /// Sends one REST request with `accept` as the media type and returns the
    /// body as bytes: for raw file contents and downloads.
    pub(crate) fn rest_bytes(
        &self,
        method: RestMethod<'_>,
        path: &str,
        accept: &str,
    ) -> Result<Vec<u8>> {
        if std::env::var_os("LOUPE_OFFLINE").is_some() {
            return Err(Error::Transport("Offline mode".into()));
        }
        let mut attempt = 0;
        loop {
            match self.send_rest(method, path, accept) {
                Err(Error::Http { status, .. }) if retries(method, status, attempt) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(700 * u64::from(attempt)));
                }
                result => return result,
            }
        }
    }

    fn send_rest(&self, method: RestMethod<'_>, path: &str, accept: &str) -> Result<Vec<u8>> {
        let url = format!("{REST_URL}{path}");
        let sent = match method {
            RestMethod::Get => self.authorize(self.agent.get(&url), accept).call(),
            RestMethod::Patch => self.authorize(self.agent.patch(&url), accept).send_empty(),
            RestMethod::Post => self.authorize(self.agent.post(&url), accept).send_empty(),
            RestMethod::Put(Some(body)) => {
                self.authorize(self.agent.put(&url), accept).send_json(body)
            }
            RestMethod::Put(None) => self.authorize(self.agent.put(&url), accept).send_empty(),
        };
        let response = sent.map_err(|error| Error::Transport(error.to_string()))?;
        self.read_success(response)
    }

    /// A GET that sends `etag` as `If-None-Match`. GitHub answers 304 when
    /// nothing changed, and a 304 does not count against the rate limit.
    fn conditional_get(&self, path: &str, etag: Option<&str>) -> Result<Conditional> {
        if std::env::var_os("LOUPE_OFFLINE").is_some() {
            return Err(Error::Transport("Offline mode".into()));
        }
        let mut request = self.authorize(
            self.agent.get(format!("{REST_URL}{path}")),
            "application/vnd.github+json",
        );
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let response = request
            .call()
            .map_err(|error| Error::Transport(error.to_string()))?;
        if response.status().as_u16() == 304 {
            self.record_rate(response.headers());
            return Ok(Conditional::NotModified);
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        self.read_success(response)?;
        Ok(Conditional::Modified(etag))
    }

    /// Records the rate limit, then returns the body of a 2xx response or
    /// the error the status stands for.
    fn read_success(&self, mut response: ureq::http::Response<ureq::Body>) -> Result<Vec<u8>> {
        self.record_rate(response.headers());
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|error| Error::Decode(error.to_string()))?;
        if status == 401 {
            return Err(Error::Unauthorized);
        }
        if !(200..300).contains(&status) {
            let message = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| value["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(Error::Http { status, message });
        }
        Ok(body)
    }

    /// Whether the check runs or the commit statuses of commit `sha`
    /// changed since `seen` was taken. Both requests are conditional, so
    /// asking while nothing changed costs no rate limit. Without `seen` for
    /// this commit the answer is `Changed`, with the ETags to send next time.
    pub fn poll_checks(
        &self,
        repo: &RepoRef,
        sha: &str,
        seen: Option<&ChecksEtag>,
    ) -> Result<ChecksPoll> {
        if sha.is_empty() || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Decode(format!("invalid commit sha {sha:?}")));
        }
        let seen = seen.filter(|seen| seen.sha == sha);
        let base = format!("/repos/{}/{}/commits/{sha}", repo.owner, repo.name);
        let runs = self.conditional_get(
            &format!("{base}/check-runs?per_page=100"),
            seen.and_then(|seen| seen.runs.as_deref()),
        )?;
        let statuses = self.conditional_get(
            &format!("{base}/status"),
            seen.and_then(|seen| seen.statuses.as_deref()),
        )?;
        Ok(ChecksPoll::of(sha, seen, runs, statuses))
    }

    pub(crate) fn authorize<B>(
        &self,
        request: RequestBuilder<B>,
        accept: &str,
    ) -> RequestBuilder<B> {
        request
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", API_VERSION)
    }
}

/// What a conditional GET found.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Conditional {
    NotModified,
    /// The resource changed or was never seen; its new ETag, if GitHub sent
    /// one.
    Modified(Option<String>),
}

/// The ETags of a commit's check runs and combined status, which
/// [`GitHub::poll_checks`] sends back to learn whether either changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksEtag {
    sha: String,
    runs: Option<String>,
    statuses: Option<String>,
}

impl ChecksEtag {
    /// The commit these ETags belong to.
    pub fn sha(&self) -> &str {
        &self.sha
    }
}

/// The answer of [`GitHub::poll_checks`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChecksPoll {
    /// GitHub answered 304 to both requests.
    Unchanged,
    /// Something changed (or there was nothing to compare with): load the
    /// checks again, and send these ETags next time.
    Changed(ChecksEtag),
}

impl ChecksPoll {
    fn of(sha: &str, seen: Option<&ChecksEtag>, runs: Conditional, statuses: Conditional) -> Self {
        let kept = |conditional: Conditional, seen: Option<&String>| match conditional {
            Conditional::NotModified => seen.cloned(),
            Conditional::Modified(etag) => etag,
        };
        match (seen, &runs, &statuses) {
            (Some(_), Conditional::NotModified, Conditional::NotModified) => ChecksPoll::Unchanged,
            _ => ChecksPoll::Changed(ChecksEtag {
                sha: sha.to_owned(),
                runs: kept(runs, seen.and_then(|seen| seen.runs.as_ref())),
                statuses: kept(statuses, seen.and_then(|seen| seen.statuses.as_ref())),
            }),
        }
    }
}

/// Whether a failed request goes out again: a read that met a gateway error
/// (GitHub answers a large page of Actions jobs with a 502 now and then, in
/// bursts), at most three times. A write never repeats: it may have
/// happened.
fn retries(method: RestMethod<'_>, status: u16, attempt: u32) -> bool {
    matches!(method, RestMethod::Get) && matches!(status, 502..=504) && attempt < 3
}

/// `text` as the value of a query parameter: everything but RFC 3986's
/// unreserved characters is percent-encoded, so `feat/x` and `a&b` stay one
/// value.
pub(crate) fn query_value(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[derive(Deserialize)]
struct WirePull {
    number: u64,
    title: String,
    state: String,
    merged_at: Option<Timestamp>,
    #[serde(default)]
    draft: bool,
}

fn parse_branch_pull_request(repo: &RepoRef, body: &str) -> Result<Option<BranchPullRequest>> {
    let pulls: Vec<WirePull> =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(pulls.into_iter().next().map(|pull| BranchPullRequest {
        pr: PrRef::new(repo.clone(), pull.number),
        title: pull.title,
        state: match (pull.merged_at, pull.state.as_str()) {
            (Some(_), _) => PrState::Merged,
            (None, "open") => PrState::Open,
            (None, _) => PrState::Closed,
        },
        is_draft: pull.draft,
    }))
}

#[derive(Deserialize)]
struct WireRuns {
    workflow_runs: Vec<WireRun>,
}

#[derive(Deserialize)]
struct WireRun {
    id: u64,
    name: Option<String>,
    run_number: u64,
    updated_at: Timestamp,
    html_url: String,
}

/// The runs of a page as `(run id, run)`, in GitHub's order: newest first.
fn parse_failed_runs(body: &str) -> Result<Vec<(u64, FailedRun)>> {
    let runs: WireRuns =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(runs
        .workflow_runs
        .into_iter()
        .map(|run| {
            (
                run.id,
                FailedRun {
                    workflow: run.name.unwrap_or_default(),
                    run_number: run.run_number,
                    finished_at: run.updated_at,
                    url: run.html_url,
                    failed_jobs: Vec::new(),
                },
            )
        })
        .collect())
}

#[derive(Deserialize)]
struct WireJobs {
    jobs: Vec<WireJob>,
}

#[derive(Deserialize)]
struct WireJob {
    name: String,
    conclusion: Option<String>,
}

/// The names of the jobs that failed or timed out.
fn parse_failed_jobs(body: &str) -> Result<Vec<String>> {
    let jobs: WireJobs =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(jobs
        .jobs
        .into_iter()
        .filter(|job| matches!(job.conclusion.as_deref(), Some("failure" | "timed_out")))
        .map(|job| job.name)
        .collect())
}

#[derive(Deserialize)]
struct WireFile {
    filename: String,
    previous_filename: Option<String>,
    status: String,
    additions: u32,
    deletions: u32,
    patch: Option<String>,
}

fn file_status(status: &str) -> FileStatus {
    match status {
        "added" => FileStatus::Added,
        "removed" => FileStatus::Removed,
        "renamed" => FileStatus::Renamed,
        "copied" => FileStatus::Copied,
        "changed" => FileStatus::Changed,
        "unchanged" => FileStatus::Unchanged,
        _ => FileStatus::Modified,
    }
}

/// Parses one page of `GET /pulls/{n}/files`; every file starts `Unviewed`.
fn parse_files_page(body: &str) -> Result<Vec<ChangedFile>> {
    let wire: Vec<WireFile> =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(wire
        .into_iter()
        .map(|file| ChangedFile {
            path: file.filename,
            previous_path: file.previous_filename,
            status: file_status(&file.status),
            additions: file.additions,
            deletions: file.deletions,
            patch: file.patch,
            viewed: ViewedState::Unviewed,
        })
        .collect())
}

fn merge_viewed_states(files: &mut [ChangedFile], states: &HashMap<String, ViewedState>) {
    for file in files {
        file.viewed = states
            .get(&file.path)
            .copied()
            .unwrap_or(ViewedState::Unviewed);
    }
}

#[derive(Deserialize)]
struct ViewedStatesData {
    repository: Option<ViewedStatesRepository>,
}

#[derive(Deserialize)]
struct ViewedStatesRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<ViewedStatesPullRequest>,
}

#[derive(Deserialize)]
struct ViewedStatesPullRequest {
    files: ViewedStatesFiles,
}

#[derive(Deserialize)]
struct ViewedStatesFiles {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Option<Vec<Option<ViewedStateNode>>>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct ViewedStateNode {
    path: String,
    #[serde(rename = "viewerViewedState")]
    viewer_viewed_state: WireViewedState,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireViewedState {
    Viewed,
    Dismissed,
    #[serde(other)]
    Unviewed,
}

impl From<WireViewedState> for ViewedState {
    fn from(state: WireViewedState) -> Self {
        match state {
            WireViewedState::Viewed => ViewedState::Viewed,
            WireViewedState::Dismissed => ViewedState::Dismissed,
            WireViewedState::Unviewed => ViewedState::Unviewed,
        }
    }
}

#[derive(Deserialize)]
struct WireNotification {
    id: String,
    repository: WireNotificationRepo,
    subject: WireSubject,
    reason: String,
    unread: bool,
    updated_at: Timestamp,
}

#[derive(Deserialize)]
struct WireNotificationRepo {
    full_name: String,
}

#[derive(Deserialize)]
struct WireSubject {
    title: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
}

fn parse_notifications_page(body: &str) -> Result<Vec<Notification>> {
    let wire: Vec<WireNotification> =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    wire.into_iter()
        .map(|notification| {
            let repo = RepoRef::parse(&notification.repository.full_name).ok_or_else(|| {
                Error::Decode(format!(
                    "invalid repository name {:?}",
                    notification.repository.full_name
                ))
            })?;
            Ok(Notification {
                id: notification.id,
                repo,
                title: notification.subject.title,
                subject: subject(
                    notification.subject.kind,
                    notification.subject.url.as_deref(),
                ),
                reason: notification.reason,
                unread: notification.unread,
                updated_at: notification.updated_at,
            })
        })
        .collect()
}

fn sort_notifications(notifications: &mut [Notification]) {
    notifications.sort_by_key(|n| (Reverse(n.unread), Reverse(n.updated_at)));
}

/// Reads `.../repos/{owner}/{name}/{pulls|issues}/{number}`; any other subject
/// (releases, commits, discussions) stays `Other`.
fn subject(kind: String, url: Option<&str>) -> NotificationSubject {
    let parsed = url.and_then(|url| {
        let (_, rest) = url.split_once("/repos/")?;
        let mut parts = rest.split('/');
        let repo = RepoRef::new(parts.next()?, parts.next()?);
        let segment = parts.next()?;
        let number: u64 = parts.next()?.parse().ok()?;
        parts.next().is_none().then_some((repo, segment, number))
    });
    match parsed {
        Some((repo, "pulls", number)) => NotificationSubject::PullRequest(PrRef::new(repo, number)),
        Some((repo, "issues", number)) => NotificationSubject::Issue { repo, number },
        _ => NotificationSubject::Other { kind },
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn only_reads_retry_and_only_on_gateway_errors() {
        assert!(retries(RestMethod::Get, 502, 0));
        assert!(retries(RestMethod::Get, 504, 2));
        assert!(!retries(RestMethod::Get, 502, 3));
        assert!(!retries(RestMethod::Get, 500, 0));
        assert!(!retries(RestMethod::Get, 404, 0));
        assert!(!retries(RestMethod::Post, 502, 0));
        assert!(!retries(RestMethod::Put(None), 503, 0));
    }

    const FILES_PAGE: &str = r#"[
        {"sha":"a1","filename":"src/new_name.rs","status":"renamed","additions":3,"deletions":1,
         "changes":4,"previous_filename":"src/old_name.rs","patch":"@@ -1,2 +1,4 @@\n a\n+b"},
        {"sha":"b2","filename":"assets/logo.png","status":"added","additions":0,"deletions":0,"changes":0},
        {"sha":"c3","filename":"README.md","status":"mystery","additions":1,"deletions":0,"changes":1,
         "patch":"@@ -0,0 +1 @@\n+hi"}
    ]"#;

    const NOTIFICATIONS_PAGE: &str = r#"[
        {"id":"1","unread":false,"reason":"mention","updated_at":"2026-11-09T10:00:00Z",
         "subject":{"title":"Fix the thing","url":"https://api.github.com/repos/acme/web/pulls/123","type":"PullRequest"},
         "repository":{"full_name":"acme/web"}},
        {"id":"2","unread":true,"reason":"review_requested","updated_at":"2026-11-08T10:00:00Z",
         "subject":{"title":"Crash on start","url":"https://api.github.com/repos/acme/api/issues/45","type":"Issue"},
         "repository":{"full_name":"acme/api"}},
        {"id":"3","unread":true,"reason":"subscribed","updated_at":"2026-11-09T09:00:00Z",
         "subject":{"title":"v1.2.0","url":"https://api.github.com/repos/acme/web/releases/9","type":"Release"},
         "repository":{"full_name":"acme/web"}},
        {"id":"4","unread":true,"reason":"subscribed","updated_at":"2026-11-07T09:00:00Z",
         "subject":{"title":"Question","url":null,"type":"Discussion"},
         "repository":{"full_name":"acme/web"}}
    ]"#;

    #[test]
    fn maps_renamed_binary_and_unknown_status_files() {
        let files = parse_files_page(FILES_PAGE).expect("fixture parses");
        assert_eq!(files.len(), 3);

        assert_eq!(files[0].path, "src/new_name.rs");
        assert_eq!(files[0].previous_path.as_deref(), Some("src/old_name.rs"));
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!((files[0].additions, files[0].deletions), (3, 1));
        assert_eq!(files[0].patch.as_deref(), Some("@@ -1,2 +1,4 @@\n a\n+b"));

        assert_eq!(files[1].status, FileStatus::Added);
        assert_eq!(files[1].patch, None);
        assert_eq!(files[1].previous_path, None);

        assert_eq!(files[2].status, FileStatus::Modified);
        assert!(files.iter().all(|f| f.viewed == ViewedState::Unviewed));
    }

    #[test]
    fn rejects_a_files_page_with_the_wrong_shape() {
        assert!(matches!(
            parse_files_page(r#"{"message":"nope"}"#),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn merges_viewed_states_keeping_rest_order() {
        let mut files = parse_files_page(FILES_PAGE).expect("fixture parses");
        let page = r#"{
            "repository":{"pullRequest":{"files":{
                "pageInfo":{"hasNextPage":false,"endCursor":"Y3Vyc29y"},
                "nodes":[
                    {"path":"README.md","viewerViewedState":"VIEWED"},
                    null,
                    {"path":"src/new_name.rs","viewerViewedState":"DISMISSED"},
                    {"path":"elsewhere.rs","viewerViewedState":"UNVIEWED"}
                ]}}}
        }"#;
        let data: ViewedStatesData = serde_json::from_str(page).expect("fixture parses");
        let files_page = data
            .repository
            .and_then(|r| r.pull_request)
            .expect("pull request present")
            .files;
        assert!(!files_page.page_info.has_next_page);
        let states: HashMap<String, ViewedState> = files_page
            .nodes
            .into_iter()
            .flatten()
            .flatten()
            .map(|node| (node.path, node.viewer_viewed_state.into()))
            .collect();

        merge_viewed_states(&mut files, &states);

        let merged: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.viewed)).collect();
        assert_eq!(
            merged,
            [
                ("src/new_name.rs", ViewedState::Dismissed),
                ("assets/logo.png", ViewedState::Unviewed),
                ("README.md", ViewedState::Viewed),
            ]
        );
    }

    #[test]
    fn maps_pull_request_issue_and_release_notifications() {
        let notifications = parse_notifications_page(NOTIFICATIONS_PAGE).expect("fixture parses");
        assert_eq!(notifications.len(), 4);

        let pr = &notifications[0];
        assert_eq!(pr.id, "1");
        assert_eq!(pr.repo, RepoRef::new("acme", "web"));
        assert_eq!(pr.title, "Fix the thing");
        assert_eq!(pr.reason, "mention");
        assert!(!pr.unread);
        assert_eq!(
            pr.updated_at,
            Utc.with_ymd_and_hms(2026, 11, 9, 10, 0, 0).unwrap()
        );
        assert_eq!(
            pr.subject,
            NotificationSubject::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 123))
        );

        assert_eq!(
            notifications[1].subject,
            NotificationSubject::Issue {
                repo: RepoRef::new("acme", "api"),
                number: 45
            }
        );
        assert_eq!(
            notifications[2].subject,
            NotificationSubject::Other {
                kind: "Release".into()
            }
        );
        assert_eq!(
            notifications[3].subject,
            NotificationSubject::Other {
                kind: "Discussion".into()
            }
        );
    }

    #[test]
    fn sorts_unread_first_then_newest_first() {
        let mut notifications =
            parse_notifications_page(NOTIFICATIONS_PAGE).expect("fixture parses");
        sort_notifications(&mut notifications);
        let ids: Vec<_> = notifications.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["3", "2", "4", "1"]);
    }

    #[test]
    fn rejects_a_notification_with_a_bad_repository_name() {
        let page = r#"[{"id":"1","unread":true,"reason":"mention","updated_at":"2026-11-09T10:00:00Z",
            "subject":{"title":"t","url":null,"type":"Issue"},"repository":{"full_name":"nope"}}]"#;
        assert!(matches!(
            parse_notifications_page(page),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn a_branch_name_is_one_query_value() {
        assert_eq!(query_value("feat/comment-order"), "feat%2Fcomment-order");
        assert_eq!(query_value("a&b=c d"), "a%26b%3Dc%20d");
        assert_eq!(query_value("v1.2_x~y-z"), "v1.2_x~y-z");
    }

    #[test]
    fn reads_the_branch_pull_request_and_its_state() {
        let repo = RepoRef::new("acme", "web");
        let merged = r#"[{"number":19,"title":"Sort comments","state":"closed",
            "merged_at":"2026-11-11T01:26:26Z","draft":false}]"#;
        let pull = parse_branch_pull_request(&repo, merged)
            .expect("parses")
            .expect("a pull request");
        assert_eq!(pull.pr, PrRef::new(repo.clone(), 19));
        assert_eq!(pull.title, "Sort comments");
        assert_eq!(pull.state, PrState::Merged);
        let open = r#"[{"number":7,"title":"WIP","state":"open","merged_at":null,"draft":true}]"#;
        let pull = parse_branch_pull_request(&repo, open).unwrap().unwrap();
        assert_eq!((pull.state, pull.is_draft), (PrState::Open, true));
        assert_eq!(parse_branch_pull_request(&repo, "[]").unwrap(), None);
    }

    #[test]
    fn reads_failed_runs_newest_first_and_the_jobs_that_failed() {
        let runs = r#"{"total_count":2,"workflow_runs":[
            {"id":42,"name":"CI","run_number":99,"updated_at":"2026-11-11T01:21:53Z",
             "html_url":"https://github.com/acme/web/actions/runs/42","conclusion":"failure"},
            {"id":41,"name":"CI","run_number":97,"updated_at":"2026-11-11T01:21:34Z",
             "html_url":"https://github.com/acme/web/actions/runs/41","conclusion":"failure"}]}"#;
        let parsed = parse_failed_runs(runs).expect("parses");
        let numbers: Vec<(u64, u64)> = parsed
            .iter()
            .map(|(id, run)| (*id, run.run_number))
            .collect();
        assert_eq!(numbers, [(42, 99), (41, 97)]);
        assert_eq!(parsed[0].1.workflow, "CI");
        let jobs = r#"{"total_count":3,"jobs":[
            {"name":"fmt","conclusion":"success"},
            {"name":"check","conclusion":"failure"},
            {"name":"e2e","conclusion":"timed_out"},
            {"name":"deploy","conclusion":null}]}"#;
        assert_eq!(parse_failed_jobs(jobs).expect("parses"), ["check", "e2e"]);
    }

    #[test]
    fn marking_a_malformed_thread_id_fails_locally() {
        let github = GitHub::new("unused");
        for id in ["", "1/2", "1?x=y", "1#frag"] {
            assert!(
                matches!(
                    github.mark_notification_read(id),
                    Err(Error::Decode(message)) if message == "invalid notification id"
                ),
                "{id:?} should be rejected before any request"
            );
        }
    }

    fn etag(sha: &str, runs: &str, statuses: &str) -> ChecksEtag {
        ChecksEtag {
            sha: sha.into(),
            runs: Some(runs.into()),
            statuses: Some(statuses.into()),
        }
    }

    #[test]
    fn checks_are_unchanged_only_when_both_requests_answer_304() {
        let seen = etag("abc", "r1", "s1");
        assert_eq!(
            ChecksPoll::of(
                "abc",
                Some(&seen),
                Conditional::NotModified,
                Conditional::NotModified
            ),
            ChecksPoll::Unchanged
        );
        assert_eq!(
            ChecksPoll::of(
                "abc",
                Some(&seen),
                Conditional::Modified(Some("r2".into())),
                Conditional::NotModified
            ),
            ChecksPoll::Changed(etag("abc", "r2", "s1"))
        );
        assert_eq!(
            ChecksPoll::of(
                "abc",
                Some(&seen),
                Conditional::NotModified,
                Conditional::Modified(Some("s2".into()))
            ),
            ChecksPoll::Changed(etag("abc", "r1", "s2"))
        );
    }

    #[test]
    fn a_first_poll_is_a_change_that_brings_the_etags() {
        assert_eq!(
            ChecksPoll::of(
                "abc",
                None,
                Conditional::Modified(Some("r1".into())),
                Conditional::Modified(Some("s1".into()))
            ),
            ChecksPoll::Changed(etag("abc", "r1", "s1"))
        );
    }

    #[test]
    fn polling_rejects_a_sha_that_would_change_the_path() {
        let github = GitHub::new("unused");
        let repo = RepoRef::new("acme", "app");
        for sha in ["", "abc/../../user", "abc?x=1", "main"] {
            assert!(
                matches!(github.poll_checks(&repo, sha, None), Err(Error::Decode(_))),
                "{sha:?} should be rejected before any request"
            );
        }
    }
}
