//! Transport: requests, status classification, and decoding into wire types.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::wire::{self, EnvironmentWire, ProjectsWire, SessionWire, ShellWire};
use crate::{
    Environment, Error, LinkedThread, Project, PullRequestKey, Result, ReviewFacts, Session, T3,
};

const TIMEOUT: Duration = Duration::from_secs(30);
/// A shell snapshot lists every thread and project, which runs to several MB.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;
const PROTOCOL_HEADER: &str = "x-t3-orchestration-protocol";
const PROTOCOL_VERSION: &str = crate::rpc::PROTOCOL_VERSION;
const MESSAGE_PREFIX_CHARS: usize = 200;

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("Loupe/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub(crate) fn environment(t3: &T3) -> Result<Environment> {
    let wire: EnvironmentWire = get(t3, "/.well-known/t3/environment", Auth::None)?;
    Ok(wire.into())
}

pub(crate) fn linked_threads(t3: &T3, pr: &PullRequestKey) -> Result<Vec<LinkedThread>> {
    let shell: ShellWire = get(t3, "/api/orchestration/shell", Auth::Orchestration)?;
    Ok(wire::linked_threads(shell, pr))
}

pub(crate) fn session(t3: &T3) -> Result<Session> {
    let wire: SessionWire = get(t3, "/api/auth/session", Auth::Orchestration)?;
    wire.into_session()
}

pub(crate) fn projects(t3: &T3) -> Result<Vec<Project>> {
    let wire: ProjectsWire = get(t3, "/api/projects", Auth::Orchestration)?;
    Ok(wire.into_projects())
}

pub(crate) fn review_facts(t3: &T3, models: &[&str]) -> Result<ReviewFacts> {
    let shell: ShellWire = get(t3, "/api/orchestration/shell", Auth::Orchestration)?;
    Ok(wire::review_facts(shell, models))
}

#[derive(Clone, Copy)]
enum Auth {
    None,
    Orchestration,
}

fn get<T: DeserializeOwned>(t3: &T3, path: &str, auth: Auth) -> Result<T> {
    let mut request = t3.agent.get(format!("{}{path}", t3.base_url));
    if let Auth::Orchestration = auth {
        request = request
            .header("Authorization", format!("Bearer {}", t3.token))
            .header(PROTOCOL_HEADER, PROTOCOL_VERSION);
    }
    let mut response = request
        .call()
        .map_err(|error| Error::Transport(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_vec()
        .map_err(|error| Error::Transport(error.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(classify(status, &body));
    }
    serde_json::from_slice(&body).map_err(|error| Error::Decode(error.to_string()))
}

/// Turns a non-2xx answer into an [`Error`]. The body is the server's
/// `{_tag, code, reason | requiredScope, traceId}` envelope when it has one.
pub(crate) fn classify(status: u16, body: &[u8]) -> Error {
    let envelope: Option<Value> = serde_json::from_slice(body).ok();
    let field = |name: &str| {
        envelope
            .as_ref()
            .and_then(|value| value.get(name))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    match status {
        401 => Error::Unauthorized,
        403 if field("code") == Some("insufficient_scope") && field("requiredScope").is_some() => {
            Error::MissingScope(field("requiredScope").unwrap_or_default().to_owned())
        }
        _ => {
            let message = field("reason")
                .or_else(|| field("code"))
                .map(str::to_owned)
                .unwrap_or_else(|| body_prefix(body));
            Error::Http { status, message }
        }
    }
}

fn body_prefix(body: &[u8]) -> String {
    String::from_utf8_lossy(body)
        .trim()
        .chars()
        .take(MESSAGE_PREFIX_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::classify;
    use crate::Error;

    const UNAUTHORIZED: &str = include_str!("../tests/fixtures/error-unauthorized.json");
    const SCOPE: &str = include_str!("../tests/fixtures/error-scope.json");
    const BAD_REQUEST: &str = include_str!("../tests/fixtures/error-bad-request.json");

    #[test]
    fn unauthorized_body_is_unauthorized() {
        assert!(matches!(
            classify(401, UNAUTHORIZED.as_bytes()),
            Error::Unauthorized
        ));
    }

    #[test]
    fn insufficient_scope_names_the_required_scope() {
        match classify(403, SCOPE.as_bytes()) {
            Error::MissingScope(scope) => assert_eq!(scope, "orchestration:read"),
            other => panic!("expected MissingScope, got {other:?}"),
        }
    }

    #[test]
    fn a_404_is_a_plain_http_error() {
        match classify(404, b"Not Found") {
            Error::Http { status, message } => {
                assert_eq!(status, 404);
                assert_eq!(message, "Not Found");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn other_statuses_carry_the_reason() {
        match classify(400, BAD_REQUEST.as_bytes()) {
            Error::Http { status, message } => {
                assert_eq!(status, 400);
                assert_eq!(message, "invalid_history_cursor");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn forbidden_without_a_scope_is_a_plain_http_error() {
        match classify(403, br#"{"code":"forbidden"}"#) {
            Error::Http { status, message } => {
                assert_eq!(status, 403);
                assert_eq!(message, "forbidden");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn a_body_that_is_not_json_becomes_a_short_prefix() {
        let body = "x".repeat(500);
        match classify(502, body.as_bytes()) {
            Error::Http { status, message } => {
                assert_eq!(status, 502);
                assert_eq!(message.chars().count(), 200);
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_body_gives_an_empty_message() {
        match classify(500, b"") {
            Error::Http { status, message } => {
                assert_eq!(status, 500);
                assert!(message.is_empty());
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }
}
