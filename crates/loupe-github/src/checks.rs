//! The head commit's checks as GitHub's checks page reads them: one check
//! per job of each workflow and event, grouped by the workflow that ran it.

use crate::types::{CheckRun, CheckStatus, Checks, ChecksState};

/// Where a check comes from, the way GitHub's checks page groups them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CheckSource {
    /// A GitHub Actions workflow, apart for each event that runs it: the
    /// `push` and `pull_request` runs of one workflow are two groups.
    Workflow { name: String, event: Option<String> },
    /// Commit statuses and other apps' check runs.
    Other,
}

impl CheckRun {
    pub fn source(&self) -> CheckSource {
        match &self.workflow {
            Some(name) => CheckSource::Workflow {
                name: name.clone(),
                event: self.event.clone(),
            },
            None => CheckSource::Other,
        }
    }

    /// `CI / check (push)`, as GitHub's merge box names a check: the
    /// workflow, the job, and the event, so the `push` and `pull_request`
    /// runs of one job read apart.
    pub fn full_name(&self) -> String {
        match (&self.workflow, &self.event) {
            (Some(workflow), Some(event)) => format!("{workflow} / {} ({event})", self.name),
            (Some(workflow), None) => format!("{workflow} / {}", self.name),
            (None, _) => self.name.clone(),
        }
    }
}

impl Checks {
    /// The indices of `runs` by source: the workflows by name and event,
    /// then the other checks, each keeping GitHub's order of its runs.
    pub fn by_source(&self) -> Vec<(CheckSource, Vec<usize>)> {
        let mut groups: Vec<(CheckSource, Vec<usize>)> = Vec::new();
        for (ix, run) in self.runs.iter().enumerate() {
            let source = run.source();
            match groups.iter_mut().find(|(group, _)| *group == source) {
                Some((_, members)) => members.push(ix),
                None => groups.push((source, vec![ix])),
            }
        }
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        groups
    }
}

/// The head commit's checks from GitHub's rollup, one per context (see
/// [`collapse`]). GitHub's rolled-up state counts every run, so a failure
/// only an older run of a job had leaves it failing; once every context is
/// on hand (`complete`) and none of the checks left fails, the state is
/// what they say.
pub(crate) fn from_rollup(state: ChecksState, runs: Vec<CheckRun>, complete: bool) -> Checks {
    let runs = collapse(runs);
    let worst = runs.iter().map(|run| weight(run.status)).max();
    let state = match (state, worst) {
        (ChecksState::Failing, Some(worst)) if complete && worst < FAILING => {
            if worst == PENDING {
                ChecksState::Pending
            } else {
                ChecksState::Passing
            }
        }
        _ => state,
    };
    Checks {
        state: Some(state),
        runs,
    }
}

/// One check per context, from the contexts GitHub lists for a commit.
///
/// A job that ran again in a later run of its workflow for the same event
/// (the commit pushed again, a label that triggers the workflow) keeps
/// only its latest run; a re-run inside one workflow run already comes as
/// its latest attempt. A commit status a job posts about its own workflow
/// run (its name, linking to that run) folds into the job's check: the
/// status's description, the worse outcome, required if either is.
fn collapse(runs: Vec<CheckRun>) -> Vec<CheckRun> {
    let mut kept: Vec<CheckRun> = Vec::with_capacity(runs.len());
    let mut statuses: Vec<CheckRun> = Vec::new();
    for run in runs {
        if run.workflow.is_none() {
            statuses.push(run);
            continue;
        }
        let same_job = kept.iter_mut().find(|other| {
            other.workflow == run.workflow && other.event == run.event && other.name == run.name
        });
        match same_job {
            Some(other) if started_later(&run, other) => *other = run,
            Some(_) => {}
            None => kept.push(run),
        }
    }
    for status in statuses {
        let reported = status.details_url.as_deref().and_then(reported_run);
        let job = kept.iter_mut().find(|job| {
            job.name == status.name
                && reported.is_some()
                && job.actions.map(|actions| actions.run_id) == reported
        });
        match job {
            Some(job) => {
                job.description = job.description.take().or(status.description);
                job.is_required |= status.is_required;
                if weight(status.status) > weight(job.status) {
                    job.status = status.status;
                }
            }
            None => kept.push(status),
        }
    }
    kept
}

/// Whether `run` belongs to a later workflow run than `other`: run ids
/// grow with time, and a run GitHub names no id for falls back to its
/// start.
fn started_later(run: &CheckRun, other: &CheckRun) -> bool {
    let key = |run: &CheckRun| (run.actions.map(|job| job.run_id), run.started_at);
    key(run) > key(other)
}

/// The workflow run an Actions link points at (`…/actions/runs/<id>`).
fn reported_run(url: &str) -> Option<u64> {
    let (_, rest) = url.split_once("/actions/runs/")?;
    rest.split(['/', '?', '#']).next()?.parse().ok()
}

/// How much an outcome weighs when two reports of one check disagree:
/// failing over pending over passed over the rest, as the merge box
/// counts them.
fn weight(status: CheckStatus) -> u8 {
    match status {
        CheckStatus::Failed | CheckStatus::Cancelled | CheckStatus::ActionRequired => FAILING,
        CheckStatus::Queued | CheckStatus::Running => PENDING,
        CheckStatus::Passed => 1,
        CheckStatus::Neutral | CheckStatus::Skipped => 0,
    }
}

const FAILING: u8 = 3;
const PENDING: u8 = 2;

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{CheckSource, collapse, from_rollup};
    use crate::types::{ActionsJob, CheckRun, CheckStatus, Checks, ChecksState};

    fn job(workflow: &str, event: &str, name: &str, run_id: u64, status: CheckStatus) -> CheckRun {
        CheckRun {
            name: name.into(),
            workflow: Some(workflow.into()),
            event: Some(event.into()),
            description: None,
            status,
            started_at: None,
            completed_at: None,
            details_url: Some(format!(
                "https://github.com/acme/web/actions/runs/{run_id}/job/{}",
                run_id * 10
            )),
            is_required: false,
            actions: Some(ActionsJob {
                job_id: run_id * 10,
                run_id,
            }),
        }
    }

    fn status(name: &str, url: &str, status: CheckStatus) -> CheckRun {
        CheckRun {
            name: name.into(),
            workflow: None,
            event: None,
            description: Some("Some checks failed".into()),
            status,
            started_at: None,
            completed_at: None,
            details_url: Some(url.into()),
            is_required: false,
            actions: None,
        }
    }

    fn names(runs: &[CheckRun]) -> Vec<String> {
        runs.iter().map(CheckRun::full_name).collect()
    }

    #[test]
    fn push_and_pull_request_runs_of_one_job_stay_apart_and_read_apart() {
        let runs = collapse(vec![
            job("CI", "pull_request", "check", 2, CheckStatus::Passed),
            job("CI", "push", "check", 1, CheckStatus::Passed),
        ]);
        assert_eq!(
            names(&runs),
            ["CI / check (pull_request)", "CI / check (push)"]
        );
    }

    #[test]
    fn a_later_run_of_the_same_workflow_and_event_replaces_the_job() {
        let runs = collapse(vec![
            job("pr", "pull_request", "lint title", 7, CheckStatus::Failed),
            job("CI", "pull_request", "build", 7, CheckStatus::Passed),
            job("pr", "pull_request", "lint title", 9, CheckStatus::Passed),
            job("pr", "pull_request", "lint title", 8, CheckStatus::Skipped),
        ]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].name, "lint title", "it keeps its place");
        assert_eq!(runs[0].status, CheckStatus::Passed);
        assert_eq!(runs[0].actions.map(|job| job.run_id), Some(9));
    }

    #[test]
    fn a_queued_job_of_a_newer_run_counts_as_the_latest() {
        let mut old = job("CI", "pull_request", "test", 3, CheckStatus::Failed);
        old.started_at = DateTime::<Utc>::from_timestamp(1_000, 0);
        let queued = job("CI", "pull_request", "test", 4, CheckStatus::Queued);
        let runs = collapse(vec![old, queued]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, CheckStatus::Queued);
    }

    #[test]
    fn matrix_jobs_stay_one_check_each() {
        let runs = collapse(vec![
            job(
                "CI",
                "push",
                "test (ubuntu, stable)",
                1,
                CheckStatus::Passed,
            ),
            job("CI", "push", "test (macos, stable)", 1, CheckStatus::Failed),
        ]);
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn a_status_a_job_posts_about_its_own_run_folds_into_the_job() {
        let mut summary = status(
            "Summary",
            "https://github.com/acme/web/actions/runs/5",
            CheckStatus::Failed,
        );
        summary.is_required = true;
        let runs = collapse(vec![
            job("Tests", "pull_request", "Summary", 5, CheckStatus::Passed),
            summary,
        ]);
        assert_eq!(runs.len(), 1);
        let job = &runs[0];
        assert_eq!(job.full_name(), "Tests / Summary (pull_request)");
        assert_eq!(job.status, CheckStatus::Failed, "the worse outcome wins");
        assert_eq!(job.description.as_deref(), Some("Some checks failed"));
        assert!(job.is_required);
        assert_eq!(job.actions.map(|job| job.job_id), Some(50));
    }

    #[test]
    fn a_status_from_elsewhere_with_a_job_s_name_stays_its_own_check() {
        let runs = collapse(vec![
            job("CI", "push", "build", 5, CheckStatus::Passed),
            status("build", "https://ci.example/build/5", CheckStatus::Failed),
            status(
                "build",
                "https://github.com/acme/web/actions/runs/6",
                CheckStatus::Failed,
            ),
            status(
                "deploy",
                "https://github.com/acme/web/actions/runs/5",
                CheckStatus::Passed,
            ),
        ]);
        assert_eq!(
            names(&runs),
            ["CI / build (push)", "build", "build", "deploy"]
        );
        assert_eq!(runs[0].status, CheckStatus::Passed);
    }

    #[test]
    fn checks_group_by_workflow_and_event_then_the_others() {
        let checks = Checks {
            state: None,
            runs: vec![
                status("Vercel", "https://vercel.example/1", CheckStatus::Passed),
                job("Release", "pull_request", "linux", 3, CheckStatus::Passed),
                job("CI", "push", "check", 1, CheckStatus::Passed),
                job("CI", "pull_request", "check", 2, CheckStatus::Passed),
                job("CI", "pull_request", "app-tests", 2, CheckStatus::Passed),
                job("CI", "push", "app-tests", 1, CheckStatus::Passed),
            ],
        };
        let workflow = |name: &str, event: &str| CheckSource::Workflow {
            name: name.into(),
            event: Some(event.into()),
        };
        assert_eq!(
            checks.by_source(),
            vec![
                (workflow("CI", "pull_request"), vec![3, 4]),
                (workflow("CI", "push"), vec![2, 5]),
                (workflow("Release", "pull_request"), vec![1]),
                (CheckSource::Other, vec![0]),
            ]
        );
    }

    #[test]
    fn a_failure_only_an_older_run_had_no_longer_fails_the_checks() {
        let runs = || {
            vec![
                job("pr", "pull_request", "lint title", 7, CheckStatus::Failed),
                job("pr", "pull_request", "lint title", 9, CheckStatus::Passed),
                job("CI", "pull_request", "build", 9, CheckStatus::Passed),
            ]
        };
        let checks = from_rollup(ChecksState::Failing, runs(), true);
        assert_eq!(checks.state, Some(ChecksState::Passing));
        assert_eq!(checks.runs.len(), 2);

        let mut running = runs();
        running[2].status = CheckStatus::Running;
        let checks = from_rollup(ChecksState::Failing, running, true);
        assert_eq!(checks.state, Some(ChecksState::Pending));

        let checks = from_rollup(ChecksState::Failing, runs(), false);
        assert_eq!(
            checks.state,
            Some(ChecksState::Failing),
            "a context GitHub did not list may be the one failing"
        );
    }

    #[test]
    fn github_s_state_stands_while_a_check_left_fails_or_it_was_not_failing() {
        let failing = vec![job("CI", "push", "build", 1, CheckStatus::Failed)];
        let checks = from_rollup(ChecksState::Failing, failing, true);
        assert_eq!(checks.state, Some(ChecksState::Failing));
        let passed = vec![job("CI", "push", "build", 1, CheckStatus::Passed)];
        let checks = from_rollup(ChecksState::Pending, passed, true);
        assert_eq!(
            checks.state,
            Some(ChecksState::Pending),
            "a required check that has not reported yet keeps it pending"
        );
    }

    #[test]
    fn a_check_without_an_event_is_named_by_its_workflow_and_a_status_by_itself() {
        let mut check = job("CI", "push", "check", 1, CheckStatus::Passed);
        check.event = None;
        assert_eq!(check.full_name(), "CI / check");
        let status = status("ci/legacy", "https://ci.example/1", CheckStatus::Running);
        assert_eq!(status.full_name(), "ci/legacy");
    }
}
