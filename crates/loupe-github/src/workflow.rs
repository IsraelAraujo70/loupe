//! GitHub Actions workflow runs through REST: the runs of a repository or a
//! branch, one run with its jobs, steps, artifacts, and annotations, a job's
//! log split by step, and the run's actions (re-run, cancel, download).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::actions::{split_timestamp, strip_ansi};
use crate::rest::{REST_URL, RestMethod, query_value};
use crate::workflow_file::{WorkflowSpec, parse_workflow};
use crate::{Actor, CheckStatus, Error, GitHub, RepoRef, Result, Timestamp};

/// How many runs [`GitHub::workflow_runs`] reads: one page.
pub const RUNS_PAGE: usize = 50;
const JOBS_PER_PAGE: usize = 100;
/// A run of a matrix can have up to 256 jobs per job key.
const MAX_JOB_PAGES: usize = 5;

/// One run of a workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    /// The run's number in its workflow, the `#57` GitHub shows.
    pub number: u64,
    pub attempt: u32,
    /// The workflow's name, such as `Release`.
    pub workflow: String,
    /// What GitHub titles the run: the commit message or the pull request's
    /// title.
    pub title: String,
    /// The workflow file, such as `.github/workflows/release.yml`.
    pub path: String,
    /// What triggered the run: `push`, `pull_request`, `schedule`…
    pub event: String,
    pub branch: Option<String>,
    pub head_sha: String,
    pub status: CheckStatus,
    /// Who triggered the run (a re-run's is whoever re-ran it).
    pub actor: Option<Actor>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub updated_at: Timestamp,
    pub url: String,
    /// The check suite's GraphQL id, which holds the annotations.
    pub check_suite: Option<String>,
    /// Numbers of the pull requests the run belongs to.
    pub pull_requests: Vec<u64>,
}

impl WorkflowRun {
    /// Whether the run has ended, or waits on a person (an approval) rather
    /// than on GitHub; a run that has not is worth polling.
    pub fn is_finished(&self) -> bool {
        !matches!(self.status, CheckStatus::Queued | CheckStatus::Running)
    }

    /// The run's total time in seconds, as GitHub counts it: from its start
    /// to its last update, or to `now` while it goes on.
    pub fn duration_seconds(&self, now: DateTime<Utc>) -> i64 {
        let start = self.started_at.unwrap_or(self.created_at);
        let end = if self.is_finished() {
            self.updated_at
        } else {
            now
        };
        (end - start).num_seconds().max(0)
    }
}

/// One job of a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJob {
    pub id: u64,
    pub name: String,
    pub status: CheckStatus,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub url: Option<String>,
    pub runner: Option<String>,
    pub steps: Vec<JobStep>,
}

impl WorkflowJob {
    /// Seconds the job ran, or ran so far at `now`; `None` before it starts.
    pub fn duration_seconds(&self, now: DateTime<Utc>) -> Option<i64> {
        let start = self.started_at?;
        let end = match self.status {
            CheckStatus::Running => now,
            _ => self.completed_at?,
        };
        Some((end - start).num_seconds().max(0))
    }
}

/// One step of a job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStep {
    pub number: u32,
    pub name: String,
    pub status: CheckStatus,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
}

/// A file a run uploaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunArtifact {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    /// Past its retention: GitHub no longer serves it.
    pub expired: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnnotationLevel {
    Failure,
    Warning,
    Notice,
}

/// A failure, warning, or notice a job left on a line of a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    /// The job's name.
    pub job: String,
    pub job_id: Option<u64>,
    pub level: AnnotationLevel,
    pub title: Option<String>,
    pub message: String,
    pub path: String,
    pub line: Option<u32>,
    /// The file on GitHub at the run's commit, at the line.
    pub url: String,
}

/// How a log line reads: GitHub's workflow commands become kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LogLineKind {
    Plain,
    /// The title of a folded group (`##[group]`).
    Group,
    /// A command the runner ran (`[command]`).
    Command,
    Error,
    Warning,
    Notice,
    Debug,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub kind: LogLineKind,
    /// Without its timestamp, its workflow command, or color codes.
    pub text: String,
}

/// A job's log, split by step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLog {
    /// `(step number, lines)`, in step order; a step with no output has no
    /// entry.
    pub steps: Vec<(u32, Vec<LogLine>)>,
}

impl JobLog {
    pub fn lines_of(&self, step: u32) -> &[LogLine] {
        self.steps
            .iter()
            .find(|(number, _)| *number == step)
            .map_or(&[], |(_, lines)| lines.as_slice())
    }
}

impl GitHub {
    /// The newest runs of `repo`, of `branch` when given.
    pub fn workflow_runs(&self, repo: &RepoRef, branch: Option<&str>) -> Result<Vec<WorkflowRun>> {
        let mut path = format!(
            "/repos/{}/{}/actions/runs?per_page={RUNS_PAGE}",
            repo.owner, repo.name
        );
        if let Some(branch) = branch {
            path.push_str(&format!("&branch={}", query_value(branch)));
        }
        let page: WireRuns = decode(&self.rest(RestMethod::Get, &path)?)?;
        Ok(page
            .workflow_runs
            .into_iter()
            .map(WireRun::into_run)
            .collect())
    }

    pub fn workflow_run(&self, repo: &RepoRef, run_id: u64) -> Result<WorkflowRun> {
        let path = format!("/repos/{}/{}/actions/runs/{run_id}", repo.owner, repo.name);
        let run: WireRun = decode(&self.rest(RestMethod::Get, &path)?)?;
        Ok(run.into_run())
    }

    /// The jobs of the run's latest attempt, in the order GitHub lists them.
    pub fn run_jobs(&self, repo: &RepoRef, run_id: u64) -> Result<Vec<WorkflowJob>> {
        let mut jobs = Vec::new();
        for page in 1..=MAX_JOB_PAGES {
            let path = format!(
                "/repos/{}/{}/actions/runs/{run_id}/jobs?filter=latest&per_page={JOBS_PER_PAGE}&page={page}",
                repo.owner, repo.name
            );
            let batch: WireJobs = decode(&self.rest(RestMethod::Get, &path)?)?;
            let full = batch.jobs.len() == JOBS_PER_PAGE;
            jobs.extend(batch.jobs.into_iter().map(WireJob::into_job));
            if !full {
                break;
            }
        }
        Ok(jobs)
    }

    pub fn run_artifacts(&self, repo: &RepoRef, run_id: u64) -> Result<Vec<RunArtifact>> {
        let path = format!(
            "/repos/{}/{}/actions/runs/{run_id}/artifacts?per_page=100",
            repo.owner, repo.name
        );
        let page: WireArtifacts = decode(&self.rest(RestMethod::Get, &path)?)?;
        Ok(page
            .artifacts
            .into_iter()
            .map(|artifact| RunArtifact {
                id: artifact.id,
                name: artifact.name,
                size_bytes: artifact.size_in_bytes,
                expired: artifact.expired,
            })
            .collect())
    }

    /// Writes the zip GitHub serves for an artifact into `out` as it
    /// arrives, and returns its size. An artifact can be hundreds of
    /// megabytes, so this request has no overall time limit and nothing is
    /// held in memory.
    pub fn download_artifact(
        &self,
        repo: &RepoRef,
        artifact_id: u64,
        out: &mut dyn std::io::Write,
    ) -> Result<u64> {
        if std::env::var_os("LOUPE_OFFLINE").is_some() {
            return Err(Error::Transport("Offline mode".into()));
        }
        let url = format!(
            "{REST_URL}/repos/{}/{}/actions/artifacts/{artifact_id}/zip",
            repo.owner, repo.name
        );
        // GitHub answers with a redirect to a short-lived download URL; ureq
        // follows it and does not forward the token to the other host
        // (`redirect_auth_headers` is `Never` by default).
        let mut response = self
            .authorize(self.agent.get(&url), "application/vnd.github+json")
            .config()
            .timeout_global(None)
            .build()
            .call()
            .map_err(|error| Error::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        if status == 401 {
            return Err(Error::Unauthorized);
        }
        if !(200..300).contains(&status) {
            let message = response
                .body_mut()
                .read_to_string()
                .ok()
                .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                .and_then(|body| body["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(Error::Http { status, message });
        }
        let mut body = response.into_body().into_reader();
        std::io::copy(&mut body, out).map_err(|error| Error::Transport(error.to_string()))
    }

    /// The annotations of every job of the check suite `check_suite` (a
    /// run's [`WorkflowRun::check_suite`]).
    pub fn run_annotations(&self, check_suite: &str) -> Result<Vec<Annotation>> {
        let data: AnnotationsData =
            self.graphql(ANNOTATIONS_QUERY, json!({ "id": check_suite }))?;
        Ok(annotations_of(data))
    }

    /// The log of a finished job, split by its steps. GitHub publishes a
    /// job's log only once the job has finished: before that this fails
    /// with a 404.
    pub fn job_log(&self, repo: &RepoRef, job: &WorkflowJob) -> Result<JobLog> {
        let path = format!(
            "/repos/{}/{}/actions/jobs/{}/logs",
            repo.owner, repo.name, job.id
        );
        let log = self.rest(RestMethod::Get, &path)?;
        Ok(split_log(&job.steps, &log))
    }

    /// The jobs of the workflow file at `path` at commit `sha`, with what each
    /// one needs: what the run's graph is drawn from.
    pub fn workflow_spec(&self, repo: &RepoRef, path: &str, sha: &str) -> Result<WorkflowSpec> {
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
        let raw = self.rest_bytes(RestMethod::Get, &path, "application/vnd.github.raw+json")?;
        let text = String::from_utf8(raw).map_err(|error| Error::Decode(error.to_string()))?;
        Ok(parse_workflow(&text))
    }

    /// Re-runs every job of the run.
    pub fn rerun_run(&self, repo: &RepoRef, run_id: u64) -> Result<()> {
        let path = format!(
            "/repos/{}/{}/actions/runs/{run_id}/rerun",
            repo.owner, repo.name
        );
        self.rest(RestMethod::Post, &path).map(drop)
    }

    pub fn cancel_run(&self, repo: &RepoRef, run_id: u64) -> Result<()> {
        let path = format!(
            "/repos/{}/{}/actions/runs/{run_id}/cancel",
            repo.owner, repo.name
        );
        self.rest(RestMethod::Post, &path).map(drop)
    }
}

fn decode<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(|error| Error::Decode(error.to_string()))
}

/// The status of a run, a job, or a step, from GitHub's `status` and
/// `conclusion`.
fn status_of(status: &str, conclusion: Option<&str>) -> CheckStatus {
    match (status, conclusion) {
        ("completed", Some("success")) => CheckStatus::Passed,
        ("completed", Some("failure" | "timed_out" | "startup_failure")) => CheckStatus::Failed,
        ("completed", Some("cancelled")) => CheckStatus::Cancelled,
        ("completed", Some("skipped")) => CheckStatus::Skipped,
        ("completed", Some("action_required")) => CheckStatus::ActionRequired,
        ("completed", _) => CheckStatus::Neutral,
        ("in_progress", _) => CheckStatus::Running,
        ("action_required", _) => CheckStatus::ActionRequired,
        _ => CheckStatus::Queued,
    }
}

/// What the runner writes first in a step: a `run` or `uses` step's
/// command, a post step's cleanup, the job's last step. "Set up job" has
/// `##[group]` titles of its own, so a group alone does not start a step.
const STEP_STARTS: [&str; 3] = [
    "##[group]Run ",
    "Post job cleanup.",
    "Cleaning up orphan processes",
];

/// Splits a job's log by its steps. Lines go to the step running when they
/// were written (a line without a time stays with the line before it; lines
/// before the first step go to the first one). Step times have whole
/// seconds and log lines fractions, so one second can hold several steps:
/// the log moves to the next step once the current one has ended (the
/// second after its end), or when a line that starts a step appears and the
/// next step has started, one step per such line.
pub fn split_log(steps: &[JobStep], log: &str) -> JobLog {
    // A skipped step can carry the start time of the step after it.
    let mut ran: Vec<&JobStep> = steps
        .iter()
        .filter(|step| step.status != CheckStatus::Skipped && step.started_at.is_some())
        .collect();
    ran.sort_by_key(|step| (step.started_at, step.number));
    let started = |ix: usize, at: DateTime<Utc>| {
        ran.get(ix)
            .and_then(|step| step.started_at)
            .is_some_and(|start| start <= at)
    };
    let mut out: Vec<(u32, Vec<LogLine>)> = Vec::new();
    let mut current = 0usize;
    for raw in log.lines() {
        let (at, text) = split_timestamp(raw.trim_start_matches('\u{feff}'));
        let text = strip_ansi(text);
        if let Some(at) = at {
            let before = current;
            while started(current + 1, at)
                && ran[current]
                    .completed_at
                    .is_some_and(|end| at >= end + chrono::Duration::seconds(1))
            {
                current += 1;
            }
            let starts_step = STEP_STARTS.iter().any(|start| text.starts_with(start));
            // A composite action writes a `Run` title for each step inside
            // it: those move nothing while the step is still going.
            let current_ended = ran[current].completed_at.is_some_and(|end| end <= at);
            if current == before && starts_step && current_ended && started(current + 1, at) {
                current += 1;
            }
        }
        let Some(line) = log_line(&text) else {
            continue;
        };
        let current = ran.get(current).map_or(1, |step| step.number);
        match out.last_mut() {
            Some((number, lines)) if *number == current => lines.push(line),
            _ => match out.iter_mut().find(|(number, _)| *number == current) {
                Some((_, lines)) => lines.push(line),
                None => out.push((current, vec![line])),
            },
        }
    }
    out.sort_by_key(|(number, _)| *number);
    JobLog { steps: out }
}

/// One log line's kind and text; `None` for the end of a group, which shows
/// nothing.
fn log_line(text: &str) -> Option<LogLine> {
    const COMMANDS: [(&str, LogLineKind); 6] = [
        ("##[group]", LogLineKind::Group),
        ("##[error]", LogLineKind::Error),
        ("##[warning]", LogLineKind::Warning),
        ("##[notice]", LogLineKind::Notice),
        ("##[debug]", LogLineKind::Debug),
        ("[command]", LogLineKind::Command),
    ];
    if text.starts_with("##[endgroup]") {
        return None;
    }
    let (kind, text) = COMMANDS
        .iter()
        .find_map(|(prefix, kind)| text.strip_prefix(prefix).map(|rest| (*kind, rest)))
        .unwrap_or((LogLineKind::Plain, text));
    Some(LogLine {
        kind,
        text: text.to_owned(),
    })
}

#[derive(Deserialize)]
struct WireRuns {
    workflow_runs: Vec<WireRun>,
}

#[derive(Deserialize)]
struct WireActor {
    login: String,
    avatar_url: String,
}

#[derive(Deserialize)]
struct WirePull {
    number: u64,
}

#[derive(Deserialize)]
struct WireRun {
    id: u64,
    run_number: u64,
    #[serde(default)]
    run_attempt: Option<u32>,
    name: Option<String>,
    #[serde(default)]
    display_title: Option<String>,
    #[serde(default)]
    path: String,
    event: String,
    head_branch: Option<String>,
    head_sha: String,
    status: Option<String>,
    conclusion: Option<String>,
    #[serde(default)]
    triggering_actor: Option<WireActor>,
    #[serde(default)]
    actor: Option<WireActor>,
    created_at: Timestamp,
    #[serde(default)]
    run_started_at: Option<Timestamp>,
    updated_at: Timestamp,
    html_url: String,
    #[serde(default)]
    check_suite_node_id: Option<String>,
    #[serde(default)]
    pull_requests: Vec<WirePull>,
}

impl WireRun {
    fn into_run(self) -> WorkflowRun {
        let actor = self.triggering_actor.or(self.actor).map(|actor| Actor {
            login: actor.login,
            avatar_url: actor.avatar_url,
        });
        // A run of a reusable workflow's caller names the file with `@ref`.
        let path = self
            .path
            .split_once('@')
            .map_or(self.path.as_str(), |(path, _)| path)
            .to_owned();
        WorkflowRun {
            id: self.id,
            number: self.run_number,
            attempt: self.run_attempt.unwrap_or(1),
            workflow: self.name.unwrap_or_else(|| path.clone()),
            title: self.display_title.unwrap_or_default(),
            path,
            event: self.event,
            branch: self.head_branch,
            head_sha: self.head_sha,
            status: status_of(
                self.status.as_deref().unwrap_or("queued"),
                self.conclusion.as_deref(),
            ),
            actor,
            created_at: self.created_at,
            started_at: self.run_started_at,
            updated_at: self.updated_at,
            url: self.html_url,
            check_suite: self.check_suite_node_id,
            pull_requests: self.pull_requests.into_iter().map(|pr| pr.number).collect(),
        }
    }
}

#[derive(Deserialize)]
struct WireJobs {
    jobs: Vec<WireJob>,
}

#[derive(Deserialize)]
struct WireJob {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
    html_url: Option<String>,
    runner_name: Option<String>,
    #[serde(default)]
    steps: Vec<WireStep>,
}

#[derive(Deserialize)]
struct WireStep {
    number: u32,
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
}

impl WireJob {
    fn into_job(self) -> WorkflowJob {
        // A job that never ran (skipped) can report an end before its start.
        let completed_at = match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) if end < start => Some(start),
            (_, end) => end,
        };
        WorkflowJob {
            id: self.id,
            name: self.name,
            status: status_of(&self.status, self.conclusion.as_deref()),
            started_at: self.started_at,
            completed_at,
            url: self.html_url,
            runner: self.runner_name.filter(|name| !name.is_empty()),
            steps: self
                .steps
                .into_iter()
                .map(|step| JobStep {
                    number: step.number,
                    name: step.name,
                    status: status_of(&step.status, step.conclusion.as_deref()),
                    started_at: step.started_at,
                    completed_at: step.completed_at,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct WireArtifacts {
    artifacts: Vec<WireArtifact>,
}

#[derive(Deserialize)]
struct WireArtifact {
    id: u64,
    name: String,
    size_in_bytes: u64,
    #[serde(default)]
    expired: bool,
}

const ANNOTATIONS_QUERY: &str = "query RunAnnotations($id: ID!) { node(id: $id) { ... on CheckSuite { checkRuns(first: 100) { nodes { databaseId name annotations(first: 50) { nodes { annotationLevel title message path blobUrl location { start { line } } } } } } } } }";

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[("run_annotations", ANNOTATIONS_QUERY)];

#[derive(Deserialize)]
struct AnnotationsData {
    node: Option<AnnotationsSuite>,
}

#[derive(Deserialize)]
struct AnnotationsSuite {
    #[serde(rename = "checkRuns")]
    check_runs: Option<Nodes<AnnotatedRun>>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Option<Vec<Option<T>>>,
}

#[derive(Deserialize)]
struct AnnotatedRun {
    #[serde(rename = "databaseId")]
    database_id: Option<u64>,
    name: String,
    annotations: Option<Nodes<WireAnnotation>>,
}

#[derive(Deserialize)]
struct WireAnnotation {
    #[serde(rename = "annotationLevel")]
    level: Option<String>,
    title: Option<String>,
    message: String,
    path: String,
    #[serde(rename = "blobUrl")]
    blob_url: String,
    location: WireSpan,
}

#[derive(Deserialize)]
struct WireSpan {
    start: WirePosition,
}

#[derive(Deserialize)]
struct WirePosition {
    line: u32,
}

fn annotations_of(data: AnnotationsData) -> Vec<Annotation> {
    let runs = data
        .node
        .and_then(|suite| suite.check_runs)
        .and_then(|runs| runs.nodes)
        .unwrap_or_default();
    let mut out = Vec::new();
    for run in runs.into_iter().flatten() {
        let annotations = run
            .annotations
            .and_then(|list| list.nodes)
            .unwrap_or_default();
        for annotation in annotations.into_iter().flatten() {
            let level = match annotation.level.as_deref() {
                Some("FAILURE") => AnnotationLevel::Failure,
                Some("WARNING") => AnnotationLevel::Warning,
                _ => AnnotationLevel::Notice,
            };
            // Annotations from the runner itself (`.github`) point at no line.
            let line =
                (annotation.location.start.line > 0).then_some(annotation.location.start.line);
            let url = match line {
                Some(line) => format!("{}#L{line}", annotation.blob_url),
                None => annotation.blob_url,
            };
            out.push(Annotation {
                job: run.name.clone(),
                job_id: run.database_id,
                level,
                title: annotation.title.filter(|title| !title.is_empty()),
                message: annotation.message,
                path: annotation.path,
                line,
                url,
            });
        }
    }
    out.sort_by_key(|annotation| annotation.level as u8);
    out
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{
        AnnotationLevel, AnnotationsData, JobStep, LogLineKind, WireJobs, WireRun, annotations_of,
        split_log, status_of,
    };
    use crate::CheckStatus;

    fn at(text: &str) -> Option<DateTime<Utc>> {
        Some(text.parse().expect("timestamp"))
    }

    fn step(number: u32, start: &str, end: &str) -> JobStep {
        JobStep {
            number,
            name: format!("step {number}"),
            status: CheckStatus::Passed,
            started_at: at(start),
            completed_at: at(end),
        }
    }

    #[test]
    fn statuses_follow_status_then_conclusion() {
        assert_eq!(status_of("completed", Some("success")), CheckStatus::Passed);
        assert_eq!(
            status_of("completed", Some("timed_out")),
            CheckStatus::Failed
        );
        assert_eq!(
            status_of("completed", Some("skipped")),
            CheckStatus::Skipped
        );
        assert_eq!(
            status_of("completed", Some("cancelled")),
            CheckStatus::Cancelled
        );
        assert_eq!(status_of("in_progress", None), CheckStatus::Running);
        assert_eq!(status_of("waiting", None), CheckStatus::Queued);
        assert_eq!(status_of("queued", None), CheckStatus::Queued);
    }

    #[test]
    fn a_run_decodes_with_its_trigger_and_pull_requests() {
        let run: WireRun = serde_json::from_str(
            r#"{"id":43045919120,"run_number":21,"run_attempt":2,"name":"Release",
            "display_title":"ci: build on Boat","path":".github/workflows/release.yml@refs/pull/52/merge",
            "event":"pull_request","head_branch":"ci/linux-on-boat","head_sha":"be46fc0",
            "status":"in_progress","conclusion":null,
            "actor":{"login":"someone","avatar_url":"a"},
            "triggering_actor":{"login":"IsraelAraujo70","avatar_url":"b"},
            "created_at":"2026-11-11T15:11:52Z","run_started_at":"2026-11-11T15:11:52Z",
            "updated_at":"2026-11-11T15:12:12Z",
            "html_url":"https://github.com/acme/web/actions/runs/43045919120",
            "check_suite_node_id":"CS_1","pull_requests":[{"number":52}]}"#,
        )
        .expect("run");
        let run = run.into_run();
        assert_eq!(run.path, ".github/workflows/release.yml");
        assert_eq!(run.status, CheckStatus::Running);
        assert_eq!(run.actor.as_ref().expect("actor").login, "IsraelAraujo70");
        assert_eq!(run.pull_requests, [52]);
        assert_eq!(run.attempt, 2);
        assert!(!run.is_finished());
        let later = "2026-11-11T15:13:52Z".parse().expect("now");
        assert_eq!(run.duration_seconds(later), 120);
    }

    #[test]
    fn a_finished_run_lasts_until_its_last_update() {
        let run: WireRun = serde_json::from_str(
            r#"{"id":1,"run_number":1,"name":"CI","event":"push","head_branch":"main",
            "head_sha":"a","status":"completed","conclusion":"failure",
            "created_at":"2026-11-11T15:00:00Z","run_started_at":"2026-11-11T15:00:10Z",
            "updated_at":"2026-11-11T15:01:15Z","html_url":"u"}"#,
        )
        .expect("run");
        let run = run.into_run();
        assert!(run.is_finished());
        let much_later = "2026-11-12T00:00:00Z".parse().expect("now");
        assert_eq!(run.duration_seconds(much_later), 65);
    }

    #[test]
    fn jobs_keep_their_steps_and_a_skipped_job_has_no_negative_time() {
        let jobs: WireJobs = serde_json::from_str(
            r#"{"jobs":[{"id":7,"name":"Build","status":"completed","conclusion":"skipped",
            "started_at":"2026-11-11T14:40:27Z","completed_at":"2026-11-11T14:40:26Z",
            "html_url":"u","runner_name":"","steps":[]},
            {"id":8,"name":"Test","status":"in_progress","conclusion":null,
            "started_at":"2026-11-11T14:40:27Z","completed_at":null,"html_url":"u","runner_name":"GitHub Actions 2",
            "steps":[{"number":1,"name":"Set up job","status":"completed","conclusion":"success",
            "started_at":"2026-11-11T14:40:27Z","completed_at":"2026-11-11T14:40:29Z"}]}]}"#,
        )
        .expect("jobs");
        let jobs: Vec<_> = jobs.jobs.into_iter().map(|job| job.into_job()).collect();
        let now = "2026-11-11T14:41:27Z".parse().expect("now");
        assert_eq!(jobs[0].status, CheckStatus::Skipped);
        assert_eq!(jobs[0].duration_seconds(now), Some(0));
        assert_eq!(jobs[0].runner, None);
        assert_eq!(jobs[1].duration_seconds(now), Some(60));
        assert_eq!(jobs[1].steps[0].status, CheckStatus::Passed);
    }

    const LOG: &str = "\u{feff}2026-11-10T21:00:00.1000000Z Current runner version: '2.329.0'\n\
        2026-11-10T21:00:01.9000000Z ##[group]Runner Image\n\
        2026-11-10T21:00:01.9100000Z ##[endgroup]\n\
        2026-11-10T21:00:02.3000000Z ##[group]Run cargo test\n\
        2026-11-10T21:00:02.3100000Z [command]/usr/bin/cargo test\n\
        2026-11-10T21:00:04.0000000Z \u{1b}[31merror\u{1b}[0m: test failed\n\
        continued line without a timestamp\n\
        2026-11-10T21:00:05.8000000Z ##[error]Process completed with exit code 101.\n\
        2026-11-10T21:00:06.5000000Z test result: FAILED\n\
        2026-11-10T21:00:07.2000000Z Post job cleanup.\n";

    #[test]
    fn the_log_splits_by_the_time_each_step_started() {
        let steps = [
            step(1, "2026-11-10T21:00:00Z", "2026-11-10T21:00:02Z"),
            step(2, "2026-11-10T21:00:02Z", "2026-11-10T21:00:06Z"),
            step(3, "2026-11-10T21:00:06Z", "2026-11-10T21:00:08Z"),
        ];
        let log = split_log(&steps, LOG);
        let texts = |number: u32| -> Vec<(LogLineKind, String)> {
            log.lines_of(number)
                .iter()
                .map(|line| (line.kind, line.text.clone()))
                .collect()
        };
        assert_eq!(
            texts(1),
            [
                (
                    LogLineKind::Plain,
                    "Current runner version: '2.329.0'".into()
                ),
                (LogLineKind::Group, "Runner Image".into()),
            ]
        );
        assert_eq!(
            texts(2),
            [
                (LogLineKind::Group, "Run cargo test".into()),
                (LogLineKind::Command, "/usr/bin/cargo test".into()),
                (LogLineKind::Plain, "error: test failed".into()),
                (
                    LogLineKind::Plain,
                    "continued line without a timestamp".into()
                ),
                (
                    LogLineKind::Error,
                    "Process completed with exit code 101.".into()
                ),
                // Written in the second step 3 started, before step 2 had
                // ended: it stays with step 2.
                (LogLineKind::Plain, "test result: FAILED".into()),
            ]
        );
        assert_eq!(texts(3), [(LogLineKind::Plain, "Post job cleanup.".into())]);
        assert!(log.lines_of(4).is_empty());
    }

    #[test]
    fn steps_that_share_one_second_split_where_each_one_starts() {
        // The fixture's job (IsraelAraujo70/loupe, "Loupe
        // fixture CI"): four steps that all started and ended at 16:30:34.
        let second = "2026-11-11T16:30:34Z";
        let steps = [
            step(1, second, second),
            step(2, second, second),
            step(3, second, second),
            step(4, second, second),
        ];
        let log = "2026-11-11T16:30:34.2466523Z ##[group]Runner Image Provisioner\n\
            2026-11-11T16:30:34.2479429Z ##[group]Operating System\n\
            2026-11-11T16:30:34.3347630Z Complete job name: fixture\n\
            2026-11-11T16:30:34.5740730Z ##[group]Run echo \"this step passes\"\n\
            2026-11-11T16:30:34.6877248Z this step passes\n\
            2026-11-11T16:30:34.7150723Z ##[group]Run echo \"checking the calculator\"\n\
            2026-11-11T16:30:34.7448537Z fixture log line 1\n\
            2026-11-11T16:30:34.7555029Z ##[error]Loupe fixture failure on purpose\n\
            2026-11-11T16:30:34.7673526Z Cleaning up orphan processes\n";
        let log = split_log(&steps, log);
        let texts = |number: u32| -> Vec<String> {
            log.lines_of(number)
                .iter()
                .map(|line| line.text.clone())
                .collect()
        };
        assert_eq!(
            texts(1),
            [
                "Runner Image Provisioner",
                "Operating System",
                "Complete job name: fixture"
            ]
        );
        assert_eq!(
            texts(2),
            ["Run echo \"this step passes\"", "this step passes"]
        );
        assert_eq!(
            texts(3),
            [
                "Run echo \"checking the calculator\"",
                "fixture log line 1",
                "Loupe fixture failure on purpose"
            ]
        );
        assert_eq!(texts(4), ["Cleaning up orphan processes"]);
    }

    #[test]
    fn a_step_reached_by_time_keeps_its_own_title() {
        // Step 1 ended long before; steps 2 and 3 share the second the title
        // of step 2 is written in. The title moves the log on once, to step 2.
        let steps = [
            step(1, "2026-11-10T21:00:00Z", "2026-11-10T21:00:01Z"),
            step(2, "2026-11-10T21:00:03Z", "2026-11-10T21:00:03Z"),
            step(3, "2026-11-10T21:00:03Z", "2026-11-10T21:00:04Z"),
        ];
        let log = "2026-11-10T21:00:00.5000000Z setting up\n\
            2026-11-10T21:00:03.5000000Z ##[group]Run make\n\
            2026-11-10T21:00:03.6000000Z built\n";
        let log = split_log(&steps, log);
        assert_eq!(log.lines_of(1).len(), 1);
        assert_eq!(log.lines_of(2).len(), 2);
        assert!(log.lines_of(3).is_empty());
    }

    #[test]
    fn a_composite_step_keeps_the_titles_of_its_own_steps() {
        // Step 1 is a composite action that ends at :12; its inner `Run`
        // titles and its error stay with it, though step 2 starts at :10.
        let steps = [
            step(1, "2026-11-10T21:00:00Z", "2026-11-10T21:00:12Z"),
            step(2, "2026-11-10T21:00:10Z", "2026-11-10T21:00:13Z"),
        ];
        let log = "2026-11-10T21:00:01.0000000Z ##[group]Run ./.github/actions/test\n\
            2026-11-10T21:00:10.2000000Z ##[group]Run make test\n\
            2026-11-10T21:00:10.3000000Z ##[error]tests failed\n\
            2026-11-10T21:00:12.5000000Z ##[group]Run deploy\n";
        let log = split_log(&steps, log);
        assert_eq!(log.lines_of(1).len(), 3);
        assert_eq!(log.lines_of(2).len(), 1);
    }

    #[test]
    fn a_run_waiting_on_an_approval_is_not_polled() {
        let run: WireRun = serde_json::from_str(
            r#"{"id":1,"run_number":1,"name":"CI","event":"pull_request","head_branch":"x",
            "head_sha":"a","status":"completed","conclusion":"action_required",
            "created_at":"2026-11-11T15:00:00Z","updated_at":"2026-11-11T15:00:00Z","html_url":"u"}"#,
        )
        .expect("run");
        let run = run.into_run();
        assert_eq!(run.status, CheckStatus::ActionRequired);
        assert!(run.is_finished());
    }

    #[test]
    fn annotations_link_to_their_line_and_failures_come_first() {
        let data: AnnotationsData = serde_json::from_str(
            r#"{"node":{"checkRuns":{"nodes":[{"databaseId":7,"name":"build","annotations":{"nodes":[
            {"annotationLevel":"NOTICE","title":"","message":"Cache restored","path":".github",
             "blobUrl":"https://github.com/acme/web/blob/abc/.github","location":{"start":{"line":0}}},
            {"annotationLevel":"FAILURE","title":"Test failed","message":"assertion","path":"src/lib.rs",
             "blobUrl":"https://github.com/acme/web/blob/abc/src/lib.rs","location":{"start":{"line":12}}}]}}]}}}"#,
        )
        .expect("annotations");
        let annotations = annotations_of(data);
        assert_eq!(annotations[0].level, AnnotationLevel::Failure);
        assert_eq!(
            annotations[0].url,
            "https://github.com/acme/web/blob/abc/src/lib.rs#L12"
        );
        assert_eq!(annotations[0].job_id, Some(7));
        assert_eq!(annotations[1].line, None);
        assert_eq!(annotations[1].title, None);
        assert_eq!(
            annotations[1].url,
            "https://github.com/acme/web/blob/abc/.github"
        );
    }
}
