//! The merge box at the top of the conversation, after GitHub's Overview:
//! what stands between the pull request and a merge, the merge button,
//! progress bars for checks, resolved conversations, and approvals, and
//! under the checks' bar the list of checks it opens.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{CheckRun, PullRequestDetail};

use super::PullRequestView;
use super::model::{
    MergeAction, MergeTone, Panel, Progress, approvals, available_actions, check_groups,
    check_outcome, check_tally, checks_open_at_first, merge_status, resolved_threads,
};
use crate::github::pull_request::checks::status_style;
use crate::route::Route;
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

fn tone_look(tone: MergeTone, cx: &App) -> (IconName, Hsla) {
    match tone {
        MergeTone::Ready => (IconName::Check, tokens::open()),
        MergeTone::Warning => (IconName::CircleAlert, tokens::pending()),
        MergeTone::Blocked => (IconName::CircleX, tokens::closed()),
        MergeTone::Neutral => (IconName::GitPullRequestDraft, cx.theme().muted_foreground),
        MergeTone::Merged => (IconName::GitMerge, tokens::merged()),
        MergeTone::Closed => (IconName::GitPullRequestClosed, tokens::closed()),
    }
}

/// One colored run of a progress bar: its share of the whole and color.
type Run = (f32, Hsla);

fn share(part: usize, total: usize) -> f32 {
    if total == 0 {
        0.
    } else {
        part as f32 / total as f32
    }
}

/// A labeled progress cell: an icon and the title, a count on the right,
/// then a 4 px bar of one width in every cell. With `total` at zero there
/// is nothing to make progress on: the icon is a green check, the count
/// says so in words, and the bar is its neutral track.
/// A cell with `open` set is the button that opens and closes the list it
/// summarizes, with a chevron after its count.
#[allow(clippy::too_many_arguments)]
fn bar_cell(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    count: String,
    done: f32,
    total: usize,
    runs: Vec<Run>,
    open: Option<bool>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, foreground) = (theme.muted_foreground, theme.foreground);
    let settled = total == 0;
    let (icon, icon_color) = if settled {
        (IconName::CircleCheck, tokens::open())
    } else {
        (icon, muted)
    };
    let hover = theme.list_hover;
    div()
        .id(id)
        .aria_label(format!("{title}: {count}"))
        .map(|cell| match open {
            Some(open) => cell
                .role(Role::Button)
                .aria_expanded(open)
                .hover(move |cell| cell.bg(hover)),
            None => cell
                .role(Role::ProgressIndicator)
                .aria_min_numeric_value(0.)
                .aria_max_numeric_value(100.)
                .aria_numeric_value(f64::from(done * 100.).round()),
        })
        .flex_1()
        .min_w_0()
        .p_4()
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_size(scale::meta())
                        .child(Icon::new(icon).with_size(dp(14.)).text_color(icon_color))
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(foreground)
                                .child(title),
                        )
                        .child(div().flex_1())
                        .child(div().truncate().text_color(muted).child(count))
                        .children(open.map(|open| {
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .with_size(dp(14.))
                            .text_color(muted)
                        })),
                )
                .child(
                    h_flex()
                        .w_full()
                        .h(dp(4.))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(tokens::border())
                        .children(
                            runs.into_iter()
                                .filter(|(part, _)| *part > 0.)
                                .map(|(part, color)| div().h_full().w(relative(part)).bg(color)),
                        ),
                ),
        )
}

fn progress_count(progress: Progress, verb: &str, empty: &str) -> String {
    if progress.total == 0 {
        empty.to_owned()
    } else {
        format!("{} of {} {verb}", progress.done, progress.total)
    }
}

impl PullRequestView {
    pub(super) fn render_merge_box(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = merge_status(detail);
        let (icon, color) = tone_look(status.tone, cx);
        let busy = self.write.is_busy();
        let merge = match available_actions(detail).merge {
            MergeAction::Available { methods, .. } => {
                Some(self.render_merge_controls(&methods, busy, cx))
            }
            MergeAction::Hidden | MergeAction::Unavailable(_) => None,
        };
        let confirm =
            matches!(self.panel, Panel::ConfirmMerge(_)).then(|| self.render_panel(detail, cx));

        let checks = check_tally(detail);
        let threads = resolved_threads(detail);
        let approved = approvals(detail);
        let checks_count = if checks.total == 0 {
            "No checks".to_owned()
        } else {
            format!("{} of {} passed", checks.passed, checks.total)
        };
        let has_runs = !detail.checks.runs.is_empty();
        let checks_open = self
            .checks_open
            .unwrap_or_else(|| checks_open_at_first(detail));
        let theme = cx.theme();
        let (muted, foreground, border) = (theme.muted_foreground, theme.foreground, theme.border);
        let divider = || div().w_px().bg(border);

        div()
            .id("merge-box")
            .role(Role::Group)
            .aria_label(format!("{}. {}", status.title, status.detail))
            .mx(scale::content_inset())
            .mt_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(border)
            .bg(theme.group_box)
            .overflow_hidden()
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size(dp(32.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(color.opacity(0.16))
                            .child(Icon::new(icon).small().text_color(color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(scale::body())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(foreground)
                                    .child(status.title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(scale::meta())
                                    .text_color(muted)
                                    .child(status.detail),
                            ),
                    )
                    .children(merge),
            )
            .children(confirm.map(|confirm| {
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .child(confirm)
            }))
            .child(
                h_flex()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        bar_cell(
                            "merge-box-checks",
                            IconName::CircleCheck,
                            "Checks",
                            checks_count,
                            share(checks.passed, checks.total),
                            checks.total.max(1),
                            vec![
                                (share(checks.passed, checks.total), tokens::open()),
                                (share(checks.pending, checks.total), tokens::pending()),
                                (share(checks.failed, checks.total), tokens::closed()),
                            ],
                            has_runs.then_some(checks_open),
                            cx,
                        )
                        .when(has_runs, |cell| {
                            cell.on_click(cx.listener(move |this, _, _, cx| {
                                this.checks_open = Some(!checks_open);
                                cx.notify();
                            }))
                        }),
                    )
                    .child(divider())
                    .child(bar_cell(
                        "merge-box-comments",
                        IconName::MessageSquare,
                        "Comments",
                        progress_count(threads, "resolved", "None unresolved"),
                        share(threads.done, threads.total),
                        threads.total,
                        vec![(share(threads.done, threads.total), tokens::open())],
                        None,
                        cx,
                    ))
                    .child(divider())
                    .child(bar_cell(
                        "merge-box-approvals",
                        IconName::UserCheck,
                        "Approvals",
                        progress_count(approved, "approved", "No reviewers"),
                        share(approved.done, approved.total),
                        approved.total.max(1),
                        vec![(share(approved.done, approved.total), tokens::open())],
                        None,
                        cx,
                    )),
            )
            .when(has_runs && checks_open, |merge_box| {
                merge_box.child(self.render_check_list(detail, cx))
            })
            .into_any_element()
    }

    /// The checks under the merge box, in sections by outcome as on GitHub:
    /// each section's heading folds it, and each row says what the check
    /// did and opens it in Actions or in the browser.
    fn render_check_list(&self, detail: &PullRequestDetail, cx: &mut Context<Self>) -> Div {
        let runs = &detail.checks.runs;
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let mut list = v_flex()
            .id("merge-box-check-list")
            .role(Role::List)
            .aria_label("Checks")
            .max_h(dp(360.))
            .overflow_y_scroll()
            .px_2()
            .py_2();
        for (group, members) in check_groups(runs) {
            let folded = self.folded_check_groups.contains(&group);
            let heading = group.heading(members.len());
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("check-group-{}", group.key())))
                    .role(Role::Button)
                    .aria_label(heading.clone())
                    .aria_expanded(!folded)
                    .h_6()
                    .px_2()
                    .gap_1()
                    .items_center()
                    .rounded(theme.radius)
                    .text_size(scale::meta())
                    .text_color(muted)
                    .hover(move |row| row.bg(hover))
                    .child(heading)
                    .child(
                        Icon::new(if folded {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .with_size(dp(12.))
                        .text_color(muted),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.folded_check_groups.remove(&group) {
                            this.folded_check_groups.insert(group);
                        }
                        cx.notify();
                    })),
            );
            if !folded {
                list = list.children(
                    members
                        .into_iter()
                        .map(|ix| self.render_check_row(ix, &runs[ix], cx)),
                );
            }
        }
        div().border_t_1().border_color(theme.border).child(list)
    }

    fn render_check_row(&self, ix: usize, run: &CheckRun, cx: &App) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, foreground, hover) =
            (theme.muted_foreground, theme.foreground, theme.list_hover);
        let (icon, color) = status_style(run.status);
        let name = match &run.workflow {
            Some(workflow) => format!("{workflow} / {}", run.name),
            None => run.name.clone(),
        };
        let outcome = check_outcome(run);
        let actions = run.actions.map(|job| Route::WorkflowRun {
            repo: self.pr.repo.clone(),
            run_id: job.run_id,
            job_id: Some(job.job_id),
            title: run.workflow.clone().unwrap_or_else(|| run.name.clone()),
        });
        h_flex()
            .id(("merge-box-check", ix))
            .role(Role::ListItem)
            .aria_label(format!("{name}: {outcome}"))
            .h(scale::row_height())
            .px_2()
            .gap_2()
            .items_center()
            .rounded(theme.radius)
            .hover(move |row| row.bg(hover))
            .child(Icon::new(icon).small().text_color(color))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .text_size(scale::body())
                    .child(
                        div()
                            .flex_shrink(1.)
                            .min_w_0()
                            .truncate()
                            .text_color(foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child(outcome),
                    ),
            )
            .when(run.is_required, |row| {
                row.child(ui::chip().small().child("Required"))
            })
            .child(
                h_flex()
                    .flex_none()
                    .w_6()
                    .justify_center()
                    .children(actions.map(|route| {
                        ui::icon_button(
                            ("merge-box-check-run", ix),
                            IconName::Workflow,
                            "Open in Actions",
                        )
                        .xsmall()
                        .accessibility_label(format!("Open {} in Actions", run.name))
                        .on_click(move |event, window, cx| {
                            crate::shell::open(route.clone(), event, window, cx)
                        })
                    })),
            )
            .child(h_flex().flex_none().w_6().justify_center().children(
                run.details_url.clone().map(|url| {
                    ui::icon_button(
                        ("merge-box-check-details", ix),
                        IconName::ExternalLink,
                        "Open details in browser",
                    )
                    .xsmall()
                    .accessibility_label(format!("Open the details of {} in the browser", run.name))
                    .on_click(move |_, _, cx| cx.open_url(&url))
                }),
            ))
    }
}
