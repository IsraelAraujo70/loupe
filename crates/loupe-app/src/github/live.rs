//! Keeping a page's GitHub data fresh on its own. A page asks GitHub a
//! cheap question (did anything change?) every [`WATCHED`] while you look
//! at it and every [`UNWATCHED`] while its window is behind another or it
//! sits in a background tab; it loads everything again only when the answer
//! says something changed, or after [`RELOAD_WATCHED`] (or
//! [`RELOAD_UNWATCHED`]) for what the question cannot see. Nothing is asked
//! while a rate limit runs low.
//!
//! [`changed`] says what a reload brought, and [`Marks`] keeps those rows
//! pointed out until you have looked at them for [`SEEN_FOR`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use loupe_github::{PrRef, PrSummary, RateLimits};

/// How often a page looks at the clock to decide.
pub const TICK: Duration = Duration::from_secs(2);
/// Between two questions while the page is on screen in the active window.
pub const WATCHED: Duration = Duration::from_secs(30);
/// Between two questions otherwise.
pub const UNWATCHED: Duration = Duration::from_secs(180);
/// The longest a watched page goes without a full reload.
pub const RELOAD_WATCHED: Duration = Duration::from_secs(10 * 60);
/// The longest an unwatched page goes without a full reload.
pub const RELOAD_UNWATCHED: Duration = Duration::from_secs(30 * 60);
/// How long a row stays pointed out once the page is on screen.
pub const SEEN_FOR: Duration = Duration::from_secs(10);

/// What a page does on a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The last answer is recent enough.
    Wait,
    /// Ask whether anything changed.
    Ask,
    /// Load everything again.
    Reload,
    /// A rate limit is low: nothing until it resets.
    Paused(DateTime<Utc>),
}

/// When asking on its own may start again: the latest reset among the
/// buckets running low (under a tenth left), if any.
pub fn paused_until(rate: RateLimits, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    [rate.core, rate.graphql]
        .into_iter()
        .flatten()
        .filter(|limit| limit.is_low() && limit.reset > now)
        .map(|limit| limit.reset)
        .max()
}

/// When a page last asked and last reloaded, and when GitHub last
/// confirmed what it shows.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    asked_at: Instant,
    reloaded_at: Instant,
    /// The last reload or answer that found nothing new; `None` until one.
    pub confirmed_at: Option<DateTime<Local>>,
    /// Set while asking waits for a low rate limit to reset.
    pub paused_until: Option<DateTime<Utc>>,
}

impl Pace {
    /// A page that has just loaded (or started loading).
    pub fn new(now: Instant) -> Self {
        Self {
            asked_at: now,
            reloaded_at: now,
            confirmed_at: None,
            paused_until: None,
        }
    }

    /// What to do now, given whether the page is watched.
    pub fn step(&self, watched: bool, now: Instant, rate: RateLimits, utc: DateTime<Utc>) -> Step {
        if let Some(until) = paused_until(rate, utc) {
            return Step::Paused(until);
        }
        let (ask, reload) = if watched {
            (WATCHED, RELOAD_WATCHED)
        } else {
            (UNWATCHED, RELOAD_UNWATCHED)
        };
        if now.duration_since(self.reloaded_at) >= reload {
            Step::Reload
        } else if now.duration_since(self.asked_at) >= ask {
            Step::Ask
        } else {
            Step::Wait
        }
    }

    /// How long since the last reload started.
    pub fn since_reload(&self, now: Instant) -> Duration {
        now.duration_since(self.reloaded_at)
    }

    pub fn asked(&mut self, now: Instant) {
        self.asked_at = now;
    }

    /// A reload started: it also counts as an answer.
    pub fn reloaded(&mut self, now: Instant) {
        self.asked_at = now;
        self.reloaded_at = now;
    }

    /// The line beside Refresh: why asking stopped, or when GitHub last
    /// confirmed the page.
    pub fn label(&self) -> Option<String> {
        if let Some(until) = self.paused_until {
            return Some(format!(
                "Auto-refresh paused by the rate limit until {}",
                until.with_timezone(&Local).format("%H:%M")
            ));
        }
        self.confirmed_at
            .map(|at| format!("Updated {}", at.format("%H:%M")))
    }
}

/// How a pull request differs from the copy before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// It was not in the list.
    Entered,
    /// Something it shows changed: checks, reviews, title, updates.
    Updated,
}

/// The pull requests of `after` that are new or differ from `before`, in
/// `after`'s order, each once. A pull request listed twice (two searches)
/// counts once.
pub fn changed<'a>(
    before: impl IntoIterator<Item = &'a PrSummary>,
    after: impl IntoIterator<Item = &'a PrSummary>,
) -> Vec<(PrRef, Change)> {
    let before: HashMap<&PrRef, &PrSummary> = before
        .into_iter()
        .map(|summary| (&summary.pr, summary))
        .collect();
    let mut out: Vec<(PrRef, Change)> = Vec::new();
    for summary in after {
        if out.iter().any(|(pr, _)| *pr == summary.pr) {
            continue;
        }
        match before.get(&summary.pr) {
            None => out.push((summary.pr.clone(), Change::Entered)),
            Some(old) if *old != summary => out.push((summary.pr.clone(), Change::Updated)),
            Some(_) => {}
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
struct Mark {
    change: Change,
    /// When the page first showed it on screen.
    seen_since: Option<Instant>,
}

/// The rows to point out: what came or changed in a refresh, until the page
/// has been on screen for [`SEEN_FOR`] after it.
#[derive(Clone, Debug, Default)]
pub struct Marks(HashMap<PrRef, Mark>);

impl Marks {
    /// Points out `changes`. A row already pointed out keeps `Entered`, and
    /// starts its time on screen over.
    pub fn add(&mut self, changes: Vec<(PrRef, Change)>) {
        for (pr, change) in changes {
            let change = match self.0.get(&pr) {
                Some(mark) if mark.change == Change::Entered => Change::Entered,
                _ => change,
            };
            self.0.insert(
                pr,
                Mark {
                    change,
                    seen_since: None,
                },
            );
        }
    }

    pub fn get(&self, pr: &PrRef) -> Option<Change> {
        self.0.get(pr).map(|mark| mark.change)
    }

    /// The page is on screen at `now`: starts the time of the rows not yet
    /// seen and drops those seen for [`SEEN_FOR`]. Whether any went away.
    pub fn see(&mut self, now: Instant) -> bool {
        let before = self.0.len();
        self.0.retain(|_, mark| {
            let since = *mark.seen_since.get_or_insert(now);
            now.duration_since(since) < SEEN_FOR
        });
        self.0.len() != before
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use chrono::{DateTime, Utc};
    use loupe_github::{
        ChecksState, Mergeable, PrRef, PrState, PrSummary, RateLimit, RateLimits, RepoRef,
    };

    use super::{
        Change, Marks, Pace, RELOAD_UNWATCHED, RELOAD_WATCHED, SEEN_FOR, Step, UNWATCHED, WATCHED,
        changed,
    };

    fn summary(number: u64) -> PrSummary {
        PrSummary {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new("acme", "app"), number),
            title: format!("Change {number}"),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            author: None,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
            updated_at: DateTime::from_timestamp(0, 0).unwrap(),
            comment_count: 0,
            additions: 0,
            deletions: 0,
            checks: Some(ChecksState::Pending),
            review_decision: None,
            head_ref: String::new(),
            base_ref: String::new(),
            cross_repository: false,
            stack: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: Mergeable::Unknown,
            review_requests: Vec::new(),
            reviewed_by: Vec::new(),
        }
    }

    fn pr(number: u64) -> PrRef {
        PrRef::new(RepoRef::new("acme", "app"), number)
    }

    #[test]
    fn a_reload_reports_what_entered_and_what_changed_once_each() {
        let before = [summary(1), summary(2), summary(3)];
        let mut passed = summary(2);
        passed.checks = Some(ChecksState::Passing);
        let after = [summary(4), passed.clone(), summary(1), passed];
        assert_eq!(
            changed(&before, &after),
            [(pr(4), Change::Entered), (pr(2), Change::Updated)]
        );
        assert!(changed(&before, &before).is_empty(), "nothing new");
    }

    #[test]
    fn a_mark_stays_until_the_page_was_on_screen_for_a_while() {
        let start = Instant::now();
        let mut marks = Marks::default();
        marks.add(vec![(pr(1), Change::Entered), (pr(2), Change::Updated)]);
        assert_eq!(marks.get(&pr(1)), Some(Change::Entered));
        assert!(!marks.see(start), "the first look starts the clock");
        assert!(!marks.see(start + SEEN_FOR / 2));
        marks.add(vec![(pr(1), Change::Updated)]);
        assert_eq!(marks.get(&pr(1)), Some(Change::Entered), "still new");
        assert!(marks.see(start + SEEN_FOR), "pr 2 was seen long enough");
        assert_eq!(marks.get(&pr(2)), None);
        assert_eq!(
            marks.get(&pr(1)),
            Some(Change::Entered),
            "pr 1 changed again, so its time on screen started over"
        );
        assert!(!marks.see(start + SEEN_FOR + SEEN_FOR - Duration::from_secs(1)));
        assert!(marks.see(start + SEEN_FOR + SEEN_FOR));
        assert_eq!(marks.get(&pr(1)), None);
    }

    #[test]
    fn asks_often_while_watched_seldom_otherwise_and_reloads_now_and_then() {
        let start = Instant::now();
        let utc = DateTime::from_timestamp(1_000, 0).unwrap();
        let pace = Pace::new(start);
        let none = RateLimits::default();
        assert_eq!(pace.step(true, start, none, utc), Step::Wait);
        assert_eq!(pace.step(true, start + WATCHED, none, utc), Step::Ask);
        assert_eq!(pace.step(false, start + WATCHED, none, utc), Step::Wait);
        assert_eq!(pace.step(false, start + UNWATCHED, none, utc), Step::Ask);
        assert_eq!(
            pace.step(true, start + RELOAD_WATCHED, none, utc),
            Step::Reload
        );
        assert_eq!(
            pace.step(false, start + RELOAD_WATCHED, none, utc),
            Step::Ask
        );
        assert_eq!(
            pace.step(false, start + RELOAD_UNWATCHED, none, utc),
            Step::Reload
        );
        let mut asked = pace;
        asked.asked(start + WATCHED);
        assert_eq!(
            asked.step(true, start + WATCHED + WATCHED / 2, none, utc),
            Step::Wait
        );
    }

    #[test]
    fn a_low_rate_limit_pauses_until_it_resets() {
        let start = Instant::now();
        let now: DateTime<Utc> = DateTime::from_timestamp(1_000, 0).unwrap();
        let reset = DateTime::from_timestamp(2_000, 0).unwrap();
        let low = RateLimits {
            core: None,
            graphql: Some(RateLimit {
                limit: 5000,
                remaining: 400,
                reset,
            }),
        };
        let pace = Pace::new(start);
        assert_eq!(
            pace.step(true, start + RELOAD_WATCHED, low, now),
            Step::Paused(reset)
        );
        let after_reset = DateTime::from_timestamp(2_001, 0).unwrap();
        assert_eq!(
            pace.step(true, start + WATCHED, low, after_reset),
            Step::Ask
        );
        let mut paused = pace;
        paused.paused_until = Some(reset);
        assert!(
            paused.label().is_some_and(
                |label| label.starts_with("Auto-refresh paused by the rate limit until")
            )
        );
    }
}
