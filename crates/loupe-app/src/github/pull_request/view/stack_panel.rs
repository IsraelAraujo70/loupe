//! Graphite-style stack overview panel on a pull request's Conversation tab:
//! lists every open PR in the stack from top down to the base branch, with
//! number, title, position, approval, and checks, highlighting the PR on screen.

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{PrRef, ReviewDecision, StackDisplayEntry, StackGraph};

use crate::github::widgets;
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;
use crate::ui;

fn approval_chip(decision: ReviewDecision) -> Tag {
    match decision {
        ReviewDecision::Approved => ui::tinted_chip(tokens::open()).child("Approved"),
        ReviewDecision::ChangesRequested => {
            ui::tinted_chip(tokens::closed()).child("Changes requested")
        }
        ReviewDecision::ReviewRequired => ui::chip().child("Review required"),
    }
}

/// Renders the vertical Graphite-style stack rail dot and connecting segments
/// for row `ix` out of `total` (above the base branch footer).
fn rail_node(ix: usize, is_current: bool, cx: &App) -> Div {
    let line = cx.theme().border;
    let dot_color = if is_current {
        tokens::accent()
    } else {
        cx.theme().muted_foreground
    };
    div()
        .flex_none()
        .w(dp(16.))
        .h_full()
        .relative()
        .when(ix > 0, |rail| {
            rail.child(
                div()
                    .absolute()
                    .top_0()
                    .h_1_2()
                    .left(dp(7.5))
                    .w(px(1.))
                    .bg(line),
            )
        })
        // Bottom segment connects down to the next PR or the base branch row.
        .child(
            div()
                .absolute()
                .top_1_2()
                .bottom_0()
                .left(dp(7.5))
                .w(px(1.))
                .bg(line),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(if is_current { dp(8.) } else { dp(6.) })
                        .rounded_full()
                        .bg(dot_color),
                ),
        )
}

fn render_entry_row(
    ix: usize,
    entry: &StackDisplayEntry<'_>,
    current: &PrRef,
    is_tree: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, foreground, hover, active) = (
        theme.muted_foreground,
        theme.foreground,
        theme.list_hover,
        theme.list_active,
    );
    let node = entry.node;
    let is_current = node.pr == *current;
    let checks = node.checks.map_or("No checks", widgets::checks_label);
    let position = format!("{}/{}", entry.position, entry.total);
    let approval_label = match node.review_decision {
        Some(ReviewDecision::Approved) => ", approved",
        Some(ReviewDecision::ChangesRequested) => ", changes requested",
        Some(ReviewDecision::ReviewRequired) => ", review required",
        None => "",
    };
    let aria = format!(
        "#{} {}, {position}, {}{approval_label}{}",
        node.pr.number,
        node.title,
        checks.to_lowercase(),
        if is_current {
            ", current pull request"
        } else {
            ""
        }
    );
    let route = Route::PullRequest(node.pr.clone());
    let indent = if is_tree && entry.depth > 0 {
        Some(dp(entry.depth as f32 * 16.))
    } else {
        None
    };

    h_flex()
        .id(("pr-stack-panel-row", node.pr.number))
        .debug_selector({
            let num = node.pr.number;
            move || {
                if is_current {
                    format!("pr-stack-row-{num}:current")
                } else {
                    format!("pr-stack-row-{num}")
                }
            }
        })
        .role(if is_current {
            Role::ListItem
        } else {
            Role::Link
        })
        .aria_label(aria)
        .h(scale::row_height())
        .px_3()
        .gap_2()
        .items_center()
        .when(is_current, |row| row.bg(active))
        .when(!is_current, |row| {
            row.hover(move |row| row.bg(hover)).opens(route)
        })
        .child(rail_node(ix, is_current, cx))
        .when_some(indent, |row, width| row.child(div().flex_none().w(width)))
        .child(widgets::state_icon(node.state, node.is_draft))
        .child(
            h_flex()
                .flex_none()
                .w_10()
                .justify_end()
                .text_size(scale::meta())
                .text_color(if is_current { foreground } else { muted })
                .child(format!("#{}", node.pr.number)),
        )
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
                        .text_color(foreground)
                        .child(node.title.clone()),
                )
                .when(is_current, |title| title.child(ui::chip().child("Current")))
                .child(
                    div()
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(position),
                ),
        )
        .children(node.review_decision.map(approval_chip))
        .child(
            h_flex()
                .flex_none()
                .w_32()
                .gap_1()
                .items_center()
                .text_size(scale::meta())
                .map(|lane| match node.checks {
                    Some(state) => lane.child(widgets::checks_icon(state)).child(
                        div()
                            .text_color(if state == loupe_github::ChecksState::Failing {
                                widgets::checks_color(state)
                            } else {
                                muted
                            })
                            .child(widgets::checks_label(state)),
                    ),
                    None => lane.text_color(muted).child("No checks"),
                }),
        )
}

/// Renders the Graphite-style Stack card for `graph`, highlighting `current`.
pub(super) fn render_stack_panel(graph: &StackGraph, current: &PrRef, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, foreground, border) = (theme.muted_foreground, theme.foreground, theme.border);
    let entries = graph.display_entries();
    let is_tree = graph.shape == loupe_github::StackShape::Tree;

    div()
        .id("pr-stack-panel")
        .debug_selector(|| "pr-stack-panel".to_owned())
        .role(Role::Group)
        .aria_label(format!(
            "Stack of {} pull requests into {}",
            graph.len(),
            graph.base_ref
        ))
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
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(border)
                .child(Icon::new(IconName::Layers).small().text_color(muted))
                .child(
                    div()
                        .text_size(scale::body())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child("Stack"),
                )
                .child(ui::count_badge(graph.len(), cx))
                .child(ui::chip().child(graph.shape.label()))
                .child(div().flex_1())
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child("into")
                        .child(ui::branch_chip(graph.base_ref.clone(), cx)),
                ),
        )
        .child(
            v_flex()
                .id("pr-stack-panel-list")
                .role(Role::List)
                .aria_label("Pull requests in this stack")
                .children(
                    entries
                        .iter()
                        .enumerate()
                        .map(|(ix, entry)| render_entry_row(ix, entry, current, is_tree, cx)),
                ),
        )
        .child(
            h_flex()
                .id("pr-stack-panel-base")
                .h_7()
                .px_3()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(tokens::border_subtle())
                .child(
                    div()
                        .flex_none()
                        .w(dp(16.))
                        .h_full()
                        .relative()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .h_1_2()
                                .left(dp(7.5))
                                .w(px(1.))
                                .bg(border),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::GitBranch)
                                        .with_size(scale::icon_xs())
                                        .text_color(muted),
                                ),
                        ),
                )
                .child(ui::branch_chip(graph.base_ref.clone(), cx))
                .child(
                    div()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child("base branch"),
                ),
        )
        .into_any_element()
}
