//! The connection part of the user's own Claude Code settings.
//!
//! `--restricted` makes Claude Code ignore every settings file, which also
//! keeps a pull request from planting its own `.claude/settings.json` in the
//! checkout. It would also drop the user's `apiKeyHelper` and `env`, which is
//! how a proxy or gateway is configured. So Loupe reads the user-level
//! settings file itself, keeps only what says how to reach the API, and hands
//! that to Claude Code with `--settings` (which `--restricted` still applies).
//! Permissions, hooks, MCP servers, and every other variable stay out.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::private_dir::PrivateDir;

/// `env` keys forwarded by prefix: Anthropic's own connection variables
/// (base URL, auth token, API key, custom headers, model aliases) and
/// Vertex's per-model regions.
const FORWARDED_ENV_PREFIXES: [&str; 2] = ["ANTHROPIC_", "VERTEX_REGION_"];

/// `env` keys forwarded by name: provider switches, cloud credentials, and
/// the network settings a gateway may need.
const FORWARDED_ENV_KEYS: [&str; 26] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "CLAUDE_CODE_CLIENT_CERT",
    "CLAUDE_CODE_CLIENT_KEY",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
    "AWS_REGION",
    "AWS_PROFILE",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "CLOUD_ML_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "NODE_EXTRA_CA_CERTS",
    "API_TIMEOUT_MS",
    "CLAUDE_CODE_MAX_RETRIES",
];

/// Words that mark a forwarded variable's value as a secret to mask in
/// error messages.
const SECRET_MARKERS: [&str; 5] = ["TOKEN", "KEY", "SECRET", "PASSWORD", "PASSPHRASE"];

/// `ANTHROPIC_*` keys that are not connection settings: debug logging prints
/// request headers, credentials included.
const DENIED_ENV_KEYS: [&str; 1] = ["ANTHROPIC_LOG"];

fn is_forwarded_env(key: &str) -> bool {
    !DENIED_ENV_KEYS.contains(&key)
        && (FORWARDED_ENV_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
            || FORWARDED_ENV_KEYS.contains(&key))
}

/// Where the forwarded `apiKeyHelper` runs: the user's home, as in a new
/// terminal, else `/`. Never the working directory Claude Code would give it,
/// which during a guide run is the pull request's checkout.
pub fn helper_dir(env: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    env("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute() && home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `helper` as a `/bin/sh` script that first moves to `dir`. Claude Code runs
/// the helper with its own working directory, the pull request's checkout, so
/// a helper that resolves anything relative to it (a relative path, Python's
/// `sys.path[0]`, `node_modules`, `npx`) would run code from the pull request.
fn helper_in(dir: &Path, helper: &str) -> String {
    let quoted = dir.to_string_lossy().replace('\'', r"'\''");
    format!("cd -- '{quoted}' || exit 1\n{helper}")
}

/// The user-level Claude Code settings file: `$CLAUDE_CONFIG_DIR/settings.json`,
/// else `$HOME/.claude/settings.json`.
pub fn user_settings_path(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let non_empty = |key: &str| env(key).filter(|value| !value.is_empty());
    if let Some(dir) = non_empty("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    non_empty("HOME").map(|home| PathBuf::from(home).join(".claude/settings.json"))
}

/// The settings Loupe forwards from `user`: its `apiKeyHelper` (run from
/// `helper_dir`, see [`helper_in`]) and the connection variables of its `env`.
/// `None` when there is nothing to forward.
pub fn connection_settings(user: &Value, helper_dir: &Path) -> Option<Value> {
    let mut forwarded = Map::new();
    if let Some(helper) = user
        .get("apiKeyHelper")
        .and_then(Value::as_str)
        .filter(|helper| !helper.trim().is_empty())
    {
        forwarded.insert(
            "apiKeyHelper".into(),
            Value::String(helper_in(helper_dir, helper)),
        );
    }
    let env: Map<String, Value> = user
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(key, value)| is_forwarded_env(key) && value.is_string())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !env.is_empty() {
        forwarded.insert("env".into(), Value::Object(env));
    }
    (!forwarded.is_empty()).then_some(Value::Object(forwarded))
}

/// The forwarded settings written to an owner-only file in a private temporary
/// directory outside any checkout (Claude Code's file tools cannot reach it),
/// removed on drop. A file instead of inline JSON keeps the values out of
/// process listings.
#[derive(Debug)]
pub(crate) struct ForwardedSettings {
    path: PathBuf,
    /// Declared after `path` only for reading order; dropping it removes the file.
    _dir: PrivateDir,
    secrets: Vec<String>,
}

/// What [`ForwardedSettings::load`] found.
#[derive(Debug)]
pub(crate) enum UserSettings {
    /// Nothing to forward (no file, or no connection settings in it).
    None,
    Forwarded(ForwardedSettings),
    /// The file exists but could not be used; the reason never quotes it.
    Unreadable {
        path: PathBuf,
        reason: String,
    },
}

impl UserSettings {
    pub(crate) fn forwarded(&self) -> Option<&ForwardedSettings> {
        match self {
            UserSettings::Forwarded(settings) => Some(settings),
            _ => None,
        }
    }
}

impl ForwardedSettings {
    /// Reads the user-level settings file this process would use.
    pub(crate) fn load() -> UserSettings {
        let env = |key: &str| std::env::var_os(key);
        match user_settings_path(env) {
            Some(path) => Self::load_from(&path, &helper_dir(env)),
            None => UserSettings::None,
        }
    }

    pub(crate) fn load_from(path: &Path, helper_dir: &Path) -> UserSettings {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return UserSettings::None,
            Err(err) => {
                return UserSettings::Unreadable {
                    path: path.to_path_buf(),
                    reason: err.kind().to_string(),
                };
            }
        };
        let Ok(user) = serde_json::from_str::<Value>(&text) else {
            return UserSettings::Unreadable {
                path: path.to_path_buf(),
                reason: "not valid JSON".into(),
            };
        };
        let Some(forwarded) = connection_settings(&user, helper_dir) else {
            return UserSettings::None;
        };
        match Self::write(&forwarded) {
            Ok(settings) => UserSettings::Forwarded(settings),
            Err(err) => UserSettings::Unreadable {
                path: path.to_path_buf(),
                reason: format!("could not prepare them for Claude Code: {}", err.kind()),
            },
        }
    }

    fn write(forwarded: &Value) -> std::io::Result<Self> {
        let dir = PrivateDir::create("loupe-claude-settings")?;
        let settings = Self {
            path: dir.path().join("settings.json"),
            _dir: dir,
            secrets: secret_values(forwarded),
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        std::io::Write::write_all(
            &mut options.open(&settings.path)?,
            forwarded.to_string().as_bytes(),
        )?;
        Ok(settings)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// `text` with every forwarded secret value masked.
    pub(crate) fn redact(&self, text: &str) -> String {
        self.secrets
            .iter()
            .fold(text.to_owned(), |text, secret| text.replace(secret, "***"))
    }
}

/// Values of forwarded variables whose names mark them as secrets, longest
/// first so a secret that contains another is masked whole.
fn secret_values(forwarded: &Value) -> Vec<String> {
    let mut secrets: Vec<String> = forwarded
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(key, _)| SECRET_MARKERS.iter().any(|marker| key.contains(marker)))
        .filter_map(|(_, value)| value.as_str())
        .filter(|value| value.len() >= 6)
        .map(str::to_owned)
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{
        ForwardedSettings, UserSettings, connection_settings, helper_dir, user_settings_path,
    };

    const HOME: &str = "/home/me";

    #[test]
    fn only_the_helper_and_connection_variables_are_forwarded() {
        let user = json!({
            "apiKeyHelper": "cat ~/.config/proxy/key",
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:4141",
                "ANTHROPIC_AUTH_TOKEN": "sk-proxy",
                "HTTPS_PROXY": "http://proxy.local:3128",
                "GITHUB_TOKEN": "ghp_user",
                "GH_TOKEN": "gho_user",
                "LINEAR_API_KEY": "lin_api_user",
                "CLAUDE_CODE_RESTRICTED": "0",
                "CLAUDE_CODE_SAFE_MODE": "0",
                "PATH": "/evil/bin",
                "ANTHROPIC_LOG": "debug",
                "ANTHROPIC_NUMBER": 1
            },
            "permissions": { "allow": ["Bash", "Write"], "defaultMode": "bypassPermissions" },
            "hooks": { "PreToolUse": [] },
            "mcpServers": { "x": {} },
            "enabledPlugins": { "p": true },
            "model": "opus"
        });
        assert_eq!(
            connection_settings(&user, Path::new(HOME)),
            Some(json!({
                "apiKeyHelper": "cd -- '/home/me' || exit 1\ncat ~/.config/proxy/key",
                "env": {
                    "ANTHROPIC_BASE_URL": "http://127.0.0.1:4141",
                    "ANTHROPIC_AUTH_TOKEN": "sk-proxy",
                    "HTTPS_PROXY": "http://proxy.local:3128"
                }
            }))
        );
        assert_eq!(
            connection_settings(&json!({"model": "opus"}), Path::new(HOME)),
            None
        );
        assert_eq!(
            connection_settings(&json!({"apiKeyHelper": "  "}), Path::new(HOME)),
            None
        );
    }

    /// Claude Code runs `apiKeyHelper` with `/bin/sh -c` in its own working
    /// directory, which during a guide run is the pull request's checkout.
    /// The forwarded helper must still resolve relative names from the
    /// user's directory, so a script the pull request adds never runs.
    #[cfg(unix)]
    #[test]
    fn the_forwarded_helper_never_runs_from_the_checkout() {
        let root = std::env::temp_dir().join(format!("loupe-helper-cwd-{}", std::process::id()));
        let checkout = root.join("checkout");
        // A quote and a space in the trusted path exercise the quoting.
        let trusted = root.join("user's home");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&trusted).unwrap();
        let marker = root.join("pwned");
        let evil = checkout.join("key.sh");
        crate::test_support::write_executable(
            &evil,
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );

        let forwarded =
            connection_settings(&json!({"apiKeyHelper": "pwd; ./key.sh"}), &trusted).unwrap();
        let script = forwarded["apiKeyHelper"].as_str().unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .current_dir(&checkout)
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&output.stdout);
        let trusted_real = std::fs::canonicalize(&trusted).unwrap();
        assert_eq!(printed.lines().next(), Some(trusted_real.to_str().unwrap()));
        assert!(!marker.exists(), "a script from the checkout ran");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_helper_runs_from_home_or_the_root() {
        let home = std::env::temp_dir();
        let env = |home: Option<&Path>| {
            let home = home.map(|h| OsString::from(h.as_os_str()));
            move |key: &str| (key == "HOME").then(|| home.clone()).flatten()
        };
        assert_eq!(helper_dir(env(Some(&home))), home);
        assert_eq!(helper_dir(env(None)), PathBuf::from("/"));
        assert_eq!(
            helper_dir(env(Some(Path::new("relative/home")))),
            PathBuf::from("/")
        );
    }

    #[test]
    fn the_settings_file_follows_claude_config_dir_then_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        assert_eq!(
            user_settings_path(env(&[("HOME", "/home/me"), ("CLAUDE_CONFIG_DIR", "/cfg")])),
            Some(PathBuf::from("/cfg/settings.json"))
        );
        assert_eq!(
            user_settings_path(env(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.claude/settings.json"))
        );
        assert_eq!(user_settings_path(env(&[])), None);
    }

    #[test]
    fn forwarded_settings_live_in_a_private_file_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("loupe-user-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("settings.json");
        std::fs::write(
            &source,
            r#"{"apiKeyHelper":"cat key","env":{"ANTHROPIC_AUTH_TOKEN":"sk-very-secret","GITHUB_TOKEN":"ghp_x"},"permissions":{"allow":["Bash"]}}"#,
        )
        .unwrap();

        let UserSettings::Forwarded(forwarded) =
            ForwardedSettings::load_from(&source, Path::new(HOME))
        else {
            panic!("expected forwarded settings");
        };
        let path = forwarded.path().to_path_buf();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "apiKeyHelper": "cd -- '/home/me' || exit 1\ncat key",
                "env": {"ANTHROPIC_AUTH_TOKEN": "sk-very-secret"}
            })
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        assert_eq!(
            forwarded.redact("API Error: 401 bad token sk-very-secret"),
            "API Error: 401 bad token ***"
        );
        drop(forwarded);
        assert!(!path.exists(), "the forwarded settings must be removed");

        std::fs::write(&source, "{not json").unwrap();
        let UserSettings::Unreadable { reason, .. } =
            ForwardedSettings::load_from(&source, Path::new(HOME))
        else {
            panic!("expected unreadable");
        };
        assert!(!reason.contains("not json"));
        assert!(matches!(
            ForwardedSettings::load_from(&dir.join("missing.json"), Path::new(HOME)),
            UserSettings::None
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
