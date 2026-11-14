//! State shapes and helpers every GitHub screen shares.

use std::sync::Arc;

use gpui_kit::*;
use loupe_github::GitHub;

use crate::services::{Services, background};
use crate::theme::scale;
use crate::ui;

/// Why a screen could not load. `NotConnected` and `Unauthorized` point the
/// user to Settings; everything else shows GitHub's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    NotConnected,
    Unauthorized,
    Other(String),
}

impl From<loupe_github::Error> for LoadError {
    fn from(error: loupe_github::Error) -> Self {
        match error {
            loupe_github::Error::Unauthorized => LoadError::Unauthorized,
            other => LoadError::Other(other.to_string()),
        }
    }
}

impl LoadError {
    pub fn title(&self) -> &'static str {
        match self {
            LoadError::NotConnected => "GitHub is not connected",
            LoadError::Unauthorized => "GitHub rejected the token",
            LoadError::Other(_) => "Could not load from GitHub",
        }
    }

    /// Title and detail as one sentence pair for an inline alert.
    pub fn message(&self) -> String {
        format!("{}. {}", self.title(), self.detail())
    }

    pub fn detail(&self) -> String {
        match self {
            LoadError::NotConnected => "Connect your GitHub account in Settings.".into(),
            LoadError::Unauthorized => {
                "The token is missing, revoked, or lacks a scope. Sign in again in Settings.".into()
            }
            LoadError::Other(message) => message.clone(),
        }
    }
}

/// A screen's data: loading (with the previous value kept during a refresh),
/// loaded, or failed.
#[derive(Clone, Debug, Default)]
pub enum Loadable<T> {
    #[default]
    Loading,
    /// A refresh is in flight; keep showing the stale value.
    Refreshing(T),
    Loaded(T),
    Failed(LoadError),
}

impl<T> Loadable<T> {
    /// The value to show, fresh or stale.
    pub fn value(&self) -> Option<&T> {
        match self {
            Loadable::Refreshing(value) | Loadable::Loaded(value) => Some(value),
            Loadable::Loading | Loadable::Failed(_) => None,
        }
    }

    pub fn value_mut(&mut self) -> Option<&mut T> {
        match self {
            Loadable::Refreshing(value) | Loadable::Loaded(value) => Some(value),
            Loadable::Loading | Loadable::Failed(_) => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(self, Loadable::Loading | Loadable::Refreshing(_))
    }

    /// Moves to `Refreshing` keeping the current value, or to `Loading`.
    pub fn start_refresh(&mut self) {
        *self = match std::mem::take(self) {
            Loadable::Loaded(value) | Loadable::Refreshing(value) => Loadable::Refreshing(value),
            Loadable::Loading | Loadable::Failed(_) => Loadable::Loading,
        };
    }

    pub fn finish(&mut self, result: Result<T, LoadError>) {
        *self = match result {
            Ok(value) => Loadable::Loaded(value),
            Err(error) => match std::mem::take(self) {
                Loadable::Refreshing(value) | Loadable::Loaded(value) => Loadable::Loaded(value),
                _ => Loadable::Failed(error),
            },
        };
    }
}

/// A write in progress from a form or button.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Submit {
    #[default]
    Idle,
    Busy,
    Failed(String),
}

impl Submit {
    pub fn is_busy(&self) -> bool {
        matches!(self, Submit::Busy)
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Submit::Failed(message) => Some(message),
            Submit::Idle | Submit::Busy => None,
        }
    }
}

/// The signed-in GitHub client, if any.
pub fn client(cx: &App) -> Option<Arc<GitHub>> {
    Services::get(cx).github.clone()
}

/// Runs `work` with the GitHub client on a background thread, then hands the
/// result to `apply` on the view. Without a client, `apply` gets
/// `LoadError::NotConnected` right away. Dropping the returned task cancels
/// the update, so keep it in the view while it matters.
pub fn run<V, T>(
    cx: &mut Context<V>,
    work: impl FnOnce(&GitHub) -> loupe_github::Result<T> + Send + 'static,
    apply: impl FnOnce(&mut V, Result<T, LoadError>, &mut Context<V>) + 'static,
) -> Task<()>
where
    V: 'static,
    T: Send + 'static,
{
    let Some(github) = client(cx) else {
        return cx.spawn(async move |this, cx| {
            this.update(cx, |view, cx| apply(view, Err(LoadError::NotConnected), cx))
                .ok();
        });
    };
    let job = background(cx, move || work(&github));
    cx.spawn(async move |this, cx| {
        let result = job.await.map_err(LoadError::from);
        this.update(cx, |view, cx| {
            apply(view, result, cx);
            cx.notify();
        })
        .ok();
    })
}

/// A failed load as an inline alert with GitHub's message and Retry, inset
/// like the content it replaces.
pub fn load_error(
    error: &LoadError,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w_full()
        .px(scale::content_inset())
        .py_4()
        .child(ui::error_banner(error.message(), on_retry))
}
