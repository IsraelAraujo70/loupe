//! Persistent preferences (`settings.json`) and secrets. The shell front owns
//! this module; other fronts read and change settings only through
//! [`Settings::get`] and [`update_settings`], and secrets only through
//! [`load_secret`], [`save_secret`] and [`delete_secret`].

use std::path::PathBuf;

use gpui_kit::{App, Global};
use loupe_github::RepoRef;
use serde::{Deserialize, Serialize};

/// Something starred into the sidebar's Favorites, after Linear's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Favorite {
    /// A saved view, by its id in the views file, which keeps its name and
    /// team current.
    View { id: String },
    /// A team's issues with no saved view applied.
    Team { team_id: String, name: String },
    /// One Linear issue.
    Issue { identifier: String, title: String },
}

impl Favorite {
    /// The same item, whatever name it was starred under.
    fn same_item(&self, other: &Favorite) -> bool {
        match (self, other) {
            (Favorite::View { id: a }, Favorite::View { id: b }) => a == b,
            (Favorite::Team { team_id: a, .. }, Favorite::Team { team_id: b, .. }) => a == b,
            (Favorite::Issue { identifier: a, .. }, Favorite::Issue { identifier: b, .. }) => {
                a == b
            }
            _ => false,
        }
    }
}

/// How the files tab lays out a diff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLayout {
    #[default]
    Unified,
    Split,
}

/// The side of the diff the files tab puts its tree of changed files on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTreePosition {
    #[default]
    Left,
    Right,
}

/// The order of a pull request's conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentOrder {
    #[default]
    OldestFirst,
    NewestFirst,
}

/// Where the new-comment box sits in a pull request's conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentBoxPlace {
    Top,
    Bottom,
}

/// A GitHub org (or user) or one repository whose pull requests link to
/// issues of one Linear workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMapping {
    /// `owner` for every repository of an org or user, or `owner/name` for
    /// one repository.
    pub github: String,
    /// The Linear workspace's organization id.
    pub workspace_id: String,
    /// The workspace's name when the mapping was made, shown while that
    /// workspace is not connected.
    pub workspace_name: String,
}

impl WorkspaceMapping {
    /// How closely the mapping names `owner/name`: 2 for the repository
    /// itself, 1 for its owner, `None` when it names neither. GitHub names
    /// ignore case.
    fn specificity(&self, owner: &str, name: &str) -> Option<u8> {
        match self.github.split_once('/') {
            Some((o, n)) => {
                (o.eq_ignore_ascii_case(owner) && n.eq_ignore_ascii_case(name)).then_some(2)
            }
            None => self.github.eq_ignore_ascii_case(owner).then_some(1),
        }
    }
}

/// `text` as a mapping's GitHub side: `owner` or `owner/name`, trimmed, with
/// a leading `https://github.com/` or `@` dropped. `None` when it is not one.
pub fn parse_github_target(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches('/');
    let text = text
        .strip_prefix("https://github.com/")
        .or_else(|| text.strip_prefix("github.com/"))
        .unwrap_or(text);
    let text = text.strip_prefix('@').unwrap_or(text);
    // GitHub's own rules: a login is letters, digits, and hyphens; a
    // repository also takes `_` and `.`, but is never `.` or `..`.
    let owner_ok = |owner: &str| {
        !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let name_ok = |name: &str| {
        !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match text.split_once('/') {
        Some((owner, name)) if owner_ok(owner) && name_ok(name) => Some(format!("{owner}/{name}")),
        None if owner_ok(text) => Some(text.to_owned()),
        _ => None,
    }
}

/// Preferences that survive restarts. Unknown or missing fields fall back to
/// defaults, so older files keep loading.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub watched_repos: Vec<RepoRef>,
    /// The sidebar's Favorites, in the order they were starred.
    pub favorites: Vec<Favorite>,
    /// Sidebar sections the user folded, by key such as `workspace` or
    /// `team:<id>`.
    pub collapsed_sections: Vec<String>,
    pub diff_layout: DiffLayout,
    /// Whether a diff wraps long lines instead of scrolling them
    /// sideways. Off (scrolling) is the default.
    pub diff_wrap: bool,
    /// Where the tree of changed files sits in every pull request.
    pub file_tree_position: FileTreePosition,
    /// The order of every pull request's conversation.
    pub comment_order: CommentOrder,
    /// Where the new-comment box sits, once chosen; until then it follows
    /// [`Settings::comment_order`] (see [`Settings::comment_box_place`]).
    pub comment_box: Option<CommentBoxPlace>,
    /// Base URL of the T3 Code server, such as `https://t3.example.com:3774`.
    pub t3_server_url: Option<String>,
    /// The request template and the risk rules of "Review with T3 Code".
    pub t3_review: crate::t3::ReviewSettings,
    /// Whether the sidebar shows as an icon rail (Cmd+B).
    pub sidebar_collapsed: bool,
    /// The expanded sidebar's width in points, set by dragging its edge.
    /// `None` is the default width, which a double-click on the edge restores.
    pub sidebar_width: Option<f32>,
    /// The tabs of files written before panes: moved into `panes` when the
    /// file loads, never written.
    #[serde(skip_serializing)]
    pub open_tabs: Vec<crate::route::Route>,
    /// Which of `open_tabs` was active.
    #[serde(skip_serializing)]
    pub active_tab: usize,
    /// The window's panes and the page each of their tabs showed, so the
    /// layout comes back after a restart (the tabs' back and forward history
    /// does not). `None` until the first tab opens.
    pub panes: Option<PaneLayout>,
    /// Which pane was active, counting the panes in reading order: left
    /// before right, top before bottom.
    pub active_pane: usize,
    /// The groups of tabs closed with "Close group", the latest first, at
    /// most [`CLOSED_TAB_GROUPS_KEPT`]; the new-tab button's menu reopens
    /// them.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "readable_entries"
    )]
    pub closed_tab_groups: Vec<ClosedTabGroup>,
    /// The whole-window zoom (Cmd+=, Cmd+-, Cmd+0).
    pub zoom: crate::theme::Zoom,
    /// The theme, color scheme, contrast, density, and fonts.
    pub appearance: crate::theme::Appearance,
    /// The Inbox's filter, one token per term (`org:x`, `repo:x`, or a
    /// word), as `github::notification_filter` reads it.
    pub notification_filter: Vec<String>,
    /// The organization id of the Linear workspace the sidebar, the views,
    /// and My issues show; `None` picks the first connected one.
    pub linear_workspace: Option<String>,
    /// Which Linear workspace each GitHub org or repository links to.
    pub workspace_mappings: Vec<WorkspaceMapping>,
}

impl Global for Settings {}

/// How a pane is split: `Horizontal` puts the two side by side,
/// `Vertical` one over the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

/// The colors a group of tabs can take, as Chrome offers them. Each theme
/// gives every one a fill (`Palette::tab_group`). Saved by name, so a new
/// color goes at the end of `ALL` and the saved ones keep theirs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabGroupColor {
    #[default]
    Grey,
    Blue,
    Red,
    Yellow,
    Green,
    Purple,
    Pink,
    Cyan,
    Orange,
}

impl TabGroupColor {
    pub const ALL: [TabGroupColor; 9] = [
        TabGroupColor::Grey,
        TabGroupColor::Blue,
        TabGroupColor::Red,
        TabGroupColor::Yellow,
        TabGroupColor::Green,
        TabGroupColor::Purple,
        TabGroupColor::Pink,
        TabGroupColor::Cyan,
        TabGroupColor::Orange,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TabGroupColor::Grey => "Grey",
            TabGroupColor::Blue => "Blue",
            TabGroupColor::Red => "Red",
            TabGroupColor::Yellow => "Yellow",
            TabGroupColor::Green => "Green",
            TabGroupColor::Purple => "Purple",
            TabGroupColor::Pink => "Pink",
            TabGroupColor::Cyan => "Cyan",
            TabGroupColor::Orange => "Orange",
        }
    }
}

/// A group of tabs as saved: its tabs are the `len` tabs from `first` in
/// its pane's list, since a group's tabs always sit next to each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTabGroup {
    #[serde(default)]
    pub name: String,
    /// A color this build does not know, from a newer one, reads as grey.
    #[serde(default, deserialize_with = "color_or_grey")]
    pub color: TabGroupColor,
    #[serde(default)]
    pub collapsed: bool,
    pub first: usize,
    pub len: usize,
    /// The saved Reviews view the group keeps in sync with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<ViewSync>,
}

/// What a tab group kept in sync with a saved Reviews view follows: the
/// view, the tab of Reviews it was opened from, and the pull requests that
/// tab listed when the group last synced. A pull request new to the list
/// opens in the group; one that left it closes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSync {
    pub view: String,
    pub tab: loupe_github::reviews::ReviewTab,
    #[serde(default)]
    pub seen: Vec<loupe_github::PrRef>,
}

/// How many closed groups of tabs [`Settings::closed_tab_groups`] keeps.
pub const CLOSED_TAB_GROUPS_KEPT: usize = 10;

/// A group of tabs closed with "Close group": its name, its color, the page
/// each of its tabs showed, in order, and where its first tab was in its
/// pane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClosedTabGroup {
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "color_or_grey")]
    pub color: TabGroupColor,
    pub tabs: Vec<crate::route::Route>,
    #[serde(default)]
    pub position: usize,
}

fn color_or_grey<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<TabGroupColor, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// The entries of a saved list that can be read; an entry that cannot is
/// dropped, so it never costs the tabs and the rest of the settings.
fn readable_entries<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    let serde_json::Value::Array(values) = serde_json::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    Ok(values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// The window's panes as saved: a pane's tabs, or a split of two layouts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaneLayout {
    /// One pane: the page of each tab in order, which one is active, the
    /// groups its tabs are in, and how many of its first tabs are pinned.
    Pane {
        tabs: Vec<crate::route::Route>,
        active: usize,
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "readable_entries"
        )]
        groups: Vec<SavedTabGroup>,
        #[serde(default, skip_serializing_if = "is_zero")]
        pinned: usize,
    },
    /// Two layouts along `axis`; `ratio` is the first one's share, 0 to 1.
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

impl Settings {
    pub fn get(cx: &App) -> &Settings {
        cx.global::<Settings>()
    }

    pub fn is_favorite(&self, favorite: &Favorite) -> bool {
        self.favorites
            .iter()
            .any(|starred| starred.same_item(favorite))
    }

    /// Unstars `favorite` when it is starred, else appends it.
    pub fn toggle_favorite(&mut self, favorite: Favorite) {
        let before = self.favorites.len();
        self.favorites
            .retain(|starred| !starred.same_item(&favorite));
        if self.favorites.len() == before {
            self.favorites.push(favorite);
        }
    }

    /// The saved panes, or one pane with no tab before any was saved.
    pub fn pane_layout(&self) -> PaneLayout {
        self.panes.clone().unwrap_or(PaneLayout::Pane {
            tabs: Vec::new(),
            active: 0,
            groups: Vec::new(),
            pinned: 0,
        })
    }

    /// The tabs of a window with one pane, and which is active, as tests
    /// read them; panics on a split.
    #[cfg(test)]
    pub fn single_pane_tabs(&self) -> (Vec<crate::route::Route>, usize) {
        match self.pane_layout() {
            PaneLayout::Pane { tabs, active, .. } => (tabs, active),
            split => panic!("expected one pane, got {split:?}"),
        }
    }

    /// Where the new-comment box sits: the chosen place, else on top for
    /// newest first and at the end for oldest first, so it stays next to the
    /// latest comment.
    pub fn comment_box_place(&self) -> CommentBoxPlace {
        self.comment_box.unwrap_or(match self.comment_order {
            CommentOrder::NewestFirst => CommentBoxPlace::Top,
            CommentOrder::OldestFirst => CommentBoxPlace::Bottom,
        })
    }

    /// The Linear workspaces mapped to the repository `owner/name`, the
    /// mapping of the repository itself before the one of its owner.
    pub fn workspaces_for_repo(&self, owner: &str, name: &str) -> Vec<&str> {
        let mut found: Vec<(u8, &str)> = self
            .workspace_mappings
            .iter()
            .filter_map(|mapping| {
                Some((
                    mapping.specificity(owner, name)?,
                    mapping.workspace_id.as_str(),
                ))
            })
            .collect();
        found.sort_by_key(|(specificity, _)| std::cmp::Reverse(*specificity));
        found.into_iter().map(|(_, workspace)| workspace).collect()
    }

    /// Maps `github` to the workspace, replacing a mapping of the same org
    /// or repository.
    pub fn map_workspace(&mut self, mapping: WorkspaceMapping) {
        match self
            .workspace_mappings
            .iter_mut()
            .find(|known| known.github.eq_ignore_ascii_case(&mapping.github))
        {
            Some(known) => *known = mapping,
            None => self.workspace_mappings.push(mapping),
        }
    }

    /// Keeps `group` first among the closed groups, dropping the oldest
    /// past [`CLOSED_TAB_GROUPS_KEPT`].
    pub fn remember_closed_group(&mut self, group: ClosedTabGroup) {
        self.closed_tab_groups.insert(0, group);
        self.closed_tab_groups.truncate(CLOSED_TAB_GROUPS_KEPT);
    }

    pub fn is_collapsed(&self, section: &str) -> bool {
        self.collapsed_sections.iter().any(|key| key == section)
    }

    /// Folds `section` when it is open, else opens it.
    pub fn toggle_collapsed(&mut self, section: &str) {
        let before = self.collapsed_sections.len();
        self.collapsed_sections.retain(|key| key != section);
        if self.collapsed_sections.len() == before {
            self.collapsed_sections.push(section.to_owned());
        }
    }

    /// Reads `settings.json`, or defaults when it is missing or unreadable.
    pub fn load() -> Settings {
        std::fs::read(settings_path())
            .ok()
            .and_then(|bytes| Settings::parse(&bytes))
            .unwrap_or_default()
    }

    /// A settings file's preferences. A file from before panes has its tabs
    /// moved into one pane here, so the next save, whatever it changes,
    /// keeps them.
    fn parse(bytes: &[u8]) -> Option<Settings> {
        let mut settings: Settings = serde_json::from_slice(bytes).ok()?;
        let tabs = std::mem::take(&mut settings.open_tabs);
        let active = std::mem::take(&mut settings.active_tab);
        if settings.panes.is_none() && !tabs.is_empty() {
            settings.panes = Some(PaneLayout::Pane {
                tabs,
                active,
                groups: Vec::new(),
                pinned: 0,
            });
            settings.active_pane = 0;
        }
        Some(settings)
    }

    /// Writes a temporary file and renames it over the settings, so a crash
    /// mid-write never leaves JSON that `load` would replace with defaults.
    fn save(&self) -> anyhow::Result<()> {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    }
}

/// Changes the settings, writes them to disk, and refreshes every window.
pub fn update_settings(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let settings = cx.global_mut::<Settings>();
    change(settings);
    if let Err(error) = settings.save() {
        eprintln!("loupe: could not save settings: {error:#}");
    }
    cx.refresh_windows();
}

/// The secrets Loupe stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    GitHubToken,
    /// The one key older versions kept. Read once and moved into
    /// [`Secret::LinearApiKeys`].
    LinearApiKey,
    /// Every connected workspace's key, as a JSON array of strings.
    LinearApiKeys,
    T3Token,
}

impl Secret {
    fn file_name(self) -> &'static str {
        match self {
            Secret::GitHubToken => "github-token",
            Secret::LinearApiKey => "linear-api-key",
            Secret::LinearApiKeys => "linear-api-keys",
            Secret::T3Token => "t3-token",
        }
    }
}

/// Keychain service name for every Loupe secret.
const KEYCHAIN_SERVICE: &str = "dev.loupe.app";

/// Where secrets live.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SecretStore {
    /// macOS Keychain generic passwords. `fallback` is the file store used when
    /// the Keychain cannot answer, and the place old installs left their files.
    Keychain { service: String, fallback: PathBuf },
    /// One file per secret under `dir`.
    Files { dir: PathBuf },
}

impl SecretStore {
    /// Dev and test runs (`LOUPE_DATA_DIR` set) and non-macOS targets must never
    /// touch the real Keychain.
    fn select(data_dir_overridden: bool, macos: bool, files_dir: PathBuf) -> SecretStore {
        if data_dir_overridden || !macos {
            SecretStore::Files { dir: files_dir }
        } else {
            SecretStore::Keychain {
                service: KEYCHAIN_SERVICE.to_owned(),
                fallback: files_dir,
            }
        }
    }

    fn current() -> SecretStore {
        SecretStore::select(
            std::env::var_os("LOUPE_DATA_DIR").is_some(),
            cfg!(target_os = "macos"),
            secrets_dir(),
        )
    }

    fn load(&self, secret: Secret) -> Option<String> {
        let account = secret.file_name();
        match self {
            SecretStore::Files { dir } => files::read(dir, account),
            SecretStore::Keychain { service, fallback } => {
                match keychain::get(service, account) {
                    Ok(Some(value)) => return Some(value),
                    Ok(None) => {
                        // Moves a secret an older install kept in a file into the Keychain.
                        let legacy = files::read(fallback, account)?;
                        if keychain::set(service, account, &legacy).is_ok() {
                            let _ = files::remove(fallback, account);
                        }
                        return Some(legacy);
                    }
                    Err(code) => log_keychain_failure("read", code),
                }
                files::read(fallback, account)
            }
        }
    }

    fn save(&self, secret: Secret, value: &str) -> anyhow::Result<()> {
        let account = secret.file_name();
        let value = value.trim();
        match self {
            SecretStore::Files { dir } => files::write(dir, account, value),
            SecretStore::Keychain { service, fallback } => {
                match keychain::set(service, account, value) {
                    Ok(()) => {
                        let _ = files::remove(fallback, account);
                        Ok(())
                    }
                    Err(code) => {
                        log_keychain_failure("write", code);
                        files::write(fallback, account, value)
                    }
                }
            }
        }
    }

    fn delete(&self, secret: Secret) -> anyhow::Result<()> {
        let account = secret.file_name();
        match self {
            SecretStore::Files { dir } => files::remove(dir, account),
            SecretStore::Keychain { service, fallback } => {
                let in_keychain = keychain::delete(service, account);
                let in_files = files::remove(fallback, account);
                if let Err(code) = in_keychain {
                    anyhow::bail!("could not delete the secret from the Keychain (code {code})");
                }
                in_files
            }
        }
    }
}

/// Logs the OSStatus only: the system's message text is not needed and a
/// secret must never reach a log.
fn log_keychain_failure(action: &str, code: i32) {
    eprintln!("loupe: Keychain {action} failed (code {code}), using the file store");
}

/// Reads a stored secret, or `None` when it is missing or empty.
pub fn load_secret(secret: Secret) -> Option<String> {
    SecretStore::current().load(secret)
}

/// Stores a secret (trimmed), replacing any previous value.
pub fn save_secret(secret: Secret, value: &str) -> anyhow::Result<()> {
    SecretStore::current().save(secret, value)
}

/// Removes a secret. Succeeds when there is nothing to remove.
pub fn delete_secret(secret: Secret) -> anyhow::Result<()> {
    SecretStore::current().delete(secret)
}

mod files {
    use std::path::Path;

    pub fn read(dir: &Path, account: &str) -> Option<String> {
        let value = std::fs::read_to_string(dir.join(account)).ok()?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    /// Writes the secret with owner-only permissions from the first byte: a
    /// new file is created 0600, and an existing one is narrowed before the
    /// secret goes in.
    pub fn write(dir: &Path, account: &str, value: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(account);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            if path.exists() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
            options.mode(0o600);
        }
        std::io::Write::write_all(&mut options.open(&path)?, value.as_bytes())?;
        Ok(())
    }

    pub fn remove(dir: &Path, account: &str) -> anyhow::Result<()> {
        match std::fs::remove_file(dir.join(account)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }
}

/// Keychain access. Errors are the raw OSStatus code, so no message text can
/// carry a secret.
#[cfg(target_os = "macos")]
mod keychain {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    /// `errSecItemNotFound`.
    const ITEM_NOT_FOUND: i32 = -25300;

    pub fn get(service: &str, account: &str) -> Result<Option<String>, i32> {
        match get_generic_password(service, account) {
            Ok(bytes) => {
                let value = String::from_utf8_lossy(&bytes);
                let value = value.trim();
                Ok((!value.is_empty()).then(|| value.to_owned()))
            }
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(error) => Err(error.code()),
        }
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<(), i32> {
        set_generic_password(service, account, value.trim().as_bytes()).map_err(|e| e.code())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), i32> {
        match delete_generic_password(service, account) {
            Err(error) if error.code() != ITEM_NOT_FOUND => Err(error.code()),
            _ => Ok(()),
        }
    }
}

/// Stand-in so non-macOS builds compile; `SecretStore::select` never picks the
/// Keychain there.
#[cfg(not(target_os = "macos"))]
mod keychain {
    /// `errSecUnimplemented`.
    const UNAVAILABLE: i32 = -4;

    pub fn get(_: &str, _: &str) -> Result<Option<String>, i32> {
        Err(UNAVAILABLE)
    }

    pub fn set(_: &str, _: &str, _: &str) -> Result<(), i32> {
        Err(UNAVAILABLE)
    }

    pub fn delete(_: &str, _: &str) -> Result<(), i32> {
        Err(UNAVAILABLE)
    }
}

/// `~/Library/Application Support/Loupe` on macOS, `$XDG_CONFIG_HOME/loupe`
/// (or `~/.config/loupe`) elsewhere. `LOUPE_DATA_DIR` overrides both.
/// Under `cargo test`, a folder of the test's own; see [`test_data_dir`].
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LOUPE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(test) {
        return test_data_dir();
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Loupe")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("loupe")
    }
}

/// The data folder of the running test, made on first use and deleted when
/// the test ends.
///
/// UI tests save settings, themes and views; they must never write over the
/// real ones. libtest runs each test on its own thread, at the same time as
/// others, so one folder per thread keeps one test from reading what another
/// saved. One folder per process let two tests write `custom-theme.json` or
/// `reviews-views.json` at once.
fn test_data_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Folder(PathBuf);
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
    thread_local! {
        static FOLDER: Folder = Folder(std::env::temp_dir().join(format!(
            "loupe-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
    }
    FOLDER.with(|folder| folder.0.clone())
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

fn secrets_dir() -> PathBuf {
    data_dir().join("secrets")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        CLOSED_TAB_GROUPS_KEPT, ClosedTabGroup, CommentBoxPlace, CommentOrder, Favorite,
        FileTreePosition, KEYCHAIN_SERVICE, PaneLayout, SavedTabGroup, Secret, SecretStore,
        Settings, SplitAxis, TabGroupColor, WorkspaceMapping, parse_github_target,
    };
    use crate::route::Route;

    fn mapping(github: &str, workspace_id: &str) -> WorkspaceMapping {
        WorkspaceMapping {
            github: github.into(),
            workspace_id: workspace_id.into(),
            workspace_name: workspace_id.into(),
        }
    }

    #[test]
    fn a_repository_mapping_wins_over_its_owner() {
        let settings = Settings {
            workspace_mappings: vec![
                mapping("hooli-inc", "hooli"),
                mapping("IsraelAraujo70", "initech"),
                mapping("israelaraujo70/loupe", "globex"),
            ],
            ..Settings::default()
        };
        assert_eq!(
            settings.workspaces_for_repo("hooli-inc", "hooli-api"),
            ["hooli"]
        );
        assert_eq!(
            settings.workspaces_for_repo("IsraelAraujo70", "dotfiles"),
            ["initech"]
        );
        assert_eq!(
            settings.workspaces_for_repo("IsraelAraujo70", "loupe"),
            ["globex", "initech"]
        );
        assert!(settings.workspaces_for_repo("acme", "web").is_empty());
        // An org's name is not a prefix match.
        assert!(settings.workspaces_for_repo("Hooli", "x").is_empty());
    }

    #[test]
    fn mapping_the_same_org_again_replaces_it() {
        let mut settings = Settings::default();
        settings.map_workspace(mapping("acme", "one"));
        settings.map_workspace(mapping("acme/web", "two"));
        settings.map_workspace(mapping("ACME", "three"));
        assert_eq!(
            settings.workspace_mappings,
            vec![mapping("ACME", "three"), mapping("acme/web", "two")]
        );
    }

    #[test]
    fn a_github_target_is_an_owner_or_a_repository() {
        assert_eq!(
            parse_github_target(" hooli-inc ").as_deref(),
            Some("hooli-inc")
        );
        assert_eq!(
            parse_github_target("@IsraelAraujo70").as_deref(),
            Some("IsraelAraujo70")
        );
        assert_eq!(
            parse_github_target("https://github.com/acme/web.js/").as_deref(),
            Some("acme/web.js")
        );
        assert_eq!(parse_github_target("acme/").as_deref(), Some("acme"));
        for invalid in [
            "",
            "/web",
            "a/b/c",
            "acme web",
            "acme/we b",
            "../x",
            "acme/..",
            "a.b",
        ] {
            assert_eq!(parse_github_target(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn settings_from_before_workspaces_still_load() {
        let settings: Settings =
            serde_json::from_str(r#"{"diff_wrap": true}"#).expect("an older settings file");
        assert_eq!(settings.linear_workspace, None);
        assert!(settings.workspace_mappings.is_empty());
    }

    #[test]
    fn the_comment_box_follows_the_order_until_it_is_placed() {
        let mut settings = Settings::default();
        assert_eq!(settings.comment_box_place(), CommentBoxPlace::Bottom);
        settings.comment_order = CommentOrder::NewestFirst;
        assert_eq!(settings.comment_box_place(), CommentBoxPlace::Top);
        settings.comment_box = Some(CommentBoxPlace::Bottom);
        assert_eq!(settings.comment_box_place(), CommentBoxPlace::Bottom);
    }

    #[test]
    fn settings_without_comment_fields_still_load() {
        let settings: Settings = serde_json::from_str(r#"{"sidebar_collapsed": true}"#)
            .expect("an older settings file loads");
        assert_eq!(settings.comment_order, CommentOrder::OldestFirst);
        assert_eq!(settings.comment_box, None);
    }

    #[test]
    fn the_inbox_filter_survives_a_restart() {
        let settings = Settings {
            notification_filter: vec!["org:IsraelAraujo70".into(), "fix".into()],
            ..Settings::default()
        };
        let saved = serde_json::to_string(&settings).expect("saves");
        let loaded: Settings = serde_json::from_str(&saved).expect("loads");
        assert_eq!(loaded.notification_filter, settings.notification_filter);
        let older: Settings = serde_json::from_str("{}").expect("an older file loads");
        assert!(older.notification_filter.is_empty());
    }

    #[test]
    fn the_panes_survive_a_restart_and_older_tabs_become_one_pane() {
        let older = Settings::parse(br#"{"open_tabs": ["Inbox", "Settings"], "active_tab": 1}"#)
            .expect("a settings file from before panes loads");
        let tabs = PaneLayout::Pane {
            tabs: vec![Route::Inbox, Route::Settings],
            active: 1,
            groups: Vec::new(),
            pinned: 0,
        };
        assert_eq!(older.pane_layout(), tabs);
        // Any other change saves the file before a tab moves.
        let collapsed = Settings {
            sidebar_collapsed: true,
            ..older.clone()
        };
        let saved = serde_json::to_vec(&collapsed).expect("settings serialize");
        assert_eq!(
            Settings::parse(&saved).expect("loads").pane_layout(),
            tabs,
            "the older tabs survive a save that is not about tabs"
        );

        let split = PaneLayout::Split {
            axis: SplitAxis::Vertical,
            ratio: 0.3,
            first: Box::new(PaneLayout::Pane {
                tabs: vec![Route::Inbox],
                active: 0,
                groups: Vec::new(),
                pinned: 0,
            }),
            second: Box::new(PaneLayout::Pane {
                tabs: vec![Route::Notifications, Route::Settings],
                active: 1,
                groups: vec![SavedTabGroup {
                    name: "Review".into(),
                    color: TabGroupColor::Green,
                    collapsed: true,
                    first: 1,
                    len: 1,
                    sync: None,
                }],
                pinned: 1,
            }),
        };
        let settings = Settings {
            panes: Some(split.clone()),
            active_pane: 1,
            ..older
        };
        let saved = serde_json::to_string(&settings).expect("settings serialize");
        assert!(
            !saved.contains("open_tabs"),
            "the flat list is only read, so it cannot drift from the panes"
        );
        let loaded: Settings = serde_json::from_str(&saved).expect("saved settings load");
        assert_eq!(loaded.pane_layout(), split);
        assert_eq!(loaded.active_pane, 1);

        let newer = Settings::parse(
            br#"{"sidebar_collapsed": true, "panes": {"Pane": {"tabs": ["Inbox", "Settings"], "active": 0,
                "groups": [{"name": "A", "color": "Teal", "first": 0, "len": 1},
                           {"name": "B", "first": "one", "len": 1}]}}}"#,
        )
        .expect("groups from a newer build or a hand edit never cost the settings");
        assert!(newer.sidebar_collapsed);
        assert_eq!(
            newer.pane_layout(),
            PaneLayout::Pane {
                tabs: vec![Route::Inbox, Route::Settings],
                active: 0,
                groups: vec![SavedTabGroup {
                    name: "A".into(),
                    color: TabGroupColor::Grey,
                    collapsed: false,
                    first: 0,
                    len: 1,
                    sync: None,
                }],
                pinned: 0,
            },
            "an unknown color reads as grey, and an unreadable group is dropped"
        );

        let before_groups =
            Settings::parse(br#"{"panes": {"Pane": {"tabs": ["Inbox"], "active": 0}}}"#)
                .expect("a pane saved before tab groups loads");
        assert_eq!(
            before_groups.pane_layout(),
            PaneLayout::Pane {
                tabs: vec![Route::Inbox],
                active: 0,
                groups: Vec::new(),
                pinned: 0,
            }
        );
    }

    #[test]
    fn closed_groups_survive_a_restart_and_keep_the_latest_ten() {
        let mut settings = Settings::default();
        for number in 0..12 {
            settings.remember_closed_group(ClosedTabGroup {
                name: format!("Group {number}"),
                color: TabGroupColor::Orange,
                tabs: vec![Route::Inbox, Route::Settings],
                position: number,
            });
        }
        assert_eq!(settings.closed_tab_groups.len(), CLOSED_TAB_GROUPS_KEPT);
        assert_eq!(
            settings.closed_tab_groups[0].name, "Group 11",
            "the latest comes first"
        );
        assert_eq!(settings.closed_tab_groups[9].name, "Group 2");

        let saved = serde_json::to_vec(&settings).expect("settings serialize");
        let loaded = Settings::parse(&saved).expect("loads");
        assert_eq!(loaded.closed_tab_groups, settings.closed_tab_groups);

        let newer = Settings::parse(
            br#"{"sidebar_collapsed": true, "closed_tab_groups": [
                {"name": "A", "color": "Teal", "tabs": ["Inbox"], "position": 2},
                {"name": "B", "tabs": "not a list"}]}"#,
        )
        .expect("closed groups from a newer build never cost the settings");
        assert!(newer.sidebar_collapsed);
        assert_eq!(
            newer.closed_tab_groups,
            [ClosedTabGroup {
                name: "A".into(),
                color: TabGroupColor::Grey,
                tabs: vec![Route::Inbox],
                position: 2,
            }],
            "an unknown color reads as grey, and an unreadable group is dropped"
        );
        assert!(
            !String::from_utf8(serde_json::to_vec(&Settings::default()).unwrap())
                .unwrap()
                .contains("closed_tab_groups"),
            "no closed group writes nothing"
        );
    }

    #[test]
    fn groups_keep_their_color_across_the_new_colors() {
        // A pane saved before Pink, Cyan, and Orange existed, one group per
        // color it could hold.
        let before = Settings::parse(
            br#"{"panes": {"Pane": {"tabs": ["Inbox", "Notifications", "Settings", "LinearMyIssues",
                {"LinearProjects": {"team_id": null}}, {"LinearViews": {"team_id": null}}], "active": 0,
                "groups": [{"name": "a", "color": "Grey", "first": 0, "len": 1},
                           {"name": "b", "color": "Blue", "first": 1, "len": 1},
                           {"name": "c", "color": "Red", "first": 2, "len": 1},
                           {"name": "d", "color": "Yellow", "first": 3, "len": 1},
                           {"name": "e", "color": "Green", "first": 4, "len": 1},
                           {"name": "f", "color": "Purple", "first": 5, "len": 1}]}}}"#,
        )
        .expect("settings from before the new colors load");
        let PaneLayout::Pane { groups, .. } = before.pane_layout() else {
            panic!("one pane");
        };
        let colors: Vec<TabGroupColor> = groups.iter().map(|group| group.color).collect();
        assert_eq!(colors, TabGroupColor::ALL[..6]);

        for color in [
            TabGroupColor::Pink,
            TabGroupColor::Cyan,
            TabGroupColor::Orange,
        ] {
            let settings = Settings {
                panes: Some(PaneLayout::Pane {
                    tabs: vec![Route::Inbox],
                    active: 0,
                    groups: vec![SavedTabGroup {
                        name: "New".into(),
                        color,
                        collapsed: false,
                        first: 0,
                        len: 1,
                        sync: None,
                    }],
                    pinned: 0,
                }),
                ..Settings::default()
            };
            let saved = serde_json::to_vec(&settings).expect("settings serialize");
            let PaneLayout::Pane { groups, .. } =
                Settings::parse(&saved).expect("loads").pane_layout()
            else {
                panic!("one pane");
            };
            assert_eq!(groups[0].color, color, "{} comes back", color.label());
        }
    }

    #[test]
    fn settings_saved_with_grouping_by_repository_still_load_with_their_groups() {
        // Saved by a build that had "Group tabs by repository automatically"
        // on, with a group it made.
        let saved = br#"{"sidebar_collapsed": true, "group_tabs_by_repo": true,
            "panes": {"Pane": {"tabs": ["Inbox", "Notifications"], "active": 1,
                "groups": [{"name": "loupe", "color": "Blue", "first": 1, "len": 1}]}}}"#;
        let settings = Settings::parse(saved).expect("the file loads");
        assert!(settings.sidebar_collapsed, "the other settings are kept");
        assert_eq!(
            settings.pane_layout(),
            PaneLayout::Pane {
                tabs: vec![Route::Inbox, Route::Notifications],
                active: 1,
                groups: vec![SavedTabGroup {
                    name: "loupe".into(),
                    color: TabGroupColor::Blue,
                    collapsed: false,
                    first: 1,
                    len: 1,
                    sync: None,
                }],
                pinned: 0,
            },
            "the group the option made stays, as a plain group"
        );
        let resaved = serde_json::to_string(&settings).expect("settings serialize");
        assert!(
            !resaved.contains("group_tabs_by_repo"),
            "the next save drops the old field"
        );
    }

    #[test]
    fn the_file_tree_is_on_the_left_until_it_is_moved() {
        let old: Settings = serde_json::from_str(r#"{"diff_wrap": true}"#)
            .expect("a settings file from before the option loads");
        assert_eq!(old.file_tree_position, FileTreePosition::Left);

        let moved = Settings {
            file_tree_position: FileTreePosition::Right,
            ..Settings::default()
        };
        let saved = serde_json::to_string(&moved).expect("settings serialize");
        let loaded: Settings = serde_json::from_str(&saved).expect("saved settings load");
        assert_eq!(loaded.file_tree_position, FileTreePosition::Right);
    }

    #[test]
    fn starring_twice_unstars_whatever_the_name() {
        let mut settings = Settings::default();
        let view = Favorite::View { id: "v1".into() };
        let issue = |title: &str| Favorite::Issue {
            identifier: "ENG-1".into(),
            title: title.into(),
        };
        settings.toggle_favorite(view.clone());
        settings.toggle_favorite(issue("Old title"));
        assert_eq!(settings.favorites, vec![view.clone(), issue("Old title")]);
        assert!(settings.is_favorite(&issue("New title")));
        assert!(!settings.is_favorite(&Favorite::Team {
            team_id: "v1".into(),
            name: "Same id, other kind".into(),
        }));
        settings.toggle_favorite(issue("New title"));
        assert_eq!(settings.favorites, vec![view]);
    }

    #[test]
    fn sections_fold_and_open() {
        let mut settings = Settings::default();
        assert!(!settings.is_collapsed("workspace"));
        settings.toggle_collapsed("workspace");
        assert!(settings.is_collapsed("workspace"));
        settings.toggle_collapsed("workspace");
        assert!(!settings.is_collapsed("workspace"));
    }

    #[test]
    fn settings_from_before_favorites_still_load() {
        let old = r#"{"watched_repos":[],"pinned_teams":[{"id":"t","key":"T","name":"Team"}]}"#;
        let settings: Settings = serde_json::from_str(old).expect("an older settings file");
        assert!(settings.favorites.is_empty());
        assert!(settings.collapsed_sections.is_empty());
        assert_eq!(settings.zoom.percent(), 100);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("loupe-store-{name}-{}", std::process::id()))
    }

    #[test]
    fn data_dir_override_selects_files() {
        let dir = PathBuf::from("/tmp/loupe-secrets");
        assert_eq!(
            SecretStore::select(true, true, dir.clone()),
            SecretStore::Files { dir }
        );
    }

    #[test]
    fn non_macos_selects_files() {
        let dir = PathBuf::from("/tmp/loupe-secrets");
        assert_eq!(
            SecretStore::select(false, false, dir.clone()),
            SecretStore::Files { dir }
        );
    }

    #[test]
    fn macos_without_override_selects_keychain() {
        let dir = PathBuf::from("/tmp/loupe-secrets");
        assert_eq!(
            SecretStore::select(false, true, dir.clone()),
            SecretStore::Keychain {
                service: KEYCHAIN_SERVICE.to_owned(),
                fallback: dir,
            }
        );
    }

    #[test]
    fn file_store_round_trips() {
        let dir = scratch_dir("files");
        let store = SecretStore::Files { dir: dir.clone() };
        assert_eq!(store.load(Secret::T3Token), None);

        store.save(Secret::T3Token, "  dummy-value\n").unwrap();
        assert_eq!(store.load(Secret::T3Token).as_deref(), Some("dummy-value"));
        assert_eq!(store.load(Secret::GitHubToken), None);

        store.delete(Secret::T3Token).unwrap();
        assert_eq!(store.load(Secret::T3Token), None);
        store.delete(Secret::T3Token).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_value_reads_as_none() {
        let dir = scratch_dir("empty");
        let store = SecretStore::Files { dir: dir.clone() };
        store.save(Secret::LinearApiKey, "   ").unwrap();
        assert_eq!(store.load(Secret::LinearApiKey), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An ssh session cannot reach the login Keychain ("User interaction is
    /// not allowed"); run it from a GUI login session with `--ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs the GUI login session's Keychain"]
    fn keychain_round_trips() {
        let service = format!("dev.loupe.app.test.{}", std::process::id());
        let account = Secret::GitHubToken.file_name();

        super::keychain::set(&service, account, "loupe-test-dummy").unwrap();
        let read = super::keychain::get(&service, account).unwrap();
        let deleted = super::keychain::delete(&service, account);
        let after = super::keychain::get(&service, account);
        let deleted_again = super::keychain::delete(&service, account);

        assert_eq!(read.as_deref(), Some("loupe-test-dummy"));
        assert_eq!(deleted, Ok(()));
        assert_eq!(after, Ok(None));
        assert_eq!(deleted_again, Ok(()));
    }
}
