//! When the pull request view asks GitHub again on its own: only while a
//! check is queued or running, every 15 s while the pull request is on
//! screen and every 60 s behind another page, and not while a rate limit
//! runs low.

use std::time::Duration;

use chrono::{DateTime, Utc};
use loupe_github::{ActionsJob, CheckStatus, Checks, ChecksState, PullRequestDetail, RateLimits};

/// How often the view looks at the clock to decide.
pub(super) const TICK: Duration = Duration::from_secs(5);
pub(super) const ON_SCREEN: Duration = Duration::from_secs(15);
pub(super) const OFF_SCREEN: Duration = Duration::from_secs(60);
/// How long after a re-run the view keeps asking while GitHub lists no
/// check as pending yet: the new jobs take a moment to show up.
pub(super) const AFTER_RERUN: Duration = Duration::from_secs(120);

/// Whether the checks of the loaded head can still change on their own: a
/// check is queued or running, or GitHub reports the head pending before it
/// lists any check, or a re-run was asked for within `AFTER_RERUN`.
pub(super) fn checks_may_change(detail: &PullRequestDetail, since_rerun: Option<Duration>) -> bool {
    let runs = &detail.checks.runs;
    runs.iter()
        .any(|run| matches!(run.status, CheckStatus::Queued | CheckStatus::Running))
        || (runs.is_empty() && detail.checks.state == Some(ChecksState::Pending))
        || since_rerun.is_some_and(|elapsed| elapsed < AFTER_RERUN)
}

/// Whether a load after re-running `jobs` brought their replacements: a
/// job of the same workflow run that is not one of them. Until then GitHub
/// may still list the old jobs, or only the checks that were not re-run.
pub(super) fn rerun_arrived(jobs: &[ActionsJob], loaded: &Checks) -> bool {
    loaded.runs.iter().filter_map(|run| run.actions).any(|job| {
        jobs.iter().any(|old| old.run_id == job.run_id)
            && !jobs.iter().any(|old| old.job_id == job.job_id)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Poll {
    /// Nothing can change: no request at all.
    Idle,
    /// The last answer is recent enough.
    Wait,
    /// A rate limit is low: nothing until it resets.
    Paused(DateTime<Utc>),
    Now,
}

/// What to do on a tick, given whether the checks can change, whether the
/// pull request is on screen, and how long ago GitHub last answered.
pub(super) fn decide(
    may_change: bool,
    on_screen: bool,
    since_last: Duration,
    rate: RateLimits,
    now: DateTime<Utc>,
) -> Poll {
    if !may_change {
        return Poll::Idle;
    }
    let paused_until = [rate.core, rate.graphql]
        .into_iter()
        .flatten()
        .filter(|limit| limit.is_low() && limit.reset > now)
        .map(|limit| limit.reset)
        .max();
    if let Some(until) = paused_until {
        return Poll::Paused(until);
    }
    let interval = if on_screen { ON_SCREEN } else { OFF_SCREEN };
    if since_last >= interval {
        Poll::Now
    } else {
        Poll::Wait
    }
}

/// A pull request that comes back on screen after a while away loads
/// again, so what it shows is never older than a minute on arrival.
pub(super) fn reload_on_return(was_on_screen: bool, on_screen: bool, since_last: Duration) -> bool {
    on_screen && !was_on_screen && since_last >= OFF_SCREEN
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use loupe_github::{
        ActionsJob, CheckRun, CheckStatus, Checks, ChecksState, MergeStateStatus, Mergeable, PrRef,
        PrState, PullRequestDetail, RateLimit, RateLimits, RepoRef,
    };

    use super::{
        AFTER_RERUN, OFF_SCREEN, ON_SCREEN, Poll, checks_may_change, decide, reload_on_return,
        rerun_arrived,
    };

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn detail(state: Option<ChecksState>, statuses: &[CheckStatus]) -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_1".into(),
            pr: PrRef::new(RepoRef::new("acme", "app"), 1),
            url: String::new(),
            title: String::new(),
            body: String::new(),
            state: PrState::Open,
            is_draft: false,
            mergeable: Mergeable::Mergeable,
            merge_state: MergeStateStatus::Clean,
            merge_methods: Vec::new(),
            base_ref: "main".into(),
            base_oid: String::new(),
            head_ref: "feature".into(),
            head_repo: None,
            head_oid: "abc".into(),
            author: None,
            created_at: at(0),
            updated_at: at(0),
            body_edits: Vec::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            review_decision: None,
            reviewers: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            commits: Vec::new(),
            checks: Checks {
                state,
                runs: statuses
                    .iter()
                    .map(|status| CheckRun {
                        name: "ci".into(),
                        workflow: None,
                        status: *status,
                        started_at: None,
                        completed_at: None,
                        details_url: None,
                        is_required: false,
                        actions: None,
                    })
                    .collect(),
            },
            timeline: Vec::new(),
            threads: Vec::new(),
            pending_review: None,
            viewer_login: String::new(),
            viewer_can_update: false,
        }
    }

    #[test]
    fn only_queued_or_running_checks_keep_the_view_asking() {
        let pending = Some(ChecksState::Pending);
        assert!(checks_may_change(
            &detail(pending, &[CheckStatus::Passed, CheckStatus::Running]),
            None
        ));
        assert!(checks_may_change(
            &detail(pending, &[CheckStatus::Queued]),
            None
        ));
        assert!(checks_may_change(&detail(pending, &[]), None));
        assert!(!checks_may_change(
            &detail(
                Some(ChecksState::Failing),
                &[
                    CheckStatus::Passed,
                    CheckStatus::Failed,
                    CheckStatus::Skipped
                ]
            ),
            None
        ));
        assert!(!checks_may_change(&detail(None, &[]), None));
    }

    #[test]
    fn a_re_run_keeps_the_view_asking_until_the_new_jobs_show_up() {
        let done = detail(Some(ChecksState::Failing), &[CheckStatus::Failed]);
        assert!(checks_may_change(&done, Some(Duration::from_secs(10))));
        assert!(!checks_may_change(&done, Some(AFTER_RERUN)));
    }

    #[test]
    fn nothing_is_asked_while_no_check_is_pending() {
        for on_screen in [true, false] {
            assert_eq!(
                decide(
                    false,
                    on_screen,
                    Duration::from_secs(3_600),
                    RateLimits::default(),
                    at(0)
                ),
                Poll::Idle
            );
        }
    }

    #[test]
    fn on_screen_asks_every_15_s_and_off_screen_every_60_s() {
        let ask = |on_screen, since: Duration| {
            decide(true, on_screen, since, RateLimits::default(), at(0))
        };
        assert_eq!(ON_SCREEN, Duration::from_secs(15));
        assert_eq!(OFF_SCREEN, Duration::from_secs(60));
        assert_eq!(ask(true, Duration::from_secs(14)), Poll::Wait);
        assert_eq!(ask(true, Duration::from_secs(15)), Poll::Now);
        assert_eq!(ask(false, Duration::from_secs(15)), Poll::Wait);
        assert_eq!(ask(false, Duration::from_secs(59)), Poll::Wait);
        assert_eq!(ask(false, Duration::from_secs(60)), Poll::Now);
    }

    #[test]
    fn a_low_rate_limit_pauses_asking_until_it_resets() {
        let bucket = |remaining, reset| RateLimit {
            limit: 5000,
            remaining,
            reset: at(reset),
        };
        let since = Duration::from_secs(600);
        let low_rest = RateLimits {
            core: Some(bucket(120, 900)),
            graphql: Some(bucket(4000, 700)),
        };
        assert_eq!(
            decide(true, true, since, low_rest, at(100)),
            Poll::Paused(at(900))
        );
        let low_graphql = RateLimits {
            core: Some(bucket(4000, 900)),
            graphql: Some(bucket(3, 700)),
        };
        assert_eq!(
            decide(true, true, since, low_graphql, at(100)),
            Poll::Paused(at(700))
        );
        // Past its reset, a low bucket is full again.
        assert_eq!(decide(true, true, since, low_rest, at(901)), Poll::Now);
        let healthy = RateLimits {
            core: Some(bucket(4000, 900)),
            graphql: None,
        };
        assert_eq!(decide(true, true, since, healthy, at(100)), Poll::Now);
    }

    #[test]
    fn coming_back_after_a_minute_away_loads_again() {
        let minute = Duration::from_secs(60);
        assert!(reload_on_return(false, true, minute));
        assert!(!reload_on_return(false, true, Duration::from_secs(59)));
        assert!(!reload_on_return(true, true, minute));
        assert!(!reload_on_return(false, false, minute));
    }

    #[test]
    fn a_re_run_ends_its_wait_only_when_a_replacement_job_shows_up() {
        let old = ActionsJob {
            job_id: 10,
            run_id: 7,
        };
        let with = |entries: &[(CheckStatus, Option<ActionsJob>)]| {
            let mut checks = detail(None, &[]).checks;
            for (status, job) in entries {
                let mut run = detail(None, &[*status]).checks.runs.remove(0);
                run.actions = *job;
                checks.runs.push(run);
            }
            checks
        };
        let status_only = with(&[(CheckStatus::Passed, None)]);
        assert!(
            !rerun_arrived(&[old], &status_only),
            "only a commit status that was not re-run"
        );
        assert!(
            !rerun_arrived(&[old], &with(&[(CheckStatus::Failed, Some(old))])),
            "GitHub still lists the old job"
        );
        let other_run = ActionsJob {
            job_id: 11,
            run_id: 8,
        };
        assert!(!rerun_arrived(
            &[old],
            &with(&[(CheckStatus::Queued, Some(other_run))])
        ));
        let replacement = ActionsJob {
            job_id: 12,
            run_id: 7,
        };
        assert!(rerun_arrived(
            &[old],
            &with(&[
                (CheckStatus::Passed, None),
                (CheckStatus::Queued, Some(replacement))
            ])
        ));
    }
}
