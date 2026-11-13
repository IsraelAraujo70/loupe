/// Every way a GitHub call can fail, already classified at the boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The token is missing, revoked, or lacks a required scope (HTTP 401).
    #[error("GitHub rejected the token")]
    Unauthorized,
    /// A non-success HTTP status other than 401.
    #[error("GitHub returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    /// The GraphQL response carried `errors`.
    #[error("GitHub GraphQL error: {}", .0.join("; "))]
    GraphQl(Vec<String>),
    /// GitHub refused a merge or it failed in the background; the reason is
    /// GitHub's.
    #[error("GitHub did not merge the pull request: {0}")]
    MergeFailed(String),
    /// DNS, TLS, timeout, or connection failure.
    #[error("network error: {0}")]
    Transport(String),
    /// The response body did not match the expected shape.
    #[error("unexpected GitHub response: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, Error>;
