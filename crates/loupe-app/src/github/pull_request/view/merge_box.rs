//! The merge box at the top of the conversation, after GitHub's Overview:
//! what stands between the pull request and a merge, the merge button,
//! progress bars for checks, resolved conversations, and approvals, and
//! under the checks' bar the list of checks it opens.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{CheckRun, PullRequestDetail};

use super::model::{
    DraftAction, MergeAction, MergeTone, Panel, Progress, approvals, available_actions,
    check_groups, check_outcome, check_tally, checks_open_at_first, merge_status, resolved_threads,
};
use super::{PullRequestView, Write};
use crate::github::common::Submit;
use crate::github::pull_request::checks::{check_route, status_style};
use crate::route::Route;
use crate::shell::OpenOnClick as _;
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
        .px_3()
        .py_2p5()
        .child(
            v_flex()
                .gap_1p5()
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
                        .bg(if settled {
                            tokens::border_subtle()
                        } else {
                            tokens::border()
                        })
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
        let actions = available_actions(detail);
        let merge = match actions.merge {
            MergeAction::Available { methods, .. } => {
                Some(self.render_merge_controls(&methods, busy, cx))
            }
            MergeAction::Hidden | MergeAction::Unavailable(_) => None,
        };
        // A draft the viewer may change: the block's action takes it out of
        // draft, and says here when GitHub refuses.
        let marks_ready = actions.draft == Some(DraftAction::MarkReady);
        let ready = marks_ready.then(|| {
            div()
                .flex_none()
                .debug_selector(|| "pr-ready-for-review".to_owned())
                .child(
                    Button::new("pr-ready-for-review")
                        .outline()
                        .icon(IconName::GitPullRequest)
                        .label("Ready for review")
                        .tooltip("Take this pull request out of draft")
                        .loading(self.ready.is_busy())
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.start_write(Write::MarkReady, cx)),
                        ),
                )
        });
        let ready_error = self
            .ready
            .error()
            .filter(|_| marks_ready)
            .map(str::to_owned);
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
            .mt_3()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(border)
            .bg(theme.group_box)
            .shadow_xs()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2p5()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size(dp(26.))
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
                    .children(merge)
                    .children(ready),
            )
            .children(ready_error.map(|message| {
                h_flex()
                    .debug_selector(|| "pr-ready-error".to_owned())
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(border)
                    .child(Alert::error("pr-ready-error", message).flex_1())
                    .child(
                        ui::icon_button("pr-ready-error-dismiss", IconName::Close, "Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ready = Submit::Idle;
                                cx.notify();
                            })),
                    )
            }))
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
                        .map(|ix| self.render_check_row(ix, &runs[ix], &detail.head_ref, cx)),
                );
            }
        }
        let workflow_runs = Route::Actions {
            repo: self.pr.repo.clone(),
            branch: Some(detail.head_ref.clone()),
        };
        div()
            .border_t_1()
            .border_color(theme.border)
            .child(list)
            .child(
                h_flex().px_2().pb_2().child(
                    Button::new("merge-box-workflow-runs")
                        .ghost()
                        .small()
                        .icon(IconName::Workflow)
                        .label("View workflow runs")
                        .tooltip("The workflow runs of this pull request's branch")
                        .on_click(move |event, window, cx| {
                            crate::shell::open(workflow_runs.clone(), event, window, cx)
                        }),
                ),
            )
    }

    fn render_check_row(
        &self,
        ix: usize,
        run: &CheckRun,
        head_ref: &str,
        cx: &App,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, foreground, hover) =
            (theme.muted_foreground, theme.foreground, theme.list_hover);
        let (icon, color) = status_style(run.status);
        let name = match &run.workflow {
            Some(workflow) => format!("{workflow} / {}", run.name),
            None => run.name.clone(),
        };
        let outcome = check_outcome(run);
        let actions = check_route(&self.pr.repo, head_ref, run);
        h_flex()
            .id(("merge-box-check", ix))
            .role(Role::ListItem)
            .aria_label(format!("{name}: {outcome}"))
            .when_some(actions.clone(), |row, route| {
                row.role(Role::Button)
                    .aria_label(format!("{name}: {outcome}, open its run"))
                    .opens(route)
            })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
        px, size,
    };
    use loupe_github::{PrState, PullRequestDetail};

    use super::super::{PullRequestView, model};
    use crate::github::common::{Loadable, Submit};
    use crate::services::{Accounts, Services};
    use crate::store::Settings;

    /// The page of `detail` on its Conversation tab, with `ready` as the
    /// state of Ready for review; GitHub is not connected.
    fn open(
        detail: PullRequestDetail,
        ready: Submit,
        cx: &mut TestAppContext,
    ) -> VisualTestContext {
        let detail = Arc::new(detail);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| PullRequestView::new(detail.pr.clone(), window, cx)),
            )
            .expect("open a window")
        });
        let mut cx = VisualTestContext::from_window(handle, cx);
        cx.run_until_parked();
        view.update(&mut cx, |view, cx| {
            view.incoming = Some(detail.clone());
            view.detail = Loadable::Loaded(detail);
            view.ready = ready;
            cx.notify();
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx
    }

    fn draft() -> PullRequestDetail {
        PullRequestDetail {
            is_draft: true,
            ..model::tests::detail()
        }
    }

    fn has_ready_button(detail: PullRequestDetail, cx: &mut TestAppContext) -> bool {
        open(detail, Submit::Idle, cx)
            .debug_bounds("pr-ready-for-review")
            .is_some()
    }

    #[gpui_kit::test]
    fn a_draft_you_can_change_offers_ready_for_review(cx: &mut TestAppContext) {
        assert!(has_ready_button(draft(), cx));
    }

    #[gpui_kit::test]
    fn a_draft_you_cannot_change_has_no_ready_for_review(cx: &mut TestAppContext) {
        let detail = PullRequestDetail {
            viewer_can_update: false,
            ..draft()
        };
        assert!(!has_ready_button(detail, cx));
    }

    #[gpui_kit::test]
    fn a_pull_request_out_of_draft_has_no_ready_for_review(cx: &mut TestAppContext) {
        assert!(!has_ready_button(model::tests::detail(), cx));
    }

    #[gpui_kit::test]
    fn a_closed_draft_has_no_ready_for_review(cx: &mut TestAppContext) {
        let detail = PullRequestDetail {
            state: PrState::Closed,
            ..draft()
        };
        assert!(!has_ready_button(detail, cx));
    }

    #[gpui_kit::test]
    fn githubs_refusal_shows_in_the_draft_block(cx: &mut TestAppContext) {
        let refused = Submit::Failed("Resource not accessible by integration".into());
        let mut cx = open(draft(), refused, cx);
        assert!(cx.debug_bounds("pr-ready-error").is_some());
        assert!(
            cx.debug_bounds("pr-ready-for-review").is_some(),
            "the button stays to try again"
        );
    }
}
