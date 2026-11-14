//! Long and noisy comments, as on GitHub: a comment folds to its header, a
//! tall body shows cut with "Show more", `<details>` blocks start closed, and
//! long code blocks scroll on their own.

use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_github::NodeId;

use super::ConversationTab;
use super::segments::{self, Segment};
use crate::github::widgets;
use crate::theme::{scale, tokens};
use crate::ui;

/// Bodies taller than this show cut, with "Show more".
const MAX_BODY_HEIGHT: Pixels = px(600.);
/// The fade over the bottom of a cut body.
const FADE_HEIGHT: Pixels = px(72.);

/// What the reader folded or unfolded in this conversation. Anything not
/// here follows its default.
#[derive(Default)]
pub(super) struct Folds {
    /// Comments and review bodies folded (`true`) or unfolded by hand.
    comments: HashMap<NodeId, bool>,
    /// Tall bodies shown in full.
    uncapped: HashSet<NodeId>,
    /// Each body's full height in the last layout.
    heights: HashMap<NodeId, Pixels>,
    /// `<details>` blocks toggled away from how they start, by
    /// `{node}/{index path}`.
    details: HashSet<String>,
}

impl Folds {
    /// Whether the comment `id` shows only its header. Bots' comments and
    /// comments hidden on GitHub start folded.
    pub(super) fn is_folded(&self, id: &NodeId, by_default: bool) -> bool {
        self.comments.get(id).copied().unwrap_or(by_default)
    }
}

/// One line that stands for a folded comment in its header.
pub(super) fn preview_line(body: &str, cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(scale::meta())
        .text_color(cx.theme().muted_foreground)
        .child(segments::preview(body))
}

/// The tag for a comment hidden on GitHub, such as "Outdated".
pub(super) fn hidden_tag(reason: Option<&str>) -> impl IntoElement {
    let label = match reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        Some(reason) => {
            let reason = reason.replace('_', " ").to_lowercase();
            let mut chars = reason.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
        None => "Hidden".into(),
    };
    widgets::tinted_tag(tokens::pending()).xsmall().child(label)
}

impl ConversationTab {
    /// The header button that folds the comment `id` to its header or
    /// unfolds it.
    pub(super) fn fold_button(&self, id: &NodeId, folded: bool, cx: &mut Context<Self>) -> Button {
        let key = id.clone();
        ui::icon_button(
            SharedString::from(format!("fold-{id}")),
            if folded {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            },
            if folded {
                "Expand comment"
            } else {
                "Collapse comment"
            },
        )
        .xsmall()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.folds.comments.insert(key.clone(), !folded);
            cx.notify();
        }))
    }

    /// A Markdown body cut at [`MAX_BODY_HEIGHT`] with "Show more" when it
    /// is taller. Its full height is measured after layout, so the button
    /// shows from the next frame on.
    pub(super) fn capped_body(
        &self,
        id: &NodeId,
        body: &str,
        fade_to: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let uncapped = self.folds.uncapped.contains(id);
        let tall = self
            .folds
            .heights
            .get(id)
            .is_some_and(|height| *height > MAX_BODY_HEIGHT);
        let content = self.markdown(id, &segments::split(body), "", cx);
        let view = cx.entity().downgrade();
        let measured = id.clone();
        let mut frame = v_flex()
            .relative()
            .w_full()
            .on_children_prepainted(move |bounds, _, cx| {
                let Some(height) = bounds.first().map(|bounds| bounds.size.height) else {
                    return;
                };
                let view = view.clone();
                let key = measured.clone();
                cx.defer(move |cx| {
                    view.update(cx, |this, cx| {
                        if this.folds.heights.get(&key) != Some(&height) {
                            this.folds.heights.insert(key, height);
                            cx.notify();
                        }
                    })
                    .ok();
                });
            })
            .child(div().flex_none().w_full().child(content));
        if !uncapped {
            frame = frame.max_h(MAX_BODY_HEIGHT).overflow_hidden();
        }
        if tall && !uncapped {
            frame = frame.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(FADE_HEIGHT)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(fade_to.opacity(0.), 0.),
                        linear_color_stop(fade_to, 1.),
                    )),
            );
        }
        let mut column = v_flex().w_full().gap_2().child(frame);
        if tall {
            let key = id.clone();
            column = column.child(
                h_flex().justify_center().child(
                    Button::new(SharedString::from(format!("show-more-{id}")))
                        .ghost()
                        .xsmall()
                        .label(if uncapped { "Show less" } else { "Show more" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.folds.uncapped.remove(&key) {
                                this.folds.uncapped.insert(key.clone());
                            }
                            cx.notify();
                        })),
                ),
            );
        }
        column.into_any_element()
    }

    /// `segments` as Markdown text views and `<details>` folds. `path` names
    /// the nesting, so each text view and fold keeps its identity.
    fn markdown(
        &self,
        id: &NodeId,
        segments: &[Segment],
        path: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut column = v_flex().w_full().gap_2();
        for (ix, segment) in segments.iter().enumerate() {
            let path = format!("{path}{ix}");
            column = column.child(match segment {
                Segment::Markdown(text) => {
                    crate::ui::markdown(SharedString::from(format!("md-{id}-{path}")), text.clone())
                        .into_any_element()
                }
                Segment::Details {
                    summary,
                    open,
                    body,
                } => self.details(id, &path, summary, *open, body, cx),
            });
        }
        column
    }

    /// A `<details>` block: its summary as a toggle, and its content when
    /// open.
    fn details(
        &self,
        id: &NodeId,
        path: &str,
        summary: &str,
        open_at_first: bool,
        body: &[Segment],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("{id}/{path}");
        let open = open_at_first != self.folds.details.contains(&key);
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let toggle = key.clone();
        let summary_row = h_flex()
            .id(SharedString::from(format!("details-{key}")))
            .role(Role::Button)
            .aria_label(SharedString::from(summary.to_owned()))
            .aria_expanded(open)
            .gap_1p5()
            .items_center()
            .text_sm()
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(muted),
            )
            .child(summary.to_owned())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.folds.details.remove(&toggle) {
                    this.folds.details.insert(toggle.clone());
                }
                cx.notify();
            }));
        let mut block = v_flex().w_full().gap_2().child(summary_row);
        if open {
            block = block.child(
                div()
                    .w_full()
                    .pl_4()
                    .border_l_2()
                    .border_color(border)
                    .child(self.markdown(id, body, &format!("{path}."), cx)),
            );
        }
        block.into_any_element()
    }
}
