//! The only code that changes `Services` and `Accounts`. Each function updates
//! both globals together and refreshes windows; the shell's `Workspace`
//! observes `Services` and rebuilds its views when the GitHub client changes.

use std::sync::Arc;

use gpui_kit::{App, Global};
use loupe_github::{GitHub, Viewer};
use loupe_linear::Linear;
use loupe_t3::T3;

use crate::services::{self, Accounts, Connection, LinearWorkspace, Services};
use crate::settings::auth::{self, DiscoveredToken, TokenSource};
use crate::store::{self, Secret, Settings};

/// Called once at startup, after the `Settings`, `Services` and `Accounts`
/// globals exist. Sets `Accounts.github` to `Checking`, runs
/// `auth::discover_github_token` in the background, and connects or falls to
/// `Disconnected`. Does the same for Linear (stored key, then `LINEAR_API_KEY`)
/// and builds `Services.t3` from `Settings.t3_server_url` and `Secret::T3Token`.
/// Never blocks the UI thread: secrets are read in the background too.
pub fn start(cx: &mut App) {
    if std::env::var_os("LOUPE_OFFLINE").is_some() {
        if let Some(viewer) = crate::cache::get("offline/github-viewer") {
            connect_github(String::new(), viewer, cx);
        }
        let viewers: Vec<loupe_linear::Viewer> = crate::cache::get(OFFLINE_LINEAR)
            .or_else(|| crate::cache::get("offline/linear-viewer").map(|viewer| vec![viewer]))
            .unwrap_or_default();
        let keys = LinearKeys {
            connected: viewers
                .into_iter()
                .map(|viewer| (String::new(), viewer))
                .collect(),
            ..LinearKeys::default()
        };
        set_linear_keys(keys, cx);
        return;
    }
    let t3_url = Settings::get(cx).t3_server_url.clone();
    apply(cx, |_, accounts| accounts.linear = Connection::Checking);
    discover_github(cx);

    // Keychain reads can wait on an authorization prompt, so they stay off the
    // UI thread together with the network check.
    let stored = services::background(cx, || {
        let t3_token = store::load_secret(Secret::T3Token);
        let stored = stored_linear_keys();
        let from_env = std::env::var("LINEAR_API_KEY")
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty() && stored.is_empty());
        let linear: Vec<(String, Result<loupe_linear::Viewer, loupe_linear::Error>)> = stored
            .iter()
            .chain(from_env.iter())
            .map(|key| (key.clone(), auth::linear_account(key)))
            .collect();
        (t3_token, (linear, from_env))
    });
    cx.spawn(async move |cx| {
        let (t3_token, linear) = stored.await;
        cx.update(|cx| {
            // A save or clear while this ran changed the URL; theirs wins.
            if Services::get(cx).t3.is_none() && Settings::get(cx).t3_server_url == t3_url {
                let t3 = t3_client(t3_url.as_deref(), t3_token.as_deref());
                apply(cx, |services, _| services.t3 = t3);
            }
            let (checked, from_env) = linear;
            finish_linear_check(checked, from_env, cx);
        });
    })
    .detach();
}

/// Sets `Accounts.github` to `Checking` and looks for a working token in the
/// background. Startup runs it, and so does Retry on the sign-in screen after
/// GitHub could not be reached.
pub fn discover_github(cx: &mut App) {
    apply(cx, |_, accounts| accounts.github = Connection::Checking);
    let discovery = services::background(cx, auth::discover_github_token);
    cx.spawn(async move |cx| {
        let found = discovery.await;
        cx.update(|cx| finish_github_discovery(found, cx));
    })
    .detach();
}

/// Applies the discovery result unless the user signed in or out while it ran.
fn finish_github_discovery(found: Result<Option<DiscoveredToken>, String>, cx: &mut App) {
    if !matches!(Accounts::get(cx).github, Connection::Checking) {
        return;
    }
    match found {
        Ok(Some(DiscoveredToken {
            token,
            source,
            viewer,
        })) => {
            eprintln!(
                "loupe: github connected as {} via {}",
                viewer.login,
                source_label(source)
            );
            connect_github(token, viewer, cx);
        }
        Ok(None) => {
            eprintln!("loupe: github not connected");
            apply(cx, |_, accounts| accounts.github = Connection::Disconnected);
        }
        Err(message) => {
            eprintln!("loupe: github not reachable: {message}");
            apply(cx, |_, accounts| {
                accounts.github = Connection::Failed(format!("Could not reach GitHub: {message}"));
            });
        }
    }
}

/// Connects every stored key that Linear accepted, unless a key was
/// connected or disconnected in Settings while they were checked.
fn finish_linear_check(
    checked: Vec<(String, Result<loupe_linear::Viewer, loupe_linear::Error>)>,
    from_env: Option<String>,
    cx: &mut App,
) {
    if !matches!(Accounts::get(cx).linear, Connection::Checking) {
        return;
    }
    let mut keys = LinearKeys {
        from_env,
        ..LinearKeys::default()
    };
    for (key, result) in checked {
        match result {
            Ok(account) => {
                eprintln!(
                    "loupe: linear connected as {} in {}",
                    account.user.name, account.organization.name
                );
                keys.connect(key, account);
            }
            Err(error) => {
                eprintln!("loupe: a linear key did not connect: {error}");
                keys.failed.push((key, error.to_string()));
            }
        }
    }
    set_linear_keys(keys, cx);
}

fn source_label(source: TokenSource) -> &'static str {
    match source {
        TokenSource::Stored => "stored token",
        TokenSource::Environment => "GITHUB_TOKEN",
        TokenSource::GhCli => "gh-cli",
    }
}

/// Saves the token as `Secret::GitHubToken`, then connects GitHub with it.
pub fn sign_in_github(token: String, viewer: Viewer, cx: &mut App) -> anyhow::Result<()> {
    store::save_secret(Secret::GitHubToken, &token)?;
    connect_github(token, viewer, cx);
    Ok(())
}

/// Deletes the stored token and disconnects GitHub.
pub fn sign_out_github(cx: &mut App) -> anyhow::Result<()> {
    store::delete_secret(Secret::GitHubToken)?;
    crate::updater::set_token(None, cx);
    crate::cache::remove("offline/github-viewer");
    apply(cx, |services, accounts| {
        services.github = None;
        accounts.github = Connection::Disconnected;
    });
    Ok(())
}

/// Bumped by every manual Linear connect and disconnect, so a key check that
/// started before one of them cannot connect afterwards.
#[derive(Default)]
struct LinearEpoch(u64);

impl Global for LinearEpoch {}

/// The current Linear epoch. Read it before validating a key and pass it to
/// [`connect_linear`].
pub fn linear_epoch(cx: &App) -> u64 {
    cx.try_global::<LinearEpoch>().map_or(0, |epoch| epoch.0)
}

fn bump_linear_epoch(cx: &mut App) {
    cx.default_global::<LinearEpoch>().0 += 1;
}

/// Adds the workspace of `api_key` (or replaces its key when it is already
/// connected), saves every key, and makes it the active workspace when none
/// was. Fails without saving when Linear was connected or disconnected since
/// `epoch` was read.
pub fn connect_linear(
    api_key: String,
    account: loupe_linear::Viewer,
    epoch: u64,
    cx: &mut App,
) -> anyhow::Result<()> {
    if epoch != linear_epoch(cx) {
        anyhow::bail!("Linear changed while this key was being checked");
    }
    let mut keys = linear_keys(cx);
    keys.connect(api_key, account);
    keys.save()?;
    bump_linear_epoch(cx);
    set_linear_keys(keys, cx);
    Ok(())
}

/// Forgets the workspace with this organization id and its key. When it was
/// the active one, the first other workspace becomes active.
pub fn disconnect_linear(workspace_id: &str, cx: &mut App) -> anyhow::Result<()> {
    let mut keys = linear_keys(cx);
    keys.connected
        .retain(|(_, viewer)| viewer.organization.id != workspace_id);
    keys.save()?;
    bump_linear_epoch(cx);
    set_linear_keys(keys, cx);
    Ok(())
}

/// Forgets the stored key at `index` of `Accounts.linear_failed`.
pub fn forget_failed_linear_key(index: usize, cx: &mut App) -> anyhow::Result<()> {
    let mut keys = linear_keys(cx);
    if index < keys.failed.len() {
        keys.failed.remove(index);
    }
    keys.save()?;
    bump_linear_epoch(cx);
    set_linear_keys(keys, cx);
    Ok(())
}

/// Makes the connected workspace with this organization id the active one:
/// the sidebar, the views, and My issues reload from it. Saved, so it is
/// still active after a restart.
pub fn switch_linear(workspace_id: &str, cx: &mut App) {
    if Services::get(cx).linear_workspace(workspace_id).is_none() {
        return;
    }
    let id = workspace_id.to_owned();
    store::update_settings(cx, move |settings| settings.linear_workspace = Some(id));
    set_linear_keys(linear_keys(cx), cx);
}

/// Connects `workspaces` with the clients given, the way startup connects
/// stored keys, so tests drive the real switching with clients that never
/// reach Linear.
#[cfg(test)]
pub fn connect_for_test(workspaces: Vec<(String, LinearWorkspace)>, cx: &mut App) {
    let keys = LinearKeys {
        connected: workspaces
            .iter()
            .map(|(key, workspace)| (key.clone(), workspace.viewer.clone()))
            .collect(),
        ..LinearKeys::default()
    };
    cx.set_global(keys.clone());
    apply(cx, |services, _| {
        services.linear_workspaces = workspaces
            .into_iter()
            .map(|(_, workspace)| workspace)
            .collect();
    });
    set_linear_keys(keys, cx);
}

/// Where the session's Linear keys stand. Only this module reads the keys.
#[derive(Clone, Default)]
struct LinearKeys {
    /// The keys Linear accepted, with whose they are, in connection order;
    /// one per workspace.
    connected: Vec<(String, loupe_linear::Viewer)>,
    /// Stored keys that did not connect, with why (safe to show).
    failed: Vec<(String, String)>,
    /// `LINEAR_API_KEY`, used while no key is stored and never stored.
    from_env: Option<String>,
}

impl Global for LinearKeys {}

impl LinearKeys {
    /// Adds the workspace, or gives it the new key when it is connected.
    fn connect(&mut self, key: String, account: loupe_linear::Viewer) {
        self.failed.retain(|(failed, _)| *failed != key);
        match self
            .connected
            .iter_mut()
            .find(|(_, known)| known.organization.id == account.organization.id)
        {
            Some(known) => *known = (key, account),
            None => self.connected.push((key, account)),
        }
    }

    /// Every key to keep: the connected ones, then the failed ones, so a
    /// key that could not connect today (Linear unreachable) is tried again
    /// on the next start. The environment's key is not stored.
    fn stored(&self) -> Vec<String> {
        self.connected
            .iter()
            .map(|(key, _)| key)
            .chain(self.failed.iter().map(|(key, _)| key))
            .filter(|key| !key.is_empty() && Some(*key) != self.from_env.as_ref())
            .cloned()
            .collect()
    }

    fn save(&self) -> anyhow::Result<()> {
        let keys = self.stored();
        if keys.is_empty() {
            store::delete_secret(Secret::LinearApiKeys)
        } else {
            store::save_secret(Secret::LinearApiKeys, &serde_json::to_string(&keys)?)
        }
    }
}

const OFFLINE_LINEAR: &str = "offline/linear-viewers";

/// The stored Linear keys, in order. A key an older version kept alone is
/// moved into the list here, once.
fn stored_linear_keys() -> Vec<String> {
    let mut keys = parse_keys(store::load_secret(Secret::LinearApiKeys).as_deref());
    if let Some(legacy) = store::load_secret(Secret::LinearApiKey) {
        if !keys.contains(&legacy) {
            keys.push(legacy);
        }
        let moved = serde_json::to_string(&keys)
            .map_err(anyhow::Error::from)
            .and_then(|json| store::save_secret(Secret::LinearApiKeys, &json));
        if moved.is_ok() {
            let _ = store::delete_secret(Secret::LinearApiKey);
        }
    }
    keys
}

/// The keys in the stored JSON array, trimmed, without blanks or repeats.
fn parse_keys(stored: Option<&str>) -> Vec<String> {
    let listed: Vec<String> = stored
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let mut keys: Vec<String> = Vec::new();
    for key in listed {
        let key = key.trim().to_owned();
        if !key.is_empty() && !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

fn linear_keys(cx: &App) -> LinearKeys {
    cx.try_global::<LinearKeys>().cloned().unwrap_or_default()
}

/// The workspace to show: the saved choice while it is connected, else the
/// first connected one.
fn active_index(
    connected: &[(String, loupe_linear::Viewer)],
    chosen: Option<&str>,
) -> Option<usize> {
    chosen
        .and_then(|id| {
            connected
                .iter()
                .position(|(_, viewer)| viewer.organization.id == id)
        })
        .or_else(|| (!connected.is_empty()).then_some(0))
}

/// Keeps `keys` and rebuilds the Linear side of `Services` and `Accounts`
/// from them, reusing the client of every workspace that stays, so its
/// screens do not reload.
fn set_linear_keys(keys: LinearKeys, cx: &mut App) {
    let previous = Services::get(cx).linear_workspaces.clone();
    let previous_keys = linear_keys(cx);
    let workspaces: Vec<LinearWorkspace> = keys
        .connected
        .iter()
        .map(|(key, viewer)| {
            // Only the same key for the same account keeps its client.
            let unchanged = previous_keys
                .connected
                .iter()
                .any(|(old_key, old_viewer)| old_key == key && old_viewer == viewer);
            let client = previous
                .iter()
                .find(|known| unchanged && known.viewer == *viewer)
                .map(|known| known.client.clone())
                .unwrap_or_else(|| Arc::new(Linear::new(key.clone())));
            LinearWorkspace {
                viewer: viewer.clone(),
                client,
            }
        })
        .collect();
    let active = active_index(
        &keys.connected,
        Settings::get(cx).linear_workspace.as_deref(),
    );
    let viewers: Vec<&loupe_linear::Viewer> =
        keys.connected.iter().map(|(_, viewer)| viewer).collect();
    crate::cache::put(OFFLINE_LINEAR, &viewers);
    crate::shell::http::set_linear_upload_key(
        active
            .map(|index| keys.connected[index].0.clone())
            .filter(|key| !key.is_empty()),
    );
    let failed: Vec<String> = keys
        .failed
        .iter()
        .map(|(_, message)| message.clone())
        .collect();
    let state = match active {
        Some(index) => Connection::Connected(keys.connected[index].1.clone()),
        None => match failed.first() {
            Some(message) => Connection::Failed(message.clone()),
            None => Connection::Disconnected,
        },
    };
    let client = active.map(|index| workspaces[index].client.clone());
    cx.set_global(keys);
    apply(cx, |services, accounts| {
        services.linear = client;
        services.linear_workspaces = workspaces;
        accounts.linear = state;
        accounts.linear_failed = failed;
    });
}

/// Stores the T3 server URL in `Settings` and the token as `Secret::T3Token`
/// (`None` clears each), and rebuilds `Services.t3`: `Some` only when both are set.
pub fn set_t3(
    server_url: Option<String>,
    token: Option<String>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let server_url = non_empty(server_url);
    let token = non_empty(token);
    match &token {
        Some(token) => store::save_secret(Secret::T3Token, token)?,
        None => store::delete_secret(Secret::T3Token)?,
    }
    let client = t3_client(server_url.as_deref(), token.as_deref());
    store::update_settings(cx, |settings| settings.t3_server_url = server_url);
    apply(cx, |services, _| services.t3 = client);
    Ok(())
}

fn connect_github(token: String, viewer: Viewer, cx: &mut App) {
    crate::updater::set_token(Some(token.clone()), cx);
    crate::cache::put("offline/github-viewer", &viewer);
    let client = Arc::new(GitHub::new(token));
    apply(cx, |services, accounts| {
        services.github = Some(client);
        accounts.github = Connection::Connected(viewer);
    });
}

fn t3_client(server_url: Option<&str>, token: Option<&str>) -> Option<Arc<T3>> {
    let url = server_url?.trim();
    let token = token?.trim();
    (!url.is_empty() && !token.is_empty()).then(|| Arc::new(T3::new(url, token)))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Replaces both globals in one step, so observers of `Services` never see a
/// client without its account (or the reverse), then redraws.
fn apply(cx: &mut App, change: impl FnOnce(&mut Services, &mut Accounts)) {
    let mut services = Services::get(cx).clone();
    let mut accounts = Accounts::get(cx).clone();
    change(&mut services, &mut accounts);
    cx.set_global(accounts);
    cx.set_global(services);
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::{LinearKeys, active_index, non_empty, parse_keys, t3_client};

    fn viewer(organization: &str) -> loupe_linear::Viewer {
        loupe_linear::Viewer {
            user: loupe_linear::User {
                id: "u".into(),
                name: "User".into(),
                display_name: "user".into(),
                avatar_url: None,
            },
            organization: loupe_linear::Organization {
                id: organization.into(),
                name: organization.into(),
                url_key: organization.into(),
                logo_url: None,
            },
            email: "user@example.com".into(),
        }
    }

    #[test]
    fn stored_keys_are_read_in_order_without_blanks_or_repeats() {
        assert_eq!(
            parse_keys(Some(r#"["lin_a", " lin_b ", "", "lin_a"]"#)),
            ["lin_a", "lin_b"]
        );
        assert!(parse_keys(Some("not json")).is_empty());
        assert!(parse_keys(None).is_empty());
    }

    #[test]
    fn a_workspace_has_one_key_and_the_newest_wins() {
        let mut keys = LinearKeys::default();
        keys.connect("lin_a".into(), viewer("hooli"));
        keys.connect("lin_b".into(), viewer("globex"));
        keys.connect("lin_c".into(), viewer("hooli"));
        let names: Vec<(&str, &str)> = keys
            .connected
            .iter()
            .map(|(key, viewer)| (key.as_str(), viewer.organization.id.as_str()))
            .collect();
        assert_eq!(names, [("lin_c", "hooli"), ("lin_b", "globex")]);
    }

    #[test]
    fn failed_keys_are_kept_and_the_environment_key_is_not() {
        let mut keys = LinearKeys {
            failed: vec![("lin_down".into(), "Could not reach Linear".into())],
            from_env: Some("lin_env".into()),
            ..LinearKeys::default()
        };
        keys.connect("lin_env".into(), viewer("env"));
        keys.connect("lin_a".into(), viewer("a"));
        assert_eq!(keys.stored(), ["lin_a", "lin_down"]);
        // A failed key that connects later leaves the failed list.
        keys.connect("lin_down".into(), viewer("b"));
        assert!(keys.failed.is_empty());
        assert_eq!(keys.stored(), ["lin_a", "lin_down"]);
    }

    #[test]
    fn the_chosen_workspace_is_active_while_it_is_connected() {
        let connected = vec![
            ("lin_a".to_owned(), viewer("hooli")),
            ("lin_b".to_owned(), viewer("globex")),
        ];
        assert_eq!(active_index(&connected, Some("globex")), Some(1));
        assert_eq!(active_index(&connected, Some("gone")), Some(0));
        assert_eq!(active_index(&connected, None), Some(0));
        assert_eq!(active_index(&[], Some("globex")), None);
    }

    #[test]
    fn t3_needs_both_url_and_token() {
        assert!(t3_client(Some("http://t3.local:3773"), Some("t3_FAKE")).is_some());
        assert!(t3_client(Some("http://t3.local:3773"), None).is_none());
        assert!(t3_client(None, Some("t3_FAKE")).is_none());
        assert!(t3_client(Some("  "), Some("t3_FAKE")).is_none());
    }

    #[test]
    fn blank_input_clears() {
        assert_eq!(non_empty(Some("  ".into())), None);
        assert_eq!(non_empty(Some(" a ".into())).as_deref(), Some("a"));
        assert_eq!(non_empty(None), None);
    }
}
