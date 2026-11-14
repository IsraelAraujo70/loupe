use std::sync::Arc;

use gpui_kit::{App, Global, Task};

/// The signed-in API clients. A `None` means that account is not connected.
/// The shell owns this global: it builds it at startup from stored
/// credentials and replaces it on sign-in or sign-out, then refreshes windows.
#[derive(Clone, Default)]
pub struct Services {
    pub github: Option<Arc<loupe_github::GitHub>>,
    /// The active Linear workspace's client: the one the sidebar, the
    /// views, and My issues read. One of `linear_workspaces`.
    pub linear: Option<Arc<loupe_linear::Linear>>,
    /// Every connected Linear workspace, in the order they were connected.
    pub linear_workspaces: Vec<LinearWorkspace>,
    pub t3: Option<Arc<loupe_t3::T3>>,
}

/// One connected Linear workspace: who its key belongs to and the client
/// that reads it.
#[derive(Clone)]
pub struct LinearWorkspace {
    pub viewer: loupe_linear::Viewer,
    pub client: Arc<loupe_linear::Linear>,
}

impl LinearWorkspace {
    /// The workspace's organization id.
    pub fn id(&self) -> &str {
        &self.viewer.organization.id
    }

    /// A workspace named `id` whose client never reaches Linear: nothing
    /// listens at its endpoint, so a screen that loads fails at once.
    #[cfg(test)]
    pub fn for_test(id: &str, email: &str) -> LinearWorkspace {
        LinearWorkspace {
            viewer: loupe_linear::Viewer {
                user: loupe_linear::User {
                    id: format!("user-{id}"),
                    name: "Israel".into(),
                    display_name: "israel".into(),
                    avatar_url: None,
                },
                organization: loupe_linear::Organization {
                    id: id.into(),
                    name: id.into(),
                    url_key: id.into(),
                    logo_url: None,
                },
                email: email.into(),
            },
            client: Arc::new(loupe_linear::Linear::with_endpoint(
                format!("lin_api_{id}"),
                "http://127.0.0.1:9/graphql",
            )),
        }
    }
}

impl Global for Services {}

impl Services {
    pub fn get(cx: &App) -> &Services {
        cx.global::<Services>()
    }

    /// The connected workspace with this organization id.
    pub fn linear_workspace(&self, id: &str) -> Option<&LinearWorkspace> {
        self.linear_workspaces
            .iter()
            .find(|workspace| workspace.id() == id)
    }
}

/// Where one account stands. `settings::session` moves it; views only read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection<V> {
    #[default]
    Disconnected,
    /// A credential is being validated in the background.
    Checking,
    Connected(V),
    /// The last validation failed; the message is safe to show (never a token).
    Failed(String),
}

/// Who is signed in, for the sidebar footer, the sign-in gate, and Settings.
/// Kept next to [`Services`] and replaced together with it.
#[derive(Clone, Debug, Default)]
pub struct Accounts {
    pub github: Connection<loupe_github::Viewer>,
    /// The active Linear workspace.
    pub linear: Connection<loupe_linear::Viewer>,
    /// Why each stored Linear key that did not connect failed, safe to show.
    pub linear_failed: Vec<String>,
}

impl Global for Accounts {}

impl Accounts {
    pub fn get(cx: &App) -> &Accounts {
        cx.global::<Accounts>()
    }
}

/// Runs a blocking API call on a background thread. Await the task from
/// `cx.spawn` and update the view with the result.
pub fn background<T: Send + 'static>(
    cx: &App,
    work: impl FnOnce() -> T + Send + 'static,
) -> Task<T> {
    cx.background_executor().spawn(async move { work() })
}
