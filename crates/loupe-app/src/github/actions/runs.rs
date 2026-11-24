//! Workflow runs, newest first: `Route::Actions` lists one repository's (or
//! one branch's) under the repository's tabs, and `Route::AllActions` lists
//! those of every repository the sidebar lists, with filters by repository,
//! workflow, branch, and status. A list with a run in progress reloads on
//! its own.

use std::time::Duration;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{CheckStatus, GitHub, RepoRef, WorkflowRun};

use super::{POLL_EVERY, status_style, status_word};
use crate::github::common::{self, LoadError, Loadable};
use crate::github::live;
use crate::github::repo::{RepoTab, repo_header, repo_tabs};
use crate::github::repos;
use crate::route::Route;
use crate::shell::Refreshable as _;
use crate::shell::{OpenOnClick as _, navigate};
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

/// The lanes after the title, in px: repository, workflow, branch, actor,
/// duration, age.
const REPO_LANE: f32 = 150.;
const WORKFLOW_LANE: f32 = 220.;
/// Narrower beside the repository lane, so the title keeps its room.
const WORKFLOW_LANE_BESIDE_REPO: f32 = 160.;
const BRANCH_LANE: f32 = 196.;
const ACTOR_LANE: f32 = 48.;
const DURATION_LANE: f32 = 64.;
const AGE_LANE: f32 = 40.;
/// The most repositories the Actions page reads, one request each, in the
/// sidebar's order.
const MAX_REPOS: usize = 20;
/// How often the Actions page reloads while a run goes on: every listed
/// repository costs a request.
const POLL_ALL_EVERY: Duration = Duration::from_secs(60);

/// Whose runs a list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// One repository's, of one branch when given.
    Repo {
        repo: RepoRef,
        branch: Option<String>,
    },
    /// Every repository the sidebar lists.
    All,
}

/// A run and the repository it ran in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RepoRun {
    pub repo: RepoRef,
    pub run: WorkflowRun,
}

/// What one load brought: the runs, newest first, and the repositories
/// whose runs could not be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Runs {
    pub runs: Vec<RepoRun>,
    pub failed: Vec<RepoRef>,
}

/// Every repository's runs in one list, newest first. Fails only when no
/// repository could be read, with the first error.
fn merge(
    results: Vec<(RepoRef, loupe_github::Result<Vec<WorkflowRun>>)>,
) -> loupe_github::Result<Runs> {
    let mut merged = Runs::default();
    let mut first_error = None;
    let read_any = results.iter().any(|(_, result)| result.is_ok());
    for (repo, result) in results {
        match result {
            Ok(runs) => merged.runs.extend(runs.into_iter().map(|run| RepoRun {
                repo: repo.clone(),
                run,
            })),
            Err(error) => {
                merged.failed.push(repo);
                first_error.get_or_insert(error);
            }
        }
    }
    if let (false, Some(error)) = (read_any, first_error) {
        return Err(error);
    }
    merged
        .runs
        .sort_by_key(|item| std::cmp::Reverse(item.run.created_at));
    Ok(merged)
}

/// Reads the runs of `repos` at once, one thread each.
fn read_all(github: &GitHub, repos: &[RepoRef]) -> loupe_github::Result<Runs> {
    let results = std::thread::scope(|scope| {
        let reads: Vec<_> = repos
            .iter()
            .map(|repo| {
                (
                    repo.clone(),
                    scope.spawn(|| github.workflow_runs(repo, None)),
                )
            })
            .collect();
        reads
            .into_iter()
            .map(|(repo, read)| {
                let result = read.join().unwrap_or_else(|_| {
                    Err(loupe_github::Error::Decode("the read panicked".into()))
                });
                (repo, result)
            })
            .collect()
    });
    merge(results)
}

/// What the Actions page's filters keep; `None` keeps everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RunFilter {
    pub repo: Option<RepoRef>,
    pub workflow: Option<String>,
    pub branch: Option<String>,
    pub status: Option<CheckStatus>,
}

/// The order the status filter lists states in: what needs a look first.
const STATUS_ORDER: [CheckStatus; 8] = [
    CheckStatus::Running,
    CheckStatus::Queued,
    CheckStatus::ActionRequired,
    CheckStatus::Failed,
    CheckStatus::Passed,
    CheckStatus::Cancelled,
    CheckStatus::Skipped,
    CheckStatus::Neutral,
];

/// The values each filter offers: the workflows, branches, and states of
/// the runs of the chosen repository (of every one when none is chosen).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FilterOptions {
    pub workflows: Vec<String>,
    pub branches: Vec<String>,
    pub statuses: Vec<CheckStatus>,
}

impl RunFilter {
    fn is_empty(&self) -> bool {
        *self == RunFilter::default()
    }

    pub fn matches(&self, item: &RepoRun) -> bool {
        self.repo.as_ref().is_none_or(|repo| *repo == item.repo)
            && self
                .workflow
                .as_ref()
                .is_none_or(|workflow| *workflow == item.run.workflow)
            && self
                .branch
                .as_ref()
                .is_none_or(|branch| item.run.branch.as_ref() == Some(branch))
            && self.status.is_none_or(|status| status == item.run.status)
    }

    pub fn options(&self, runs: &[RepoRun]) -> FilterOptions {
        let in_repo: Vec<&RepoRun> = runs
            .iter()
            .filter(|item| self.repo.as_ref().is_none_or(|repo| *repo == item.repo))
            .collect();
        let mut workflows: Vec<String> = in_repo
            .iter()
            .map(|item| item.run.workflow.clone())
            .collect();
        workflows.sort_by_key(|workflow| workflow.to_lowercase());
        workflows.dedup();
        // Branches in the order of their latest run, as GitHub lists them.
        let mut branches: Vec<String> = Vec::new();
        for branch in in_repo.iter().filter_map(|item| item.run.branch.as_ref()) {
            if !branches.contains(branch) {
                branches.push(branch.clone());
            }
        }
        let statuses = STATUS_ORDER
            .into_iter()
            .filter(|status| in_repo.iter().any(|item| item.run.status == *status))
            .collect();
        FilterOptions {
            workflows,
            branches,
            statuses,
        }
    }
}

/// A state as a filter value: "In progress", "Failed"…
fn status_label(status: CheckStatus) -> String {
    let word = status_word(status);
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub struct RunsView {
    scope: Scope,
    /// The repositories the Actions page read, in the sidebar's order.
    repos: Vec<RepoRef>,
    runs: Loadable<Runs>,
    filter: RunFilter,
    scroll: ScrollHandle,
    _load: Option<Task<()>>,
    _poll: Option<Task<()>>,
    _list_changes: Vec<Subscription>,
}

impl RunsView {
    pub fn new(scope: Scope, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The Actions page follows the sidebar's list: a repository added,
        // removed, or found among your pull requests reloads it.
        let list_changes = match scope {
            Scope::All => vec![
                cx.observe_global::<crate::store::Settings>(Self::reload_if_list_changed),
                cx.observe_global::<repos::PullRequestRepos>(Self::reload_if_list_changed),
            ],
            Scope::Repo { .. } => Vec::new(),
        };
        let mut view = Self {
            scope,
            repos: Vec::new(),
            runs: Loadable::Loading,
            filter: RunFilter::default(),
            scroll: ScrollHandle::new(),
            _load: None,
            _poll: None,
            _list_changes: list_changes,
        };
        view.reload(cx);
        view
    }

    fn listed_repos(cx: &App) -> Vec<RepoRef> {
        repos::listed(cx)
            .into_iter()
            .take(MAX_REPOS)
            .map(|item| item.repo)
            .collect()
    }

    fn reload_if_list_changed(&mut self, cx: &mut Context<Self>) {
        if Self::listed_repos(cx) != self.repos {
            self.reload(cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.runs.start_refresh();
        self._load = Some(match self.scope.clone() {
            Scope::Repo { repo, branch } => common::run(
                cx,
                move |gh| {
                    let runs = gh.workflow_runs(&repo, branch.as_deref())?;
                    Ok(Runs {
                        runs: runs
                            .into_iter()
                            .map(|run| RepoRun {
                                repo: repo.clone(),
                                run,
                            })
                            .collect(),
                        failed: Vec::new(),
                    })
                },
                Self::finish,
            ),
            Scope::All => {
                self.repos = Self::listed_repos(cx);
                if self.repos.is_empty() {
                    self.runs = Loadable::Loaded(Runs::default());
                    self._poll = None;
                    cx.notify();
                    return;
                }
                let repos = self.repos.clone();
                common::run(cx, move |gh| read_all(gh, &repos), Self::finish)
            }
        });
        cx.notify();
    }

    fn finish(&mut self, result: Result<Runs, LoadError>, cx: &mut Context<Self>) {
        self.runs.finish(result);
        self.poll_if_running(cx);
    }

    /// Reloads in a moment while a run is queued or in progress, unless a
    /// rate limit runs low.
    fn poll_if_running(&mut self, cx: &mut Context<Self>) {
        let running = self
            .runs
            .value()
            .is_some_and(|runs| runs.runs.iter().any(|item| !item.run.is_finished()));
        let every = match self.scope {
            Scope::Repo { .. } => POLL_EVERY * 2,
            Scope::All => POLL_ALL_EVERY,
        };
        self._poll = running.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(every).await;
                this.update(cx, |view, cx| {
                    let low = common::client(cx)
                        .and_then(|github| live::paused_until(github.rate_limits(), Utc::now()));
                    if low.is_none() {
                        view.reload(cx);
                    }
                })
                .ok();
            })
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.runs.is_busy();
        let count = self.runs.value().map(|runs| self.shown(runs).len());
        let refresh = ui::icon_button("runs-refresh", IconName::RefreshCw, "Refresh")
            .loading(busy)
            .on_click(cx.listener(|view, _, _, cx| view.refresh_page(cx)));
        match &self.scope {
            Scope::Repo { repo, .. } => {
                let url = format!("{}/actions", repo.url());
                repo_header(repo)
                    .action(
                        ui::icon_button("runs-browser", IconName::ExternalLink, "Open in browser")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .action(refresh)
                    .into_any_element()
            }
            Scope::All => {
                let header = ui::page_header("Actions").icon(IconName::Workflow);
                let header = match count {
                    Some(count) => header.count(count),
                    None => header,
                };
                header.action(refresh).into_any_element()
            }
        }
    }

    /// One repository's tabs, with the count of the runs it shows.
    fn tabs(&self) -> Option<impl IntoElement> {
        let Scope::Repo { repo, .. } = &self.scope else {
            return None;
        };
        let count = self.runs.value().map(|runs| self.shown(runs).len());
        Some(repo_tabs(repo, RepoTab::Actions, count))
    }

    /// The runs the filters keep.
    fn shown<'a>(&self, runs: &'a Runs) -> Vec<&'a RepoRun> {
        runs.runs
            .iter()
            .filter(|item| self.filter.matches(item))
            .collect()
    }

    /// One repository's page: the branch the list is narrowed to, as a chip
    /// with its own ×, and Clear, only while a branch is set. The Actions
    /// page: a menu per filter, and Clear while one is set.
    fn filter_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.scope {
            Scope::Repo { repo, branch } => Some(
                self.branch_row(repo, branch.clone()?, cx)
                    .into_any_element(),
            ),
            Scope::All if self.repos.is_empty() => None,
            Scope::All => Some(self.filters(cx).into_any_element()),
        }
    }

    fn branch_row(&self, repo: &RepoRef, branch: String, cx: &mut Context<Self>) -> Div {
        let repo = repo.clone();
        let clear = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            navigate(
                Route::Actions {
                    repo: repo.clone(),
                    branch: None,
                },
                window,
                cx,
            )
        };
        let clear_chip = clear.clone();
        ui::toolbar()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_size(scale::meta())
                            .text_color(cx.theme().muted_foreground)
                            .child("Branch"),
                    )
                    .child(ui::branch_chip(branch.clone(), cx))
                    .child(
                        ui::icon_button(
                            "runs-branch-remove",
                            IconName::X,
                            format!("Show every branch, not only {branch}"),
                        )
                        .xsmall()
                        .on_click(clear_chip),
                    ),
            )
            .child(div().flex_1())
            .child(ui::icon_button("runs-clear", IconName::FunnelX, "Clear").on_click(clear))
    }

    fn filters(&self, cx: &mut Context<Self>) -> Div {
        let options = self
            .runs
            .value()
            .map(|runs| self.filter.options(&runs.runs))
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        let filter = self.filter.clone();
        let repos: Vec<(String, Option<RepoRef>)> = self
            .repos
            .iter()
            .map(|repo| (repo.to_string(), Some(repo.clone())))
            .collect();
        let workflows: Vec<(String, Option<String>)> = options
            .workflows
            .iter()
            .map(|workflow| (workflow.clone(), Some(workflow.clone())))
            .collect();
        let branches: Vec<(String, Option<String>)> = options
            .branches
            .iter()
            .map(|branch| (branch.clone(), Some(branch.clone())))
            .collect();
        let statuses: Vec<(String, Option<CheckStatus>)> = options
            .statuses
            .iter()
            .map(|status| (status_label(*status), Some(*status)))
            .collect();
        ui::toolbar()
            .gap_2()
            .child(filter_menu(
                "runs-filter-repo",
                "Repository",
                filter.repo.as_ref().map(|repo| repo.name.clone()),
                filter.repo.clone(),
                repos,
                this.clone(),
                |filter, repo| {
                    filter.repo = repo;
                    // A workflow or a branch of another repository would
                    // hide every run.
                    filter.workflow = None;
                    filter.branch = None;
                },
            ))
            .child(filter_menu(
                "runs-filter-workflow",
                "Workflow",
                filter.workflow.clone(),
                filter.workflow.clone(),
                workflows,
                this.clone(),
                |filter, workflow| filter.workflow = workflow,
            ))
            .child(filter_menu(
                "runs-filter-branch",
                "Branch",
                filter.branch.clone(),
                filter.branch.clone(),
                branches,
                this.clone(),
                |filter, branch| filter.branch = branch,
            ))
            .child(filter_menu(
                "runs-filter-status",
                "Status",
                filter.status.map(status_label),
                filter.status,
                statuses,
                this,
                |filter, status| filter.status = status,
            ))
            .child(div().flex_1())
            .when(!filter.is_empty(), |row| {
                row.child(
                    ui::icon_button("runs-clear", IconName::FunnelX, "Clear").on_click(
                        cx.listener(|view, _, _, cx| {
                            view.filter = RunFilter::default();
                            cx.notify();
                        }),
                    ),
                )
            })
    }

    fn heading(&self, cx: &App) -> impl IntoElement {
        let label = |text: &'static str, width: f32, right: bool| {
            div()
                .flex_none()
                .w(dp(width))
                .when(right, |cell| cell.text_right())
                .child(text)
        };
        let all = self.scope == Scope::All;
        ui::list_column_header(cx)
            .child(div().flex_1().min_w_0().child("Run"))
            .when(all, |row| row.child(label("Repository", REPO_LANE, false)))
            .child(label("Workflow", self.workflow_lane(), false))
            .child(label("Branch", BRANCH_LANE, false))
            .child(
                h_flex()
                    .flex_none()
                    .w(dp(ACTOR_LANE))
                    .justify_center()
                    .child("Actor"),
            )
            .child(label("Duration", DURATION_LANE, true))
            .child(label("Age", AGE_LANE, true))
    }

    fn workflow_lane(&self) -> f32 {
        match self.scope {
            Scope::Repo { .. } => WORKFLOW_LANE,
            Scope::All => WORKFLOW_LANE_BESIDE_REPO,
        }
    }

    fn row(&self, item: &RepoRun, cx: &App) -> impl IntoElement {
        let RepoRun { repo, run } = item;
        let theme = cx.theme();
        let now = Utc::now();
        let (icon, color) = status_style(run.status);
        let route = Route::WorkflowRun {
            repo: repo.clone(),
            run_id: run.id,
            job_id: None,
            title: format!("{} #{}", run.workflow, run.number),
        };
        let title = if run.title.is_empty() {
            run.workflow.clone()
        } else {
            run.title.clone()
        };
        let (login, avatar_url) = run.actor.as_ref().map_or_else(
            || (String::new(), String::new()),
            |actor| (actor.login.clone(), actor.avatar_url.clone()),
        );
        let duration = if run.status == CheckStatus::Queued {
            "queued".to_owned()
        } else {
            super::format_duration(run.duration_seconds(now))
        };
        let all = self.scope == Scope::All;
        let name = format!(
            "{}{} #{} {title}, {}",
            if all {
                format!("{repo} ")
            } else {
                String::new()
            },
            run.workflow,
            run.number,
            status_word(run.status)
        );
        let reference: SharedString = repo.to_string().into();
        ui::list_row(("run-row", run.id as usize), cx)
            .role(Role::Link)
            .aria_label(name)
            .relative()
            .child(ui::list_row_divider(cx))
            .opens(route)
            .child(Icon::new(icon).small().flex_none().text_color(color))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(scale::body())
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(scale::meta())
                            .text_color(theme.muted_foreground)
                            .child(format!("#{} · {}", run.number, run.event)),
                    ),
            )
            // The repository's name; its tooltip keeps the owner, so two
            // repositories of one name stay apart.
            .when(all, |row| {
                row.child(
                    div()
                        .id(("run-repo", run.id as usize))
                        .flex_none()
                        .w(dp(REPO_LANE))
                        .truncate()
                        .text_size(scale::meta())
                        .text_color(theme.muted_foreground)
                        .tooltip(move |window, cx| {
                            Tooltip::new(reference.clone()).build(window, cx)
                        })
                        .child(repo.name.clone()),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(dp(self.workflow_lane()))
                    .truncate()
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child(run.workflow.clone()),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w(dp(BRANCH_LANE))
                    .min_w_0()
                    .items_center()
                    .children(run.branch.clone().map(|branch| ui::branch_chip(branch, cx))),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w(dp(ACTOR_LANE))
                    .justify_center()
                    .when(!login.is_empty(), |cell| {
                        cell.child(ui::avatar_named(avatar_url, login, scale::avatar_md()))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .w(dp(DURATION_LANE))
                    .text_right()
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child(duration),
            )
            .child(
                div()
                    .flex_none()
                    .w(dp(AGE_LANE))
                    .text_right()
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child(ui::relative_time(run.created_at, now)),
            )
    }

    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        let runs = match &self.runs {
            Loadable::Failed(error) => {
                return common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .into_any_element();
            }
            Loadable::Loading => return ui::loading_rows(10).into_any_element(),
            Loadable::Loaded(runs) | Loadable::Refreshing(runs) => runs,
        };
        if self.scope == Scope::All && self.repos.is_empty() {
            return ui::empty_state(
                IconName::Workflow,
                "No repositories yet",
                "Watch a repository, or open a pull request, and its runs show here.",
            )
            .action(
                Button::new("runs-add-repo")
                    .outline()
                    .small()
                    .icon(IconName::Plus)
                    .label("Add repository…")
                    .on_click(|_, window, cx| crate::github::repo::open_add_repository(window, cx)),
            )
            .into_any_element();
        }
        let shown = self.shown(runs);
        let failed = (!runs.failed.is_empty()).then(|| {
            let names: Vec<String> = runs.failed.iter().map(ToString::to_string).collect();
            ui::empty_line(
                IconName::TriangleAlert,
                format!("Could not read the runs of {}.", names.join(", ")),
                cx,
            )
            .px(scale::content_inset())
            .py_2()
        });
        if shown.is_empty() {
            let detail = match &self.scope {
                _ if !self.filter.is_empty() => "No run matches these filters.",
                Scope::Repo {
                    branch: Some(_), ..
                } => "No workflow has run on this branch.",
                Scope::Repo { .. } => "No workflow has run in this repository.",
                Scope::All => "No workflow has run in these repositories.",
            };
            return v_flex()
                .children(failed)
                .child(ui::empty_state(
                    IconName::Workflow,
                    "No workflow runs",
                    detail,
                ))
                .into_any_element();
        }
        ui::list_container()
            .children(failed)
            .child(self.heading(cx))
            .children(shown.into_iter().map(|item| self.row(item, cx)))
            .into_any_element()
    }
}

/// A filter as an outlined button with a caret: its name, or the value it
/// keeps, and a menu of "Any" and the values.
fn filter_menu<T: Clone + PartialEq + 'static>(
    id: &'static str,
    name: &'static str,
    shown: Option<String>,
    current: Option<T>,
    values: Vec<(String, Option<T>)>,
    view: WeakEntity<RunsView>,
    apply: fn(&mut RunFilter, Option<T>),
) -> impl IntoElement {
    let label = match &shown {
        Some(value) => format!("{name}: {value}"),
        None => name.to_owned(),
    };
    Button::new(id)
        .outline()
        .small()
        .label(label.clone())
        .accessibility_label(label)
        .dropdown_caret(true)
        .menu_trigger()
        .dropdown_menu(move |menu: PopupMenu, _, _| {
            let item = |label: String, value: Option<T>| {
                let view = view.clone();
                let checked = value == current;
                PopupMenuItem::new(label)
                    .checked(checked)
                    .on_click(move |_, _, cx| {
                        let value = value.clone();
                        view.update(cx, |view, cx| {
                            apply(&mut view.filter, value);
                            cx.notify();
                        })
                        .ok();
                    })
            };
            let menu = menu
                .min_w(scale::menu_min_width())
                .max_h(dp(360.))
                .scrollable(true)
                .item(item(format!("Any {}", name.to_lowercase()), None))
                .item(PopupMenuItem::separator());
            values.iter().fold(menu, |menu, (label, value)| {
                menu.item(item(label.clone(), value.clone()))
            })
        })
}

impl crate::shell::Refreshable for RunsView {
    fn refresh_page(&mut self, cx: &mut Context<Self>) {
        if !self.runs.is_busy() {
            self.reload(cx);
        }
    }
}

impl Render for RunsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.header(cx))
            .children(self.tabs())
            .children(self.filter_row(cx))
            .child(
                div()
                    .id("runs")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(self.body(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use loupe_github::{CheckStatus, Error, RepoRef, WorkflowRun};

    use super::{RepoRun, RunFilter, merge};

    fn run(id: u64, workflow: &str, branch: &str, status: CheckStatus, at: i64) -> WorkflowRun {
        let time: DateTime<Utc> = DateTime::from_timestamp(at, 0).expect("time");
        WorkflowRun {
            id,
            number: id,
            attempt: 1,
            workflow: workflow.into(),
            title: format!("Run {id}"),
            path: ".github/workflows/ci.yml".into(),
            event: "push".into(),
            branch: Some(branch.into()),
            head_sha: "abc".into(),
            status,
            actor: None,
            created_at: time,
            started_at: Some(time),
            updated_at: time,
            url: String::new(),
            check_suite: None,
            pull_requests: Vec::new(),
        }
    }

    fn repo(name: &str) -> RepoRef {
        RepoRef::new("acme", name)
    }

    #[test]
    fn runs_of_every_repository_merge_newest_first_and_a_failed_one_is_named() {
        let merged = merge(vec![
            (
                repo("web"),
                Ok(vec![
                    run(3, "CI", "main", CheckStatus::Passed, 300),
                    run(1, "CI", "main", CheckStatus::Failed, 100),
                ]),
            ),
            (repo("gone"), Err(Error::Decode("not found".into()))),
            (
                repo("api"),
                Ok(vec![run(2, "Release", "main", CheckStatus::Running, 200)]),
            ),
        ])
        .expect("two of three read");
        let ids: Vec<(String, u64)> = merged
            .runs
            .iter()
            .map(|item| (item.repo.name.clone(), item.run.id))
            .collect();
        assert_eq!(
            ids,
            [("web".into(), 3), ("api".into(), 2), ("web".into(), 1)]
        );
        assert_eq!(merged.failed, [repo("gone")]);

        let none_read = merge(vec![(repo("gone"), Err(Error::Decode("not found".into())))]);
        assert!(
            none_read.is_err(),
            "nothing read is an error, not an empty list"
        );
    }

    #[test]
    fn filters_keep_runs_by_repository_workflow_branch_and_status() {
        let runs = vec![
            RepoRun {
                repo: repo("web"),
                run: run(1, "CI", "main", CheckStatus::Failed, 100),
            },
            RepoRun {
                repo: repo("web"),
                run: run(2, "Release", "feat/x", CheckStatus::Passed, 200),
            },
            RepoRun {
                repo: repo("api"),
                run: run(3, "CI", "main", CheckStatus::Running, 300),
            },
        ];
        let kept = |filter: &RunFilter| -> Vec<u64> {
            runs.iter()
                .filter(|item| filter.matches(item))
                .map(|item| item.run.id)
                .collect()
        };
        assert_eq!(kept(&RunFilter::default()), [1, 2, 3]);
        let by_repo = RunFilter {
            repo: Some(repo("web")),
            ..RunFilter::default()
        };
        assert_eq!(kept(&by_repo), [1, 2]);
        let ci_on_main = RunFilter {
            workflow: Some("CI".into()),
            branch: Some("main".into()),
            ..RunFilter::default()
        };
        assert_eq!(kept(&ci_on_main), [1, 3]);
        let failed = RunFilter {
            status: Some(CheckStatus::Failed),
            ..RunFilter::default()
        };
        assert_eq!(kept(&failed), [1]);

        // The values offered are those of the chosen repository's runs.
        let options = by_repo.options(&runs);
        assert_eq!(options.workflows, ["CI", "Release"]);
        assert_eq!(options.branches, ["main", "feat/x"]);
        assert_eq!(options.statuses, [CheckStatus::Failed, CheckStatus::Passed]);
        let every = RunFilter::default().options(&runs);
        assert_eq!(
            every.statuses,
            [
                CheckStatus::Running,
                CheckStatus::Failed,
                CheckStatus::Passed
            ]
        );
    }
}
