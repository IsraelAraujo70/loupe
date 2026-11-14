//! GitHub screens: inbox, notifications, repository PR lists, and the pull
//! request view (conversation, files with inline comments, commits, checks,
//! review and merge actions). Owned by the GitHub front.

mod actions;
mod checks_feed;
mod common;
pub(crate) mod highlight;
mod inbox;
pub(crate) mod notification_filter;
mod notifications;
mod palette;
mod pr_row;
mod pull_request;
mod repo;
mod widgets;

use gpui_kit::{AnyView, App, SharedString, Task, Window};
use loupe_github::{PrRef, RepoRef};

use crate::shell::PaletteItem;

pub use self::inbox::{ClearReviewsSelection, OpenReviewsFilter, SelectTabRows};

pub fn inbox_view(window: &mut Window, cx: &mut App) -> AnyView {
    inbox::view(window, cx)
}

/// The title of the Reviews page `view`: its open saved view's name, or
/// "Reviews". `None` when `view` is not a Reviews page.
pub fn inbox_title(view: &AnyView, cx: &App) -> Option<SharedString> {
    let inbox = view.clone().downcast::<inbox::InboxView>().ok()?;
    Some(inbox.read(cx).title(cx))
}

pub fn notifications_view(window: &mut Window, cx: &mut App) -> AnyView {
    notifications::view(window, cx)
}

pub fn repo_view(repo: RepoRef, window: &mut Window, cx: &mut App) -> AnyView {
    repo::view(repo, window, cx)
}

pub fn actions_view(
    repo: RepoRef,
    branch: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    actions::runs_view(repo, branch, window, cx)
}

pub fn workflow_run_view(
    repo: RepoRef,
    run_id: u64,
    job_id: Option<u64>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    actions::run_view(repo, run_id, job_id, window, cx)
}

pub fn pull_request_view(pr: PrRef, window: &mut Window, cx: &mut App) -> AnyView {
    pull_request::view(pr, window, cx)
}

/// Pull requests for a GitHub search query the command palette built.
pub fn palette_search(query: &str, first: u32, cx: &mut App) -> Task<Vec<PaletteItem>> {
    palette::search(query, first, cx)
}

/// The repositories the viewer reaches, for the command palette's `repo:`
/// and `org:` filters.
pub fn palette_repositories(cx: &mut App) -> Task<Vec<RepoRef>> {
    palette::repositories(cx)
}

/// The head branch of pull request `pr`, read from GitHub, for a menu
/// that copies it from a row that does not carry it.
pub fn head_branch(pr: PrRef, cx: &mut App) -> Task<Result<String, SharedString>> {
    let Some(client) = crate::services::Services::get(cx).github.clone() else {
        return Task::ready(Err("Sign in to GitHub first.".into()));
    };
    let task = crate::services::background(cx, move || client.pull_request(&pr));
    cx.spawn(async move |_| {
        task.await
            .map(|detail| detail.head_ref)
            .map_err(|error| SharedString::from(common::LoadError::from(error).message()))
    })
}
