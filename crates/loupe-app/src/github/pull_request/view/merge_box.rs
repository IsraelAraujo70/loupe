//! The merge box at the top of the conversation, after GitHub's Overview:
//! what stands between the pull request and a merge, the merge button, and
//! progress bars for checks, resolved conversations, and approvals.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_github::PullRequestDetail;

use super::PullRequestView;
use super::model::{
    MergeAction, MergeTone, Panel, Progress, approvals, available_actions, check_tally,
    merge_status, resolved_threads,
};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};

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
#[allow(clippy::too_many_arguments)]
fn bar_cell(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    count: String,
    done: f32,
    total: usize,
    runs: Vec<Run>,
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
    div()
        .id(id)
        .role(Role::ProgressIndicator)
        .aria_label(format!("{title}: {count}"))
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(100.)
        .aria_numeric_value(f64::from(done * 100.).round())
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
                        .child(div().truncate().text_color(muted).child(count)),
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
                    .child(bar_cell(
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
                        cx,
                    ))
                    .child(divider())
                    .child(bar_cell(
                        "merge-box-comments",
                        IconName::MessageSquare,
                        "Comments",
                        progress_count(threads, "resolved", "None unresolved"),
                        share(threads.done, threads.total),
                        threads.total,
                        vec![(share(threads.done, threads.total), tokens::open())],
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
                        cx,
                    )),
            )
            .into_any_element()
    }
}
