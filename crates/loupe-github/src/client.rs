use std::sync::{Arc, Mutex};
use std::time::Duration;

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
        let mut response = self
            .agent
            .post(GRAPHQL_URL)
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(json!({ "query": query, "variables": variables }))
            .map_err(|error| Error::Transport(error.to_string()))?;
        self.record_rate(response.headers());
        let status = response.status().as_u16();
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
