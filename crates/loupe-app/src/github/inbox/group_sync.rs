//! Tab groups kept in sync with a saved Reviews view ("Open as group <view>"
//! with "Keep in sync with the view"). Every [`SYNC_EVERY`], and whenever a
//! Reviews page loads or a view's filters are saved, each such group gets
//! the pull requests its view lists now on the tab it was opened from; the
//! shell opens the new ones and closes the ones that left
//! ([`shell::sync_group`]). A group whose view is gone stops syncing and
//! keeps its tabs.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, AppContext as _, Global, Task, Window};
use loupe_github::reviews::{self, ReviewTab, SavedView};
use loupe_github::{Inbox, Page, PrSummary};

use super::{view_rows, views_store};
use crate::services::{self, Services};
use crate::shell::{self, SyncedGroup};

/// How often the groups sync while no Reviews page reloads.
const SYNC_EVERY: Duration = Duration::from_secs(120);

struct GroupSync {
    window: AnyWindowHandle,
    _every: Task<()>,
    /// The sync a Reviews load or a save asked for; a newer one replaces it.
    _now: Option<Task<()>>,
}

impl Global for GroupSync {}

/// Starts syncing the groups of `window`'s workspace.
pub fn start(window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let every = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(SYNC_EVERY).await;
            cx.update(|cx| run(handle, None, cx)).await;
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
    (tab == ReviewTab::All && view.all_open.unwrap_or(true))
        .then(|| reviews::all_open_query(&view.filters))
        .flatten()
}

fn run(window: AnyWindowHandle, inbox: Option<Inbox>, cx: &mut App) -> Task<()> {
    let views = views_store::snapshot(cx);
    let mut live: Vec<(SyncedGroup, SavedView)> = Vec::new();
    for group in shell::synced_groups(cx) {
        match views.get(&group.sync.view) {
            Some(view) => {
                let view = view.clone();
                live.push((group, view));
            }
            None => shell::stop_syncing(group.group, cx),
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
                let all_open = match all_open_query(&view, group.sync.tab) {
                    Some(query) => match pages.get(&query) {
                        Some(page) => Some(page),
                        None => continue,
                    },
                    None => None,
                };
                let list = view_rows(&inbox, all_open, &view.filters, group.sync.tab);
                shell::sync_group(group.group, list, window, cx);
            }
        })
        .ok();
    })
}
