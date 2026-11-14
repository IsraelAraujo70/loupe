//! GitHub Actions through REST: the log of a failed job's failing step, and
//! re-running a workflow run's failed jobs. A check run made by Actions has
//! the job's id as its database id (`CheckRun::actions`).

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::rest::RestMethod;
use crate::{Error, GitHub, RepoRef, Result, StepLog};

/// The most log lines a [`StepLog`] keeps: the end of the step, where the
/// failure is.
pub const MAX_LOG_LINES: usize = 200;

impl GitHub {
    /// The last lines of the failed step of Actions job `job_id`. The job's
    /// steps name the failed one and time it; the job log is cut to that
    /// window. When no step failed or the times do not line up with the log,
    /// it is the end of the whole log.
    pub fn failed_step_log(&self, repo: &RepoRef, job_id: u64) -> Result<StepLog> {
        let base = format!("/repos/{}/{}/actions/jobs/{job_id}", repo.owner, repo.name);
        let job: WireJob = serde_json::from_str(&self.rest(RestMethod::Get, &base)?)
            .map_err(|error| Error::Decode(error.to_string()))?;
        // GitHub answers with a redirect to a short-lived download URL; ureq
        // follows it and does not forward the token to the other host.
        let log = self.rest(RestMethod::Get, &format!("{base}/logs"))?;
        Ok(cut_step_log(&job.steps, &log, MAX_LOG_LINES))
    }

    /// Re-runs the failed jobs of workflow run `run_id`, and the jobs that
    /// depend on them.
    pub fn rerun_failed_jobs(&self, repo: &RepoRef, run_id: u64) -> Result<()> {
        let path = format!(
            "/repos/{}/{}/actions/runs/{run_id}/rerun-failed-jobs",
            repo.owner, repo.name
        );
        self.rest(RestMethod::Post, &path).map(drop)
    }
}

#[derive(Deserialize)]
struct WireJob {
    #[serde(default)]
    steps: Vec<WireStep>,
}

#[derive(Deserialize)]
struct WireStep {
    name: String,
    conclusion: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

/// Keeps the log lines of the failed step (by time: step times have whole
/// seconds, log lines fractions, so the window runs to the second after the
/// step completed), then their last `max_lines`.
fn cut_step_log(steps: &[WireStep], log: &str, max_lines: usize) -> StepLog {
    let failed = steps
        .iter()
        .find(|step| step.conclusion.as_deref() == Some("failure"));
    let window =
        failed.and_then(|step| Some((step.started_at?, step.completed_at? + Duration::seconds(1))));
    let lines: Vec<(Option<DateTime<Utc>>, String)> = log
        .lines()
        .map(|line| {
            let (at, text) = split_timestamp(line.trim_start_matches('\u{feff}'));
            (at, strip_ansi(text))
        })
        .collect();
    let mut kept: Vec<String> = Vec::new();
    if let Some((from, to)) = window {
        let mut inside = false;
        for (at, text) in &lines {
            if let Some(at) = at {
                inside = *at >= from && *at < to;
            }
            if inside {
                kept.push(text.clone());
            }
        }
    }
    let step = if kept.is_empty() {
        kept = lines.into_iter().map(|(_, text)| text).collect();
        None
    } else {
        failed.map(|step| step.name.clone())
    };
    let truncated = kept.len() > max_lines;
    let lines = kept.split_off(kept.len().saturating_sub(max_lines));
    StepLog {
        step,
        lines,
        truncated,
    }
}

/// Splits `2026-11-10T21:00:00.1234567Z text` into its time and text. A line
/// without a leading timestamp continues the previous one.
pub(crate) fn split_timestamp(line: &str) -> (Option<DateTime<Utc>>, &str) {
    match line.split_once(' ') {
        Some((stamp, text)) => match DateTime::parse_from_rfc3339(stamp) {
            Ok(at) => (Some(at.to_utc()), text),
            Err(_) => (None, line),
        },
        None => match DateTime::parse_from_rfc3339(line) {
            Ok(at) => (Some(at.to_utc()), ""),
            Err(_) => (None, line),
        },
    }
}

/// Removes terminal color and cursor codes (`ESC [ ... letter`).
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{WireJob, cut_step_log, strip_ansi};

    const JOB: &str = r#"{
        "steps": [
            { "name": "Set up job", "conclusion": "success",
              "started_at": "2026-11-10T21:00:00Z", "completed_at": "2026-11-10T21:00:02Z" },
            { "name": "Run tests", "conclusion": "failure",
              "started_at": "2026-11-10T21:00:02Z", "completed_at": "2026-11-10T21:00:05Z" },
            { "name": "Post checkout", "conclusion": "success",
              "started_at": "2026-11-10T21:00:07Z", "completed_at": "2026-11-10T21:00:08Z" }
        ]
    }"#;

    const LOG: &str = "\u{feff}2026-11-10T21:00:00.1000000Z ##[group]Runner Image\n\
        2026-11-10T21:00:01.9000000Z Setting up\n\
        2026-11-10T21:00:02.3000000Z ##[group]Run cargo test\n\
        2026-11-10T21:00:04.0000000Z \u{1b}[31merror\u{1b}[0m: test failed\n\
        continued line without a timestamp\n\
        2026-11-10T21:00:05.8000000Z ##[error]Process completed with exit code 101.\n\
        2026-11-10T21:00:07.2000000Z Post job cleanup.\n";

    fn job() -> WireJob {
        serde_json::from_str(JOB).expect("job fixture")
    }

    #[test]
    fn keeps_the_failed_step_window() {
        let log = cut_step_log(&job().steps, LOG, 200);
        assert_eq!(log.step.as_deref(), Some("Run tests"));
        assert_eq!(
            log.lines,
            [
                "##[group]Run cargo test",
                "error: test failed",
                "continued line without a timestamp",
                "##[error]Process completed with exit code 101.",
            ]
        );
        assert!(!log.truncated);
    }

    #[test]
    fn keeps_only_the_last_lines() {
        let log = cut_step_log(&job().steps, LOG, 2);
        assert_eq!(
            log.lines,
            [
                "continued line without a timestamp",
                "##[error]Process completed with exit code 101."
            ]
        );
        assert!(log.truncated);
    }

    #[test]
    fn without_a_failed_step_shows_the_end_of_the_log() {
        let passing: WireJob = serde_json::from_str(r#"{ "steps": [] }"#).expect("empty job");
        let log = cut_step_log(&passing.steps, LOG, 1);
        assert_eq!(log.step, None);
        assert_eq!(log.lines, ["Post job cleanup."]);
        assert!(log.truncated);
    }

    #[test]
    fn strips_color_codes() {
        assert_eq!(strip_ansi("\u{1b}[1;31mred\u{1b}[0m ok"), "red ok");
    }
}
