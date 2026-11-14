//! The checks tab: the loaded detail's checks, the end of a failed Actions
//! job's log on demand, and re-running failed jobs.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Local};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{ActionsJob, CheckRun, CheckStatus, PullRequestDetail, StepLog};

use super::tab::{PrTabContext, PrTabEvent};
use crate::github::common::{self, Submit};
use crate::github::widgets;
use crate::route::Route;
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

/// How many checks passed, failed, or are still going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ChecksSummary {
    pub(super) passing: u32,
    pub(super) failing: u32,
    pub(super) pending: u32,
}

impl ChecksSummary {
    pub(super) fn of(runs: &[CheckRun]) -> Self {
        runs.iter().fold(Self::default(), |mut summary, run| {
            match run.status {
                CheckStatus::Passed | CheckStatus::Neutral | CheckStatus::Skipped => {
                    summary.passing += 1
                }
                CheckStatus::Failed | CheckStatus::Cancelled => summary.failing += 1,
                CheckStatus::Queued | CheckStatus::Running | CheckStatus::ActionRequired => {
                    summary.pending += 1
                }
            }
            summary
        })
    }

    /// `3 passing / 1 failing / 2 pending`, leaving out the zero counts.
    pub(super) fn label(&self) -> String {
        let parts: Vec<String> = [
            (self.passing, "passing"),
            (self.failing, "failing"),
            (self.pending, "pending"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect();
        if parts.is_empty() {
            "No checks".into()
        } else {
            parts.join(" / ")
        }
    }
}

/// `45s`, `1m 23s`, `2h 5m`.
pub(crate) fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3_600, (s % 3_600) / 60),
    }
}

/// The time a check took, or what it is doing while it has no end time.
fn run_duration(run: &CheckRun) -> Option<String> {
    match run.status {
        CheckStatus::Queued => Some("queued".into()),
        CheckStatus::Running => Some("running".into()),
        _ => match (run.started_at, run.completed_at) {
            (Some(start), Some(end)) => Some(format_duration((end - start).num_seconds())),
            _ => None,
        },
    }
}

pub(crate) fn status_style(status: CheckStatus) -> (IconName, Hsla) {
    match status {
        CheckStatus::Queued => (IconName::CircleDashed, tokens::pending()),
        CheckStatus::Running => (IconName::CircleDot, tokens::pending()),
        CheckStatus::Passed => (IconName::CircleCheck, tokens::open()),
        CheckStatus::Failed => (IconName::CircleX, tokens::closed()),
        CheckStatus::Neutral => (IconName::CircleMinus, tokens::text_muted()),
        CheckStatus::Skipped => (IconName::CircleSlash, tokens::text_muted()),
        CheckStatus::Cancelled => (IconName::Ban, tokens::text_muted()),
        CheckStatus::ActionRequired => (IconName::CircleAlert, tokens::pending()),
    }
}

/// How current the checks on screen are: the quiet line in the summary
/// card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Freshness {
    /// When GitHub last confirmed the checks on screen.
    pub(super) checked_at: Option<DateTime<Local>>,
    /// A request is in flight.
    pub(super) busy: bool,
    /// The view asks GitHub again on its own while checks run.
    pub(super) live: bool,
    /// Asking on its own waits for a low rate limit to reset.
    pub(super) paused_until: Option<DateTime<Local>>,
}

impl Freshness {
    /// `Updating…`, or the time of the last answer, with whether the checks
    /// refresh on their own.
    pub(super) fn label(&self) -> Option<String> {
        if self.busy {
            return Some("Updating…".into());
        }
        let updated = self
            .checked_at
            .map(|at| format!("Updated {}", at.format("%H:%M:%S")));
        let mode = match self.paused_until {
            Some(until) => Some(format!(
                "Auto-refresh paused by the rate limit until {}",
                until.format("%H:%M")
            )),
            None if self.live => Some("Auto-refresh on".into()),
            None => None,
        };
        match (mode, updated) {
            (Some(mode), Some(updated)) => Some(format!("{mode} · {updated}")),
            (mode, updated) => mode.or(updated),
        }
    }
}

const RERUN_NOTE: &str = "Re-run requested. The checks update as GitHub queues the jobs.";

/// A failed job's log under its check.
enum JobLog {
    Loading,
    Loaded(StepLog, ScrollHandle),
    Failed(String),
}

/// The checks tab: a summary card, then one 36 px row per check run. A failed
/// Actions job can show the end of its failed step's log, and the card can
/// re-run every failed job.
pub struct ChecksTab {
    context: PrTabContext,
    /// Logs by Actions job id, for the jobs whose log is open.
    logs: HashMap<u64, JobLog>,
    log_tasks: HashMap<u64, Task<()>>,
    rerun: Submit,
    rerun_task: Option<Task<()>>,
    /// The jobs a successful re-run replaced. The notice stays until the
    /// reloaded checks show other jobs: right after a re-run GitHub can list
    /// no checks at all for a moment.
    rerun_jobs: Vec<u64>,
    freshness: Freshness,
}

impl EventEmitter<PrTabEvent> for ChecksTab {}

/// Whether a check is a failed Actions job, which has a log and can re-run.
fn failed_job(run: &CheckRun) -> Option<ActionsJob> {
    matches!(run.status, CheckStatus::Failed | CheckStatus::Cancelled)
        .then_some(run.actions)
        .flatten()
}

impl ChecksTab {
    pub fn new(context: PrTabContext) -> Self {
        Self {
            context,
            logs: HashMap::new(),
            log_tasks: HashMap::new(),
            rerun: Submit::Idle,
            rerun_task: None,
            rerun_jobs: Vec::new(),
            freshness: Freshness::default(),
        }
    }

    pub(super) fn set_freshness(&mut self, freshness: Freshness, cx: &mut Context<Self>) {
        if self.freshness != freshness {
            self.freshness = freshness;
            cx.notify();
        }
    }

    pub fn set_detail(&mut self, detail: Arc<PullRequestDetail>, cx: &mut Context<Self>) {
        let replaced = !detail.checks.runs.is_empty()
            && !detail.checks.runs.iter().any(|run| {
                run.actions
                    .is_some_and(|job| self.rerun_jobs.contains(&job.job_id))
            });
        if replaced {
            self.rerun_jobs.clear();
        }
        self.context.detail = detail;
        cx.notify();
    }

    fn toggle_log(&mut self, job: ActionsJob, cx: &mut Context<Self>) {
        let job_id = job.job_id;
        if self.logs.remove(&job_id).is_some() {
            self.log_tasks.remove(&job_id);
            cx.notify();
            return;
        }
        self.logs.insert(job_id, JobLog::Loading);
        let repo = self.context.pr.repo.clone();
        let task = common::run(
            cx,
            move |gh| gh.failed_step_log(&repo, job_id),
            move |this, result, _| {
                this.log_tasks.remove(&job_id);
                if let Some(slot) = this.logs.get_mut(&job_id) {
                    *slot = match result {
                        Ok(log) => {
                            let scroll = ScrollHandle::new();
                            scroll.scroll_to_bottom();
                            JobLog::Loaded(log, scroll)
                        }
                        Err(error) => JobLog::Failed(error.detail()),
                    };
                }
            },
        );
        self.log_tasks.insert(job_id, task);
        cx.notify();
    }

    /// Re-runs the failed jobs of every workflow run with a failed check.
    fn rerun_failed(&mut self, cx: &mut Context<Self>) {
        if self.rerun.is_busy() {
            return;
        }
        let failed: Vec<ActionsJob> = self
            .context
            .detail
            .checks
            .runs
            .iter()
            .filter_map(failed_job)
            .collect();
        let mut run_ids: Vec<u64> = failed.iter().map(|job| job.run_id).collect();
        run_ids.sort_unstable();
        run_ids.dedup();
        if run_ids.is_empty() {
            return;
        }
        self.rerun = Submit::Busy;
        let repo = self.context.pr.repo.clone();
        self.rerun_task = Some(common::run(
            cx,
            move |gh| {
                run_ids
                    .iter()
                    .try_for_each(|run_id| gh.rerun_failed_jobs(&repo, *run_id))
            },
            move |this, result, cx| match result {
                Ok(()) => {
                    this.rerun = Submit::Idle;
                    this.rerun_jobs = failed.iter().map(|job| job.job_id).collect();
                    cx.emit(PrTabEvent::ChecksRequested(failed));
                }
                Err(error) => this.rerun = Submit::Failed(error.detail()),
            },
        ));
        cx.notify();
    }

    fn summary_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let detail = &self.context.detail;
        let runs = &detail.checks.runs;
        let muted = cx.theme().muted_foreground;
        let summary = ChecksSummary::of(runs);
        let headline = match detail.checks.state {
            Some(state) => widgets::checks_label(state),
            None => "Checks",
        };
        let can_rerun = runs.iter().any(|run| failed_job(run).is_some());
        let actions = Route::Actions {
            repo: self.context.pr.repo.clone(),
            branch: Some(detail.head_ref.clone()),
        };
        let note = match &self.rerun {
            Submit::Failed(message) => Some((message.clone(), cx.theme().danger)),
            _ if !self.rerun_jobs.is_empty() => Some((RERUN_NOTE.to_owned(), muted)),
            _ => None,
        };
        ui::card(cx).child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .children(detail.checks.state.map(|state| {
                            Icon::new(widgets::checks_icon_name(state))
                                .size_5()
                                .text_color(widgets::checks_color(state))
                        }))
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_size(scale::body())
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(headline),
                                )
                                .child(
                                    div()
                                        .text_size(scale::meta())
                                        .text_color(muted)
                                        .child(summary.label()),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .items_center()
                                .children(self.freshness.label().map(|label| {
                                    div()
                                        .id("checks-freshness")
                                        .role(Role::Status)
                                        .aria_label(label.clone())
                                        .text_size(scale::meta())
                                        .text_color(muted)
                                        .child(label)
                                }))
                                .child(
                                    ui::icon_button(
                                        "checks-refresh",
                                        IconName::RefreshCw,
                                        "Refresh checks",
                                    )
                                    .disabled(self.freshness.busy)
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(PrTabEvent::Reload)),
                                    ),
                                ),
                        )
                        .child(
                            Button::new("checks-open-actions")
                                .outline()
                                .small()
                                .icon(IconName::Workflow)
                                .label("Open in Actions")
                                .tooltip("This branch's workflow runs")
                                .on_click(move |event, window, cx| {
                                    crate::shell::open(actions.clone(), event, window, cx)
                                }),
                        )
                        .when(can_rerun, |row| {
                            row.child(
                                Button::new("rerun-failed")
                                    .outline()
                                    .small()
                                    .icon(IconName::RotateCw)
                                    .label("Re-run failed jobs")
                                    .loading(self.rerun.is_busy())
                                    .disabled(self.rerun.is_busy())
                                    .on_click(cx.listener(|this, _, _, cx| this.rerun_failed(cx))),
                            )
                        }),
                )
                .children(note.map(|(message, color)| {
                    div()
                        .text_size(scale::meta())
                        .text_color(color)
                        .child(message)
                })),
        )
    }

    fn log_panel(&self, job_id: u64, cx: &App) -> Option<AnyElement> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let panel = v_flex()
            .mx(scale::content_inset())
            .mb_2()
            .p(scale::card_padding())
            .gap_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .text_size(scale::meta());
        Some(match self.logs.get(&job_id)? {
            JobLog::Loading => panel.child(ui::skeleton_rows(3)).into_any_element(),
            JobLog::Failed(message) => panel
                .child(
                    div()
                        .text_color(danger)
                        .child(format!("Could not load the log: {message}")),
                )
                .into_any_element(),
            JobLog::Loaded(log, scroll) => {
                let title = match &log.step {
                    Some(step) => format!("Failed step: {step}"),
                    None => "End of the job log".to_owned(),
                };
                panel
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                            .when(log.truncated, |row| {
                                row.child(
                                    div()
                                        .text_color(muted)
                                        .child(format!("last {} lines", log.lines.len())),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .id(("job-log", job_id))
                            .max_h(dp(360.))
                            .overflow_y_scroll()
                            .track_scroll(scroll)
                            .font_family(theme.mono_font_family.clone())
                            .children(log.lines.iter().map(|line| {
                                let is_error = line.contains("##[error]");
                                div()
                                    .when(is_error, |line| line.text_color(danger))
                                    .child(line.clone())
                            })),
                    )
                    .into_any_element()
            }
        })
    }

    fn check_row(&self, ix: usize, run: &CheckRun, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let (icon, color) = status_style(run.status);
        let url = run.details_url.clone();
        let job = failed_job(run);
        let log_open = job.is_some_and(|job| self.logs.contains_key(&job.job_id));
        h_flex()
            .id(("check", ix))
            .h(scale::row_height())
            .px(scale::content_inset())
            .gap_3()
            .items_center()
            .hover(move |row| row.bg(hover))
            .child(Icon::new(icon).small().text_color(color))
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
                            .font_weight(FontWeight::MEDIUM)
                            .child(run.name.clone()),
                    )
                    .when_some(run.workflow.clone(), |row, workflow| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(scale::meta())
                                .text_color(muted)
                                .child(workflow),
                        )
                    }),
            )
            .when(run.is_required, |row| {
                row.child(crate::ui::chip().small().child("Required"))
            })
            .when_some(job, |row, job| {
                row.child(
                    Button::new(("check-log", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ScrollText)
                        .label(if log_open { "Hide log" } else { "Log" })
                        .accessibility_label(if log_open {
                            format!("Hide the log of {}", run.name)
                        } else {
                            format!("Show the log of {}", run.name)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_log(job, cx))),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w_16()
                    .text_right()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .children(run_duration(run)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_6()
                    .justify_center()
                    .children(run.actions.map(|job| {
                        let route = Route::WorkflowRun {
                            repo: self.context.pr.repo.clone(),
                            run_id: job.run_id,
                            job_id: Some(job.job_id),
                            title: run.workflow.clone().unwrap_or_else(|| run.name.clone()),
                        };
                        ui::icon_button(("check-run", ix), IconName::Workflow, "Open in Actions")
                            .xsmall()
                            .accessibility_label(format!("Open {} in Actions", run.name))
                            .on_click(move |event, window, cx| {
                                crate::shell::open(route.clone(), event, window, cx)
                            })
                    })),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_6()
                    .justify_center()
                    .children(url.map(|url| {
                        ui::icon_button(
                            ("check-details", ix),
                            IconName::ExternalLink,
                            "Open details in browser",
                        )
                        .xsmall()
                        .on_click(move |_, _, cx| cx.open_url(&url))
                    })),
            )
    }
}

impl Render for ChecksTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let detail = self.context.detail.clone();
        if detail.checks.runs.is_empty() {
            let (title, text) = if self.rerun_jobs.is_empty() {
                (
                    "No checks",
                    "This pull request has no checks or commit statuses.",
                )
            } else {
                ("Re-run requested", RERUN_NOTE)
            };
            return ui::empty_state(IconName::ListChecks, title, text).into_any_element();
        }
        let card = self.summary_card(cx).into_any_element();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (ix, run) in detail.checks.runs.iter().enumerate() {
            rows.push(self.check_row(ix, run, cx).into_any_element());
            if let Some(panel) = failed_job(run).and_then(|job| self.log_panel(job.job_id, cx)) {
                rows.push(panel);
            }
        }
        v_flex()
            .w_full()
            .py_4()
            .gap_3()
            .child(div().px(scale::content_inset()).child(card))
            .child(v_flex().children(rows))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use loupe_github::{CheckRun, CheckStatus};

    use chrono::{Local, TimeZone};

    use super::{ChecksSummary, Freshness, format_duration, run_duration};

    fn run(status: CheckStatus) -> CheckRun {
        CheckRun {
            name: "build".into(),
            workflow: None,
            status,
            started_at: None,
            completed_at: None,
            details_url: None,
            is_required: false,
            actions: None,
        }
    }

    fn at(seconds: i64) -> Option<DateTime<Utc>> {
        DateTime::<Utc>::from_timestamp(seconds, 0)
    }

    #[test]
    fn summary_counts_each_bucket() {
        let runs = [
            run(CheckStatus::Passed),
            run(CheckStatus::Passed),
            run(CheckStatus::Skipped),
            run(CheckStatus::Neutral),
            run(CheckStatus::Failed),
            run(CheckStatus::Cancelled),
            run(CheckStatus::Running),
            run(CheckStatus::Queued),
            run(CheckStatus::ActionRequired),
        ];
        let summary = ChecksSummary::of(&runs);
        assert_eq!(
            summary,
            ChecksSummary {
                passing: 4,
                failing: 2,
                pending: 3
            }
        );
        assert_eq!(summary.label(), "4 passing / 2 failing / 3 pending");
    }

    #[test]
    fn summary_label_leaves_out_zero_counts() {
        let runs = [run(CheckStatus::Passed), run(CheckStatus::Failed)];
        assert_eq!(ChecksSummary::of(&runs).label(), "1 passing / 1 failing");
        assert_eq!(ChecksSummary::of(&[]).label(), "No checks");
    }

    #[test]
    fn durations_use_the_two_largest_units() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(60), "1m 0s");
        assert_eq!(format_duration(83), "1m 23s");
        assert_eq!(format_duration(3_599), "59m 59s");
        assert_eq!(format_duration(7_500), "2h 5m");
        assert_eq!(format_duration(-5), "0s");
    }

    #[test]
    fn run_duration_is_end_minus_start_or_the_running_state() {
        let mut finished = run(CheckStatus::Passed);
        finished.started_at = at(1_000);
        finished.completed_at = at(1_083);
        assert_eq!(run_duration(&finished).as_deref(), Some("1m 23s"));

        let mut running = run(CheckStatus::Running);
        running.started_at = at(1_000);
        assert_eq!(run_duration(&running).as_deref(), Some("running"));

        assert_eq!(
            run_duration(&run(CheckStatus::Queued)).as_deref(),
            Some("queued")
        );
        assert_eq!(run_duration(&run(CheckStatus::Passed)), None);
    }

    #[test]
    fn freshness_says_when_the_checks_were_last_confirmed_and_whether_they_refresh() {
        let at = Local.with_ymd_and_hms(2026, 11, 11, 14, 32, 5).unwrap();
        let until = Local.with_ymd_and_hms(2026, 11, 11, 15, 4, 0).unwrap();
        let updated = Freshness {
            checked_at: Some(at),
            ..Freshness::default()
        };
        assert_eq!(updated.label().as_deref(), Some("Updated 14:32:05"));
        let live = Freshness {
            live: true,
            ..updated
        };
        assert_eq!(
            live.label().as_deref(),
            Some("Auto-refresh on · Updated 14:32:05")
        );
        let paused = Freshness {
            paused_until: Some(until),
            ..live
        };
        assert_eq!(
            paused.label().as_deref(),
            Some("Auto-refresh paused by the rate limit until 15:04 · Updated 14:32:05")
        );
        let busy = Freshness { busy: true, ..live };
        assert_eq!(busy.label().as_deref(), Some("Updating…"));
        assert_eq!(Freshness::default().label(), None);
    }
}
