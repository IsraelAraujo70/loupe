//! `Route::Actions`: a repository's workflow runs, newest first, or one
//! branch's. A list with a run in progress reloads on its own.

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{RepoRef, WorkflowRun};

use super::{POLL_EVERY, branch_chip, status_style, status_word};
use crate::github::common::{self, Loadable};
use crate::route::Route;
use crate::shell::{OpenOnClick as _, navigate};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

/// The lanes after the title, in px: workflow, branch, actor, duration, age.
const WORKFLOW_LANE: f32 = 150.;
const BRANCH_LANE: f32 = 150.;
const ACTOR_LANE: f32 = 24.;
const DURATION_LANE: f32 = 64.;
const AGE_LANE: f32 = 40.;

pub struct RunsView {
    repo: RepoRef,
    branch: Option<String>,
    runs: Loadable<Vec<WorkflowRun>>,
    scroll: ScrollHandle,
    _load: Option<Task<()>>,
    _poll: Option<Task<()>>,
}

impl RunsView {
    pub fn new(
        repo: RepoRef,
        branch: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            repo,
            branch,
            runs: Loadable::Loading,
            scroll: ScrollHandle::new(),
            _load: None,
            _poll: None,
        };
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.runs.start_refresh();
        let repo = self.repo.clone();
        let branch = self.branch.clone();
        self._load = Some(common::run(
            cx,
            move |gh| gh.workflow_runs(&repo, branch.as_deref()),
            |view: &mut Self, result, cx| {
                view.runs.finish(result);
                view.poll_if_running(cx);
            },
        ));
        cx.notify();
    }

    /// Reloads in a moment while a run is queued or in progress.
    fn poll_if_running(&mut self, cx: &mut Context<Self>) {
        let running = self
            .runs
            .value()
            .is_some_and(|runs| runs.iter().any(|run| !run.is_finished()));
        self._poll = running.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(POLL_EVERY * 2).await;
                this.update(cx, |view, cx| view.reload(cx)).ok();
            })
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.runs.is_busy();
        let url = format!("{}/actions", self.repo.url());
        let header = ui::page_header(format!("{} actions", self.repo)).icon(IconName::Workflow);
        let header = match self.runs.value() {
            Some(runs) => header.count(runs.len()),
            None => header,
        };
        header
            .action(
                ui::icon_button("runs-browser", IconName::ExternalLink, "Open in browser")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .action(
                ui::icon_button("runs-refresh", IconName::RefreshCw, "Refresh")
                    .loading(busy)
                    .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
    }

    /// The branch the list is narrowed to, as a chip with its own ×, and
    /// Clear: only while a branch is set.
    fn filter_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let branch = self.branch.clone()?;
        let repo = self.repo.clone();
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
        Some(
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
                        .child(branch_chip(branch.clone(), cx))
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
                .child(ui::icon_button("runs-clear", IconName::FunnelX, "Clear").on_click(clear)),
        )
    }

    fn heading(&self, cx: &App) -> impl IntoElement {
        let label = |text: &'static str, width: f32, right: bool| {
            div()
                .flex_none()
                .w(dp(width))
                .when(right, |cell| cell.text_right())
                .child(text)
        };
        h_flex()
            .h_6()
            .px_2()
            .gap_3()
            .items_center()
            .text_size(scale::meta())
            .text_color(cx.theme().muted_foreground)
            .child(div().flex_none().w_4())
            .child(div().flex_1().child("Run"))
            .child(label("Workflow", WORKFLOW_LANE, false))
            .child(label("Branch", BRANCH_LANE, false))
            .child(div().flex_none().w(dp(ACTOR_LANE)))
            .child(label("Duration", DURATION_LANE, true))
            .child(label("Age", AGE_LANE, true))
    }

    fn row(&self, run: &WorkflowRun, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let now = Utc::now();
        let (icon, color) = status_style(run.status);
        let route = Route::WorkflowRun {
            repo: self.repo.clone(),
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
        let duration = if run.status == loupe_github::CheckStatus::Queued {
            "queued".to_owned()
        } else {
            super::format_duration(run.duration_seconds(now))
        };
        let name = format!(
            "{} #{} {title}, {}",
            run.workflow,
            run.number,
            status_word(run.status)
        );
        let hover = theme.list_hover;
        h_flex()
            .id(("run-row", run.id as usize))
            .role(Role::Link)
            .aria_label(name)
            .w_full()
            .h(scale::row_height())
            .px_2()
            .gap_3()
            .items_center()
            .relative()
            .rounded(theme.radius)
            .hover(move |row| row.bg(hover))
            .child(
                div()
                    .absolute()
                    .left_2()
                    .right_2()
                    .bottom_0()
                    .h(px(1.))
                    .bg(tokens::border_subtle()),
            )
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
            .child(
                div()
                    .flex_none()
                    .w(dp(WORKFLOW_LANE))
                    .truncate()
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child(run.workflow.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .w(dp(BRANCH_LANE))
                    .min_w_0()
                    .children(run.branch.clone().map(|branch| branch_chip(branch, cx))),
            )
            .child(
                div()
                    .flex_none()
                    .w(dp(ACTOR_LANE))
                    .when(!login.is_empty(), |cell| {
                        cell.child(ui::avatar_named(avatar_url, login, dp(20.)))
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
}

impl Render for RunsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.runs {
            Loadable::Failed(error) => {
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .into_any_element()
            }
            Loadable::Loading => div().p_2().child(ui::skeleton_rows(10)).into_any_element(),
            Loadable::Loaded(runs) | Loadable::Refreshing(runs) if runs.is_empty() => {
                ui::empty_state(
                    IconName::Workflow,
                    "No workflow runs",
                    match self.branch {
                        Some(_) => "No workflow has run on this branch.",
                        None => "No workflow has run in this repository.",
                    },
                )
                .into_any_element()
            }
            Loadable::Loaded(runs) | Loadable::Refreshing(runs) => v_flex()
                .w_full()
                .p_2()
                .child(self.heading(cx))
                .children(runs.iter().map(|run| self.row(run, cx)))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(self.header(cx))
            .children(self.filter_row(cx))
            .child(
                div()
                    .id("runs")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}
