//! The bell of a saved Reviews view: with it on, a pull request that comes
//! into the view (a new one, or one that starts to match its filters) raises
//! a system notification, and the view's tab counts those not seen yet.
//!
//! One watcher follows every view with the bell on, whether or not a
//! Reviews page shows it, at the pace of a page behind another window
//! ([`live::UNWATCHED`]): the one-point question of [`live`] every 3 min,
//! the rows it names (one more point), and a full load every 30 min or when
//! a bell is turned on or a watched view's filters change. A view's list is
//! its All tab ([`tabs_shown`]); what entered since the last landing comes
//! from [`live::changed`]. The first list after a bell is turned on (or the
//! app starts) is the baseline and announces nothing.
//!
//! What each bell announced, and what is not seen yet, survives restarts in
//! `reviews-bells.json` in the data directory.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, SharedString, SystemNotification, Task,
};
use loupe_github::reviews::{self, Clause, Found, ReviewTab, SavedView};
use loupe_github::{Changes, ChangesQuery, Inbox, NodeId, Page, PrRef, PrStamp, PrSummary};
use serde::{Deserialize, Serialize};

use super::{CHANGED_SEARCHES, OVERFLOW_RELOAD_GAP, SINCE_MARGIN, tabs_shown, views_store};
use crate::github::common;
use crate::github::live::{self, Change, Pace, Step};
use crate::store::Settings;

/// How many announced pull requests a bell remembers, so the file stays
/// small; the oldest go first.
const NOTIFIED_KEPT: usize = 500;

/// The shortest time between two full loads a bell turned on or a failed
/// load asks for.
const RETRY: Duration = Duration::from_secs(30);

/// What one view's bell remembers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bell {
    /// Every pull request it announced, oldest first.
    notified: Vec<PrRef>,
    /// Those announced and not seen yet: the view was not on screen since.
    unseen: Vec<PrRef>,
}

impl Bell {
    /// The rows of `after`, the view's list now, to announce: those that
    /// are not in `before`, its list at the last landing, and that this bell
    /// never announced. They count as announced and unseen from then on.
    pub fn ring<'a>(&mut self, before: &[PrSummary], after: &'a [PrSummary]) -> Vec<&'a PrSummary> {
        let entered: Vec<PrRef> = live::changed(before, after)
            .into_iter()
            .filter(|(pr, change)| *change == Change::Entered && !self.notified.contains(pr))
            .map(|(pr, _)| pr)
            .collect();
        self.notified.extend(entered.iter().cloned());
        let over = self.notified.len().saturating_sub(NOTIFIED_KEPT);
        self.notified.drain(..over);
        self.unseen.extend(entered.iter().cloned());
        after
            .iter()
            .filter(|row| entered.contains(&row.pr))
            .collect()
    }

    pub fn unseen(&self) -> usize {
        self.unseen.len()
    }

    /// The view is on screen: nothing is unseen. Whether anything was.
    pub fn see(&mut self) -> bool {
        let had = !self.unseen.is_empty();
        self.unseen.clear();
        had
    }
}

/// Every view's bell, by view id, as kept on disk.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct BellsFile {
    bells: BTreeMap<String, Bell>,
}

fn path() -> PathBuf {
    crate::store::data_dir().join("reviews-bells.json")
}

impl BellsFile {
    /// The file on disk; a missing or unreadable one is empty.
    fn load() -> Self {
        std::fs::read(path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let written = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(path(), bytes));
        if let Err(error) = written {
            eprintln!("loupe: could not save the views' bells: {error}");
        }
    }
}

/// What a watched view lists: its filters and the "All open" search it
/// takes in, if any.
#[derive(Clone, Debug, PartialEq)]
struct Scope {
    filters: Vec<Clause>,
    all_open: Option<String>,
}

impl Scope {
    fn of(view: &SavedView) -> Self {
        Self {
            filters: view.filters.clone(),
            all_open: view
                .all_open
                .unwrap_or(true)
                .then(|| reviews::all_open_query(&view.filters))
                .flatten(),
        }
    }
}

/// A watched view's list at the last landing, and the scope it was read
/// under: a list read under other filters is no "before".
struct Listed {
    scope: Scope,
    rows: Vec<PrSummary>,
}

/// The inbox's three searches and each watched "All open" search, as last
/// loaded and then patched by the questions.
struct Loaded {
    inbox: Inbox,
    all_open: HashMap<String, Page<PrSummary>>,
    /// When the load went out; the questions ask about changes since a
    /// little before it.
    started: DateTime<Utc>,
}

struct Watcher {
    file: BellsFile,
    data: Option<Loaded>,
    lists: HashMap<String, Listed>,
    pace: Pace,
    last_reload: Option<Instant>,
    /// Pull requests the questions found that no list shows (one that
    /// closed, one outside every view) and were read already.
    handled: HashSet<PrStamp>,
    /// Which searches of the question found each row read since the load.
    found: HashMap<NodeId, Vec<bool>>,
    /// The load or question in flight.
    work: Option<Task<()>>,
    _ticker: Task<()>,
}

struct Bells(Entity<Watcher>);

impl Global for Bells {}

/// Starts the watcher, and opens the pull request of a notification that
/// gets clicked.
pub fn start(cx: &mut App) {
    let watcher = cx.new(|cx| Watcher {
        file: BellsFile::load(),
        data: None,
        lists: HashMap::new(),
        pace: Pace::new(Instant::now()),
        last_reload: None,
        handled: HashSet::new(),
        found: HashMap::new(),
        work: None,
        _ticker: cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(live::TICK).await;
                if this
                    .update(cx, |watcher: &mut Watcher, cx| watcher.tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        }),
    });
    cx.set_global(Bells(watcher));
    cx.on_system_notification_response(|response, cx| {
        crate::shell::open_system_urls(vec![response.tag.to_string()], cx);
    });
}

/// How many pull requests view `id` announced that were not seen yet.
pub fn unseen(id: &str, cx: &App) -> usize {
    cx.try_global::<Bells>()
        .and_then(|bells| bells.0.read(cx).file.bells.get(id).map(Bell::unseen))
        .unwrap_or(0)
}

/// View `id` is on screen: what it announced is seen.
pub fn see(id: &str, cx: &mut App) {
    let Some(watcher) = cx.try_global::<Bells>().map(|bells| bells.0.clone()) else {
        return;
    };
    watcher.update(cx, |watcher, cx| {
        if watcher.file.bells.get_mut(id).is_some_and(Bell::see) {
            watcher.file.save();
            cx.refresh_windows();
        }
    });
}

/// Turns view `id`'s bell on or off. On, the watcher loads the view now
/// and announces only what comes after; off, nothing is unseen anymore.
pub fn set(id: &str, on: bool, cx: &mut App) -> Result<(), SharedString> {
    views_store::update(cx, |file| file.set_notify(id, on, Utc::now()))?;
    let Some(watcher) = cx.try_global::<Bells>().map(|bells| bells.0.clone()) else {
        return Ok(());
    };
    watcher.update(cx, |watcher, cx| {
        watcher.lists.remove(id);
        if !on && watcher.file.bells.get_mut(id).is_some_and(Bell::see) {
            watcher.file.save();
        }
        watcher.tick(cx);
    });
    Ok(())
}

/// The saved views with the bell on.
fn watched(cx: &mut App) -> Vec<SavedView> {
    views_store::snapshot(cx)
        .views
        .into_iter()
        .filter(|view| view.notify)
        .collect()
}

/// Which searches of a question found pull request `id`, in its order.
fn found_in(changes: &Changes, id: &str) -> Vec<bool> {
    (0..changes.searches.len())
        .map(|at| changes.found(at, id))
        .collect()
}

/// What the system shows for `pr`, which came into view `view`; its tag is
/// the pull request's link, which a click opens.
fn notification(pr: &PrSummary, view: &str) -> SystemNotification {
    SystemNotification {
        tag: pr.url.clone().into(),
        title: pr.title.clone().into(),
        body: format!("{} · {view}", pr.pr).into(),
        actions: Vec::new(),
    }
}

impl Watcher {
    /// Every [`live::TICK`]: loads everything when a watched view has no
    /// list under its current filters, else asks or reloads when
    /// [`Pace::step`] says so for a page off screen.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.work.is_some() {
            return;
        }
        let views = watched(cx);
        self.lists
            .retain(|id, _| views.iter().any(|view| view.id == *id));
        if views.is_empty() {
            self.data = None;
            return;
        }
        if common::client(cx).is_none() {
            return;
        }
        let now = Instant::now();
        let listed = self.data.is_some()
            && views.iter().all(|view| {
                self.lists
                    .get(&view.id)
                    .is_some_and(|listed| listed.scope == Scope::of(view))
            });
        if !listed {
            if self
                .last_reload
                .is_none_or(|at| now.duration_since(at) >= RETRY)
            {
                self.reload(&views, cx);
            }
            return;
        }
        let rate = common::client(cx)
            .map(|github| github.rate_limits())
            .unwrap_or_default();
        match self.pace.step(false, now, rate, Utc::now()) {
            Step::Ask => self.ask(&views, cx),
            Step::Reload => self.reload(&views, cx),
            Step::Wait | Step::Paused(_) => {}
        }
    }

    /// Loads the inbox and every watched "All open" search, then rings.
    fn reload(&mut self, views: &[SavedView], cx: &mut Context<Self>) {
        let mut queries: Vec<String> = views
            .iter()
            .filter_map(|view| Scope::of(view).all_open)
            .collect();
        queries.sort();
        queries.dedup();
        let now = Instant::now();
        self.pace.reloaded(now);
        self.last_reload = Some(now);
        let started = Utc::now();
        self.work = Some(common::run(
            cx,
            move |gh| {
                let inbox = gh.inbox()?;
                let mut all_open = HashMap::new();
                for query in queries {
                    let page = gh.search_all_pull_requests(&query)?;
                    all_open.insert(query, page);
                }
                Ok(Loaded {
                    inbox,
                    all_open,
                    started,
                })
            },
            |watcher: &mut Watcher, result, cx| {
                watcher.work = None;
                // A failed load keeps the last one: the question still
                // looks from it, so no change goes unseen.
                if let Ok(loaded) = result {
                    watcher
                        .handled
                        .retain(|stamp| stamp.updated_at >= loaded.started - SINCE_MARGIN);
                    watcher.found.clear();
                    watcher.data = Some(loaded);
                    watcher.ring(cx);
                }
            },
        ));
    }

    /// Asks GitHub which pull requests changed since a little before the
    /// last load, among those that involve you, ask for your review, or
    /// sit in a watched view's repositories; reads the rows of those not
    /// known as they are, and puts them where the lists have them now.
    fn ask(&mut self, views: &[SavedView], cx: &mut Context<Self>) {
        let Some(data) = &self.data else {
            return;
        };
        let mut scopes: Vec<(String, String)> = views
            .iter()
            .filter_map(|view| {
                let query = Scope::of(view).all_open?;
                Some((query, reviews::all_open_changes_query(&view.filters)?))
            })
            .collect();
        scopes.sort();
        scopes.dedup();
        let mut searches: Vec<String> = CHANGED_SEARCHES.map(str::to_owned).to_vec();
        searches.extend(scopes.iter().map(|(_, changes)| changes.clone()));
        let pages = [
            &data.inbox.review_requested,
            &data.inbox.authored,
            &data.inbox.involved,
        ]
        .into_iter()
        .chain(data.all_open.values());
        let known: HashSet<PrStamp> = pages
            .flat_map(|page| page.items.iter())
            .map(|row| PrStamp {
                id: row.id.clone(),
                updated_at: row.updated_at,
            })
            .chain(self.handled.iter().cloned())
            .collect();
        let recorded = self.found.clone();
        let query = ChangesQuery {
            since: data.started - SINCE_MARGIN,
            searches,
            pending: Vec::new(),
        };
        self.pace.asked(Instant::now());
        let queries: Vec<String> = scopes.into_iter().map(|(query, _)| query).collect();
        self.work = Some(common::run(
            cx,
            move |gh| {
                let changes = gh.changes_since(&query)?;
                // A row read before goes again when search now finds it
                // elsewhere: the search index can trail the update.
                let unknown: Vec<NodeId> = changes
                    .updated()
                    .into_iter()
                    .filter(|stamp| {
                        !known.contains(*stamp)
                            || recorded
                                .get(&stamp.id)
                                .is_some_and(|found| *found != found_in(&changes, &stamp.id))
                    })
                    .map(|stamp| stamp.id.clone())
                    .collect();
                let rows = if changes.complete && !unknown.is_empty() {
                    gh.pull_request_rows(&unknown)?
                } else {
                    Vec::new()
                };
                Ok((changes, rows))
            },
            move |watcher: &mut Watcher, result, cx| {
                watcher.work = None;
                if let Ok((changes, rows)) = result {
                    watcher.land(changes, rows, &queries, cx);
                }
            },
        ));
    }

    /// Lands an answer of [`Self::ask`]; `queries` are the "All open"
    /// searches whose changes it asked about, in its order after the two
    /// [`CHANGED_SEARCHES`].
    fn land(
        &mut self,
        changes: Changes,
        rows: Vec<PrSummary>,
        queries: &[String],
        cx: &mut Context<Self>,
    ) {
        let stamps = changes.updated().into_iter().cloned();
        if !changes.complete {
            // More changed than one question reads: a full load, at most
            // every OVERFLOW_RELOAD_GAP.
            if self.pace.since_reload(Instant::now()) >= OVERFLOW_RELOAD_GAP {
                self.handled.extend(stamps);
                let views = watched(cx);
                self.reload(&views, cx);
            }
            return;
        }
        let Some(data) = self.data.as_mut() else {
            return;
        };
        for row in rows {
            let found = found_in(&changes, &row.id);
            let at = |search: usize| found.get(search).copied().unwrap_or(false);
            let base = Found {
                involves: at(0),
                requested: at(1),
                all_open: false,
            };
            reviews::patch_lists(&mut data.inbox, None, vec![(row.clone(), base)]);
            for (search, query) in queries.iter().enumerate() {
                if let Some(page) = data.all_open.get_mut(query) {
                    let found = Found {
                        all_open: at(CHANGED_SEARCHES.len() + search),
                        ..base
                    };
                    reviews::patch_lists(&mut data.inbox, Some(page), vec![(row.clone(), found)]);
                }
            }
            self.found.insert(row.id.clone(), found);
        }
        self.handled.extend(stamps);
        self.ring(cx);
    }

    /// Reads each watched view's list from the data, announces what came
    /// into it since its last list, and keeps the new list.
    fn ring(&mut self, cx: &mut Context<Self>) {
        let Some(data) = &self.data else {
            return;
        };
        super::auto_guide::observe_inbox(&data.inbox, &data.all_open, cx);
        let views = watched(cx);
        let mut notifications: Vec<SystemNotification> = Vec::new();
        for view in &views {
            let scope = Scope::of(view);
            let page = match &scope.all_open {
                Some(query) => match data.all_open.get(query) {
                    Some(page) => Some(page),
                    // Loaded before the view took in this search: the next
                    // tick loads it.
                    None => continue,
                },
                None => None,
            };
            let after: Vec<PrSummary> = tabs_shown(&data.inbox, page, &scope.filters)
                .into_iter()
                .find(|(tab, _)| *tab == ReviewTab::All)
                .map(|(_, shown)| shown.items.into_iter().cloned().collect())
                .unwrap_or_default();
            if let Some(before) = self.lists.get(&view.id).filter(|l| l.scope == scope) {
                let bell = self.file.bells.entry(view.id.clone()).or_default();
                notifications.extend(
                    bell.ring(&before.rows, &after)
                        .into_iter()
                        .map(|row| notification(row, &view.name)),
                );
            }
            self.lists
                .insert(view.id.clone(), Listed { scope, rows: after });
        }
        // Bells of views that are gone go with them.
        if views_store::readable(cx) {
            let all = views_store::snapshot(cx);
            self.file.bells.retain(|id, _| all.get(id).is_some());
        }
        if notifications.is_empty() {
            return;
        }
        self.file.save();
        cx.refresh_windows();
        if !Settings::get(cx).mute_view_notifications {
            for notification in notifications {
                cx.show_system_notification(notification);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use loupe_github::{ChecksState, Inbox, PrRef, PrSummary, RepoRef};

    use super::super::tests::{bounds, click, open, page};
    use super::{Bell, Bells, Loaded, Watcher, start, unseen};
    use crate::github::inbox::InboxView;

    /// Starts the watcher beside a Reviews page and saves view "Web" (no
    /// filters) with its bell on; returns its id.
    fn watched_view(
        view: &Entity<InboxView>,
        cx: &mut VisualTestContext,
    ) -> (String, Entity<Watcher>) {
        cx.update(|_, cx| {
            cx.set_app_identity("dev.loupe.app", "Loupe");
            start(cx);
        });
        view.update(cx, |view, cx| view.save_as_new("Web", cx));
        let id = view
            .read_with(cx, |view, _| view.page.view_id.clone())
            .expect("the new view is open");
        view.update(cx, |view, cx| view.switch_view(None, cx));
        cx.update(|_, cx| super::set(&id, true, cx)).expect("saved");
        let watcher = cx.update(|_, cx| cx.global::<Bells>().0.clone());
        (id, watcher)
    }

    /// GitHub answers that these pull requests of yours are open.
    fn lands(watcher: &Entity<Watcher>, numbers: &[u64], cx: &mut VisualTestContext) {
        let items: Vec<PrSummary> = numbers
            .iter()
            .map(|&number| PrSummary {
                title: format!("Change {number}"),
                url: format!("https://github.com/acme/repo/pull/{number}"),
                ..super::super::tests::pr("acme", number)
            })
            .collect();
        let total = items.len() as u32;
        let inbox = Inbox {
            review_requested: page(Vec::new(), 0, false),
            authored: page(items, total, false),
            involved: page(Vec::new(), 0, false),
            viewer_login: "me".into(),
        };
        watcher.update(cx, |watcher, cx| {
            watcher.data = Some(Loaded {
                inbox,
                all_open: HashMap::new(),
                started: Utc::now(),
            });
            watcher.ring(cx);
        });
    }

    fn titles(cx: &mut VisualTestContext) -> Vec<String> {
        cx.shown_system_notifications()
            .into_iter()
            .map(|shown| format!("{} | {}", shown.title, shown.body))
            .collect()
    }

    #[gpui_kit::test]
    fn a_pull_request_that_comes_into_a_watched_view_raises_one_notification(
        cx: &mut TestAppContext,
    ) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        let (id, watcher) = watched_view(&view, cx);
        lands(&watcher, &[1], cx);
        assert!(titles(cx).is_empty(), "the first list is the baseline");
        lands(&watcher, &[2, 1], cx);
        assert_eq!(titles(cx), ["Change 2 | acme/repo#2 · Web"]);
        let tag = cx.shown_system_notifications()[0].tag.clone();
        assert_eq!(tag.as_ref(), "https://github.com/acme/repo/pull/2");
        lands(&watcher, &[1], cx);
        lands(&watcher, &[2, 1], cx);
        assert_eq!(
            titles(cx).len(),
            1,
            "2 left and came back: no second notification"
        );

        cx.update(|_, cx| crate::store::update_settings(cx, |s| s.mute_view_notifications = true));
        lands(&watcher, &[3, 2, 1], cx);
        assert_eq!(titles(cx).len(), 1, "muted");
        assert_eq!(cx.update(|_, cx| unseen(&id, cx)), 2, "muted still counts");
        cx.update(|_, cx| crate::store::update_settings(cx, |s| s.mute_view_notifications = false));

        cx.update(|_, cx| super::set(&id, false, cx))
            .expect("saved");
        lands(&watcher, &[4, 3, 2, 1], cx);
        assert_eq!(titles(cx).len(), 1, "the bell is off");
        assert_eq!(cx.update(|_, cx| unseen(&id, cx)), 0);
    }

    #[gpui_kit::test]
    fn the_tab_counts_what_is_new_until_the_view_opens(cx: &mut TestAppContext) {
        let (mut cx, view) = open(cx);
        let cx = &mut cx;
        let (id, watcher) = watched_view(&view, cx);
        lands(&watcher, &[1], cx);
        assert!(bounds(cx, "reviews-view-unseen-0").is_none());
        lands(&watcher, &[3, 2, 1], cx);
        assert_eq!(cx.update(|_, cx| unseen(&id, cx)), 2);
        assert!(
            bounds(cx, "reviews-view-unseen-0").is_some(),
            "the count shows"
        );
        click(cx, "reviews-view-tab-0");
        assert_eq!(cx.update(|_, cx| unseen(&id, cx)), 0);
        assert!(
            bounds(cx, "reviews-view-unseen-0").is_none(),
            "opening the view clears it"
        );
    }

    fn row(number: u64) -> PrSummary {
        PrSummary {
            pr: PrRef::new(RepoRef::new("acme", "web"), number),
            ..super::super::tests::pr("acme", number)
        }
    }

    fn rows(numbers: &[u64]) -> Vec<PrSummary> {
        numbers.iter().copied().map(row).collect()
    }

    fn numbers(rang: Vec<&PrSummary>) -> Vec<u64> {
        rang.into_iter().map(|row| row.pr.number).collect()
    }

    #[test]
    fn a_bell_announces_only_what_entered_and_never_the_same_pull_request_twice() {
        let mut bell = Bell::default();
        let mut changed = row(2);
        changed.checks = Some(ChecksState::Failing);
        let after = vec![row(4), row(1), changed, row(3)];
        assert_eq!(
            numbers(bell.ring(&rows(&[1, 2, 3]), &after)),
            [4],
            "a pull request that only changed is not new"
        );
        assert_eq!(bell.unseen(), 1);
        assert!(bell.ring(&after, &after).is_empty(), "nothing came");

        // 4 leaves the view (merged, or out of the filters) and comes back.
        let without = rows(&[1, 3]);
        assert!(bell.ring(&after, &without).is_empty());
        assert!(
            numbers(bell.ring(&without, &rows(&[4, 1, 3, 5]))) == [5],
            "4 was announced already"
        );
        assert_eq!(bell.unseen(), 2);
        assert!(bell.see());
        assert_eq!(bell.unseen(), 0);
        assert!(!bell.see());
    }
}
