//! REST endpoints with no GraphQL equivalent: changed files with patches,
//! notifications, and the failed workflow runs of a branch.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::RequestBuilder;

use crate::{
    Actor, BranchCi, BranchPullRequest, ChangedFile, Commit, CommitFiles, Error, FailedRun,
    FileStatus, GitHub, Notification, NotificationSubject, PrRef, PrState, RepoRef, Result,
    Timestamp, ViewedState,
};

pub(crate) const REST_URL: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
/// A page of 100 files with full patches can pass ureq's default 10 MB cap.
const MAX_BODY_BYTES: u64 = 128 * 1024 * 1024;

const FILES_PER_PAGE: usize = 100;
const COMMITS_PER_PAGE: usize = 100;
/// The most commits [`GitHub::pull_request_commits`] lists of a pull
/// request; a list this long may have been cut.
pub const PR_COMMITS_LIMIT: usize = 3000;
/// Compare pages read at the same time.
const PARALLEL_PAGES: usize = 4;
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

const FILE_VIEWED_STATES_QUERY: &str = "query PullRequestFileViewedStates($owner: String!, $name: String!, $number: Int!, $cursor: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { files(first: 100, after: $cursor) { pageInfo { hasNextPage endCursor } nodes { path viewerViewedState additions deletions } } } } }";

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
    /// viewed state merged in from GraphQL. The two lists are read side by
    /// side.
    ///
    /// When a pull request's diff passes GitHub's size limit, REST leaves the
    /// patch out of some files and counts them as +0 −0. Their line counts
    /// come from GraphQL, which reports them all; the patch itself is
    /// [`GitHub::omitted_patch`].
    pub fn pull_request_files(&self, pr: &PrRef) -> Result<Vec<ChangedFile>> {
        let (files, facts) = std::thread::scope(|scope| {
            let facts = scope.spawn(|| self.file_facts(pr));
            let files = self.rest_files(pr);
            (files, joined(facts))
        });
        let mut files = files?;
        merge_file_facts(&mut files, &facts?);
        Ok(files)
    }

    fn rest_files(&self, pr: &PrRef) -> Result<Vec<ChangedFile>> {
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
        Ok(files)
    }

    /// The `total` commits between `base_oid` and `head_oid` (their merge
    /// base, as a pull request counts them), oldest first, up to
    /// [`PR_COMMITS_LIMIT`]. The pages are numbered, so they are read
    /// [`PARALLEL_PAGES`] at a time.
    pub(crate) fn compared_commits(
        &self,
        repo: &RepoRef,
        base_oid: &str,
        head_oid: &str,
        total: u32,
    ) -> Result<Vec<Commit>> {
        for oid in [base_oid, head_oid] {
            if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Error::Decode(format!("invalid commit sha {oid:?}")));
            }
        }
        let pages = compare_pages(total);
        let mut commits = Vec::new();
        for batch in pages.chunks(PARALLEL_PAGES) {
            let bodies: Vec<Result<String>> = std::thread::scope(|scope| {
                let reads: Vec<_> = batch
                    .iter()
                    .map(|page| {
                        let path = format!(
                            "/repos/{}/{}/compare/{base_oid}...{head_oid}?per_page={COMMITS_PER_PAGE}&page={page}",
                            repo.owner, repo.name
                        );
                        scope.spawn(move || self.rest(RestMethod::Get, &path))
                    })
                    .collect();
                reads.into_iter().map(joined).collect()
            });
            for body in bodies {
                commits.extend(parse_compare_commits(&body?)?);
            }
        }
        commits.truncate(PR_COMMITS_LIMIT);
        Ok(commits)
    }

    /// Sets each file's viewed state from GraphQL, and the line counts of a
    /// file REST sent without a patch, without downloading the patches
    /// again: for a file list kept from an earlier load of the same head
    /// commit.
    pub fn refresh_viewed_states(&self, pr: &PrRef, files: &mut [ChangedFile]) -> Result<()> {
        let facts = self.file_facts(pr)?;
        merge_file_facts(files, &facts);
        Ok(())
    }

    /// The files commit `sha` changes against its first parent, with their
    /// patches (REST, all pages). As for a pull request, GitHub leaves the
    /// patch out of a file too large to show but keeps its line counts;
    /// [`GitHub::omitted_patch`] with the parent as the base loads it. Every
    /// file starts `Unviewed`: see [`GitHub::merge_viewed_states`].
    pub fn commit_files(&self, repo: &RepoRef, sha: &str) -> Result<CommitFiles> {
        if sha.is_empty() || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Decode(format!("invalid commit sha {sha:?}")));
        }
        let mut commit = CommitFiles {
            parent_oid: None,
            files: Vec::new(),
        };
        for page in 1..=MAX_FILE_PAGES {
            let path = format!(
                "/repos/{}/{}/commits/{sha}?per_page={FILES_PER_PAGE}&page={page}",
                repo.owner, repo.name
            );
            let body = self.rest(RestMethod::Get, &path)?;
            let (parent, batch) = parse_commit_page(&body)?;
            commit.parent_oid = commit.parent_oid.or(parent);
            let full_page = batch.len() == FILES_PER_PAGE;
            commit.files.extend(batch);
            if !full_page {
                break;
            }
        }
        Ok(commit)
    }

    /// Sets each file's viewed state in pull request `pr` from GraphQL and
    /// leaves its line counts alone: for files that are not the pull
    /// request's own list, such as one commit's.
    pub fn merge_viewed_states(&self, pr: &PrRef, files: &mut [ChangedFile]) -> Result<()> {
        let facts = self.file_facts(pr)?;
        for file in files {
            file.viewed = facts
                .get(&file.path)
                .map_or(ViewedState::Unviewed, |facts| facts.viewed);
        }
        Ok(())
    }

    /// The hunks GitHub left out of `file` because the pull request's diff
    /// is too large: both versions are read, the base at the merge base of
    /// `base_oid` and `head_oid` as GitHub's own diff does, and diffed here.
    /// `None` when either version is binary.
    pub fn omitted_patch(
        &self,
        pr: &PrRef,
        base_oid: &str,
        head_oid: &str,
        file: &ChangedFile,
    ) -> Result<Option<String>> {
        let merge_base = self.merge_base(&pr.repo, base_oid, head_oid)?;
        self.patch_since(&pr.repo, &merge_base, head_oid, file)
    }

    /// The commit GitHub diffs `head_oid` against: the merge base of
    /// `base_oid` and `head_oid`. One call, so loading many left-out diffs
    /// asks for it once.
    pub fn merge_base(&self, repo: &RepoRef, base_oid: &str, head_oid: &str) -> Result<String> {
        let compare = self.rest(
            RestMethod::Get,
            &format!(
                "/repos/{}/{}/compare/{}...{}?per_page=1",
                repo.owner,
                repo.name,
                query_value(base_oid),
                query_value(head_oid)
            ),
        )?;
        parse_merge_base(&compare)
    }

    /// The diff of `file` from `merge_base` (see [`GitHub::merge_base`]) to
    /// `head_oid`, read from the two versions of the file: one call for an
    /// added or removed file, two otherwise. `None` when either version is
    /// binary.
    pub fn patch_since(
        &self,
        repo: &RepoRef,
        merge_base: &str,
        head_oid: &str,
        file: &ChangedFile,
    ) -> Result<Option<String>> {
        let old_path = match file.status {
            FileStatus::Renamed | FileStatus::Copied => {
                file.previous_path.as_deref().unwrap_or(&file.path)
            }
            _ => &file.path,
        };
        let old = match file.status {
            FileStatus::Added => Vec::new(),
            _ => self.raw_contents(repo, old_path, merge_base)?,
        };
        let new = match file.status {
            FileStatus::Removed => Vec::new(),
            _ => self.raw_contents(repo, &file.path, head_oid)?,
        };
        Ok(crate::diff::text_patch(&old, &new))
    }

    /// The bytes of the file at `path` at commit `sha`.
    pub fn raw_contents(&self, repo: &RepoRef, path: &str, sha: &str) -> Result<Vec<u8>> {
        let path = format!(
            "/repos/{}/{}/contents/{}?ref={}",
            repo.owner,
            repo.name,
            path.split('/')
                .map(query_value)
                .collect::<Vec<_>>()
                .join("/"),
            query_value(sha)
        );
        self.rest_bytes(RestMethod::Get, &path, "application/vnd.github.raw+json")
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

    fn file_facts(&self, pr: &PrRef) -> Result<HashMap<String, FileFacts>> {
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
                    .map(|node| (node.path.clone(), FileFacts::from(node))),
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

    /// Sends one JSON REST call and returns the status with the body, for
    /// endpoints whose non-2xx answers carry a result: only 401 is an error
    /// here.
    pub(crate) fn rest_with_status(
        &self,
        method: RestMethod<'_>,
        path: &str,
    ) -> Result<(u16, Vec<u8>)> {
        if std::env::var_os("LOUPE_OFFLINE").is_some() {
            return Err(Error::Transport("Offline mode".into()));
        }
        let response = self.send_request(method, path, "application/vnd.github+json")?;
        self.read_response(response)
    }

    fn send_rest(&self, method: RestMethod<'_>, path: &str, accept: &str) -> Result<Vec<u8>> {
        let response = self.send_request(method, path, accept)?;
        self.read_success(response)
    }

    fn send_request(
        &self,
        method: RestMethod<'_>,
        path: &str,
        accept: &str,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let url = format!("{REST_URL}{path}");
        let started = std::time::Instant::now();
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
        self.trace(
            &format!("rest:{}", crate::client::path_shape(path)),
            response.status().as_u16(),
            started,
            response.headers(),
        );
        Ok(response)
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
        let started = std::time::Instant::now();
        let response = request
            .call()
            .map_err(|error| Error::Transport(error.to_string()))?;
        self.trace(
            &format!("rest:{}", crate::client::path_shape(path)),
            response.status().as_u16(),
            started,
            response.headers(),
        );
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
    fn read_success(&self, response: ureq::http::Response<ureq::Body>) -> Result<Vec<u8>> {
        let (status, body) = self.read_response(response)?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &body));
        }
        Ok(body)
    }

    /// Records the rate limit and reads the status and body. A 401 and a
    /// refusal for a rate limit are the only statuses it turns into an error.
    fn read_response(
        &self,
        mut response: ureq::http::Response<ureq::Body>,
    ) -> Result<(u16, Vec<u8>)> {
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
        if let Some(refusal) = crate::rate::refusal(status, response.headers(), &message_of(&body))
        {
            return Err(refusal);
        }
        Ok((status, body))
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

    /// Whether pull request `pr` changed since `etag` was taken: its title,
    /// state, labels, assignees, comments, or the time it was last updated
    /// (a push, a review, or a comment moves it). It asks for the pull
    /// request's issue: `pulls/{n}` carries the repositories, whose
    /// counters move with any activity there. A 304 costs no rate limit.
    pub fn poll_pull_request(&self, pr: &PrRef, etag: Option<&str>) -> Result<PullRequestPoll> {
        let path = format!(
            "/repos/{}/{}/issues/{}",
            pr.repo.owner, pr.repo.name, pr.number
        );
        Ok(match self.conditional_get(&path, etag)? {
            Conditional::NotModified => PullRequestPoll::Unchanged,
            Conditional::Modified(etag) => PullRequestPoll::Changed(etag),
        })
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

/// The answer of [`GitHub::poll_pull_request`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullRequestPoll {
    /// GitHub answered 304.
    Unchanged,
    /// The pull request changed, or there was no ETag to compare with; the
    /// ETag to send next time.
    Changed(Option<String>),
}

/// The `message` GitHub put in an error body, or an empty text.
fn message_of(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value["message"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The error a non-2xx REST response stands for, with GitHub's `message`.
pub(crate) fn http_error(status: u16, body: &[u8]) -> Error {
    Error::Http {
        status,
        message: message_of(body),
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

/// The result of a scoped thread, its panic passed on.
fn joined<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The compare pages that hold `total` commits, up to [`PR_COMMITS_LIMIT`].
fn compare_pages(total: u32) -> Vec<usize> {
    let commits = (total as usize).clamp(1, PR_COMMITS_LIMIT);
    (1..=commits.div_ceil(COMMITS_PER_PAGE)).collect()
}

#[derive(Deserialize)]
struct WireComparison {
    commits: Vec<WireRestCommit>,
}

#[derive(Deserialize)]
struct WireRestCommit {
    sha: String,
    commit: WireRestCommitData,
    author: Option<WireRestUser>,
}

#[derive(Deserialize)]
struct WireRestCommitData {
    message: String,
    author: Option<WireGitPerson>,
    committer: Option<WireGitPerson>,
}

#[derive(Deserialize)]
struct WireGitPerson {
    name: Option<String>,
    date: Option<Timestamp>,
}

#[derive(Deserialize)]
struct WireRestUser {
    login: String,
    avatar_url: String,
}

/// A compare page's commits as GraphQL would give them: the message's first
/// line as the headline and the rest, past the blank line, as the body.
fn parse_compare_commits(body: &str) -> Result<Vec<Commit>> {
    let comparison: WireComparison =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(comparison
        .commits
        .into_iter()
        .map(|wire| {
            let (headline, rest) = wire
                .commit
                .message
                .split_once('\n')
                .unwrap_or((wire.commit.message.as_str(), ""));
            Commit {
                headline: headline.trim_end().to_owned(),
                body: rest.trim().to_owned(),
                author_name: wire
                    .commit
                    .author
                    .as_ref()
                    .and_then(|person| person.name.clone())
                    .unwrap_or_default(),
                author: wire.author.map(|user| Actor {
                    login: user.login,
                    avatar_url: user.avatar_url,
                }),
                committed_at: wire
                    .commit
                    .committer
                    .as_ref()
                    .or(wire.commit.author.as_ref())
                    .and_then(|person| person.date)
                    .unwrap_or_default(),
                checks: None,
                oid: wire.sha,
            }
        })
        .collect())
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
    Ok(wire.into_iter().map(changed_file).collect())
}

#[derive(Deserialize)]
struct WireCommitPage {
    #[serde(default)]
    parents: Vec<WireParent>,
    #[serde(default)]
    files: Vec<WireFile>,
}

#[derive(Deserialize)]
struct WireParent {
    sha: String,
}

/// Parses one page of `GET /commits/{sha}`: the first parent and the
/// page's files, every one `Unviewed`.
fn parse_commit_page(body: &str) -> Result<(Option<String>, Vec<ChangedFile>)> {
    let wire: WireCommitPage =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    let parent = wire.parents.into_iter().next().map(|parent| parent.sha);
    Ok((parent, wire.files.into_iter().map(changed_file).collect()))
}

fn changed_file(file: WireFile) -> ChangedFile {
    ChangedFile {
        path: file.filename,
        previous_path: file.previous_filename,
        status: file_status(&file.status),
        additions: file.additions,
        deletions: file.deletions,
        patch: file.patch,
        viewed: ViewedState::Unviewed,
    }
}

/// What GraphQL knows of a changed file that REST may not tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileFacts {
    viewed: ViewedState,
    additions: u32,
    deletions: u32,
}

impl From<ViewedStateNode> for FileFacts {
    fn from(node: ViewedStateNode) -> Self {
        FileFacts {
            viewed: node.viewer_viewed_state.into(),
            additions: node.additions,
            deletions: node.deletions,
        }
    }
}

fn merge_file_facts(files: &mut [ChangedFile], facts: &HashMap<String, FileFacts>) {
    for file in files {
        let found = facts.get(&file.path);
        file.viewed = found.map_or(ViewedState::Unviewed, |facts| facts.viewed);
        if file.patch.is_none()
            && let Some(facts) = found
        {
            file.additions = facts.additions;
            file.deletions = facts.deletions;
        }
    }
}

#[derive(Deserialize)]
struct WireCompare {
    merge_base_commit: WireCommitSha,
}

#[derive(Deserialize)]
struct WireCommitSha {
    sha: String,
}

fn parse_merge_base(body: &str) -> Result<String> {
    let compare: WireCompare =
        serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(compare.merge_base_commit.sha)
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
    #[serde(default)]
    additions: u32,
    #[serde(default)]
    deletions: u32,
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
    fn a_commit_page_has_its_first_parent_and_its_files() {
        let body = r#"{"sha":"c0ffee","parents":[{"sha":"aaa1"},{"sha":"bbb2"}],
            "files":[
                {"filename":"Cargo.lock","status":"added","additions":9557,"deletions":0},
                {"filename":"src/lib.rs","status":"modified","additions":1,"deletions":1,
                 "patch":"@@ -1 +1 @@\n-a\n+b"}
            ]}"#;
        let (parent, files) = parse_commit_page(body).expect("fixture parses");
        assert_eq!(parent.as_deref(), Some("aaa1"), "the first parent");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].patch, None, "a patch GitHub left out stays out");
        assert_eq!(
            (files[0].additions, files[0].deletions),
            (9557, 0),
            "with its line counts"
        );
        assert_eq!(files[1].patch.as_deref(), Some("@@ -1 +1 @@\n-a\n+b"));
        assert!(files.iter().all(|f| f.viewed == ViewedState::Unviewed));

        let (parent, files) =
            parse_commit_page(r#"{"sha":"root","parents":[]}"#).expect("a root commit parses");
        assert_eq!(parent, None);
        assert!(files.is_empty());
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
                    {"path":"README.md","viewerViewedState":"VIEWED","additions":1,"deletions":1},
                    null,
                    {"path":"src/new_name.rs","viewerViewedState":"DISMISSED","additions":3,"deletions":1},
                    {"path":"elsewhere.rs","viewerViewedState":"UNVIEWED","additions":0,"deletions":0}
                ]}}}
        }"#;
        let data: ViewedStatesData = serde_json::from_str(page).expect("fixture parses");
        let files_page = data
            .repository
            .and_then(|r| r.pull_request)
            .expect("pull request present")
            .files;
        assert!(!files_page.page_info.has_next_page);
        let states: HashMap<String, FileFacts> = files_page
            .nodes
            .into_iter()
            .flatten()
            .flatten()
            .map(|node| (node.path.clone(), FileFacts::from(node)))
            .collect();

        merge_file_facts(&mut files, &states);

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
    fn a_file_sent_without_a_patch_takes_its_line_counts_from_graphql() {
        let mut files = parse_files_page(
            r#"[
            {"filename":"src/big.ts","status":"modified","additions":0,"deletions":0,"changes":0},
            {"filename":"src/small.ts","status":"modified","additions":2,"deletions":1,"changes":3,
             "patch":"@@ -1 +1,2 @@\n-a\n+b\n+c"}
        ]"#,
        )
        .expect("fixture parses");
        let facts = |additions, deletions| FileFacts {
            viewed: ViewedState::Unviewed,
            additions,
            deletions,
        };
        let graphql = HashMap::from([
            ("src/big.ts".to_owned(), facts(120, 40)),
            ("src/small.ts".to_owned(), facts(9, 9)),
        ]);

        merge_file_facts(&mut files, &graphql);

        let counts: Vec<_> = files
            .iter()
            .map(|f| (f.path.as_str(), f.additions, f.deletions))
            .collect();
        assert_eq!(counts, [("src/big.ts", 120, 40), ("src/small.ts", 2, 1)]);
    }

    #[test]
    fn reads_the_merge_base_of_a_comparison() {
        let body = r#"{"merge_base_commit":{"sha":"abc123","commit":{}},"files":[]}"#;
        assert_eq!(parse_merge_base(body).expect("parses"), "abc123");
        assert!(parse_merge_base("{}").is_err());
    }

    #[test]
    fn a_comparison_page_reads_its_commits_like_graphql() {
        let body = r#"{"total_commits":769,"files":[{"filename":"a.rs"}],"commits":[
            {"sha":"c1","author":{"login":"eve","avatar_url":"https://avatars.example/eve"},
             "commit":{"message":"Fix the tab\n\nIt was cut.\r\nTwice.",
                       "author":{"name":"Eve","date":"2026-01-02T03:00:00Z"},
                       "committer":{"name":"GitHub","date":"2026-01-02T04:00:00Z"}}},
            {"sha":"c2","author":null,
             "commit":{"message":"One line","author":{"name":"Frank","date":"2026-01-03T03:00:00Z"},
                       "committer":null}}
        ]}"#;
        let commits = parse_compare_commits(body).expect("parses");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid, "c1");
        assert_eq!(commits[0].headline, "Fix the tab");
        assert_eq!(commits[0].body, "It was cut.\r\nTwice.");
        assert_eq!(commits[0].author_name, "Eve");
        assert_eq!(commits[0].author.as_ref().unwrap().login, "eve");
        assert_eq!(
            commits[0].committed_at,
            Utc.with_ymd_and_hms(2026, 1, 2, 4, 0, 0).unwrap(),
            "the committer's date, as GraphQL's committedDate"
        );
        assert_eq!(commits[0].checks, None, "REST has no checks per commit");
        assert_eq!(commits[1].headline, "One line");
        assert_eq!(commits[1].body, "");
        assert_eq!(commits[1].author, None);
        assert_eq!(
            commits[1].committed_at,
            Utc.with_ymd_and_hms(2026, 1, 3, 3, 0, 0).unwrap(),
            "without a committer, the author's date"
        );
        assert!(parse_compare_commits(r#"{"commits":[{"sha":"c1"}]}"#).is_err());
    }

    #[test]
    fn comparison_pages_cover_the_total_up_to_the_limit() {
        assert_eq!(compare_pages(769), (1..=8).collect::<Vec<_>>());
        assert_eq!(compare_pages(800), (1..=8).collect::<Vec<_>>());
        assert_eq!(compare_pages(251), vec![1, 2, 3]);
        assert_eq!(compare_pages(0), vec![1]);
        assert_eq!(
            compare_pages(u32::MAX).len(),
            PR_COMMITS_LIMIT / COMMITS_PER_PAGE
        );
    }

    #[test]
    fn comparing_rejects_a_sha_that_would_change_the_path() {
        let github = GitHub::new("token");
        let repo = RepoRef::new("acme", "web");
        for (base, head) in [("abc", "../x"), ("", "abc"), ("abc?page=9", "def")] {
            assert!(matches!(
                github.compared_commits(&repo, base, head, 300),
                Err(Error::Decode(_))
            ));
        }
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
