//! Client for a T3 Code server, used to show the T3 threads linked to a pull
//! request. Pure Rust, no GPUI: every call is blocking and is meant to run on a
//! background thread.
//!
//! It talks to the server's authenticated HTTP API with a bearer session that
//! holds the `orchestration:read` scope:
//! - `GET /.well-known/t3/environment` (no credential) names the environment,
//!   which the `t3code://threads/<environment>/<thread>` deep link needs.
//! - `GET /api/orchestration/shell` lists every thread, active and archived,
//!   with the pull requests linked to it; [`T3::linked_threads`] filters it
//!   by PR.
//!
//! - `GET /api/auth/session` says what the credential may do, and
//!   `GET /api/projects` lists the projects with their repositories.
//!
//! Loupe does not read transcripts: a thread opens in the T3 desktop app.
//!
//! Starting a thread needs the `orchestration:operate` scope and goes
//! through the server's WebSocket RPC (`rpc`): the HTTP API has no call for
//! it, and `/mcp` only takes sessions of T3's own OAuth clients. The same
//! socket reads the server's provider instances and their models
//! (`server.getConfig`), which `orchestration:read` allows.

mod http;
mod rpc;
mod socket;
mod wire;

use std::collections::HashMap;

use chrono::{DateTime, Utc};

/// Every way a T3 call can fail, already classified at the boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("T3 rejected the credential")]
    Unauthorized,
    #[error("the T3 credential lacks the {0} scope")]
    MissingScope(String),
    #[error("T3 returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("network error: {0}")]
    Transport(String),
    #[error("unexpected T3 response: {0}")]
    Decode(String),
    /// The server answered an RPC request with a failure of its own.
    #[error("T3 refused the request: {0}")]
    Rpc(String),
}

/// The scope a credential needs to start a thread.
pub const OPERATE_SCOPE: &str = "orchestration:operate";

/// What the saved credential may do on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    /// Whether it holds [`OPERATE_SCOPE`]: it can start agents.
    pub can_operate: bool,
}

/// A project of the T3 server and the repository its checkout tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub title: String,
    /// `host/owner/name` in lowercase, as T3 keys repositories; `None` for
    /// a project with no remote.
    pub repository: Option<String>,
}

impl Project {
    /// Whether this project is a checkout of the pull request's repository.
    pub fn holds(&self, pr: &PullRequestKey) -> bool {
        self.repository
            .as_deref()
            .is_some_and(|key| key.eq_ignore_ascii_case(&format!("{}/{}", pr.host, pr.repository)))
    }
}

/// The model a thread runs on: the provider instance that serves it, the
/// model's id, and the effort when it is not the server's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub instance_id: String,
    pub model: String,
    pub effort: Option<String>,
}

/// A review thread to start: where, on what, and with which first message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewLaunch {
    /// Names this request to the server. Sending the same launch again
    /// with the same id (after a lost answer) does not start a second
    /// thread.
    pub request_id: String,
    pub project_id: String,
    pub title: String,
    /// The review request, as the thread's first message.
    pub message: String,
    pub model: ModelChoice,
    /// The pull request's head branch; the worktree starts at its head as
    /// the remote has it.
    pub base_ref: String,
    /// The new branch of the worktree, such as `review/pr-374`.
    pub branch: String,
    pub pull_request: PullRequestKey,
    pub pull_request_url: String,
}

/// What the server's threads say about starting a review.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewFacts {
    /// The worktree branches of each project's threads, archived ones
    /// included, by project id. A new worktree cannot reuse one.
    pub branches: HashMap<String, Vec<String>>,
}

/// A provider instance of the server (such as a Claude account) and the
/// models it offers, as the server reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    /// What a thread's model selection names it by.
    pub instance_id: String,
    /// Its display name, such as "Claude Pool".
    pub name: String,
    /// Why it cannot run a thread now (turned off, not installed, failing),
    /// in the server's words when it gives them; `None` when it can.
    pub unavailable: Option<String>,
    pub models: Vec<ProviderModel>,
}

/// A model a provider instance offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModel {
    /// The id a thread's model selection names, such as `claude-opus-5-5`.
    pub slug: String,
    /// The display name, such as "Claude Opus 5.5".
    pub name: String,
    /// The efforts it takes, in the server's order; empty when it takes no
    /// effort setting.
    pub efforts: Vec<Effort>,
}

/// One value of a model's effort setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effort {
    pub id: String,
    pub label: String,
    /// The effort the model runs at when a thread names none.
    pub is_default: bool,
}

/// A review thread the server started.
#[derive(Debug)]
pub struct LaunchedReview {
    pub thread_id: String,
    /// Why linking the thread to the pull request failed. The thread runs
    /// anyway; it is just not listed under the pull request until linked.
    pub link_error: Option<Error>,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Connection settings for one T3 server. Cheap to clone; share it behind
/// `Arc`. `Debug` never shows the token.
#[derive(Clone)]
pub struct T3 {
    pub base_url: String,
    token: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for T3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("T3")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// The server's identity, from its public descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    pub id: String,
    pub label: String,
    pub server_version: String,
}

/// A pull request as T3 keys its links: host, `owner/name`, and number.
/// T3 stores the repository lowercased, so matching ignores ASCII case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestKey {
    pub host: String,
    pub repository: String,
    pub number: u64,
}

impl PullRequestKey {
    /// A pull request on github.com, with `repository` as `owner/name`.
    pub fn github(repository: impl Into<String>, number: u64) -> Self {
        Self {
            host: "github.com".into(),
            repository: repository.into(),
            number,
        }
    }

    /// Whether a link stored by T3 names this pull request.
    pub fn matches(&self, host: &str, repository: &str, number: u64) -> bool {
        self.number == number
            && self.host.eq_ignore_ascii_case(host)
            && self.repository.eq_ignore_ascii_case(repository)
    }
}

/// A thread's state as the T3 sidebar shows it: idle, or the status of its
/// latest run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadStatus {
    Idle,
    Queued,
    Preparing,
    Starting,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
    Cancelled,
    RolledBack,
    /// A status this client does not know yet, kept verbatim.
    Unknown(String),
}

impl ThreadStatus {
    /// Short lowercase label, such as "running" or "rolled back".
    pub fn label(&self) -> &str {
        match self {
            ThreadStatus::Idle => "idle",
            ThreadStatus::Queued => "queued",
            ThreadStatus::Preparing => "preparing",
            ThreadStatus::Starting => "starting",
            ThreadStatus::Running => "running",
            ThreadStatus::Waiting => "waiting",
            ThreadStatus::Completed => "completed",
            ThreadStatus::Interrupted => "interrupted",
            ThreadStatus::Failed => "failed",
            ThreadStatus::Cancelled => "cancelled",
            ThreadStatus::RolledBack => "rolled back",
            ThreadStatus::Unknown(status) => status,
        }
    }

    /// True while a run is queued or working.
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            ThreadStatus::Queued
                | ThreadStatus::Preparing
                | ThreadStatus::Starting
                | ThreadStatus::Running
                | ThreadStatus::Waiting
        )
    }
}

/// One T3 thread linked to a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedThread {
    pub id: String,
    pub title: String,
    /// The project's title, or its id when the snapshot does not list it.
    pub project: String,
    pub status: ThreadStatus,
    /// The user or auto-settlement marked the thread done.
    pub settled: bool,
    pub updated_at: DateTime<Utc>,
}

impl T3 {
    /// `base_url` is the server's origin, such as
    /// `https://host.tailnet.ts.net:3774`; a trailing slash is ignored.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let base_url = base_url.into().trim().trim_end_matches('/').to_owned();
        Self {
            base_url,
            token: token.into(),
            agent: http::agent(),
        }
    }

    /// Reads the public environment descriptor. Needs no credential.
    pub fn environment(&self) -> Result<Environment> {
        http::environment(self)
    }

    /// The threads linked to `pr`, archived ones included, newest first. A
    /// thread matches when one of its visible PR links, its legacy linked PR,
    /// or its branch's PR names `pr`.
    pub fn linked_threads(&self, pr: &PullRequestKey) -> Result<Vec<LinkedThread>> {
        http::linked_threads(self, pr)
    }

    /// What the credential may do. `Unauthorized` when the server does not
    /// know it.
    pub fn session(&self) -> Result<Session> {
        http::session(self)
    }

    /// The server's projects that are not archived or deleted.
    pub fn projects(&self) -> Result<Vec<Project>> {
        http::projects(self)
    }

    /// What a review launch needs to know from the server's threads: which
    /// worktree branches are taken.
    pub fn review_facts(&self) -> Result<ReviewFacts> {
        http::review_facts(self)
    }

    /// The server's provider instances, enabled or not, each with the
    /// models it offers, in the server's order.
    pub fn providers(&self) -> Result<Vec<Provider>> {
        socket::providers(self)
    }

    /// Starts a review thread and links it to its pull request. Needs
    /// [`OPERATE_SCOPE`]; without it the server refuses with
    /// [`Error::MissingScope`] and starts nothing.
    pub fn launch_review(&self, launch: &ReviewLaunch) -> Result<LaunchedReview> {
        socket::launch_review(self, launch)
    }
}

/// The T3 desktop deep link that opens a thread:
/// `t3code://threads/<environment>/<thread>`.
pub fn thread_deep_link(environment_id: &str, thread_id: &str) -> String {
    wire::thread_deep_link(environment_id, thread_id)
}
