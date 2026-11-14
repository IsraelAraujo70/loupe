//! `Route::WorkflowRun`: one run. The header names it and holds its actions
//! (re-run, cancel, the workflow file); the band under it says what started
//! it, on which branch, by whom, how long it took, and what it uploaded.
//! The jobs are listed at the leading edge under Summary, which shows the
//! graph, the annotations, and the artifacts; a job (from the list or the
//! graph) shows its steps and log. A run in progress reloads on its own.

use std::path::PathBuf;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{
    Annotation, AnnotationLevel, CheckStatus, RepoRef, RunArtifact, WorkflowJob, WorkflowRun,
    WorkflowSpec,
};

use super::graph::{GraphEvent, GraphView};
use super::layout::graph_nodes;
use super::log::JobPane;
use super::{
    POLL_EVERY, branch_chip, format_duration, job_duration_label, status_style, status_word,
};
use crate::github::common::{self, Loadable, Submit};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::{self, ControlSize as _, MenuTriggerExt as _};

const JOBS_COLUMN: f32 = 260.;

/// What one load reads.
struct Snapshot {
    run: WorkflowRun,
    jobs: Vec<WorkflowJob>,
    /// Read once per run: `Some(None)` when the file could not be read.
    spec: Option<Option<WorkflowSpec>>,
    /// Read when the run first loads and again when it finishes.
    extras: Option<Extras>,
}

#[derive(Clone, Default)]
struct Extras {
    artifacts: Vec<RunArtifact>,
    annotations: Vec<Annotation>,
}

pub struct RunView {
    repo: RepoRef,
    run_id: u64,
    run: Loadable<WorkflowRun>,
    jobs: Vec<WorkflowJob>,
    /// `None` until read; `Some(None)` when the file could not be read and
    /// the graph has no edges.
    spec: Option<Option<WorkflowSpec>>,
    extras: Extras,
    /// The attempt and whether it had finished when the extras were read.
    extras_read: Option<(u32, bool)>,
    /// After a re-run: the attempt GitHub has yet to show, and the polls
    /// left to wait for it.
    awaited: Option<(u32, u32)>,
    /// The open job; `None` is Summary.
    selected: Option<u64>,
    /// The open job's name: a re-run gives every job a new id, and a job of
    /// a later stage can be missing until GitHub queues it.
    selected_name: Option<String>,
    /// A cancel was accepted and the run has not ended yet.
    cancelling: bool,
    download_error: Option<String>,
    fullscreen: bool,
    graph: Entity<GraphView>,
    pane: Option<Entity<JobPane>>,
    action: Submit,
    note: Option<String>,
    saved: Option<PathBuf>,
    downloading: Option<u64>,
    scroll: ScrollHandle,
    _load: Option<Task<()>>,
    _poll: Option<Task<()>>,
    _action: Option<Task<()>>,
    _download: Option<Task<()>>,
    _graph: Subscription,
}

impl RunView {
    pub fn new(
        repo: RepoRef,
        run_id: u64,
        job_id: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let graph = cx.new(|_| GraphView::new());
        let subscription = cx.subscribe_in(
            &graph,
            window,
            |this: &mut Self, _, event: &GraphEvent, _, cx| match event {
                GraphEvent::OpenJob(job) => {
                    this.fullscreen = false;
                    this.graph
                        .update(cx, |graph, cx| graph.set_fullscreen(false, cx));
                    this.select(Some(*job), cx);
                }
                GraphEvent::ToggleFullscreen => {
                    this.fullscreen = !this.fullscreen;
                    let fullscreen = this.fullscreen;
                    this.graph
                        .update(cx, |graph, cx| graph.set_fullscreen(fullscreen, cx));
                    cx.notify();
                }
            },
        );
        let mut view = Self {
            repo,
            run_id,
            run: Loadable::Loading,
            jobs: Vec::new(),
            spec: None,
            extras: Extras::default(),
            extras_read: None,
            awaited: None,
            selected: job_id,
            selected_name: None,
            cancelling: false,
            download_error: None,
            fullscreen: false,
            graph,
            pane: None,
            action: Submit::Idle,
            note: None,
            saved: None,
            downloading: None,
            scroll: ScrollHandle::new(),
            _load: None,
            _poll: None,
            _action: None,
            _download: None,
            _graph: subscription,
        };
        view.graph
            .update(cx, |graph, cx| graph.set_selected(job_id, cx));
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.run.start_refresh();
        let repo = self.repo.clone();
        let run_id = self.run_id;
        let need_spec = self.spec.is_none();
        let extras_read = self.extras_read;
        self._load = Some(common::run(
            cx,
            move |gh| {
                let run = gh.workflow_run(&repo, run_id)?;
                let jobs = gh.run_jobs(&repo, run_id)?;
                let spec =
                    need_spec.then(|| gh.workflow_spec(&repo, &run.path, &run.head_sha).ok());
                let key = (run.attempt, run.is_finished());
                let extras = (extras_read != Some(key)).then(|| Extras {
                    artifacts: gh.run_artifacts(&repo, run_id).unwrap_or_default(),
                    annotations: run
                        .check_suite
                        .as_deref()
                        .and_then(|suite| gh.run_annotations(suite).ok())
                        .unwrap_or_default(),
                });
                Ok(Snapshot {
                    run,
                    jobs,
                    spec,
                    extras,
                })
            },
            |view: &mut Self, result, cx| {
                match result {
                    Ok(snapshot) => view.apply(snapshot, cx),
                    Err(error) => view.run.finish(Err(error)),
                }
                view.poll_if_running(cx);
            },
        ));
        cx.notify();
    }

    fn apply(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let Snapshot {
            run,
            jobs,
            spec,
            extras,
        } = snapshot;
        if let Some(extras) = extras {
            self.extras = extras;
            self.extras_read = Some((run.attempt, run.is_finished()));
        }
        if let Some(spec) = spec {
            self.spec = Some(spec);
        }
        // A re-run gives the jobs new ids: follow the open job by its name.
        if let Some(open) = self.selected {
            match jobs.iter().find(|job| job.id == open) {
                Some(job) => self.selected_name = Some(job.name.clone()),
                None => {
                    if let Some(job) = self
                        .selected_name
                        .as_ref()
                        .and_then(|name| jobs.iter().find(|job| &job.name == name))
                    {
                        self.selected = Some(job.id);
                    }
                }
            }
        }
        if run.is_finished() {
            self.cancelling = false;
        }
        self.jobs = jobs;
        self.run.finish(Ok(run));
        let nodes = graph_nodes(
            self.spec.as_ref().and_then(Option::as_ref),
            &self.jobs,
            Utc::now(),
        );
        let selected = self.selected;
        self.graph.update(cx, |graph, cx| {
            graph.set_nodes(nodes, cx);
            graph.set_selected(selected, cx);
        });
        self.update_pane(cx);
    }

    /// Hands the open job's latest state to its pane; a pane for another
    /// job is dropped and the next render opens the right one.
    fn update_pane(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.pane.clone() else {
            return;
        };
        let job = self
            .selected
            .and_then(|id| self.jobs.iter().find(|job| job.id == id))
            .cloned();
        match job {
            Some(job) if pane.read(cx).job_id() == job.id => {
                pane.update(cx, |pane, cx| pane.set_job(job, cx));
            }
            _ => self.pane = None,
        }
    }

    fn poll_if_running(&mut self, cx: &mut Context<Self>) {
        let run = self.run.value();
        if let (Some(run), Some((attempt, polls))) = (run, self.awaited) {
            self.awaited = (run.attempt < attempt && polls > 0).then(|| (attempt, polls - 1));
        }
        let running = keeps_polling(self.run.value(), self.awaited.map(|(attempt, _)| attempt));
        self._poll = running.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(POLL_EVERY).await;
                this.update(cx, |view, cx| view.reload(cx)).ok();
            })
        });
    }

    fn select(&mut self, job: Option<u64>, cx: &mut Context<Self>) {
        self.selected = job;
        self.selected_name = job.and_then(|id| {
            self.jobs
                .iter()
                .find(|known| known.id == id)
                .map(|known| known.name.clone())
        });
        self.update_pane(cx);
        self.graph
            .update(cx, |graph, cx| graph.set_selected(job, cx));
        cx.notify();
    }

    fn rerun(&mut self, failed_only: bool, cx: &mut Context<Self>) {
        if self.action.is_busy() {
            return;
        }
        self.action = Submit::Busy;
        self.note = None;
        let repo = self.repo.clone();
        let run_id = self.run_id;
        self._action = Some(common::run(
            cx,
            move |gh| {
                if failed_only {
                    gh.rerun_failed_jobs(&repo, run_id)
                } else {
                    gh.rerun_run(&repo, run_id)
                }
            },
            move |view: &mut Self, result, cx| match result {
                Ok(()) => {
                    view.action = Submit::Idle;
                    // GitHub can still report the last attempt, finished, for a
                    // moment: wait for the new one (for two minutes at most).
                    let attempt = view.run.value().map_or(1, |run| run.attempt + 1);
                    view.awaited = Some((attempt, 24));
                    view.note = Some(if failed_only {
                        "Re-run of the failed jobs requested. The run updates as GitHub queues them."
                            .into()
                    } else {
                        "Re-run requested. The run updates as GitHub queues its jobs.".into()
                    });
                    view.reload(cx);
                }
                Err(error) => view.action = Submit::Failed(error.detail()),
            },
        ));
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.action.is_busy() {
            return;
        }
        self.action = Submit::Busy;
        self.note = None;
        let repo = self.repo.clone();
        let run_id = self.run_id;
        self._action = Some(common::run(
            cx,
            move |gh| gh.cancel_run(&repo, run_id),
            |view: &mut Self, result, cx| match result {
                Ok(()) => {
                    view.action = Submit::Idle;
                    view.cancelling = true;
                    view.note =
                        Some("Cancel requested. The run stops as its jobs wind down.".into());
                    view.reload(cx);
                }
                Err(error) => view.action = Submit::Failed(error.detail()),
            },
        ));
        cx.notify();
    }

    fn download(&mut self, artifact: RunArtifact, cx: &mut Context<Self>) {
        if self.downloading.is_some() {
            return;
        }
        self.downloading = Some(artifact.id);
        self.saved = None;
        self.download_error = None;
        self.note = None;
        let repo = self.repo.clone();
        self._download = Some(common::run(
            cx,
            move |gh| {
                save_download(&artifact.name, |file| {
                    gh.download_artifact(&repo, artifact.id, file)
                })
            },
            |view: &mut Self, result, _| {
                view.downloading = None;
                match result {
                    Ok(path) => view.saved = Some(path),
                    Err(error) => view.download_error = Some(error.detail()),
                }
            },
        ));
        cx.notify();
    }

    fn header(&self, run: Option<&WorkflowRun>, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.run.is_busy();
        let Some(run) = run else {
            return ui::page_header("Workflow run")
                .icon(IconName::Workflow)
                .into_any_element();
        };
        let (icon, color) = status_style(run.status);
        let title = if run.title.is_empty() {
            run.workflow.clone()
        } else {
            run.title.clone()
        };
        let any_failed = self
            .jobs
            .iter()
            .any(|job| matches!(job.status, CheckStatus::Failed | CheckStatus::Cancelled));
        let acting = self.action.is_busy();
        // Until GitHub shows the new attempt, or the cancelled run ends, a
        // second click would ask again.
        let waiting_rerun = self.awaited.is_some();
        let this = cx.entity().downgrade();
        let rerun = run.is_finished().then(|| {
            Button::new("run-rerun")
                .outline()
                .control()
                .icon(IconName::RotateCw)
                .label("Re-run")
                .loading(acting || waiting_rerun)
                .disabled(acting || waiting_rerun)
                .accessibility_label("Re-run jobs")
                .menu_trigger()
                .dropdown_menu(move |menu, _, _| {
                    let (all, failed) = (this.clone(), this.clone());
                    let menu = menu.item(
                        PopupMenuItem::new("Re-run all jobs")
                            .icon(IconName::RotateCw)
                            .on_click(move |_, _, cx| {
                                all.update(cx, |view, cx| view.rerun(false, cx)).ok();
                            }),
                    );
                    if any_failed {
                        menu.item(
                            PopupMenuItem::new("Re-run failed jobs")
                                .icon(IconName::CircleX)
                                .on_click(move |_, _, cx| {
                                    failed.update(cx, |view, cx| view.rerun(true, cx)).ok();
                                }),
                        )
                    } else {
                        menu
                    }
                })
        });
        let this = cx.entity().downgrade();
        let danger = cx.theme().danger;
        let cancel = (!run.is_finished()).then(|| {
            Button::new("run-cancel")
                .outline()
                .control()
                .icon(IconName::CircleStop)
                .label("Cancel run")
                .text_color(danger)
                .loading(acting || self.cancelling)
                .disabled(acting || self.cancelling)
                .on_click(move |_, window, cx| confirm_cancel(this.clone(), window, cx))
        });
        let workflow_url = format!("{}/blob/{}/{}", self.repo.url(), run.head_sha, run.path);
        let run_url = run.url.clone();
        ui::page_header(title)
            .prefix(Icon::new(icon).size_4().text_color(color))
            .after_title(
                div()
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} #{}", run.workflow, run.number)),
            )
            .action(h_flex().gap_2().children(rerun).children(cancel))
            .action(
                ui::outline_icon_button(
                    "run-workflow-file",
                    IconName::FileCode,
                    "Open workflow file",
                )
                .on_click(move |_, _, cx| cx.open_url(&workflow_url)),
            )
            .action(
                ui::outline_icon_button("run-browser", IconName::ExternalLink, "Open in browser")
                    .on_click(move |_, _, cx| cx.open_url(&run_url)),
            )
            .action(
                ui::icon_button("run-refresh", IconName::RefreshCw, "Refresh")
                    .loading(busy)
                    .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
            .into_any_element()
    }

    /// What started the run, where, by whom, how long, and what it made.
    fn band(&self, run: &WorkflowRun, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let now = Utc::now();
        let (login, avatar_url) = run.actor.as_ref().map_or_else(
            || (String::new(), String::new()),
            |actor| (actor.login.clone(), actor.avatar_url.clone()),
        );
        let artifacts = self.extras.artifacts.len();
        let separator = || div().text_color(tokens::text_faint()).child("·");
        h_flex()
            .flex_none()
            .h_8()
            .px(scale::content_inset())
            .gap_2()
            .items_center()
            .overflow_hidden()
            .border_b_1()
            .border_color(tokens::border_subtle())
            .text_size(scale::meta())
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .flex_none()
                    .text_color(theme.foreground)
                    .child(capitalized(status_word(run.status))),
            )
            .child(separator())
            .child(
                div()
                    .flex_none()
                    .child(format!("Triggered by {}", run.event)),
            )
            .when(!login.is_empty(), |band| {
                band.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .items_center()
                        .child(ui::avatar_named(avatar_url, login.clone(), dp(16.)))
                        .child(login),
                )
            })
            .children(
                run.branch
                    .clone()
                    .map(|branch| div().min_w_0().child(branch_chip(branch, cx))),
            )
            .child(separator())
            .child(div().flex_none().child(format!(
                "Total {}",
                format_duration(run.duration_seconds(now))
            )))
            .child(separator())
            .child(div().flex_none().child(match artifacts {
                0 => "No artifacts".to_owned(),
                1 => "1 artifact".to_owned(),
                count => format!("{count} artifacts"),
            }))
            .when(run.attempt > 1, |band| {
                band.child(separator())
                    .child(div().flex_none().child(format!("Attempt {}", run.attempt)))
            })
            .child(div().flex_1())
            .child(div().flex_none().child(format!(
                "Started {} ago",
                ui::relative_time(run.created_at, now)
            )))
    }

    fn jobs_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let now = Utc::now();
        let row = |id: ElementId, selected: bool| {
            h_flex()
                .id(id)
                .role(Role::Button)
                .h(scale::nav_row_height())
                .px_2()
                .gap_2()
                .items_center()
                .rounded(theme.radius)
                .when(selected, |row| row.bg(theme.list_active))
                .hover(|row| row.bg(theme.list_hover))
                .text_size(scale::body())
        };
        let summary = row("run-summary".into(), self.selected.is_none())
            .aria_label("Summary")
            .child(
                Icon::new(IconName::Workflow)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child("Summary")
            .on_click(cx.listener(|view, _, _, cx| view.select(None, cx)));
        let jobs: Vec<AnyElement> = self
            .jobs
            .iter()
            .map(|job| {
                let (icon, color) = status_style(job.status);
                let id = job.id;
                row(
                    SharedString::from(format!("run-job-{id}")).into(),
                    self.selected == Some(id),
                )
                .aria_label(format!("{}, {}", job.name, status_word(job.status)))
                .child(Icon::new(icon).small().flex_none().text_color(color))
                .child(div().flex_1().min_w_0().truncate().child(job.name.clone()))
                .child(
                    div()
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(theme.muted_foreground)
                        .children(job_duration_label(job.status, job.duration_seconds(now))),
                )
                .on_click(cx.listener(move |view, _, _, cx| view.select(Some(id), cx)))
                .into_any_element()
            })
            .collect();
        v_flex()
            .id("run-jobs")
            .flex_none()
            .w(dp(JOBS_COLUMN))
            .h_full()
            .p_2()
            .gap_0p5()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(tokens::border_subtle())
            .child(summary)
            .child(
                h_flex()
                    .h_8()
                    .px_2()
                    .mt_2()
                    .gap_2()
                    .items_center()
                    .child(ui::section_label("Jobs", cx))
                    .child(ui::count_badge(self.jobs.len(), cx)),
            )
            .children(jobs)
    }

    fn annotations(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let annotations = &self.extras.annotations;
        if annotations.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let count = |level: AnnotationLevel, one: &str, many: &str| {
            let n = annotations.iter().filter(|a| a.level == level).count();
            match n {
                0 => None,
                1 => Some(format!("1 {one}")),
                n => Some(format!("{n} {many}")),
            }
        };
        let summary: Vec<String> = [
            count(AnnotationLevel::Failure, "error", "errors"),
            count(AnnotationLevel::Warning, "warning", "warnings"),
            count(AnnotationLevel::Notice, "notice", "notices"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let rows = annotations.iter().enumerate().map(|(ix, annotation)| {
            let (icon, color) = match annotation.level {
                AnnotationLevel::Failure => (IconName::CircleX, tokens::closed()),
                AnnotationLevel::Warning => (IconName::TriangleAlert, tokens::pending()),
                AnnotationLevel::Notice => (IconName::Info, tokens::link()),
            };
            let place = match annotation.line {
                Some(line) => format!("{}:{line}", annotation.path),
                None => annotation.path.clone(),
            };
            let url = annotation.url.clone();
            let headline = annotation.title.clone().unwrap_or_else(|| {
                annotation
                    .message
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            });
            h_flex()
                .id(("run-annotation", ix))
                .role(Role::Link)
                .aria_label(format!(
                    "{headline}, {} in {}, open {place}",
                    annotation.job, place
                ))
                .px_2()
                .py_1p5()
                .gap_2()
                .items_start()
                .rounded(theme.radius)
                .hover(|row| row.bg(theme.list_hover))
                .cursor_pointer()
                .child(
                    div()
                        .pt_0p5()
                        .child(Icon::new(icon).small().text_color(color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(scale::body()).child(headline.clone()))
                        .when(annotation.title.is_some(), |column| {
                            column.child(
                                div()
                                    .text_size(scale::meta())
                                    .text_color(theme.muted_foreground)
                                    .child(annotation.message.clone()),
                            )
                        })
                        .child(
                            h_flex()
                                .gap_1()
                                .text_size(scale::meta())
                                .text_color(theme.muted_foreground)
                                .child(annotation.job.clone())
                                .child("·")
                                .child(div().text_color(tokens::link()).child(place)),
                        ),
                )
                .on_click(move |_, _, cx| cx.open_url(&url))
        });
        Some(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .px_2()
                        .gap_2()
                        .items_center()
                        .child(ui::group_title("Annotations"))
                        .child(
                            div()
                                .text_size(scale::meta())
                                .text_color(theme.muted_foreground)
                                .child(summary.join(", ")),
                        ),
                )
                .children(rows),
        )
    }

    fn artifacts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let artifacts = self.extras.artifacts.clone();
        let rows = artifacts.into_iter().map(|artifact| {
            let busy = self.downloading == Some(artifact.id);
            let other_busy = self.downloading.is_some() && !busy;
            let name = artifact.name.clone();
            let action = if artifact.expired {
                div()
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child("Expired")
                    .into_any_element()
            } else {
                let saved = artifact.clone();
                Button::new(("run-artifact", artifact.id as usize))
                    .ghost()
                    .small()
                    .icon(IconName::Download)
                    .label("Download")
                    .accessibility_label(format!("Download {name}"))
                    .loading(busy)
                    .disabled(busy || other_busy)
                    .on_click(cx.listener(move |view, _, _, cx| view.download(saved.clone(), cx)))
                    .into_any_element()
            };
            h_flex()
                .h(scale::row_height())
                .px_2()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(IconName::Package)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .child(artifact.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(theme.muted_foreground)
                        .child(size_label(artifact.size_bytes)),
                )
                .child(action)
        });
        let rows: Vec<_> = rows.collect();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .child(ui::group_title("Artifacts"))
                    .when(!rows.is_empty(), |row| {
                        row.child(ui::count_badge(rows.len(), cx))
                    }),
            )
            .when(rows.is_empty(), |column| {
                column.child(div().px_2().child(ui::empty_line(
                    IconName::Package,
                    "This run uploaded no artifacts.",
                    cx,
                )))
            })
            .children(rows)
    }

    /// Re-run, cancel, and download outcomes, under the band.
    fn notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (text, color) = if let Some(error) = self.action.error() {
            (error.to_owned(), theme.danger)
        } else if let Some(error) = &self.download_error {
            (
                format!("Could not download the artifact. {error}"),
                theme.danger,
            )
        } else if let Some(path) = &self.saved {
            (format!("Saved {}", path.display()), theme.muted_foreground)
        } else {
            (self.note.clone()?, theme.muted_foreground)
        };
        let saved = self.saved.clone();
        Some(
            h_flex()
                .flex_none()
                .px(scale::content_inset())
                .py_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(tokens::border_subtle())
                .text_size(scale::meta())
                .child(div().flex_1().min_w_0().text_color(color).child(text))
                .when_some(
                    saved.filter(|_| self.action.error().is_none()),
                    |row, path| {
                        row.child(
                            Button::new("run-saved-reveal")
                                .ghost()
                                .xsmall()
                                .icon(IconName::FolderOpen)
                                .label("Show in folder")
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        )
                    },
                )
                .child(
                    ui::icon_button("run-notice-close", IconName::X, "Dismiss")
                        .xsmall()
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.note = None;
                            view.saved = None;
                            view.download_error = None;
                            if view.action.error().is_some() {
                                view.action = Submit::Idle;
                            }
                            cx.notify();
                        })),
                ),
        )
    }

    fn summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("run-summary-body")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .w_full()
                    .p(scale::content_inset())
                    .gap(scale::section_gap())
                    .child(
                        div()
                            .w_full()
                            .rounded(cx.theme().radius_lg)
                            .border_1()
                            .border_color(cx.theme().border)
                            .overflow_hidden()
                            .child(self.graph.clone()),
                    )
                    .children(self.annotations(cx))
                    .child(self.artifacts(cx)),
            )
    }
}

impl Render for RunView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The open job's pane, made here because it needs the window.
        if let Some(id) = self.selected
            && self
                .pane
                .as_ref()
                .is_none_or(|pane| pane.read(cx).job_id() != id)
        {
            self.pane = self
                .jobs
                .iter()
                .find(|job| job.id == id)
                .cloned()
                .map(|job| {
                    let repo = self.repo.clone();
                    cx.new(|cx| JobPane::new(repo, job, window, cx))
                });
        }
        let run = self.run.value().cloned();
        let header = self.header(run.as_ref(), cx).into_any_element();
        let body = match (&self.run, run.as_ref()) {
            (Loadable::Failed(error), _) => {
                common::load_error(error, cx.listener(|view, _, _, cx| view.reload(cx)))
                    .into_any_element()
            }
            (_, None) => div().p_2().child(ui::skeleton_rows(8)).into_any_element(),
            (_, Some(_)) if self.fullscreen => div()
                .flex_1()
                .min_h_0()
                .child(self.graph.clone())
                .into_any_element(),
            (_, Some(run)) => {
                let content = match (self.selected, &self.pane) {
                    (Some(_), Some(pane)) => pane.clone().into_any_element(),
                    (Some(_), None) => div()
                        .p(scale::content_inset())
                        .child(ui::empty_line(
                            IconName::Workflow,
                            "This job is not in the run's latest attempt yet. It opens here when GitHub queues it.",
                            cx,
                        ))
                        .into_any_element(),
                    (None, _) => self.summary(cx).into_any_element(),
                };
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.band(run, cx))
                    .children(self.notice(cx))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.jobs_column(cx))
                            .child(div().flex_1().min_w_0().h_full().child(content)),
                    )
                    .into_any_element()
            }
        };
        v_flex().size_full().child(header).child(body)
    }
}

/// Whether the run is worth reading again: it is queued or going, or a
/// re-run was asked for and GitHub does not show its attempt yet.
fn keeps_polling(run: Option<&WorkflowRun>, awaited: Option<u32>) -> bool {
    run.is_some_and(|run| {
        !run.is_finished() || awaited.is_some_and(|attempt| run.attempt < attempt)
    })
}

/// "Cancel run" behind a confirmation.
fn confirm_cancel(view: WeakEntity<RunView>, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = view.clone();
        dialog.title("Cancel this run?").w(px(400.)).child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Jobs in progress stop, and jobs that have not started are skipped.",
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("run-cancel-keep")
                                .ghost()
                                .small()
                                .label("Keep running")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("run-cancel-confirm")
                                .danger()
                                .small()
                                .label("Yes, cancel the run")
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |view, cx| view.cancel(cx)).ok();
                                    window.close_dialog(cx);
                                }),
                        ),
                ),
        )
    });
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `912 B`, `4.6 KB`, `12.3 MB`.
fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024. && unit < UNITS.len() - 1 {
        size /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Streams a downloaded artifact into the Downloads folder as `<name>.zip`,
/// or ` (2)`, ` (3)`… when the name is taken: a file is created only where
/// none exists, so nothing is ever replaced, even by a second download at
/// the same moment. A download that fails leaves no partial file.
fn save_download(
    name: &str,
    fetch: impl FnOnce(&mut dyn std::io::Write) -> loupe_github::Result<u64>,
) -> loupe_github::Result<PathBuf> {
    let saving = |error: std::io::Error| {
        loupe_github::Error::Transport(format!("Could not save the file: {error}"))
    };
    let home = std::env::var_os("HOME").ok_or_else(|| {
        loupe_github::Error::Transport("Could not save the file: no home folder".into())
    })?;
    let folder = PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&folder).map_err(saving)?;
    let (path, file) = create_unique(&folder, name).map_err(saving)?;
    let mut writer = std::io::BufWriter::new(file);
    let written =
        fetch(&mut writer).and_then(|_| std::io::Write::flush(&mut writer).map_err(saving));
    match written {
        Ok(()) => Ok(path),
        Err(error) => {
            drop(writer);
            let _ = std::fs::remove_file(&path);
            Err(error)
        }
    }
}

/// Creates `<name>.zip` in `folder`, or the first of `<name> (2).zip`,
/// `<name> (3).zip`… that does not exist. Path separators in `name` become
/// dashes, so the file stays in `folder`.
fn create_unique(
    folder: &std::path::Path,
    name: &str,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    let safe: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    for n in 1..=1000 {
        let path = match n {
            1 => folder.join(format!("{safe}.zip")),
            n => folder.join(format!("{safe} ({n}).zip")),
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "a thousand files already have this name",
    ))
}

#[cfg(test)]
mod tests {

    use chrono::{DateTime, Utc};
    use loupe_github::{CheckStatus, WorkflowRun};

    use super::{capitalized, create_unique, keeps_polling, size_label};

    fn run(status: CheckStatus, attempt: u32) -> WorkflowRun {
        let at: DateTime<Utc> = "2026-11-11T10:00:00Z".parse().expect("time");
        WorkflowRun {
            id: 1,
            number: 15,
            attempt,
            workflow: "CI".into(),
            title: String::new(),
            path: ".github/workflows/ci.yml".into(),
            event: "push".into(),
            branch: None,
            head_sha: "abc".into(),
            status,
            actor: None,
            created_at: at,
            started_at: Some(at),
            updated_at: at,
            url: String::new(),
            check_suite: None,
            pull_requests: Vec::new(),
        }
    }

    #[test]
    fn a_rerun_keeps_polling_until_github_shows_the_new_attempt() {
        // Right after a re-run GitHub still reports attempt 9, failed.
        assert!(keeps_polling(Some(&run(CheckStatus::Failed, 9)), Some(10)));
        assert!(keeps_polling(Some(&run(CheckStatus::Queued, 10)), Some(10)));
        assert!(!keeps_polling(
            Some(&run(CheckStatus::Failed, 10)),
            Some(10)
        ));
        assert!(!keeps_polling(Some(&run(CheckStatus::Passed, 3)), None));
        assert!(keeps_polling(Some(&run(CheckStatus::Running, 3)), None));
        assert!(!keeps_polling(None, Some(2)));
    }

    #[test]
    fn sizes_read_in_the_largest_unit() {
        assert_eq!(size_label(912), "912 B");
        assert_eq!(size_label(4_663), "4.6 KB");
        assert_eq!(size_label(12_900_000), "12.3 MB");
    }

    #[test]
    fn a_download_never_replaces_a_file_and_stays_in_its_folder() {
        let folder = std::env::temp_dir().join(format!("loupe-43-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("linux.zip"), b"kept").expect("first file");
        let (second, _) = create_unique(&folder, "linux").expect("second");
        let (third, _) = create_unique(&folder, "linux").expect("third");
        let (escaped, _) = create_unique(&folder, "../up/../linux").expect("escaped");
        assert_eq!(second, folder.join("linux (2).zip"));
        assert_eq!(third, folder.join("linux (3).zip"));
        assert_eq!(escaped, folder.join("..-up-..-linux.zip"));
        assert_eq!(
            std::fs::read(folder.join("linux.zip")).expect("read"),
            b"kept"
        );
        std::fs::remove_dir_all(&folder).expect("clean up");
    }

    #[test]
    fn a_state_starts_a_sentence() {
        assert_eq!(capitalized("succeeded"), "Succeeded");
    }
}
