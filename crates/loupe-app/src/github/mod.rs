//! GitHub screens: inbox, notifications, repository PR lists, and the pull
//! request view (conversation, files with inline comments, commits, checks,
//! review and merge actions). Owned by the GitHub front.

mod actions;
mod common;
pub(crate) mod guide_service;
pub(crate) mod highlight;
mod inbox;
pub(crate) mod live;
pub(crate) mod notification_filter;
mod notifications;
mod palette;
mod pr_feed;
mod pr_row;
mod pull_request;
mod repo;
pub(crate) mod repos;
mod widgets;

use gpui_kit::{AnyView, App, SharedString, Task, Window};
use loupe_github::{PrRef, RepoRef, Verdict};

use crate::shell::{PaletteItem, refresh_as};

pub use self::inbox::{
    ClearReviewsSelection, CloseStack, OpenReviewsFilter, OpenStack, SelectTabRows,
    enqueue_auto_guide_if_watched, start_auto_guide, start_bells, start_group_sync,
};
pub use self::pull_request::FindInDiff;

/// Runs page `view`'s Refresh when it is a GitHub page with one; false
/// otherwise.
pub fn refresh(view: &AnyView, cx: &mut App) -> bool {
    refresh_as::<inbox::InboxView>(view, cx)
        || refresh_as::<notifications::NotificationsView>(view, cx)
        || refresh_as::<repo::RepoView>(view, cx)
        || actions::refresh(view, cx)
        || pull_request::refresh(view, cx)
}

/// How long since the Reviews page `view` last started a reload; `None` for
/// another page.
#[cfg(test)]
pub(crate) fn reviews_since_reload(view: &AnyView, cx: &App) -> Option<std::time::Duration> {
    let inbox = view.clone().downcast::<inbox::InboxView>().ok()?;
    Some(inbox.read(cx).since_reload())
}

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

/// The Actions page: the runs of every repository the sidebar lists.
pub fn all_actions_view(window: &mut Window, cx: &mut App) -> AnyView {
    actions::all_runs_view(window, cx)
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

/// What a pull request's tab shows of its loaded page: the title its tooltip
/// names, and the viewer's approval or change request while no commit has
/// landed since.
pub struct PullRequestTab {
    pub title: SharedString,
    pub verdict: Option<Verdict>,
}

/// The pull request tab facts of page `view`, read from what the page already
/// loaded. `None` when `view` is not a pull request page or has nothing yet.
pub fn pull_request_tab(view: &AnyView, cx: &App) -> Option<PullRequestTab> {
    let detail = pull_request::loaded_detail(view, cx)?;
    Some(PullRequestTab {
        title: detail.title.clone().into(),
        verdict: detail.standing_verdict(),
    })
}

/// Pull requests for a GitHub search query the command palette built.
pub fn palette_search(query: &str, first: u32, cx: &mut App) -> Task<Vec<PaletteItem>> {
    palette::search(query, first, cx)
}

/// The palette row of the pull request a query names exactly, with its title.
pub fn palette_pull_request(pr: PrRef, cx: &mut App) -> Task<Vec<PaletteItem>> {
    palette::find(pr, cx)
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
