//! Per-saved-view automatic guided review generator.
//!
//! When a saved Reviews view enables "Auto-summarize Needs review", Loupe
//! watches pull requests entering that view's Needs review tab (and pull
//! requests that receive new commits after an earlier guide), queuing them and
//! generating guides strictly one at a time up to `Settings.guide.auto_max_per_hour`.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use loupe_github::reviews::{self, ReviewTab, SavedView};
use loupe_github::{Inbox, Page, PrRef, PrSummary, PullRequestDetail};

use super::{tabs_shown, views_store};
use crate::github::{common, guide_service};
use crate::store::Settings;

/// How often the auto-guide worker polls for queued work or fresh saved views.
const TICK: Duration = Duration::from_secs(20);

/// How often the watcher fetches GitHub inbox data on its own when no Reviews
/// page or bell watcher has fed it recently.
const POLL_INBOX_EVERY: Duration = Duration::from_secs(180);

/// Rolling rate-limit window (1 hour).
const RATE_WINDOW_SECS: i64 = 3600;

/// Pure sequential scheduler enforcing deduplication and a rolling hourly cap.
#[derive(Clone, Debug, Default)]
pub struct AutoGuideScheduler {
    queue: VecDeque<PrRef>,
    started_in_window: VecDeque<DateTime<Utc>>,
    /// Tracks the latest `updated_at` already inspected per PR so an unchanged
    /// PR is never re-queued in a loop.
    seen_updated: HashMap<PrRef, DateTime<Utc>>,
}

impl AutoGuideScheduler {
    /// Queues `pr` if `updated_at` is newer than the last time we queued/checked
    /// it and it is not already waiting in the queue.
    pub fn enqueue_if_new(&mut self, pr: PrRef, updated_at: DateTime<Utc>) -> bool {
        if self
            .seen_updated
            .get(&pr)
            .is_some_and(|seen| *seen >= updated_at)
        {
            return false;
        }
        self.seen_updated.insert(pr.clone(), updated_at);
        if self.queue.contains(&pr) {
            return false;
        }
        self.queue.push_back(pr);
        true
    }

    /// Forces `pr` onto the queue (e.g. when an open PR view detects a new commit
    /// on an already-guided PR inside an auto-guide view).
    pub fn enqueue_now(&mut self, pr: PrRef) {
        if !self.queue.contains(&pr) {
            self.queue.push_back(pr);
        }
    }

    /// Drops rate-limit timestamps older than one hour.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - chrono::Duration::seconds(RATE_WINDOW_SECS);
        while self
            .started_in_window
            .front()
            .is_some_and(|at| *at <= cutoff)
        {
            self.started_in_window.pop_front();
        }
    }

    /// How many automatic runs remain available in the current rolling hour.
    pub fn remaining_budget(&mut self, now: DateTime<Utc>, max_per_hour: u32) -> usize {
        self.prune(now);
        (max_per_hour as usize).saturating_sub(self.started_in_window.len())
    }

    /// Pops the next queued PR only if no run is currently active and the hourly
    /// cap has room. Does NOT consume budget yet — call [`Self::record_started`]
    /// once generation actually begins (so cache hits stay free).
    pub fn peek_next(
        &mut self,
        now: DateTime<Utc>,
        max_per_hour: u32,
        busy: bool,
    ) -> Option<PrRef> {
        if busy || self.remaining_budget(now, max_per_hour) == 0 {
            return None;
        }
        self.queue.pop_front()
    }

    /// Records that an automatic guide generation actually started at `now`.
    pub fn record_started(&mut self, now: DateTime<Utc>) {
        self.prune(now);
        self.started_in_window.push_back(now);
    }

    #[cfg(test)]
    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }
}

struct Watcher {
    scheduler: AutoGuideScheduler,
    last_inbox_poll: Option<std::time::Instant>,
    poll_task: Option<Task<()>>,
    fetch_task: Option<Task<()>>,
    _ticker: Task<()>,
}

struct AutoGuide(Entity<Watcher>);

impl Global for AutoGuide {}

/// Starts the background auto-guide coordinator.
pub fn start(cx: &mut App) {
    if cx.has_global::<AutoGuide>() {
        return;
    }
    let watcher = cx.new(|cx| Watcher {
        scheduler: AutoGuideScheduler::default(),
        last_inbox_poll: None,
        poll_task: None,
        fetch_task: None,
        _ticker: cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this
                    .update(cx, |watcher: &mut Watcher, cx| watcher.tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        }),
    });
    cx.set_global(AutoGuide(watcher));
}

/// Turns "Auto-summarize Needs review" on or off for saved view `id`.
pub fn set(id: &str, on: bool, cx: &mut App) -> Result<(), SharedString> {
    views_store::update(cx, |file| file.set_auto_guide(id, on, Utc::now()))?;
    if on && let Some(watcher) = cx.try_global::<AutoGuide>().map(|g| g.0.clone()) {
        watcher.update(cx, |watcher, cx| {
            watcher.last_inbox_poll = None;
            watcher.tick(cx);
        });
    }
    Ok(())
}

/// Feeds freshly loaded inbox/all-open data so auto-guide views queue Needs
/// review pull requests without extra GitHub requests.
pub fn observe_inbox(inbox: &Inbox, all_open: &HashMap<String, Page<PrSummary>>, cx: &mut App) {
    let Some(watcher) = cx.try_global::<AutoGuide>().map(|g| g.0.clone()) else {
        return;
    };
    let inbox = inbox.clone();
    let all_open = all_open.clone();
    watcher.update(cx, |watcher, cx| {
        watcher.last_inbox_poll = Some(std::time::Instant::now());
        watcher.collect_from_inbox(&inbox, &all_open, cx);
        watcher.pump(cx);
    });
}

/// Called when a `PullRequestView` reloads and sees a new commit on a PR that
/// already had a guide: if any `auto_guide` saved view matches this PR's
/// repository/filters, queues automatic regeneration.
pub fn enqueue_stale_if_watched(detail: Arc<PullRequestDetail>, cx: &mut App) {
    let views = watched_views(cx);
    if views.is_empty() {
        return;
    }
    if !guide_service::has_stale_guide(cx, &detail) {
        return;
    }
    let matches_any = views.iter().any(|view| {
        // Match repository/org clauses if any; views without repo clauses cover all inbox PRs.
        let repo_str = detail.pr.repo.to_string();
        let owner = &detail.pr.repo.owner;
        view.filters.iter().all(|clause| match clause.field {
            reviews::Field::Repo => {
                let any = clause
                    .values
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(&repo_str));
                match clause.op {
                    reviews::Op::In => any,
                    reviews::Op::NotIn => !any,
                }
            }
            reviews::Field::Org => {
                let any = clause.values.iter().any(|v| v.eq_ignore_ascii_case(owner));
                match clause.op {
                    reviews::Op::In => any,
                    reviews::Op::NotIn => !any,
                }
            }
            _ => true,
        })
    });
    if !matches_any {
        return;
    }
    if let Some(watcher) = cx.try_global::<AutoGuide>().map(|g| g.0.clone()) {
        watcher.update(cx, |watcher, cx| {
            watcher.scheduler.enqueue_now(detail.pr.clone());
            watcher.pump(cx);
        });
    }
}

fn watched_views(cx: &mut App) -> Vec<SavedView> {
    views_store::snapshot(cx)
        .views
        .into_iter()
        .filter(|view| view.auto_guide)
        .collect()
}

impl Watcher {
    fn tick(&mut self, cx: &mut Context<Self>) {
        let views = watched_views(cx);
        if views.is_empty() {
            return;
        }
        self.pump(cx);
        if self.poll_task.is_some() || common::client(cx).is_none() {
            return;
        }
        let due = self
            .last_inbox_poll
            .is_none_or(|at| at.elapsed() >= POLL_INBOX_EVERY);
        if !due {
            return;
        }
        let mut queries: Vec<String> = views
            .iter()
            .filter_map(|view| {
                view.all_open
                    .unwrap_or(true)
                    .then(|| reviews::all_open_query(&view.filters))
                    .flatten()
            })
            .collect();
        queries.sort();
        queries.dedup();
        self.last_inbox_poll = Some(std::time::Instant::now());
        self.poll_task = Some(common::run(
            cx,
            move |gh| {
                let inbox = gh.inbox()?;
                let mut all_open = HashMap::new();
                for query in queries {
                    if let Ok(page) = gh.search_all_pull_requests(&query) {
                        all_open.insert(query, page);
                    }
                }
                Ok((inbox, all_open))
            },
            |watcher: &mut Watcher, result, cx| {
                watcher.poll_task = None;
                if let Ok((inbox, all_open)) = result {
                    watcher.collect_from_inbox(&inbox, &all_open, cx);
                    watcher.pump(cx);
                }
            },
        ));
    }

    fn collect_from_inbox(
        &mut self,
        inbox: &Inbox,
        all_open: &HashMap<String, Page<PrSummary>>,
        cx: &mut Context<Self>,
    ) {
        let views = watched_views(cx);
        for view in &views {
            let page = view
                .all_open
                .unwrap_or(true)
                .then(|| reviews::all_open_query(&view.filters))
                .flatten()
                .and_then(|q| all_open.get(&q));
            let needs_review = tabs_shown(inbox, page, &view.filters)
                .into_iter()
                .find(|(tab, _)| *tab == ReviewTab::NeedsReview)
                .map(|(_, shown)| shown.items)
                .unwrap_or_default();
            let language = Settings::get(cx).guide.language.clone();
            for row in needs_review {
                // Skip if a guide already exists and the PR hasn't been updated since.
                // The list has no description, so the title alone picks the language here;
                // `has_fresh_guide` settles it once the pull request is loaded.
                let language = language.resolve(&row.title, "");
                let needs_guide = match guide_service::latest_generated_at(cx, &row.pr, &language) {
                    None => true,
                    Some(generated_at) => row.updated_at > generated_at,
                };
                if needs_guide {
                    self.scheduler
                        .enqueue_if_new(row.pr.clone(), row.updated_at);
                }
            }
        }
    }

    /// Starts at most one automatic guide generation when idle and within hourly quota.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.fetch_task.is_some() {
            return;
        }
        let max_per_hour = Settings::get(cx).guide.auto_max_per_hour;
        let busy = guide_service::any_generating(cx);
        let Some(pr) = self.scheduler.peek_next(Utc::now(), max_per_hour, busy) else {
            return;
        };
        if common::client(cx).is_none() {
            return;
        }
        let fetch_pr = pr.clone();
        self.fetch_task = Some(common::run(
            cx,
            move |gh| gh.pull_request(&fetch_pr),
            move |watcher: &mut Watcher, result, cx| {
                watcher.fetch_task = None;
                let Ok(detail) = result else {
                    watcher.pump(cx);
                    return;
                };
                if guide_service::has_fresh_guide(cx, &detail) {
                    // Already cached for this exact commit: keep budget intact and move on.
                    watcher.pump(cx);
                    return;
                }
                let max_per_hour = Settings::get(cx).guide.auto_max_per_hour;
                if guide_service::any_generating(cx)
                    || watcher.scheduler.remaining_budget(Utc::now(), max_per_hour) == 0
                {
                    watcher.scheduler.enqueue_now(detail.pr.clone());
                    return;
                }
                watcher.scheduler.record_started(Utc::now());
                guide_service::start_generation(Arc::new(detail), cx);
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use loupe_github::{PrRef, RepoRef};

    use super::AutoGuideScheduler;

    fn pr(n: u64) -> PrRef {
        PrRef::new(RepoRef::new("acme", "app"), n)
    }

    #[test]
    fn scheduler_runs_one_at_a_time_dedups_and_enforces_hourly_limit() {
        let mut sched = AutoGuideScheduler::default();
        let t0 = Utc::now();

        assert!(sched.enqueue_if_new(pr(1), t0));
        assert!(
            !sched.enqueue_if_new(pr(1), t0),
            "same PR with same timestamp is deduplicated"
        );
        assert!(sched.enqueue_if_new(pr(2), t0));
        assert!(sched.enqueue_if_new(pr(3), t0));
        assert_eq!(sched.queued_len(), 3);

        // Busy blocks starting any job.
        assert_eq!(sched.peek_next(t0, 2, true), None);

        // First two start within hourly limit of 2.
        assert_eq!(sched.peek_next(t0, 2, false), Some(pr(1)));
        sched.record_started(t0);
        assert_eq!(sched.peek_next(t0, 2, false), Some(pr(2)));
        sched.record_started(t0 + Duration::minutes(5));

        // Third is blocked by the 2/hour limit.
        assert_eq!(sched.peek_next(t0 + Duration::minutes(10), 2, false), None);

        // Once the first run falls outside the 1-hour rolling window, PR #3 starts.
        assert_eq!(
            sched.peek_next(t0 + Duration::minutes(61), 2, false),
            Some(pr(3))
        );

        // A newer commit (`updated_at` advanced) re-enqueues PR #1.
        assert!(sched.enqueue_if_new(pr(1), t0 + Duration::minutes(70)));
    }
}
