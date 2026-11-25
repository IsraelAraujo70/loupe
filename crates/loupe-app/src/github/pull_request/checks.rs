//! The checks tab: the loaded detail's checks, the end of a failed Actions
//! job's log on demand, and re-running failed jobs.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::sync::Arc;

use chrono::{DateTime, Local};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{
    ActionsJob, CheckRun, CheckSource, CheckStatus, PullRequestDetail, RepoRef, StepLog,
};

use super::tab::{PrTabContext, PrTabEvent};
use crate::github::common::{self, Submit};
use crate::github::widgets;
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::{self, ListRowFocus as _};

/// How many checks passed, failed, are still going, or were skipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ChecksSummary {
    pub(super) passing: u32,
    pub(super) failing: u32,
    pub(super) pending: u32,
    pub(super) skipped: u32,
}

impl ChecksSummary {
    pub(super) fn of(runs: &[CheckRun]) -> Self {
        runs.iter().fold(Self::default(), |mut summary, run| {
            match run.status {
                CheckStatus::Passed | CheckStatus::Neutral => summary.passing += 1,
                CheckStatus::Skipped => summary.skipped += 1,
                CheckStatus::Failed | CheckStatus::Cancelled | CheckStatus::ActionRequired => {
                    summary.failing += 1
                }
                CheckStatus::Queued | CheckStatus::Running => summary.pending += 1,
            }
            summary
        })
    }

    /// `3 passing / 1 failing / 2 pending / 1 skipped`, leaving out the
    /// zero counts. The one wording of a pull request's checks: the header,
    /// the merge box, and the Checks tab all show it. A check waiting for
    /// an action counts as failing, as in the merge box's check list.
    pub(super) fn label(&self) -> String {
        let parts: Vec<String> = [
            (self.passing, "passing"),
            (self.failing, "failing"),
            (self.pending, "pending"),
            (self.skipped, "skipped"),
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

/// The time a check took, what it is doing while it has no end time, or
/// the outcome in words when it did not run to a verdict: an icon alone does
/// not say "skipped".
fn run_duration(run: &CheckRun) -> Option<String> {
    match run.status {
        CheckStatus::Queued => Some("queued".into()),
        CheckStatus::Running => Some("running".into()),
        CheckStatus::Skipped => Some("skipped".into()),
        CheckStatus::Cancelled => Some("cancelled".into()),
        CheckStatus::Neutral => Some("neutral".into()),
        CheckStatus::ActionRequired => Some("action required".into()),
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

/// Where a check opens in Loupe: its Actions run, on its own job, whether it
/// is queued, running, or done. The job comes from the check itself, or else
/// from its Actions link (`…/actions/runs/<run>/job/<job>`); an Actions check
/// GitHub names no run for opens the pull request's workflow runs. A commit
/// status or another app's check has no page in Loupe.
pub(crate) fn check_route(repo: &RepoRef, head_ref: &str, run: &CheckRun) -> Option<Route> {
    let title = run.workflow.clone().unwrap_or_else(|| run.name.clone());
    if let Some(job) = run.actions {
        return Some(Route::WorkflowRun {
            repo: repo.clone(),
            run_id: job.run_id,
            job_id: Some(job.job_id),
            title,
        });
    }
    if let Some((run_id, job_id)) = run
        .details_url
        .as_deref()
        .and_then(|url| actions_link(repo, url))
    {
        return Some(Route::WorkflowRun {
            repo: repo.clone(),
            run_id,
            job_id,
            title,
        });
    }
    run.workflow.is_some().then(|| Route::Actions {
        repo: repo.clone(),
        branch: Some(head_ref.to_owned()),
    })
}

/// The run and job of an Actions link of `repo`.
fn actions_link(repo: &RepoRef, url: &str) -> Option<(u64, Option<u64>)> {
    let prefix = format!("{}/actions/runs/", repo.url());
    if !url.get(..prefix.len())?.eq_ignore_ascii_case(&prefix) {
        return None;
    }
    let rest = url[prefix.len()..].split(['?', '#']).next()?;
    let mut parts = rest.split('/');
    let run_id = parts.next()?.parse().ok()?;
    let job_id = match (parts.next(), parts.next()) {
        (Some("job"), Some(job)) => Some(job.parse().ok()?),
        _ => None,
    };
    Some((run_id, job_id))
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

/// The checks tab: a summary card, then the checks grouped by the workflow
/// and event that ran them, as on GitHub's checks page, one row per job. A
/// failed Actions job can show the end of its failed step's log, and the
/// card can re-run every failed job.
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
    /// The groups the user folded.
    folded: HashSet<CheckSource>,
    /// The pull request page's scroll, which the tab is drawn in.
    scroll: ScrollHandle,
    /// The list's children (per group its heading, then its checks and an
    /// open log's panel), to scroll the keyboard cursor's row into view.
    rows: ScrollHandle,
    focus: FocusHandle,
    /// The full name of the check under the keyboard cursor (J and K, or
    /// the arrows).
    cursor: Option<String>,
}

/// One check of the list as it is on screen: its full name, the route that
/// opens it, if any, and the list's children to bring into view when the
/// keyboard cursor comes to it (its row, with its group's heading above its
/// group's first row).
struct Line {
    name: String,
    route: Option<Route>,
    children: RangeInclusive<usize>,
}

impl EventEmitter<PrTabEvent> for ChecksTab {}

/// Whether a check is a failed Actions job, which has a log and can re-run.
fn failed_job(run: &CheckRun) -> Option<ActionsJob> {
    matches!(run.status, CheckStatus::Failed | CheckStatus::Cancelled)
        .then_some(run.actions)
        .flatten()
}

impl ChecksTab {
    /// `scroll` is the pull request page's scroll, which the tab is drawn
    /// in.
    pub fn new(context: PrTabContext, scroll: ScrollHandle, cx: &mut Context<Self>) -> Self {
        Self {
            context,
            logs: HashMap::new(),
            log_tasks: HashMap::new(),
            rerun: Submit::Idle,
            rerun_task: None,
            rerun_jobs: Vec::new(),
            freshness: Freshness::default(),
            folded: HashSet::new(),
            scroll,
            rows: ScrollHandle::new(),
            focus: crate::shell::page_focus(cx),
            cursor: None,
        }
    }

    /// The checks on screen, top to bottom: those of every group that is
    /// not folded, in the order the list draws its children.
    fn lines(&self) -> Vec<Line> {
        let detail = &self.context.detail;
        let mut lines = Vec::new();
        let mut child = 0;
        for (source, members) in detail.checks.by_source() {
            let heading = child;
            child += 1;
            if self.folded.contains(&source) {
                continue;
            }
            for (at, ix) in members.into_iter().enumerate() {
                let run = &detail.checks.runs[ix];
                lines.push(Line {
                    name: run.full_name(),
                    route: check_route(&self.context.pr.repo, &detail.head_ref, run),
                    children: if at == 0 { heading } else { child }..=child,
                });
                child += 1;
                if failed_job(run).is_some_and(|job| self.logs.contains_key(&job.job_id)) {
                    child += 1;
                }
            }
        }
        lines
    }

    /// Moves the keyboard cursor one check and scrolls it into view, with
    /// its group's heading when it is the group's first check.
    fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let lines = self.lines();
        let names: Vec<String> = lines.iter().map(|line| line.name.clone()).collect();
        let Some(next) = ui::step_cursor(&names, self.cursor.as_ref(), down) else {
            return;
        };
        if let Some(line) = lines.iter().find(|line| line.name == next) {
            ui::scroll_to_rows(&self.scroll, &self.rows, line.children.clone());
        }
        self.cursor = Some(next);
        cx.notify();
    }

    /// Enter or O: opens the run of the check under the cursor in this tab,
    /// as a click on its row does. A check with nothing to open, or no
    /// cursor, passes the key on to whoever handles it next.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self
            .lines()
            .into_iter()
            .find(|line| self.cursor.as_ref() == Some(&line.name))
            .and_then(|line| line.route);
        match route {
            Some(route) => crate::shell::navigate(route, window, cx),
            None => cx.propagate(),
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
                                .label("View workflow runs")
                                .tooltip("The workflow runs of this pull request's branch")
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
            .mx_2()
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

    /// One check's row. `focused` marks the row under the keyboard cursor
    /// while the list has the focus.
    fn check_row(
        &self,
        ix: usize,
        run: &CheckRun,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (icon, color) = status_style(run.status);
        let url = run.details_url.clone();
        let job = failed_job(run);
        let log_open = job.is_some_and(|job| self.logs.contains_key(&job.job_id));
        let route = check_route(&self.context.pr.repo, &self.context.detail.head_ref, run);
        let full_name = run.full_name();
        ui::list_row(("check", ix), cx)
            .role(Role::ListItem)
            .aria_label(match &run.description {
                Some(description) => format!("{full_name}: {description}"),
                None => full_name.clone(),
            })
            .when_some(route, |row, route| {
                row.role(Role::Button)
                    .aria_label(format!("{full_name}, open its run"))
                    .opens(route)
            })
            .keyboard_focus(focused, cx)
            .child(Icon::new(icon).small().text_color(color))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink(0.1)
                            .min_w_0()
                            .truncate()
                            .child(run.name.clone()),
                    )
                    .when_some(run.description.clone(), |row, description| {
                        let tip = description.clone();
                        row.child(
                            div()
                                .id(("check-description", ix))
                                .min_w_0()
                                .truncate()
                                .text_size(scale::meta())
                                .text_color(muted)
                                .tooltip(move |window, cx| {
                                    Tooltip::new(tip.clone()).build(window, cx)
                                })
                                .child(description),
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
                            format!("Hide the log of {full_name}")
                        } else {
                            format!("Show the log of {full_name}")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_log(job, cx))),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w_24()
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
                    .children(url.map(|url| {
                        ui::icon_button(
                            ("check-details", ix),
                            IconName::ExternalLink,
                            "Open details in browser",
                        )
                        .xsmall()
                        .accessibility_label(format!(
                            "Open the details of {full_name} in the browser"
                        ))
                        .on_click(move |_, _, cx| cx.open_url(&url))
                    })),
            )
    }

    /// A group's heading: the workflow and the event that ran it, as on
    /// GitHub's checks page, and how many checks it holds. A click folds it.
    fn source_header(
        &self,
        source: &CheckSource,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.folded.contains(source);
        let muted = cx.theme().muted_foreground;
        let (title, event) = match source {
            CheckSource::Workflow { name, event } => (name.clone(), event.clone()),
            CheckSource::Other => ("Other checks".to_owned(), None),
        };
        let spoken = match &event {
            Some(event) => format!("{title} on {event}"),
            None => title.clone(),
        };
        let noun = if count == 1 { "check" } else { "checks" };
        let toggle = source.clone();
        ui::list_group_header(
            SharedString::from(format!("check-source-{spoken}")),
            folded,
            cx,
        )
        .aria_label(format!("{spoken}, {count} {noun}"))
        .aria_expanded(!folded)
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.folded.remove(&toggle) {
                this.folded.insert(toggle.clone());
            }
            cx.notify();
        }))
        .child(ui::group_title(title).min_w_0().truncate())
        .children(event.map(|event| {
            div()
                .flex_none()
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!("on {event}"))
        }))
        .child(ui::count_badge(count, cx))
        .into_any_element()
    }
}

impl Render for ChecksTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        let cursor = self
            .cursor
            .clone()
            .filter(|_| self.focus.is_focused(window));
        let card = self.summary_card(cx).into_any_element();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (source, members) in detail.checks.by_source() {
            rows.push(self.source_header(&source, members.len(), cx));
            if self.folded.contains(&source) {
                continue;
            }
            for ix in members {
                let run = &detail.checks.runs[ix];
                let at_cursor = cursor.as_ref() == Some(&run.full_name());
                rows.push(self.check_row(ix, run, at_cursor, cx).into_any_element());
                if let Some(panel) = failed_job(run).and_then(|job| self.log_panel(job.job_id, cx))
                {
                    rows.push(panel);
                }
            }
        }
        v_flex()
            .id("pr-checks-list")
            .role(Role::Group)
            .aria_label("Checks")
            .w_full()
            .py_4()
            .gap_2()
            .key_context("ChecksTab")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_cursor(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_cursor(false, cx)))
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.open_cursor(window, cx)),
            )
            .child(div().px(scale::content_inset()).child(card))
            .child(
                ui::list_container()
                    .id("pr-checks-rows")
                    .track_scroll(&self.rows)
                    .children(rows),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{DateTime, Utc};
    use gpui_kit::{AppContext as _, ScrollHandle, TestAppContext, VisualTestContext};
    use loupe_github::{CheckRun, CheckSource, CheckStatus};

    use chrono::{Local, TimeZone};

    use loupe_github::{ActionsJob, RepoRef};

    use super::{ChecksSummary, ChecksTab, Freshness, check_route, format_duration, run_duration};
    use crate::github::pull_request::files::tests::detail;
    use crate::github::pull_request::tab::PrTabContext;
    use crate::route::Route;

    fn run(status: CheckStatus) -> CheckRun {
        CheckRun {
            name: "build".into(),
            workflow: None,
            event: None,
            description: None,
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
                passing: 3,
                failing: 3,
                pending: 2,
                skipped: 1
            }
        );
        assert_eq!(
            summary.label(),
            "3 passing / 3 failing / 2 pending / 1 skipped"
        );
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

        let mut skipped = run(CheckStatus::Skipped);
        skipped.started_at = at(1_000);
        skipped.completed_at = at(1_000);
        assert_eq!(run_duration(&skipped).as_deref(), Some("skipped"));
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

    #[test]
    fn a_check_opens_its_own_job_whether_it_is_pending_running_or_done() {
        let repo = RepoRef::new("IsraelAraujo70", "loupe");
        let job = |run: u64, job: u64| Route::WorkflowRun {
            repo: repo.clone(),
            run_id: run,
            job_id: Some(job),
            title: "CI".into(),
        };
        for status in [
            CheckStatus::Queued,
            CheckStatus::Running,
            CheckStatus::Failed,
            CheckStatus::Passed,
        ] {
            let check = CheckRun {
                workflow: Some("CI".into()),
                actions: Some(ActionsJob {
                    job_id: 138,
                    run_id: 45,
                }),
                ..run(status)
            };
            assert_eq!(
                check_route(&repo, "issue/121", &check),
                Some(job(45, 138)),
                "{status:?}"
            );
        }
        // No ids on the check: its Actions link names the run and the job.
        let linked = CheckRun {
            workflow: Some("CI".into()),
            details_url: Some(
                "https://github.com/israelaraujo70/loupe/actions/runs/46/job/139?pr=124".into(),
            ),
            ..run(CheckStatus::Queued)
        };
        assert_eq!(check_route(&repo, "issue/121", &linked), Some(job(46, 139)));
    }

    #[test]
    fn an_actions_check_without_a_run_opens_the_branch_runs_and_a_status_opens_nothing() {
        let repo = RepoRef::new("IsraelAraujo70", "loupe");
        let waiting = CheckRun {
            workflow: Some("Release".into()),
            ..run(CheckStatus::Queued)
        };
        assert_eq!(
            check_route(&repo, "issue/121", &waiting),
            Some(Route::Actions {
                repo: repo.clone(),
                branch: Some("issue/121".into()),
            })
        );
        let status = CheckRun {
            details_url: Some("https://ci.example/build/7".into()),
            ..run(CheckStatus::Running)
        };
        assert_eq!(check_route(&repo, "issue/121", &status), None);
        let other_repo = CheckRun {
            details_url: Some("https://github.com/acme/web/actions/runs/1/job/2".into()),
            ..run(CheckStatus::Running)
        };
        assert_eq!(check_route(&repo, "issue/121", &other_repo), None);
    }

    /// J and K walk the checks group by group, as the tab lists them, and
    /// pass over the checks of a folded group.
    #[gpui_kit::test]
    fn j_and_k_walk_the_checks_and_pass_over_a_folded_group(cx: &mut TestAppContext) {
        let check = |name: &str, workflow: Option<&str>| CheckRun {
            name: name.into(),
            workflow: workflow.map(Into::into),
            event: workflow.map(|_| "push".into()),
            ..run(CheckStatus::Passed)
        };
        let mut detail = detail();
        detail.checks.runs = vec![
            check("deploy", None),
            check("lint", Some("Lint")),
            check("build", Some("CI")),
            check("test", Some("CI")),
        ];
        let context = PrTabContext {
            pr: detail.pr.clone(),
            detail: Arc::new(detail),
        };
        let (mut cx, view) = crate::ui::open_list(cx, move |_, cx| {
            cx.new(|cx| ChecksTab::new(context, ScrollHandle::new(), cx))
        });
        let cx = &mut cx;
        cx.run_until_parked();
        let cursor = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.cursor.clone());
        cx.simulate_keystrokes("j");
        assert_eq!(cursor(cx).as_deref(), Some("CI / build (push)"));
        cx.simulate_keystrokes("j j");
        assert_eq!(cursor(cx).as_deref(), Some("Lint / lint (push)"));
        cx.simulate_keystrokes("k");
        assert_eq!(cursor(cx).as_deref(), Some("CI / test (push)"));

        view.update(cx, |view, cx| {
            view.folded.insert(CheckSource::Workflow {
                name: "Lint".into(),
                event: Some("push".into()),
            });
            cx.notify();
        });
        cx.simulate_keystrokes("j");
        assert_eq!(
            cursor(cx).as_deref(),
            Some("deploy"),
            "the folded group is passed over"
        );
        cx.simulate_keystrokes("j");
        assert_eq!(
            cursor(cx).as_deref(),
            Some("deploy"),
            "and the cursor stops at the end"
        );
    }
}
