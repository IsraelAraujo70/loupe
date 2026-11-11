//! Blocking credential checks and the GitHub OAuth device flow. No GPUI here:
//! every function runs on a background thread through `services::background`.
//! Tokens never reach logs, `Debug` output, or error messages.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use loupe_github::{GitHub, Viewer};
use serde_json::Value;

use crate::store::{self, Secret};

const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const USER_AGENT: &str = concat!("Loupe/", env!("CARGO_PKG_VERSION"));
/// GitHub's default device-flow polling interval, and the step `slow_down` adds.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// Prism's public OAuth app, shared so device-flow sign-in needs no setup.
pub const GITHUB_CLIENT_ID: &str = "Ov23livH5iIGo31MqRiB";
pub const GITHUB_SCOPES: &str = "repo read:org notifications";
pub const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Where a working GitHub token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenSource {
    Stored,
    Environment,
    GhCli,
}

/// A GitHub token that `GitHub::viewer()` accepted. No `Debug`: it holds a token.
pub struct DiscoveredToken {
    pub token: String,
    pub source: TokenSource,
    pub viewer: Viewer,
}

/// Tries, in order, the stored secret, `GITHUB_TOKEN`, and `gh auth token`
/// (`/opt/homebrew/bin/gh`, `/usr/local/bin/gh`, then `gh` on `PATH`), and
/// returns the first one `GitHub::viewer()` accepts. Only a rejected token
/// moves on to the next candidate. Any other failure (offline, a 5xx) stops
/// with its message: the next candidate may belong to another account, and
/// every write would then go out under that identity.
pub fn discover_github_token() -> Result<Option<DiscoveredToken>, String> {
    let stored = store::load_secret(Secret::GitHubToken).map(|token| (token, TokenSource::Stored));
    let environment = std::env::var("GITHUB_TOKEN")
        .ok()
        .map(|token| (token, TokenSource::Environment));
    let from_cli = || gh_cli_token().map(|token| (token, TokenSource::GhCli));

    let candidates = [stored, environment]
        .into_iter()
        .flatten()
        .chain(std::iter::once_with(from_cli).flatten());
    for (token, source) in candidates {
        let token = token.trim().to_owned();
        if token.is_empty() {
            continue;
        }
        match GitHub::new(token.clone()).viewer() {
            Ok(viewer) => {
                return Ok(Some(DiscoveredToken {
                    token,
                    source,
                    viewer,
                }));
            }
            Err(loupe_github::Error::Unauthorized) => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(None)
}

/// Runs `gh auth token`. A Finder-launched app has a minimal `PATH`, so the
/// Homebrew locations come first. Only the first `gh` that prints a token wins;
/// whether the token works is the caller's check.
fn gh_cli_token() -> Option<String> {
    let fixed = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
        .into_iter()
        .filter(|path| Path::new(path).exists());
    fixed.chain(std::iter::once("gh")).find_map(|program| {
        let output = Command::new(program)
            .args(["auth", "token"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
        (!token.is_empty()).then_some(token)
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// POSTs a form to GitHub's OAuth endpoints and returns the JSON body. GitHub
/// answers OAuth errors with HTTP 200 and an `error` field, so the status is
/// not inspected; a body that is not JSON is a decode error.
fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<Value, AuthError> {
    let mut response = agent()
        .post(url)
        .header("Accept", "application/json")
        .send_form(fields.iter().copied())
        .map_err(|error| AuthError::Transport(error.to_string()))?;
    response
        .body_mut()
        .read_json::<Value>()
        .map_err(|error| AuthError::Decode(error.to_string()))
}

/// What `POST /login/device/code` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: Duration,
    pub interval: Duration,
}

/// One poll of `POST /login/oauth/access_token`. No `Debug`: `Granted` holds a token.
#[derive(PartialEq, Eq)]
pub enum PollOutcome {
    /// `authorization_pending`: poll again after the current interval.
    Pending,
    /// `slow_down`: poll again after `interval` (GitHub's new value).
    SlowDown {
        interval: Duration,
    },
    /// `expired_token`: the user code expired; start over.
    Expired,
    /// `access_denied`: the user cancelled on github.com.
    Denied,
    Granted {
        token: String,
    },
}

/// A device-flow call that failed outside the normal poll states.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("network error: {0}")]
    Transport(String),
    #[error("unexpected GitHub response: {0}")]
    Decode(String),
    /// GitHub answered with an OAuth error code other than the poll states.
    #[error("GitHub refused the request: {0}")]
    Rejected(String),
}

/// Starts the device flow: asks GitHub for a user code to show and a device
/// code to poll with.
pub fn request_device_code() -> Result<DeviceCode, AuthError> {
    let body = post_form(
        DEVICE_CODE_URL,
        &[("client_id", GITHUB_CLIENT_ID), ("scope", GITHUB_SCOPES)],
    )?;
    parse_device_code(&body)
}

/// Polls once for the access token. The caller waits the current interval
/// between calls and stops on any outcome but `Pending` and `SlowDown`.
pub fn poll_device_token(device_code: &str) -> Result<PollOutcome, AuthError> {
    let body = post_form(
        ACCESS_TOKEN_URL,
        &[
            ("client_id", GITHUB_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", GRANT_TYPE),
        ],
    )?;
    parse_poll(&body)
}

fn parse_device_code(body: &Value) -> Result<DeviceCode, AuthError> {
    if let Some(error) = oauth_error(body) {
        return Err(error);
    }
    let text = |key: &str| {
        body[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| AuthError::Decode(format!("missing `{key}`")))
    };
    let seconds = |key: &str| {
        body[key]
            .as_u64()
            .map(Duration::from_secs)
            .ok_or_else(|| AuthError::Decode(format!("missing `{key}`")))
    };
    Ok(DeviceCode {
        device_code: text("device_code")?,
        user_code: text("user_code")?,
        verification_uri: text("verification_uri")?,
        expires_in: seconds("expires_in")?,
        interval: seconds("interval")?,
    })
}

fn parse_poll(body: &Value) -> Result<PollOutcome, AuthError> {
    if let Some(token) = body["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
    {
        return Ok(PollOutcome::Granted {
            token: token.to_owned(),
        });
    }
    let Some(code) = body["error"].as_str() else {
        return Err(AuthError::Decode(
            "neither `access_token` nor `error` in the response".into(),
        ));
    };
    match code {
        "authorization_pending" => Ok(PollOutcome::Pending),
        "slow_down" => Ok(PollOutcome::SlowDown {
            interval: body["interval"]
                .as_u64()
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_INTERVAL + DEFAULT_INTERVAL),
        }),
        "expired_token" => Ok(PollOutcome::Expired),
        "access_denied" => Ok(PollOutcome::Denied),
        _ => Err(rejected(code, body)),
    }
}

/// The `error` + `error_description` GitHub puts in a 200 response, if any.
fn oauth_error(body: &Value) -> Option<AuthError> {
    body["error"].as_str().map(|code| rejected(code, body))
}

fn rejected(code: &str, body: &Value) -> AuthError {
    match body["error_description"].as_str() {
        Some(description) if !description.is_empty() => {
            AuthError::Rejected(format!("{code}: {description}"))
        }
        _ => AuthError::Rejected(code.to_owned()),
    }
}

/// Validates a Linear API key: the viewer, with their workspace, or the error.
pub fn linear_account(api_key: &str) -> Result<loupe_linear::Viewer, loupe_linear::Error> {
    loupe_linear::Linear::new(api_key).viewer()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::{
        AuthError, PollOutcome, parse_device_code, parse_poll, poll_device_token,
        request_device_code,
    };

    #[test]
    fn device_code_response_is_parsed() {
        let body = json!({
            "device_code": "dc_FAKE",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        });
        let code = parse_device_code(&body).expect("parses");
        assert_eq!(code.device_code, "dc_FAKE");
        assert_eq!(code.user_code, "WDJB-MJHT");
        assert_eq!(code.verification_uri, "https://github.com/login/device");
        assert_eq!(code.expires_in, Duration::from_secs(900));
        assert_eq!(code.interval, Duration::from_secs(5));
    }

    #[test]
    fn device_code_error_and_missing_fields_are_errors() {
        let refused = json!({
            "error": "device_flow_disabled",
            "error_description": "Device Flow must be explicitly enabled for this App"
        });
        match parse_device_code(&refused) {
            Err(AuthError::Rejected(message)) => {
                assert!(message.starts_with("device_flow_disabled: "))
            }
            _ => panic!("expected Rejected"),
        }
        assert!(matches!(
            parse_device_code(&json!({ "user_code": "X" })),
            Err(AuthError::Decode(_))
        ));
    }

    #[test]
    fn poll_maps_every_documented_state() {
        let granted = json!({
            "access_token": "gho_FAKE",
            "token_type": "bearer",
            "scope": "repo"
        });
        assert!(
            matches!(parse_poll(&granted), Ok(PollOutcome::Granted { token }) if token == "gho_FAKE")
        );
        assert!(matches!(
            parse_poll(&json!({ "error": "authorization_pending" })),
            Ok(PollOutcome::Pending)
        ));
        assert!(matches!(
            parse_poll(&json!({ "error": "expired_token" })),
            Ok(PollOutcome::Expired)
        ));
        assert!(matches!(
            parse_poll(&json!({ "error": "access_denied" })),
            Ok(PollOutcome::Denied)
        ));
    }

    #[test]
    fn slow_down_uses_github_interval_or_adds_five_seconds() {
        let with_interval = json!({ "error": "slow_down", "interval": 15 });
        assert!(matches!(
            parse_poll(&with_interval),
            Ok(PollOutcome::SlowDown { interval }) if interval == Duration::from_secs(15)
        ));
        assert!(matches!(
            parse_poll(&json!({ "error": "slow_down" })),
            Ok(PollOutcome::SlowDown { interval }) if interval == Duration::from_secs(10)
        ));
    }

    #[test]
    fn other_poll_errors_are_rejected_with_the_description() {
        let body = json!({
            "error": "incorrect_device_code",
            "error_description": "The device_code provided is not valid."
        });
        match parse_poll(&body) {
            Err(AuthError::Rejected(message)) => assert_eq!(
                message,
                "incorrect_device_code: The device_code provided is not valid."
            ),
            _ => panic!("expected Rejected"),
        }
        assert!(matches!(parse_poll(&json!({})), Err(AuthError::Decode(_))));
    }

    /// Live: asks GitHub for a device code and polls once. Grants nothing: the
    /// code is never authorized, so the poll must be `Pending`.
    #[test]
    #[ignore = "calls github.com"]
    fn device_flow_polls_pending_live() {
        let code = request_device_code().expect("device code");
        assert!(!code.user_code.is_empty());
        let outcome = poll_device_token(&code.device_code).expect("poll");
        assert!(matches!(outcome, PollOutcome::Pending));
    }
}
