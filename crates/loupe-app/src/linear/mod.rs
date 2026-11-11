//! Linear screens: my issues, team boards and lists, the issue view, and the
//! panel of Linear issues linked to a pull request. Owned by the Linear front.

mod data;
mod issue;
pub mod issue_menu;
mod my_issues;
mod new_issue;
mod palette;
mod pr_panel;
mod project;
mod projects;
mod remote_views;
mod team;
mod views_list;
mod views_store;
mod widgets;
mod workspace_logo;

use std::sync::Arc;

use gpui_kit::{AnyView, App, SharedString, Task, Window};
use loupe_linear::TeamSummary;

pub use self::issue::{ChangeAssignee, ChangeLabels, ChangePriority, ChangeState, WriteComment};
pub use self::new_issue::{MoveToTeam, SetCycle, SetDueDate, SetEstimate, SetProject};
pub use self::team::OpenFilter;
use crate::links::PrLinkContext;
use crate::shell::PaletteItem;

pub fn my_issues_view(window: &mut Window, cx: &mut App) -> AnyView {
    my_issues::view(window, cx)
}

pub fn team_view(
    team_id: String,
    name: String,
    view_id: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    team::view(team_id, name, view_id, window, cx)
}

pub fn projects_view(team_id: Option<String>, window: &mut Window, cx: &mut App) -> AnyView {
    projects::view(team_id, window, cx)
}

pub fn project_view(id: String, name: String, window: &mut Window, cx: &mut App) -> AnyView {
    project::view(id, name, window, cx)
}

pub fn views_view(team_id: Option<String>, window: &mut Window, cx: &mut App) -> AnyView {
    views_list::view(team_id, window, cx)
}

pub fn issue_view(identifier: String, window: &mut Window, cx: &mut App) -> AnyView {
    issue::view(identifier, window, cx)
}

/// Opens the new issue dialog for `team_id`, or for the user's first team,
/// and shows the created issue in the current tab.
pub fn compose_issue(team_id: Option<String>, window: &mut Window, cx: &mut App) {
    new_issue::compose(team_id, window, cx)
}

/// The teams the Linear user is a member of, sorted by name.
pub fn my_teams(cx: &mut App) -> Task<Result<Arc<Vec<TeamSummary>>, SharedString>> {
    data::my_teams(cx)
}

/// Every team the key can see, sorted by name.
pub fn all_teams(cx: &mut App) -> Task<Result<Arc<Vec<TeamSummary>>, SharedString>> {
    data::teams(cx)
}

/// A saved view as the sidebar lists it.
pub struct SavedView {
    pub id: String,
    pub name: String,
    /// `None`: a view for every team.
    pub team_id: Option<String>,
}

/// Every saved view, in tab order.
pub fn saved_views(cx: &mut App) -> Vec<SavedView> {
    views_store::snapshot(cx)
        .views
        .into_iter()
        .map(|view| SavedView {
            id: view.id,
            name: view.name,
            team_id: view.team_id,
        })
        .collect()
}

/// The Linear issues linked to a pull request, for the PR view's side column.
pub fn pr_linked_issues(context: PrLinkContext, window: &mut Window, cx: &mut App) -> AnyView {
    pr_panel::view(context, window, cx)
}

/// Issues matching a command palette query, only the team's when `team_key`
/// (`ENG`) is given.
pub fn palette_search(query: &str, team_key: Option<&str>, cx: &mut App) -> Task<Vec<PaletteItem>> {
    palette::search(query, team_key, cx)
}

/// The connected workspace's logo, for the sidebar's mark: `None` when
/// Linear is not connected, the workspace has no logo, or it has not been
/// read yet.
pub fn workspace_logo(cx: &mut App) -> Option<Arc<gpui_kit::Image>> {
    workspace_logo::image(cx)
}

/// The organization id of the active Linear workspace.
pub fn active_workspace_id(cx: &App) -> Option<String> {
    data::active_workspace(cx).map(|workspace| workspace.id().to_owned())
}

/// The connected user's Linear favorites, in sidebar order.
pub fn remote_favorites(cx: &mut App) -> Vec<loupe_linear::LinearFavorite> {
    remote_views::ensure(cx);
    cx.global::<remote_views::RemoteViews>().favorites.clone()
}

/// Presentation of a Linear icon, with a stable fallback for unknown names.
pub fn view_icon(name: Option<&str>) -> gpui_kit::assets::IconName {
    use gpui_kit::assets::IconName;
    match name {
        Some("Bolt") => IconName::Zap,
        Some("Folder") => IconName::Folder,
        Some("PieChart") => IconName::ChartPie,
        Some("Alert") => IconName::TriangleAlert,
        _ => IconName::Layers,
    }
}
/// Color supplied by Linear, parsed at the boundary of the UI.
pub fn view_color(color: Option<&str>) -> Option<gpui_kit::Hsla> {
    color.and_then(widgets::hex_color)
}

/// Loupe's session key for a favorite that opens an issue view.
pub fn favorite_view_id(favorite: &loupe_linear::LinearFavorite) -> Option<String> {
    use loupe_linear::FavoriteTarget;
    match &favorite.target {
        FavoriteTarget::CustomView { view_id, .. } => Some(format!("linear:{view_id}")),
        FavoriteTarget::PredefinedView {
            kind,
            team: Some(team),
        } if kind == "allIssues" => Some(format!("linear-all:{}", team.id)),
        _ => None,
    }
}
