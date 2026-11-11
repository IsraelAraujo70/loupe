//! Shared Linear state for every screen: the client, the async-state enum,
//! and a per-workspace cache of the team list and team catalogs
//! (states, members, labels, projects, cycles). The cache belongs to one
//! client: when `Services.linear` points at a different client, it starts
//! over.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use gpui_kit::{App, Global, SharedString, Task};
use loupe_linear::{IssueSummary, Linear, Team, TeamSummary};

use crate::services::{Accounts, Connection, LinearWorkspace, Services};
use crate::store::Settings;

/// What a view shows while it loads something.
#[derive(Clone, Debug)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

/// The last issue an edit in the app got back from Linear. A screen that
/// lists issues observes it and replaces its own copy, so a board shows a
/// state changed on the issue's page without a reload.
#[derive(Default)]
pub struct IssueChange {
    pub issue: Option<IssueSummary>,
    /// Linear returned this copy. False for a change shown before Linear
    /// answered (a pick from a menu or the palette), which lists show at
    /// once and an issue's page, with its own queue of edits, does not
    /// adopt.
    pub confirmed: bool,
}

impl Global for IssueChange {}

/// Tells every screen that lists issues that Linear now holds `issue`.
pub fn publish_issue(issue: IssueSummary, cx: &mut App) {
    cx.set_global(IssueChange {
        issue: Some(issue),
        confirmed: true,
    });
}

/// Shows `issue` on every screen that lists it before Linear answers.
pub fn publish_shown(issue: IssueSummary, cx: &mut App) {
    cx.set_global(IssueChange {
        issue: Some(issue),
        confirmed: false,
    });
}

/// The signed-in client, or `None` when Linear is not connected.
pub fn client(cx: &App) -> Option<Arc<Linear>> {
    crate::services::Services::get(cx).linear.clone()
}

/// The active workspace: the one whose client is `Services.linear`.
pub fn active_workspace(cx: &App) -> Option<LinearWorkspace> {
    let services = Services::get(cx);
    let active = services.linear.as_ref()?;
    services
        .linear_workspaces
        .iter()
        .find(|workspace| Arc::ptr_eq(&workspace.client, active))
        .cloned()
}

/// The workspace whose issues the pull requests of `owner/name` link to:
/// the one Settings maps the repository to, else the one it maps its owner
/// to, among the connected ones; else the active one.
pub fn workspace_for_repo(owner: &str, name: &str, cx: &App) -> Option<LinearWorkspace> {
    let services = Services::get(cx);
    Settings::get(cx)
        .workspaces_for_repo(owner, name)
        .into_iter()
        .find_map(|id| services.linear_workspace(id).cloned())
        .or_else(|| active_workspace(cx))
}

/// True when both name the same client, or both are `None`. Screens compare
/// the client they loaded with against the current one on every render, so
/// signing in, out, or into another workspace reloads them.
pub fn same_client(a: &Option<Arc<Linear>>, b: &Option<Arc<Linear>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// A short message for the user. Never includes the API key.
pub fn error_text(error: &loupe_linear::Error) -> SharedString {
    use loupe_linear::Error;
    match error {
        Error::Unauthorized => "Linear rejected the API key. Check it in Settings.".into(),
        Error::Transport(_) => "Could not reach Linear.".into(),
        Error::GraphQl(messages) => match messages.first() {
            Some(message) => format!("Linear: {message}").into(),
            None => "Linear: the request failed.".into(),
        },
        Error::Http { status, .. } => format!("Linear returned HTTP {status}.").into(),
        Error::Decode(_) => "Linear sent an unexpected answer.".into(),
    }
}

/// The key owner's user id, for "me" in filters. The session validates the
/// key with the viewer query before it sets `Services.linear`, so the id is
/// known whenever a client is.
pub fn me(cx: &App) -> Option<String> {
    match &Accounts::get(cx).linear {
        Connection::Connected(viewer) => Some(viewer.user.id.clone()),
        _ => None,
    }
}

/// Every team the key can see, sorted by name, cached after the first load.
pub fn teams(cx: &mut App) -> Task<Result<Arc<Vec<TeamSummary>>, SharedString>> {
    cached(
        cx,
        |cache| cache.teams.clone(),
        |linear| {
            let mut teams = linear.teams()?;
            teams.sort_by_cached_key(|team| (team.name.to_lowercase(), team.name.clone()));
            Ok(teams)
        },
        |cache, teams| cache.teams = Some(teams),
    )
}

/// The teams the key's owner is a member of, sorted by name, cached after
/// the first load.
pub fn my_teams(cx: &mut App) -> Task<Result<Arc<Vec<TeamSummary>>, SharedString>> {
    cached(
        cx,
        |cache| cache.my_teams.clone(),
        |linear| linear.my_teams(),
        |cache, teams| cache.my_teams = Some(teams),
    )
}

/// One team's catalog, cached after the first load.
pub fn team(team_id: String, cx: &mut App) -> Task<Result<Arc<Team>, SharedString>> {
    let read_id = team_id.clone();
    cached(
        cx,
        move |cache| cache.catalogs.get(&read_id).cloned(),
        move |linear| linear.team(&team_id),
        |cache, team| {
            cache.catalogs.insert(team.id.clone(), team);
        },
    )
}

/// One team's catalog if a screen already loaded it, without a request.
pub fn cached_team(team_id: &str, cx: &mut App) -> Option<Arc<Team>> {
    let linear = client(cx)?;
    cache_for(cx, &linear).catalogs.get(team_id).cloned()
}

/// Puts `team` in the cache of the connected client, as a screen that
/// loaded it would.
#[cfg(test)]
pub fn cache_team(team: Team, cx: &mut App) {
    let linear = client(cx).expect("a Linear client");
    cache_for(cx, &linear)
        .catalogs
        .insert(team.id.clone(), Arc::new(team));
}

/// What the screens share. `owner` is weak so that a dropped client's address
/// is never reused while the cache still remembers it.
#[derive(Default)]
struct Cache {
    owner: Option<Weak<Linear>>,
    teams: Option<Arc<Vec<TeamSummary>>>,
    my_teams: Option<Arc<Vec<TeamSummary>>>,
    catalogs: HashMap<String, Arc<Team>>,
}

impl Global for Cache {}

impl Cache {
    fn belongs_to(&self, client: &Arc<Linear>) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|owner| std::ptr::eq(Weak::as_ptr(owner), Arc::as_ptr(client)))
    }
}

/// The cache of `client`, emptied first if it belonged to another client.
fn cache_for<'a>(cx: &'a mut App, client: &Arc<Linear>) -> &'a mut Cache {
    let cache = cx.default_global::<Cache>();
    if !cache.belongs_to(client) {
        *cache = Cache {
            owner: Some(Arc::downgrade(client)),
            ..Cache::default()
        };
    }
    cache
}

/// Returns the cached value, or fetches it on a background thread and stores
/// it. A result that arrives after the client was replaced is returned but not
/// stored, so it never lands in the new client's cache.
fn cached<T: Send + 'static>(
    cx: &mut App,
    read: impl FnOnce(&Cache) -> Option<Arc<T>>,
    fetch: impl FnOnce(&Linear) -> loupe_linear::Result<T> + Send + 'static,
    store: impl FnOnce(&mut Cache, Arc<T>) + 'static,
) -> Task<Result<Arc<T>, SharedString>> {
    let Some(linear) = client(cx) else {
        return Task::ready(Err("Connect Linear in Settings".into()));
    };
    if let Some(hit) = read(cache_for(cx, &linear)) {
        return Task::ready(Ok(hit));
    }
    let worker = linear.clone();
    let task = crate::services::background(cx, move || fetch(&worker));
    cx.spawn(async move |cx| {
        let value = Arc::new(task.await.map_err(|error| error_text(&error))?);
        let stored = value.clone();
        cx.update(move |cx| {
            let current = client(cx).is_some_and(|current| Arc::ptr_eq(&current, &linear));
            if current {
                store(cache_for(cx, &linear), stored);
            }
        });
        Ok(value)
    })
}

#[cfg(test)]
mod tests {
    use super::{error_text, workspace_for_repo};
    use crate::services::{Accounts, LinearWorkspace, Services};
    use crate::store::{Settings, WorkspaceMapping};
    use gpui_kit::TestAppContext;
    use loupe_linear::Error;

    fn mapping(github: &str, workspace_id: &str) -> WorkspaceMapping {
        WorkspaceMapping {
            github: github.into(),
            workspace_id: workspace_id.into(),
            workspace_name: workspace_id.into(),
        }
    }

    #[gpui_kit::test]
    fn a_pull_request_reads_the_workspace_its_repository_is_mapped_to(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Settings {
                workspace_mappings: vec![
                    mapping("hooli-inc", "hooli"),
                    mapping("IsraelAraujo70", "initech"),
                    mapping("IsraelAraujo70/old", "gone"),
                ],
                linear_workspace: Some("hooli".into()),
                ..Settings::default()
            });
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            crate::settings::session::connect_for_test(
                vec![
                    ("lin_a".into(), LinearWorkspace::for_test("hooli", "w@x")),
                    ("lin_b".into(), LinearWorkspace::for_test("initech", "m@x")),
                ],
                cx,
            );
            let read = |owner: &str, name: &str, cx: &gpui_kit::App| {
                let workspace = workspace_for_repo(owner, name, cx).expect("a workspace");
                let client = Services::get(cx)
                    .linear_workspace(workspace.id())
                    .map(|known| known.client.clone())
                    .expect("connected");
                assert!(std::sync::Arc::ptr_eq(&workspace.client, &client));
                workspace.id().to_owned()
            };
            assert_eq!(read("IsraelAraujo70", "loupe", cx), "initech");
            assert_eq!(read("hooli-inc", "hooli-api", cx), "hooli");
            // The repository's own workspace is no longer connected: its
            // owner's.
            assert_eq!(read("IsraelAraujo70", "old", cx), "initech");
            // Not mapped: the active one.
            assert_eq!(read("acme", "web", cx), "hooli");
        });
    }

    #[test]
    fn each_error_has_a_short_message() {
        assert_eq!(
            error_text(&Error::Unauthorized),
            "Linear rejected the API key. Check it in Settings."
        );
        assert_eq!(
            error_text(&Error::Transport("dns".into())),
            "Could not reach Linear."
        );
        assert_eq!(
            error_text(&Error::GraphQl(vec!["Entity not found".into(), "x".into()])),
            "Linear: Entity not found"
        );
        assert_eq!(
            error_text(&Error::Http {
                status: 502,
                message: "bad gateway".into()
            }),
            "Linear returned HTTP 502."
        );
        assert_eq!(
            error_text(&Error::Decode("missing field".into())),
            "Linear sent an unexpected answer."
        );
    }

    #[test]
    fn details_of_the_failure_never_reach_the_user() {
        let leaky = Error::Transport("lin_api_secret".into());
        assert!(!error_text(&leaky).contains("lin_api_secret"));
        let leaky = Error::Http {
            status: 401,
            message: "lin_api_secret".into(),
        };
        assert!(!error_text(&leaky).contains("lin_api_secret"));
    }
}
