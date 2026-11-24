//! GitHub Actions: `Route::Actions` lists a repository's (or a branch's)
//! workflow runs, and `Route::WorkflowRun` shows one run: its graph, jobs,
//! steps and logs, annotations and artifacts, and its actions.

mod graph;
mod jobs;
mod layout;
mod log;
mod run;
mod runs;

use std::time::Duration;

use gpui_kit::*;
use loupe_github::{CheckStatus, RepoRef};

pub(super) use crate::github::pull_request::checks::{format_duration, status_style};

/// How often a run or a list with a run in progress reloads.
const POLL_EVERY: Duration = Duration::from_secs(5);

pub(super) fn refresh(view: &AnyView, cx: &mut App) -> bool {
    crate::shell::refresh_as::<runs::RunsView>(view, cx)
        || crate::shell::refresh_as::<run::RunView>(view, cx)
}

pub fn runs_view(
    repo: RepoRef,
    branch: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    cx.new(|cx| runs::RunsView::new(repo, branch, window, cx))
        .into()
}

pub fn run_view(
    repo: RepoRef,
    run_id: u64,
    job_id: Option<u64>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    cx.new(|cx| run::RunView::new(repo, run_id, job_id, window, cx))
        .into()
}

/// A job's or a step's time: how long it took, how long it has run so far,
/// or nothing while it waits or when it was skipped.
pub(super) fn job_duration_label(status: CheckStatus, seconds: Option<i64>) -> Option<String> {
    match status {
        CheckStatus::Queued => Some("queued".into()),
        CheckStatus::Skipped => None,
        _ => seconds.map(format_duration),
    }
}

/// What a state is called in a sentence: "succeeded", "failed"…
pub(super) fn status_word(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Queued => "queued",
        CheckStatus::Running => "in progress",
        CheckStatus::Passed => "succeeded",
        CheckStatus::Failed => "failed",
        CheckStatus::Neutral => "completed",
        CheckStatus::Skipped => "skipped",
        CheckStatus::Cancelled => "cancelled",
        CheckStatus::ActionRequired => "waiting for approval",
    }
}

#[cfg(test)]
mod tests {
    use loupe_github::CheckStatus;

    use super::job_duration_label;

    #[test]
    fn a_skipped_job_shows_no_time_and_a_waiting_one_says_so() {
        assert_eq!(job_duration_label(CheckStatus::Skipped, Some(0)), None);
        assert_eq!(
            job_duration_label(CheckStatus::Queued, None).as_deref(),
            Some("queued")
        );
        assert_eq!(
            job_duration_label(CheckStatus::Passed, Some(83)).as_deref(),
            Some("1m 23s")
        );
        assert_eq!(job_duration_label(CheckStatus::Running, None), None);
    }
}
