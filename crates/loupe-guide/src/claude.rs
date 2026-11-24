//! Hardened Claude Code CLI integration: executable discovery, auth check,
//! strictly read-only command construction, and `stream-json` event parsing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Exact set of built-in Claude Code tools permitted during guided review.
/// Read-only filesystem inspection inside the temporary checkout only.
pub const READ_ONLY_TOOLS: &str = "Read,Grep,Glob";

/// Explicit defense-in-depth deny list: no shell, no file writes, no network
/// fetch/search, no subagents, no skills/slash commands, and no MCP servers.
pub const DISALLOWED_TOOLS: &str = "Bash,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Agent,Task,TodoWrite,Skill,SlashCommand,mcp__*";

/// Tools allowed to appear in Claude Code's `init` event.
pub const PERMITTED_INIT_TOOLS: [&str; 4] = ["Read", "Grep", "Glob", "StructuredOutput"];

/// Model choice for local guided review generation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuideModel {
    #[default]
    Sonnet,
    Opus,
    Haiku,
    Custom(String),
}

impl GuideModel {
    pub const PRESETS: [(&'static str, &'static str); 3] = [
        ("sonnet", "Claude Sonnet (recommended)"),
        ("opus", "Claude Opus"),
        ("haiku", "Claude Haiku (fast)"),
    ];

    pub fn as_cli_arg(&self) -> &str {
        match self {
            GuideModel::Sonnet => "sonnet",
            GuideModel::Opus => "opus",
            GuideModel::Haiku => "haiku",
            GuideModel::Custom(name) => name.trim(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            GuideModel::Sonnet => "Claude Sonnet".into(),
            GuideModel::Opus => "Claude Opus".into(),
            GuideModel::Haiku => "Claude Haiku".into(),
            GuideModel::Custom(name) => name.clone(),
        }
    }
}

/// Reasoning effort passed to Claude Code's `--effort` flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuideEffort {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl GuideEffort {
    pub const ALL: [GuideEffort; 5] = [
        GuideEffort::Low,
        GuideEffort::Medium,
        GuideEffort::High,
        GuideEffort::XHigh,
        GuideEffort::Max,
    ];

    pub fn as_cli_arg(self) -> &'static str {
        match self {
            GuideEffort::Low => "low",
            GuideEffort::Medium => "medium",
            GuideEffort::High => "high",
            GuideEffort::XHigh => "xhigh",
            GuideEffort::Max => "max",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GuideEffort::Low => "Low",
            GuideEffort::Medium => "Medium",
            GuideEffort::High => "High",
            GuideEffort::XHigh => "Extra high",
            GuideEffort::Max => "Max",
        }
    }
}

/// User configuration for local Claude Code guide generation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeConfig {
    pub model: GuideModel,
    pub effort: GuideEffort,
    /// Optional explicit path to the `claude` binary; `None` searches `PATH`
    /// and standard install locations.
    pub executable: Option<String>,
}

impl ClaudeConfig {
    pub fn executable_path(&self) -> Option<&Path> {
        self.executable
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Path::new)
    }
}

/// Result of probing the local Claude Code installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeAvailability {
    Ready { path: PathBuf, version: String },
    NotInstalled { tried: String },
    NotLoggedIn { path: PathBuf, detail: String },
}

/// Resolves the `claude` executable path from an explicit setting, `PATH`, or
/// common user installation directories (important for GUI apps on macOS).
pub fn resolve_executable(custom: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = custom {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        return None;
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(executable_name());
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let mut fallbacks = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        fallbacks.push(home.join(".local/bin").join(executable_name()));
        fallbacks.push(home.join(".local/share/mise/shims").join(executable_name()));
        fallbacks.push(home.join(".claude/local").join(executable_name()));
    }
    fallbacks.push(PathBuf::from("/opt/homebrew/bin").join(executable_name()));
    fallbacks.push(PathBuf::from("/usr/local/bin").join(executable_name()));

    fallbacks.into_iter().find(|p| p.is_file())
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    }
}

/// Checks whether Claude Code is installed and authenticated.
/// Never logs or returns tokens or credentials.
pub fn check_availability(custom: Option<&Path>) -> ClaudeAvailability {
    let Some(path) = resolve_executable(custom) else {
        let tried = custom
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "claude (on PATH)".into());
        return ClaudeAvailability::NotInstalled { tried };
    };

    let version = match run_quick(&path, &["--version"], Duration::from_secs(5)) {
        Ok(out) if out.status_ok => out.stdout.trim().to_owned(),
        _ => {
            return ClaudeAvailability::NotInstalled {
                tried: path.display().to_string(),
            };
        }
    };

    match run_quick(
        &path,
        &["auth", "status", "--json"],
        Duration::from_secs(10),
    ) {
        Ok(out) => {
            let logged_in = serde_json::from_str::<Value>(&out.stdout)
                .ok()
                .and_then(|v| v.get("loggedIn")?.as_bool())
                .unwrap_or(false);
            if logged_in {
                ClaudeAvailability::Ready { path, version }
            } else {
                ClaudeAvailability::NotLoggedIn {
                    path,
                    detail: "Claude Code is installed, but no active login was detected. Run `claude auth login` in your terminal.".into(),
                }
            }
        }
        Err(reason) => ClaudeAvailability::NotLoggedIn {
            path,
            detail: format!("Could not verify Claude Code login status ({reason})."),
        },
    }
}

struct QuickOutput {
    status_ok: bool,
    stdout: String,
}

fn run_quick(exe: &Path, args: &[&str], timeout: Duration) -> Result<QuickOutput, String> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_safe_env(&mut cmd);

    let mut child = cmd.spawn().map_err(|err| err.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().map_err(|err| err.to_string())?;
                return Ok(QuickOutput {
                    status_ok: output.status.success(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(err) => return Err(err.to_string()),
        }
    }
}

/// Environment variables allowed into the Claude Code subprocess.
/// Excludes `GITHUB_TOKEN`, `GH_TOKEN`, `LINEAR_API_KEY`, `LOUPE_DATA_DIR`, etc.
const SAFE_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

fn apply_safe_env(cmd: &mut Command) {
    let preserved: Vec<(&str, OsString)> = SAFE_ENV_ALLOWLIST
        .iter()
        .filter_map(|&key| std::env::var_os(key).map(|val| (key, val)))
        .collect();
    cmd.env_clear();
    for (key, val) in preserved {
        cmd.env(key, val);
    }
    cmd.env("CLAUDE_CODE_RESTRICTED", "1");
    cmd.env("CLAUDE_CODE_SAFE_MODE", "1");
    cmd.env("CLAUDE_CODE_SKIP_PROMPT_HISTORY", "1");
    // Defense-in-depth in case a future CLI release invokes git inside the checkout.
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_COUNT", "2");
    cmd.env("GIT_CONFIG_KEY_0", "core.fsmonitor");
    cmd.env("GIT_CONFIG_VALUE_0", "false");
    cmd.env("GIT_CONFIG_KEY_1", "core.hooksPath");
    cmd.env("GIT_CONFIG_VALUE_1", "/dev/null");
}

/// Builds the hardened headless Claude Code command for a temporary checkout.
///
/// The prompt is fed through `stdin` (never argv) so large PR diffs never hit
/// OS argument limits and untrusted content never appears in process listings.
pub fn build_command(
    exe: &Path,
    checkout_dir: &Path,
    config: &ClaudeConfig,
    schema_json: &str,
    system_prompt: &str,
) -> Command {
    let mut cmd = Command::new(exe);
    cmd.current_dir(checkout_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            config.model.as_cli_arg(),
            "--effort",
            config.effort.as_cli_arg(),
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--disable-slash-commands",
            "--no-session-persistence",
            "--no-chrome",
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
            "--tools",
            READ_ONLY_TOOLS,
            "--allowedTools",
            READ_ONLY_TOOLS,
            "--disallowedTools",
            DISALLOWED_TOOLS,
            "--system-prompt",
            system_prompt,
            "--json-schema",
            schema_json,
        ]);
    apply_safe_env(&mut cmd);
    cmd
}

/// Parsed event from Claude Code's `--output-format stream-json --verbose` stream.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Init {
        model: String,
        tools: Vec<String>,
    },
    Progress(String),
    Result {
        is_error: bool,
        error_message: Option<String>,
        structured_output: Option<Value>,
        model: Option<String>,
        duration_ms: Option<u64>,
    },
}

/// Parses one NDJSON line emitted by Claude Code `--output-format stream-json`.
pub fn parse_stream_line(line: &str) -> Option<StreamEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let event_type = value.get("type")?.as_str()?;

    match event_type {
        "system" => {
            if value.get("subtype").and_then(Value::as_str) != Some("init") {
                return None;
            }
            let model = value
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("claude")
                .to_owned();
            let tools = value
                .get("tools")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            Some(StreamEvent::Init { model, tools })
        }
        "assistant" => {
            let content = value.get("message")?.get("content")?.as_array()?;
            for item in content {
                if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let name = item.get("name").and_then(Value::as_str)?;
                let input = item.get("input");
                let label = match name {
                    "Read" => {
                        let file = input
                            .and_then(|v| v.get("file_path"))
                            .and_then(Value::as_str)
                            .map(short_display_path);
                        match file {
                            Some(path) => format!("Reading {path}"),
                            None => "Reading repository files…".into(),
                        }
                    }
                    "Grep" => {
                        let pat = input.and_then(|v| v.get("pattern")).and_then(Value::as_str);
                        match pat {
                            Some(pat) => format!("Searching for `{}`", truncate_label(pat, 36)),
                            None => "Searching repository…".into(),
                        }
                    }
                    "Glob" => "Inspecting repository layout…".into(),
                    "StructuredOutput" => "Formatting guided review…".into(),
                    _ => format!("Running {name}…"),
                };
                return Some(StreamEvent::Progress(label));
            }
            None
        }
        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let structured_output = value.get("structured_output").cloned();
            let error_message = if is_error {
                value
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        value
                            .get("subtype")
                            .and_then(Value::as_str)
                            .map(|s| format!("Claude Code exited with {s}"))
                    })
            } else {
                None
            };
            let model = value
                .get("modelUsage")
                .and_then(Value::as_object)
                .and_then(|map| map.keys().next().cloned());
            let duration_ms = value.get("duration_ms").and_then(Value::as_u64);
            Some(StreamEvent::Result {
                is_error,
                error_message,
                structured_output,
                model,
                duration_ms,
            })
        }
        _ => None,
    }
}

/// Returns any unexpected tool names found in Claude Code's `init` tool list.
pub fn unexpected_init_tools(tools: &[String]) -> Vec<String> {
    tools
        .iter()
        .filter(|tool| !PERMITTED_INIT_TOOLS.contains(&tool.as_str()))
        .cloned()
        .collect()
}

fn short_display_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 3 {
        path.to_owned()
    } else {
        format!("…/{}", parts[parts.len() - 3..].join("/"))
    }
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        ClaudeConfig, DISALLOWED_TOOLS, GuideEffort, GuideModel, READ_ONLY_TOOLS, StreamEvent,
        build_command, parse_stream_line, unexpected_init_tools,
    };

    #[test]
    fn build_command_enforces_read_only_sandbox_and_scrubs_secrets() {
        let config = ClaudeConfig {
            model: GuideModel::Sonnet,
            effort: GuideEffort::Medium,
            executable: None,
        };
        let cmd = build_command(
            Path::new("/usr/bin/claude"),
            Path::new("/tmp/checkout"),
            &config,
            r#"{"type":"object"}"#,
            "system",
        );

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-p".to_owned()));
        assert!(args.contains(&"--restricted".to_owned()));
        assert!(args.contains(&"--safe-mode".to_owned()));
        assert!(args.contains(&"--strict-mcp-config".to_owned()));
        assert!(args.contains(&"--disable-slash-commands".to_owned()));
        assert!(args.contains(&"--no-session-persistence".to_owned()));
        assert!(args.contains(&"--no-chrome".to_owned()));
        assert!(!args.contains(&"--add-dir".to_owned()));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_owned()));
        assert!(!args.contains(&"bypassPermissions".to_owned()));

        let arg_value = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|ix| args.get(ix + 1))
                .map(String::as_str)
        };
        assert_eq!(arg_value("--tools"), Some(READ_ONLY_TOOLS));
        assert_eq!(arg_value("--allowedTools"), Some(READ_ONLY_TOOLS));
        assert_eq!(arg_value("--disallowedTools"), Some(DISALLOWED_TOOLS));
        assert_eq!(arg_value("--permission-mode"), Some("dontAsk"));
        assert_eq!(arg_value("--permission-prompts"), Some("none"));
        assert_eq!(arg_value("--output-format"), Some("stream-json"));
        assert_eq!(arg_value("--model"), Some("sonnet"));
        assert_eq!(arg_value("--effort"), Some("medium"));

        // Verify no write, network, or shell tool is in `--tools`.
        for forbidden in ["Bash", "Edit", "Write", "WebFetch", "WebSearch", "Agent"] {
            assert!(
                !READ_ONLY_TOOLS.contains(forbidden),
                "{forbidden} must never be allowed"
            );
            assert!(
                DISALLOWED_TOOLS.contains(forbidden),
                "{forbidden} must be explicitly disallowed"
            );
        }

        // Verify sensitive environment variables are never inherited.
        let envs: Vec<String> = cmd
            .get_envs()
            .filter_map(|(k, v)| v.map(|_| k.to_string_lossy().into_owned()))
            .collect();
        for secret_env in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "LINEAR_API_KEY",
            "LOUPE_DATA_DIR",
        ] {
            assert!(
                !envs.contains(&secret_env.to_owned()),
                "{secret_env} must not leak to Claude Code"
            );
        }
        assert!(envs.contains(&"CLAUDE_CODE_RESTRICTED".to_owned()));
        assert!(envs.contains(&"CLAUDE_CODE_SAFE_MODE".to_owned()));
    }

    #[test]
    fn parses_stream_json_init_progress_and_result_events() {
        let init = r#"{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":["Glob","Grep","Read","StructuredOutput"]}"#;
        assert_eq!(
            parse_stream_line(init),
            Some(StreamEvent::Init {
                model: "claude-sonnet-5-5".into(),
                tools: vec![
                    "Glob".into(),
                    "Grep".into(),
                    "Read".into(),
                    "StructuredOutput".into()
                ],
            })
        );

        let tool_use = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/checkout/crates/loupe-app/src/main.rs"}}]}}"#;
        assert_eq!(
            parse_stream_line(tool_use),
            Some(StreamEvent::Progress(
                "Reading …/loupe-app/src/main.rs".into()
            ))
        );

        let result = r#"{"type":"result","is_error":false,"structured_output":{"schema_version":1},"modelUsage":{"claude-sonnet-5-5":{}},"duration_ms":4200}"#;
        assert_eq!(
            parse_stream_line(result),
            Some(StreamEvent::Result {
                is_error: false,
                error_message: None,
                structured_output: Some(serde_json::json!({"schema_version": 1})),
                model: Some("claude-sonnet-5-5".into()),
                duration_ms: Some(4200),
            })
        );
    }

    #[test]
    fn flags_any_unexpected_tool_in_init_event() {
        assert!(
            unexpected_init_tools(&[
                "Read".into(),
                "Grep".into(),
                "Glob".into(),
                "StructuredOutput".into()
            ])
            .is_empty()
        );
        assert_eq!(
            unexpected_init_tools(&["Read".into(), "Bash".into(), "Write".into()]),
            vec!["Bash".to_owned(), "Write".to_owned()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawned_child_does_not_inherit_non_allowlisted_env_vars() {
        use std::os::unix::fs::PermissionsExt;

        // `CARGO_MANIFEST_DIR` is guaranteed to exist in the parent `cargo test`
        // environment and is NOT in `SAFE_ENV_ALLOWLIST`.
        assert!(
            std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
            "parent test process must have CARGO_MANIFEST_DIR set"
        );

        let stub = std::env::temp_dir().join(format!(
            "loupe-env-probe-{}-{}.sh",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::write(&stub, "#!/bin/sh\nenv\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut cmd = build_command(
            &stub,
            &std::env::temp_dir(),
            &ClaudeConfig::default(),
            r#"{"type":"object"}"#,
            "system",
        );
        let output = cmd.output().expect("spawn env probe");
        let _ = std::fs::remove_file(&stub);

        assert!(output.status.success());
        let env_dump = String::from_utf8_lossy(&output.stdout);
        assert!(
            !env_dump
                .lines()
                .any(|l| l.starts_with("CARGO_MANIFEST_DIR=")),
            "env_clear() must prevent parent env vars from leaking into Claude Code"
        );
        assert!(env_dump.lines().any(|l| l == "CLAUDE_CODE_RESTRICTED=1"));
        assert!(env_dump.lines().any(|l| l == "CLAUDE_CODE_SAFE_MODE=1"));
        assert!(env_dump.lines().any(|l| l == "GIT_CONFIG_GLOBAL=/dev/null"));
        assert!(env_dump.lines().any(|l| l == "GIT_CONFIG_NOSYSTEM=1"));
    }
}
