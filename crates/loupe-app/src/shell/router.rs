use gpui_kit::{AnyView, App, Window};

use crate::route::Route;
use crate::{github, linear, settings};

/// Builds the view a route shows. Each arm calls the front that owns it.
pub fn build(route: &Route, window: &mut Window, cx: &mut App) -> AnyView {
    match route.clone() {
        Route::Inbox => github::inbox_view(window, cx),
        Route::Notifications => github::notifications_view(window, cx),
        Route::Repo(repo) => github::repo_view(repo, window, cx),
        Route::PullRequest(pr) => github::pull_request_view(pr, window, cx),
        Route::Actions { repo, branch } => github::actions_view(repo, branch, window, cx),
        Route::WorkflowRun {
            repo,
            run_id,
            job_id,
            ..
        } => github::workflow_run_view(repo, run_id, job_id, window, cx),
        Route::LinearMyIssues => linear::my_issues_view(window, cx),
        Route::LinearTeam {
            team_id,
            name,
            view_id,
        } => linear::team_view(team_id, name, view_id.clone(), window, cx),
        Route::LinearProjects { team_id } => linear::projects_view(team_id.clone(), window, cx),
        Route::LinearProject { id, name } => linear::project_view(id, name, window, cx),
        Route::LinearViews { team_id } => linear::views_view(team_id.clone(), window, cx),
        Route::LinearIssue { identifier } => linear::issue_view(identifier, window, cx),
        Route::Settings => settings::settings_view(window, cx),
    }
}
