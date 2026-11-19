use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::rate::RateLimits;
use crate::{Error, Result};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const USER_AGENT: &str = concat!("Loupe/", env!("CARGO_PKG_VERSION"));
pub(crate) const VIEWER_QUERY: &str = "query Viewer { viewer { login name avatarUrl } }";

/// An authenticated GitHub API client. Cheap to clone; share it behind `Arc`.
#[derive(Clone)]
pub struct GitHub {
    pub(crate) token: String,
    pub(crate) agent: ureq::Agent,
    /// What the latest responses said about the rate limits, shared by the
    /// clones.
    rate: Arc<Mutex<RateLimits>>,
}

/// The signed-in user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    pub login: String,
    pub name: Option<String>,
    #[serde(rename = "avatarUrl")]
    pub avatar_url: String,
}

impl GitHub {
    pub fn new(token: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            token: token.into(),
            agent,
            rate: Arc::default(),
        }
    }

    /// The rate limits as the latest REST and GraphQL responses reported
    /// them.
    pub fn rate_limits(&self) -> RateLimits {
        self.rate.lock().map(|rate| *rate).unwrap_or_default()
    }

    pub(crate) fn record_rate(&self, headers: &ureq::http::HeaderMap) {
        if let Ok(mut rate) = self.rate.lock() {
            rate.record(headers);
        }
    }

    /// With `LOUPE_TRACE_GITHUB` set, prints one line per answered request
    /// to stderr: what was asked, the status, how long it took, and the
    /// rate-limit bucket it used. For measuring how much the app asks.
    pub(crate) fn trace(
        &self,
        what: &str,
        status: u16,
        started: Instant,
        headers: &ureq::http::HeaderMap,
    ) {
        if std::env::var_os("LOUPE_TRACE_GITHUB").is_none() {
            return;
        }
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-")
                .to_owned()
        };
        eprintln!(
            "github-call at={} what={what} status={status} ms={} bucket={} remaining={}",
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            started.elapsed().as_millis(),
            header("x-ratelimit-resource"),
            header("x-ratelimit-remaining"),
        );
    }

    pub fn viewer(&self) -> Result<Viewer> {
        #[derive(Deserialize)]
        struct Data {
            viewer: Viewer,
        }
        let data: Data = self.graphql(VIEWER_QUERY, json!({}))?;
        Ok(data.viewer)
    }

    /// Runs one GraphQL document and decodes `data` into `T`.
    pub(crate) fn graphql<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        if std::env::var_os("LOUPE_OFFLINE").is_some() {
            return Err(Error::Transport("Offline mode".into()));
        }
        let started = Instant::now();
        let mut response = self
            .agent
            .post(GRAPHQL_URL)
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(json!({ "query": query, "variables": variables }))
            .map_err(|error| Error::Transport(error.to_string()))?;
        self.record_rate(response.headers());
        let status = response.status().as_u16();
        self.trace(
            &format!("graphql:{}", operation_name(query)),
            status,
            started,
            response.headers(),
        );
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| Error::Transport(error.to_string()))?;
        if status == 401 {
            return Err(Error::Unauthorized);
        }
        let parsed = serde_json::from_str::<Value>(&text);
        if !(200..300).contains(&status) {
            // Gateway timeouts and other proxy errors arrive as HTML, not JSON.
            let message = parsed
                .ok()
                .and_then(|body| body["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(Error::Http { status, message });
        }
        let body = parsed.map_err(|error| Error::Decode(error.to_string()))?;
        if let Some(errors) = body["errors"]
            .as_array()
            .filter(|errors| !errors.is_empty())
        {
            let messages = errors
                .iter()
                .map(|error| {
                    error["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                        .to_owned()
                })
                .collect();
            return Err(Error::GraphQl(messages));
        }
        serde_json::from_value(body["data"].clone())
            .map_err(|error| Error::Decode(error.to_string()))
    }
}

/// The name after `query` or `mutation` in a GraphQL document, or `-`.
fn operation_name(document: &str) -> &str {
    document
        .split_whitespace()
        .skip_while(|word| *word != "query" && *word != "mutation")
        .nth(1)
        .map(|name| name.split(['(', '{']).next().unwrap_or(name))
        .filter(|name| !name.is_empty())
        .unwrap_or("-")
}

/// `path` without its query and with the owner, repository, numbers and
/// commit hashes replaced by `*`, so a trace line names no repository.
pub(crate) fn path_shape(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let mut after_repos = None;
    path.split('/')
        .enumerate()
        .map(|(at, segment)| {
            if segment == "repos" {
                after_repos = Some(at);
            }
            let owner_or_name = after_repos.is_some_and(|repos| at == repos + 1 || at == repos + 2);
            let id = !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_hexdigit());
            if owner_or_name || id { "*" } else { segment }
        })
        .collect::<Vec<_>>()
        .join("/")
}
