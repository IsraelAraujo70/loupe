//! Tab groups kept in sync with a saved Reviews view ("Open as group <view>"
//! with "Keep in sync with the view"). Every [`SYNC_EVERY`], and whenever a
//! Reviews page loads or a view's filters are saved, each such group gets
//! the pull requests its view lists now on the tab it was opened from; the
//! shell opens the new ones and closes the ones that left
//! ([`shell::sync_group`]). A group whose view is gone stops syncing and
//! keeps its tabs. One sync runs at a time: a new one cancels the one in
//! flight, so an older answer never lands after a newer one.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, AppContext as _, Global, Task, Window};
use loupe_github::reviews::{self, ReviewTab, SavedView};
use loupe_github::{Inbox, Page, PrSummary};

use super::{ViewList, uses_all_open, view_rows, views_store};
use crate::services::{self, Services};
use crate::shell::{self, SyncedGroup};

/// How often the groups sync while no Reviews page reloads.
const SYNC_EVERY: Duration = Duration::from_secs(120);

struct GroupSync {
    window: AnyWindowHandle,
    _every: Task<()>,
    /// The sync in flight; a newer one replaces it, which cancels it.
    _now: Option<Task<()>>,
}

impl Global for GroupSync {}

/// Starts syncing the groups of `window`'s workspace.
pub fn start(window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let every = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(SYNC_EVERY).await;
            cx.update(|cx| now(None, cx));
        }
    });
    cx.set_global(GroupSync {
        window: handle,
        _every: every,
        _now: None,
    });
}

/// Syncs every group now; `inbox` is a Reviews page's fresh copy, which
/// spares asking GitHub for it again.
pub fn now(inbox: Option<Inbox>, cx: &mut App) {
    let Some(window) = cx.try_global::<GroupSync>().map(|sync| sync.window) else {
        return;
    };
    let task = run(window, inbox, cx);
    cx.global_mut::<GroupSync>()._now = Some(task);
}

/// The "All open" search a group on `tab` of `view` needs, if any; it
/// mirrors what the Reviews page shows for the view.
fn all_open_query(view: &SavedView, tab: ReviewTab) -> Option<String> {
    (uses_all_open(tab) && view.all_open.unwrap_or(true))
        .then(|| reviews::all_open_query(&view.filters))
        .flatten()
}

/// What a group on `tab` of `view` follows now, from `inbox` and the "All
/// open" `pages` that loaded; `None` when the search it needs failed, since
/// a list without it would close the tabs that came from it.
fn list_for(
    inbox: &Inbox,
    pages: &HashMap<String, Page<PrSummary>>,
    view: &SavedView,
    tab: ReviewTab,
) -> Option<ViewList> {
    let all_open = match all_open_query(view, tab) {
        Some(query) => Some(pages.get(&query)?),
        None => None,
    };
    Some(view_rows(inbox, all_open, &view.filters, tab))
}

fn run(window: AnyWindowHandle, inbox: Option<Inbox>, cx: &mut App) -> Task<()> {
    // Views read from a file that could not be used are not the user's:
    // their absence stops nothing.
    let readable = views_store::readable(cx);
    let views = views_store::snapshot(cx);
    let mut live: Vec<(SyncedGroup, SavedView)> = Vec::new();
    for group in shell::synced_groups(cx) {
        match views.get(&group.sync.view) {
            Some(view) => {
                let view = view.clone();
                live.push((group, view));
            }
            None if readable => shell::stop_syncing(group.group, cx),
            None => {}
        }
    }
    if live.is_empty() {
        return Task::ready(());
    }
    let Some(client) = Services::get(cx).github.clone() else {
        return Task::ready(());
    };
    let mut queries: Vec<String> = live
        .iter()
        .filter_map(|(group, view)| all_open_query(view, group.sync.tab))
        .collect();
    queries.sort();
    queries.dedup();
    let fetch = services::background(cx, move || {
        let inbox = match inbox {
            Some(inbox) => inbox,
            None => client.inbox().ok()?,
        };
        let pages: HashMap<String, Page<PrSummary>> = queries
            .into_iter()
            .filter_map(|query| {
                let page = client.search_all_pull_requests(&query).ok()?;
                Some((query, page))
            })
            .collect();
        Some((inbox, pages))
    });
    cx.spawn(async move |cx| {
        // A failed load syncs nothing: an empty list would close every tab.
        let Some((inbox, pages)) = fetch.await else {
            return;
        };
        cx.update_window(window, |_, window, cx| {
            for (group, view) in live {
                if let Some(list) = list_for(&inbox, &pages, &view, group.sync.tab) {
                    shell::sync_group(group.group, list.rows, list.complete, window, cx);
                }
            }
        })
        .ok();
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use loupe_github::reviews::{Clause, Field, Op, ReviewTab, SavedView};
    use loupe_github::{Inbox, Page, PrRef, PrSummary, RepoRef};

    use super::list_for;

    fn pr(number: u64) -> PrSummary {
        PrSummary {
            pr: PrRef::new(RepoRef::new("acme", "web"), number),
            ..super::super::tests::pr("acme", number)
        }
    }

    fn page(numbers: &[u64], total: u32, more: bool) -> Page<PrSummary> {
        Page {
            items: numbers.iter().copied().map(pr).collect(),
            total_count: total,
            next_cursor: more.then(|| "next".into()),
        }
    }

    fn view() -> SavedView {
        SavedView {
            id: "rv_1".into(),
            name: "Web".into(),
            filters: vec![Clause {
                field: Field::Repo,
                op: Op::In,
                values: vec!["acme/web".into()],
            }],
            all_open: None,
            sort: None,
            tab: None,
            collapsed: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn numbers(rows: &[PrRef]) -> Vec<u64> {
        rows.iter().map(|pr| pr.number).collect()
    }

    #[test]
    fn a_failed_all_open_search_syncs_nothing_and_a_partial_one_closes_nothing() {
        let inbox = Inbox {
            review_requested: page(&[], 0, false),
            authored: page(&[1], 1, false),
            involved: page(&[], 0, false),
            viewer_login: "me".into(),
        };
        let view = view();
        let query = loupe_github::reviews::all_open_query(&view.filters).unwrap();
        let none = HashMap::new();
        assert_eq!(
            list_for(&inbox, &none, &view, ReviewTab::All),
            None,
            "All needs the view's search"
        );
        let mine = list_for(&inbox, &none, &view, ReviewTab::Mine).unwrap();
        assert_eq!((numbers(&mine.rows), mine.complete), (vec![1], true));

        let whole = HashMap::from([(query.clone(), page(&[1, 2], 2, false))]);
        let all = list_for(&inbox, &whole, &view, ReviewTab::All).unwrap();
        assert_eq!((numbers(&all.rows), all.complete), (vec![1, 2], true));

        let part = HashMap::from([(query.clone(), page(&[1, 2], 180, true))]);
        let all = list_for(&inbox, &part, &view, ReviewTab::All).unwrap();
        assert_eq!(
            (numbers(&all.rows), all.complete),
            (vec![1, 2], false),
            "GitHub sent part of the search"
        );
    }

    #[test]
    fn a_whole_all_open_search_is_the_whole_of_all_whatever_the_dashboard_misses() {
        // Over 100 pull requests involve you across GitHub, so the
        // dashboard's search is cut; the view's own search is whole.
        let inbox = Inbox {
            review_requested: page(&[], 0, false),
            authored: page(&[1], 1, false),
            involved: page(&[7], 300, true),
            viewer_login: "me".into(),
        };
        let view = view();
        let query = loupe_github::reviews::all_open_query(&view.filters).unwrap();
        let whole = HashMap::from([(query, page(&[1, 2], 2, false))]);
        let all = list_for(&inbox, &whole, &view, ReviewTab::All).unwrap();
        assert!(all.complete);
        let involved = list_for(&inbox, &whole, &view, ReviewTab::Involved).unwrap();
        assert!(
            !involved.complete,
            "Involved is the dashboard's search alone"
        );

        let asked = Inbox {
            review_requested: page(&[], 150, true),
            ..inbox
        };
        let needs = list_for(&asked, &whole, &view, ReviewTab::NeedsReview).unwrap();
        assert!(
            !needs.complete,
            "Needs review reads the review requests, which are cut"
        );
    }
}
