//! One job of a run: its steps, each with its log under it, a search over
//! the log, and the steps' states as the run goes. GitHub publishes a job's
//! log only when the job finishes, so a job in progress shows its steps
//! live and its log once it ends.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{CheckStatus, JobLog, LogLineKind, RepoRef, WorkflowJob};

use super::{job_duration_label, status_style, status_word};
use crate::github::common;
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

const STEP_HEIGHT: f32 = 32.;
const LINE_HEIGHT: f32 = 20.;

enum LogState {
    /// The job has not finished: GitHub has no log for it yet.
    NotYet,
    Loading,
    Loaded(Arc<JobLog>),
    Failed(String),
}

/// One row of the pane's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// The step at this index of the job's steps.
    Step(usize),
    /// Line `line` of step number `step`.
    Line { step: u32, line: usize },
}

/// The rows to show: every step (only the ones with a match while
/// searching), and under each open step its lines (only the matching ones
/// while searching, when every step with a match is open).
fn rows(job: &WorkflowJob, log: Option<&JobLog>, open: &BTreeSet<u32>, query: &str) -> Vec<Row> {
    let needle = query.trim().to_lowercase();
    let mut rows = Vec::new();
    for (ix, step) in job.steps.iter().enumerate() {
        let lines = log.map_or(&[][..], |log| log.lines_of(step.number));
        let shown: Vec<usize> = if needle.is_empty() {
            if open.contains(&step.number) {
                (0..lines.len()).collect()
            } else {
                Vec::new()
            }
        } else {
            lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.text.to_lowercase().contains(&needle))
                .map(|(ix, _)| ix)
                .collect()
        };
        if !needle.is_empty() && shown.is_empty() {
            continue;
        }
        rows.push(Row::Step(ix));
        rows.extend(shown.into_iter().map(|line| Row::Line {
            step: step.number,
            line,
        }));
    }
    rows
}

pub struct JobPane {
    repo: RepoRef,
    job: WorkflowJob,
    log: LogState,
    /// Step numbers whose log is open.
    open: BTreeSet<u32>,
    search: Entity<InputState>,
    rows: Vec<Row>,
    list: ListState,
    _load: Option<Task<()>>,
    _search: Subscription,
}

impl JobPane {
    pub fn new(
        repo: RepoRef,
        job: WorkflowJob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search logs"));
        let subscription = cx.subscribe(&search, |pane: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                pane.rebuild(cx);
            }
        });
        let mut pane = Self {
            repo,
            job,
            log: LogState::NotYet,
            open: BTreeSet::new(),
            search,
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, dp(400.)),
            _load: None,
            _search: subscription,
        };
        pane.load_if_finished(cx);
        pane.rebuild(cx);
        pane
    }

    pub fn job_id(&self) -> u64 {
        self.job.id
    }

    /// The same job, as the latest poll read it: new step states, and the
    /// log once the job has finished.
    pub fn set_job(&mut self, job: WorkflowJob, cx: &mut Context<Self>) {
        if job == self.job {
            return;
        }
        self.job = job;
        self.load_if_finished(cx);
        self.rebuild(cx);
    }

    fn load_if_finished(&mut self, cx: &mut Context<Self>) {
        let finished = !matches!(
            self.job.status,
            CheckStatus::Queued | CheckStatus::Running | CheckStatus::ActionRequired
        );
        if !finished || !matches!(self.log, LogState::NotYet) {
            return;
        }
        if self.job.status == CheckStatus::Skipped {
            self.log = LogState::Loaded(Arc::default());
            return;
        }
        self.log = LogState::Loading;
        let repo = self.repo.clone();
        let job = self.job.clone();
        self._load = Some(common::run(
            cx,
            move |gh| gh.job_log(&repo, &job),
            |pane: &mut Self, result, cx| {
                pane.log = match result {
                    Ok(log) => {
                        // A failed step opens on its own: that is what one
                        // comes to read.
                        pane.open.extend(
                            pane.job
                                .steps
                                .iter()
                                .filter(|step| step.status == CheckStatus::Failed)
                                .map(|step| step.number),
                        );
                        LogState::Loaded(Arc::new(log))
                    }
                    Err(error) => LogState::Failed(error.detail()),
                };
                pane.rebuild(cx);
            },
        ));
    }

    fn loaded(&self) -> Option<&Arc<JobLog>> {
        match &self.log {
            LogState::Loaded(log) => Some(log),
            _ => None,
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string();
        let rows = rows(
            &self.job,
            self.loaded().map(Arc::as_ref),
            &self.open,
            &query,
        );
        if rows != self.rows {
            self.list.reset(rows.len());
            self.rows = rows;
        }
        cx.notify();
    }

    fn toggle(&mut self, step: u32, cx: &mut Context<Self>) {
        if !self.open.remove(&step) {
            self.open.insert(step);
        }
        self.rebuild(cx);
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let job = &self.job;
        let (icon, color) = status_style(job.status);
        let now = Utc::now();
        let mut sentence = status_word(job.status).to_owned();
        if let Some(end) = job
            .completed_at
            .filter(|_| job.status != CheckStatus::Running)
        {
            sentence.push_str(&format!(" {} ago", ui::relative_time(end, now)));
        }
        if let Some(time) = job_duration_label(job.status, job.duration_seconds(now))
            && job.status != CheckStatus::Queued
        {
            sentence.push_str(&format!(" in {time}"));
        }
        if let Some(runner) = &job.runner {
            sentence.push_str(&format!(" · {runner}"));
        }
        let matches = {
            let query = self.search.read(cx).value();
            (!query.trim().is_empty()).then(|| {
                let count = self
                    .rows
                    .iter()
                    .filter(|row| matches!(row, Row::Line { .. }))
                    .count();
                match count {
                    1 => "1 match".to_owned(),
                    count => format!("{count} matches"),
                }
            })
        };
        let url = job.url.clone();
        h_flex()
            .flex_none()
            .px(scale::content_inset())
            .py_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(tokens::border_subtle())
            .child(Icon::new(icon).size_5().flex_none().text_color(color))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(scale::title())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(job.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(scale::meta())
                            .text_color(theme.muted_foreground)
                            .child(sentence),
                    ),
            )
            .children(matches.map(|text| {
                div()
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(theme.muted_foreground)
                    .child(text)
            }))
            .child(
                div().w(dp(240.)).flex_none().child(
                    Input::new(&self.search)
                        .small()
                        .prefix(Icon::new(IconName::Search).xsmall())
                        .cleanable(true),
                ),
            )
            .children(url.map(|url| {
                ui::outline_icon_button(
                    "job-browser",
                    IconName::ExternalLink,
                    "Open job in browser",
                )
                .on_click(move |_, _, cx| cx.open_url(&url))
            }))
    }

    fn note(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let line = |text: &'static str| {
            div()
                .flex_none()
                .px(scale::content_inset())
                .py_2()
                .text_size(scale::meta())
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        match &self.log {
            LogState::NotYet if self.job.status == CheckStatus::Running => Some(line(
                "The steps update as the job runs. GitHub publishes the log when the job finishes.",
            )),
            LogState::NotYet => Some(line(
                "The job has not started. Its steps appear when a runner picks it up.",
            )),
            LogState::Failed(message) => Some(
                div()
                    .flex_none()
                    .px(scale::content_inset())
                    .py_2()
                    .child(ui::error_banner(
                        format!("Could not load the log. {message}"),
                        cx.listener(|pane, _, _, cx| {
                            pane.log = LogState::NotYet;
                            pane.load_if_finished(cx);
                            cx.notify();
                        }),
                    ))
                    .into_any_element(),
            ),
            LogState::Loading | LogState::Loaded(_) => None,
        }
    }
}

impl Render for JobPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = Rc::new(self.rows.clone());
        let job = Rc::new(self.job.clone());
        let log = self.loaded().cloned();
        let loading = matches!(self.log, LogState::Loading);
        let open = Rc::new(self.open.clone());
        let searching = !self.search.read(cx).value().trim().is_empty();
        let pane = cx.entity().downgrade();
        let body = if self.job.steps.is_empty() && !loading {
            div()
                .px(scale::content_inset())
                .py_3()
                .child(ui::empty_line(
                    IconName::ListChecks,
                    "This job has no steps.",
                    cx,
                ))
                .into_any_element()
        } else if searching && self.rows.is_empty() {
            div()
                .px(scale::content_inset())
                .py_3()
                .child(ui::empty_line(IconName::Search, "No line matches.", cx))
                .into_any_element()
        } else {
            list(self.list.clone(), move |ix, _, cx| {
                let Some(row) = rows.get(ix).copied() else {
                    return div().into_any_element();
                };
                match row {
                    Row::Step(step_ix) => {
                        let Some(step) = job.steps.get(step_ix) else {
                            return div().into_any_element();
                        };
                        let lines = log
                            .as_ref()
                            .map_or(0, |log| log.lines_of(step.number).len());
                        let is_open = searching || open.contains(&step.number);
                        step_row(step, step_ix, lines, is_open, loading, pane.clone(), cx)
                    }
                    Row::Line { step, line } => {
                        let Some(text) = log.as_ref().and_then(|log| log.lines_of(step).get(line))
                        else {
                            return div().into_any_element();
                        };
                        line_row(line + 1, text.kind, &text.text, cx)
                    }
                }
            })
            .size_full()
            .px_2()
            .into_any_element()
        };
        v_flex()
            .size_full()
            .child(self.header(cx))
            .children(self.note(cx))
            .child(div().flex_1().min_h_0().py_1().child(body))
    }
}

fn step_row(
    step: &loupe_github::JobStep,
    ix: usize,
    lines: usize,
    is_open: bool,
    loading: bool,
    pane: WeakEntity<JobPane>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (icon, color) = status_style(step.status);
    let seconds = step
        .started_at
        .zip(step.completed_at)
        .map(|(start, end)| (end - start).num_seconds().max(0))
        .or_else(|| {
            step.started_at
                .filter(|_| step.status == CheckStatus::Running)
                .map(|start| (Utc::now() - start).num_seconds().max(0))
        });
    let number = step.number;
    let can_open = lines > 0;
    let name = format!(
        "{}, {}{}",
        step.name,
        status_word(step.status),
        if can_open {
            if is_open {
                ", log open"
            } else {
                ", log closed"
            }
        } else {
            ""
        }
    );
    h_flex()
        .id(("job-step", ix))
        .role(Role::Button)
        .aria_label(name)
        .h(dp(STEP_HEIGHT))
        .w_full()
        .px_2()
        .gap_2()
        .items_center()
        .rounded(theme.radius)
        .hover(|row| row.bg(theme.list_hover))
        .child(div().flex_none().size_4().children(can_open.then(|| {
            Icon::new(if is_open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .small()
            .text_color(theme.muted_foreground)
        })))
        .child(Icon::new(icon).small().flex_none().text_color(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(scale::body())
                .child(step.name.clone()),
        )
        .when(loading && step.status != CheckStatus::Skipped, |row| {
            row.child(
                div()
                    .text_size(scale::meta())
                    .text_color(tokens::text_faint())
                    .child("loading log"),
            )
        })
        .child(
            div()
                .flex_none()
                .w_16()
                .text_right()
                .text_size(scale::meta())
                .text_color(theme.muted_foreground)
                .children(job_duration_label(step.status, seconds)),
        )
        .when(can_open, |row| {
            row.on_click(move |_, _, cx| {
                pane.update(cx, |pane, cx| pane.toggle(number, cx)).ok();
            })
        })
        .into_any_element()
}

fn line_row(number: usize, kind: LogLineKind, text: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = match kind {
        LogLineKind::Error => theme.danger,
        LogLineKind::Warning => tokens::pending(),
        LogLineKind::Notice => tokens::link(),
        LogLineKind::Command => tokens::link(),
        LogLineKind::Debug => tokens::text_faint(),
        LogLineKind::Plain | LogLineKind::Group => theme.foreground,
    };
    let prefix = match kind {
        LogLineKind::Error => "Error: ",
        LogLineKind::Warning => "Warning: ",
        LogLineKind::Notice => "Notice: ",
        _ => "",
    };
    h_flex()
        .h(dp(LINE_HEIGHT))
        .w_full()
        .pl_2()
        .pr(scale::content_inset())
        .gap_3()
        .items_center()
        .font_family(theme.mono_font_family.clone())
        .text_size(scale::code())
        .when(kind == LogLineKind::Error, |row| {
            row.bg(theme.danger.opacity(0.08))
        })
        .child(
            div()
                .flex_none()
                .w_12()
                .text_right()
                .text_color(tokens::text_faint())
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(color)
                .when(kind == LogLineKind::Group, |text| {
                    text.font_weight(FontWeight::SEMIBOLD)
                })
                .child(format!("{prefix}{text}")),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use loupe_github::{CheckStatus, JobLog, JobStep, LogLine, LogLineKind, WorkflowJob};

    use super::{Row, rows};

    fn job() -> WorkflowJob {
        let step = |number: u32, name: &str| JobStep {
            number,
            name: name.into(),
            status: CheckStatus::Passed,
            started_at: None,
            completed_at: None,
        };
        WorkflowJob {
            id: 1,
            name: "build".into(),
            status: CheckStatus::Passed,
            started_at: None,
            completed_at: None,
            url: None,
            runner: None,
            steps: vec![
                step(1, "Set up job"),
                step(2, "Run tests"),
                step(3, "Complete job"),
            ],
        }
    }

    fn log() -> JobLog {
        let line = |text: &str| LogLine {
            kind: LogLineKind::Plain,
            text: text.into(),
        };
        JobLog {
            steps: vec![
                (1, vec![line("Runner image"), line("Cache restored")]),
                (
                    2,
                    vec![
                        line("test a ... ok"),
                        line("test b ... FAILED"),
                        line("cache hit"),
                    ],
                ),
            ],
        }
    }

    #[test]
    fn a_closed_step_hides_its_lines_and_an_open_one_shows_them() {
        let open = BTreeSet::from([2]);
        assert_eq!(
            rows(&job(), Some(&log()), &open, ""),
            [
                Row::Step(0),
                Row::Step(1),
                Row::Line { step: 2, line: 0 },
                Row::Line { step: 2, line: 1 },
                Row::Line { step: 2, line: 2 },
                Row::Step(2),
            ]
        );
    }

    #[test]
    fn a_search_shows_only_matching_lines_under_their_steps() {
        assert_eq!(
            rows(&job(), Some(&log()), &BTreeSet::new(), " CACHE "),
            [
                Row::Step(0),
                Row::Line { step: 1, line: 1 },
                Row::Step(1),
                Row::Line { step: 2, line: 2 },
            ]
        );
        assert!(rows(&job(), Some(&log()), &BTreeSet::new(), "nothing").is_empty());
    }

    #[test]
    fn without_a_log_the_steps_still_show() {
        assert_eq!(
            rows(&job(), None, &BTreeSet::from([1]), ""),
            [Row::Step(0), Row::Step(1), Row::Step(2)]
        );
    }
}
