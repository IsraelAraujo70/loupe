//! Every way a Linear call can fail, and the one place that turns a failure
//! into words for the user.

use std::fmt;

use serde_json::Value;

/// Every way a Linear call can fail, already classified at the boundary.
///
/// `Display` is the plain message of [`Error::kind`]: it never carries the
/// API key, a response body, or Linear's own wording, so it is safe on screen
/// and in logs. The raw detail stays in the variants' fields for `Debug`.
#[derive(Debug)]
pub enum Error {
    /// The key is missing, invalid, or was revoked (HTTP 401, or
    /// `AUTHENTICATION_ERROR`).
    Unauthorized,
    /// The key is valid but may not do this (HTTP 403, or `FORBIDDEN`).
    Forbidden,
    /// Linear has nothing at that id, or this key cannot see it (HTTP 404, or
    /// an `Entity not found` error).
    NotFound,
    /// Too many requests (HTTP 429, or `RATELIMITED`).
    RateLimited,
    /// Linear failed on its side (HTTP 5xx, or `INTERNAL_SERVER_ERROR`).
    Server,
    /// Any other non-success HTTP status.
    Http { status: u16 },
    /// A GraphQL error that is none of the above, such as a refused input.
    GraphQl(Vec<String>),
    /// DNS, TLS, timeout, or connection failure.
    Transport(String),
    /// The response body did not match the expected shape.
    Decode(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What went wrong, in the terms the user can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Offline,
    KeyRejected,
    Forbidden,
    NotFound,
    RateLimited,
    Server,
    Other,
}

impl ErrorKind {
    /// A few words that name the problem.
    pub fn title(self) -> &'static str {
        match self {
            ErrorKind::Offline => "Can't reach Linear",
            ErrorKind::KeyRejected => "Linear rejected the API key",
            ErrorKind::Forbidden => "No permission in Linear",
            ErrorKind::NotFound => "Not found on Linear",
            ErrorKind::RateLimited => "Linear is limiting requests",
            ErrorKind::Server => "Linear had a problem",
            ErrorKind::Other => "Linear could not do that",
        }
    }

    /// What it means and what to do, as one sentence.
    pub fn detail(self) -> &'static str {
        match self {
            ErrorKind::Offline => "Check your connection, then try again.",
            ErrorKind::KeyRejected => "The key is invalid or was revoked. Replace it in Settings.",
            ErrorKind::Forbidden => "This key is not allowed to do that. Ask a workspace admin.",
            ErrorKind::NotFound => "It doesn't exist, or this key doesn't have access to it.",
            ErrorKind::RateLimited => "Wait a minute, then try again.",
            ErrorKind::Server => "It is on Linear's side. Try again in a moment.",
            ErrorKind::Other => "Try again. If it keeps failing, check Linear's status page.",
        }
    }

    /// The title and the detail as one sentence pair.
    pub fn message(self) -> String {
        format!("{}. {}", self.title(), self.detail())
    }

    /// Whether asking again can help. A missing page stays missing, and a
    /// rejected key stays rejected until the user replaces it.
    pub fn is_retryable(self) -> bool {
        !matches!(
            self,
            ErrorKind::NotFound | ErrorKind::KeyRejected | ErrorKind::Forbidden
        )
    }
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Unauthorized => ErrorKind::KeyRejected,
            Error::Forbidden => ErrorKind::Forbidden,
            Error::NotFound => ErrorKind::NotFound,
            Error::RateLimited => ErrorKind::RateLimited,
            Error::Server => ErrorKind::Server,
            Error::Transport(_) => ErrorKind::Offline,
            Error::Http { .. } | Error::GraphQl(_) | Error::Decode(_) => ErrorKind::Other,
        }
    }

    /// The title and the detail as one sentence pair.
    pub fn message(&self) -> String {
        self.kind().message()
    }

    /// Classifies a response that is not a plain success. `errors` is the
    /// GraphQL `errors` array, empty when the body had none.
    pub(crate) fn from_response(status: u16, errors: &[Value]) -> Error {
        let has_code = |code: &str| {
            errors
                .iter()
                .any(|error| error["extensions"]["code"] == code)
        };
        let messages: Vec<String> = errors
            .iter()
            .map(|error| {
                error["message"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_owned()
            })
            .collect();
        if has_code("AUTHENTICATION_ERROR") || status == 401 {
            Error::Unauthorized
        } else if has_code("FORBIDDEN") || status == 403 {
            Error::Forbidden
        } else if has_code("RATELIMITED") || status == 429 {
            Error::RateLimited
        } else if status == 404 || messages.iter().any(|m| m.starts_with("Entity not found")) {
            Error::NotFound
        } else if status >= 500 || has_code("INTERNAL_SERVER_ERROR") {
            Error::Server
        } else if !messages.is_empty() {
            Error::GraphQl(messages)
        } else {
            Error::Http { status }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Error, ErrorKind};

    fn coded(code: &str, message: &str) -> Vec<serde_json::Value> {
        vec![json!({ "message": message, "extensions": { "code": code } })]
    }

    #[test]
    fn a_response_is_classified_by_its_code_then_by_its_status() {
        let kind =
            |status, errors: &[serde_json::Value]| Error::from_response(status, errors).kind();
        assert_eq!(
            kind(200, &coded("AUTHENTICATION_ERROR", "x")),
            ErrorKind::KeyRejected
        );
        assert_eq!(kind(401, &[]), ErrorKind::KeyRejected);
        assert_eq!(kind(200, &coded("FORBIDDEN", "x")), ErrorKind::Forbidden);
        assert_eq!(kind(403, &[]), ErrorKind::Forbidden);
        assert_eq!(
            kind(400, &coded("RATELIMITED", "x")),
            ErrorKind::RateLimited
        );
        assert_eq!(kind(429, &[]), ErrorKind::RateLimited);
        assert_eq!(kind(404, &[]), ErrorKind::NotFound);
        assert_eq!(
            kind(200, &coded("INVALID_INPUT", "Entity not found: Issue")),
            ErrorKind::NotFound
        );
        assert_eq!(kind(500, &[]), ErrorKind::Server);
        assert_eq!(kind(502, &coded("BAD_GATEWAY", "x")), ErrorKind::Server);
        assert_eq!(
            kind(200, &coded("INTERNAL_SERVER_ERROR", "x")),
            ErrorKind::Server
        );
        assert_eq!(
            kind(200, &coded("INVALID_INPUT", "Title is required")),
            ErrorKind::Other
        );
        assert_eq!(kind(418, &[]), ErrorKind::Other);
    }

    #[test]
    fn a_network_failure_is_offline() {
        assert_eq!(Error::Transport("dns".into()).kind(), ErrorKind::Offline);
        assert_eq!(
            Error::Decode("missing field".into()).kind(),
            ErrorKind::Other
        );
    }

    #[test]
    fn each_kind_says_what_happened_and_what_to_do() {
        assert_eq!(
            Error::Transport("dns".into()).message(),
            "Can't reach Linear. Check your connection, then try again."
        );
        assert_eq!(
            Error::Unauthorized.message(),
            "Linear rejected the API key. The key is invalid or was revoked. Replace it in Settings."
        );
        assert_eq!(
            Error::NotFound.message(),
            "Not found on Linear. It doesn't exist, or this key doesn't have access to it."
        );
        assert_eq!(
            Error::Forbidden.message(),
            "No permission in Linear. This key is not allowed to do that. Ask a workspace admin."
        );
        assert_eq!(
            Error::RateLimited.message(),
            "Linear is limiting requests. Wait a minute, then try again."
        );
        assert_eq!(
            Error::Server.message(),
            "Linear had a problem. It is on Linear's side. Try again in a moment."
        );
        assert_eq!(
            Error::Decode("x".into()).message(),
            "Linear could not do that. Try again. If it keeps failing, check Linear's status page."
        );
    }

    #[test]
    fn only_what_can_change_on_its_own_is_worth_retrying() {
        assert!(ErrorKind::Offline.is_retryable());
        assert!(ErrorKind::RateLimited.is_retryable());
        assert!(ErrorKind::Server.is_retryable());
        assert!(!ErrorKind::NotFound.is_retryable());
        assert!(!ErrorKind::KeyRejected.is_retryable());
        assert!(!ErrorKind::Forbidden.is_retryable());
    }

    #[test]
    fn the_message_never_carries_the_key_nor_the_body() {
        let secret = "lin_api_SECRETKEY";
        let errors = [
            Error::Transport(format!("connect failed for {secret}")),
            Error::GraphQl(vec![secret.into()]),
            Error::Decode(secret.into()),
            Error::from_response(200, &coded("INVALID_INPUT", secret)),
            Error::from_response(500, &coded("X", secret)),
        ];
        for error in errors {
            assert!(!error.to_string().contains(secret), "{error:?}");
            assert!(!error.message().contains("lin_api"), "{error:?}");
        }
    }
}
