//! The side column: reviewers, labels, assignees, and the Linear and T3
//! panels.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{Label, PullRequestDetail, Reviewer, ReviewerKind, ReviewerState};

use super::model::{approvals, comment_count};
use super::{Loaded, PullRequestView};
use crate::github::common::Loadable;
use crate::github::pr_row::hint;
use crate::github::widgets;
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

fn reviewer_name(reviewer: &Reviewer) -> &str {
    match &reviewer.kind {
        ReviewerKind::User(actor) => &actor.login,
        ReviewerKind::Team { name, .. } => name,
    }
}

fn reviewer_style(state: ReviewerState) -> (IconName, Hsla, &'static str) {
    match state {
        ReviewerState::Requested => (
            IconName::CircleDashed,
            tokens::pending(),
            "Review requested",
        ),
        ReviewerState::Commented => (IconName::MessageSquare, tokens::text_muted(), "Commented"),
        ReviewerState::Approved => (IconName::CircleCheck, tokens::open(), "Approved"),
        ReviewerState::ChangesRequested => {
            (IconName::CircleX, tokens::closed(), "Changes requested")
        }
        ReviewerState::Dismissed => (IconName::Ban, tokens::text_faint(), "Review dismissed"),
    }
}

fn section(title: &'static str, body: impl IntoElement, cx: &App) -> Div {
    section_with(title, None::<Div>, body, cx)
}

/// A section whose heading carries its action at the trailing edge, such as
/// the gear that edits the labels.
fn section_with(
    title: &'static str,
    action: Option<impl IntoElement>,
    body: impl IntoElement,
    cx: &App,
) -> Div {
    // The heading carries the section's action (where there is one) at
    // its trailing edge; `separated` draws the hairline between sections.
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .h_6()
                .justify_between()
                .items_center()
                .child(ui::section_label(title, cx))
                .children(action),
        )
        .child(body)
}

/// The side column's sections, 24 px apart with a hairline halfway: the
/// first starts at the column's top, level with the merge box.
fn separated(sections: Vec<AnyElement>) -> impl Iterator<Item = Div> {
    sections.into_iter().enumerate().map(|(ix, section)| {
        let slot = div().w_full();
        if ix == 0 {
            slot.child(section)
        } else {
            slot.mt_3()
                .pt_3()
                .border_t_1()
                .border_color(crate::theme::tokens::border_subtle())
                .child(section)
        }
    })
}

/// What an empty section says: "None", in the muted color.
fn none(cx: &App) -> Div {
    div()
        .text_size(scale::body())
        .text_color(cx.theme().muted_foreground)
        .child("None")
}

/// One Activity counter: icon, name, and the count on the right.
fn activity_row(
    id: &'static str,
    icon: IconName,
    name: &'static str,
    count: usize,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .role(Role::ListItem)
        .aria_label(format!("{name}: {count}"))
        .gap_2()
        .items_center()
        .text_size(scale::body())
        .child(Icon::new(icon).small().text_color(theme.muted_foreground))
        .child(div().flex_1().child(name))
        .child(
            div()
                .px_1p5()
                .rounded_full()
                .bg(theme.secondary)
                .text_size(scale::meta())
                .text_color(theme.muted_foreground)
                .child(count.to_string()),
        )
}

/// A person row: 20 px avatar with initials fallback, then the login.
fn person(avatar_url: &str, login: &str) -> Div {
    h_flex()
        .gap_2()
        .min_w_0()
        .items_center()
        .text_size(scale::body())
        .child(ui::avatar_named(
            avatar_url.to_owned(),
            login.to_owned(),
            dp(20.),
        ))
        .child(div().min_w_0().truncate().child(login.to_owned()))
}

impl PullRequestView {
    /// The gear in the Labels heading that opens and closes the editor, for
    /// viewers who can update.
    fn render_label_toggle(&self, cx: &mut Context<Self>) -> Button {
        let open = self.labels_picker.is_some();
        ui::icon_button(
            "pr-edit-labels",
            IconName::Settings,
            if open { "Done" } else { "Edit labels" },
        )
        .selected(open)
        .on_click(cx.listener(|this, _, _, cx| this.toggle_labels_picker(cx)))
    }

    /// The repository's labels with a mark on the applied ones; a click adds
    /// or removes one. Shown while the editor is open.
    fn render_label_editor(
        &self,
        detail: &PullRequestDetail,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        match &self.labels_picker {
            None => None,
            Some(Loadable::Loading | Loadable::Refreshing(_)) => {
                Some(ui::skeleton_rows(2).into_any_element())
            }
            Some(Loadable::Failed(error)) => Some(
                div()
                    .text_size(scale::meta())
                    .text_color(cx.theme().danger)
                    .child(error.message())
                    .into_any_element(),
            ),
            Some(Loadable::Loaded(labels)) if labels.is_empty() => Some(
                div()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child("This repository has no labels.")
                    .into_any_element(),
            ),
            Some(Loadable::Loaded(labels)) => Some(
                v_flex()
                    .gap_0p5()
                    .children(labels.iter().map(|label: &Label| {
                        let applied = detail.labels.iter().any(|shown| shown.id == label.id);
                        let clicked = label.clone();
                        h_flex()
                            .id(SharedString::from(format!("pr-label-{}", label.id)))
                            .role(Role::MenuItem)
                            .aria_label(if applied {
                                format!("Remove label {}", label.name)
                            } else {
                                format!("Add label {}", label.name)
                            })
                            .gap_2()
                            .px_1()
                            .py_0p5()
                            .items_center()
                            .rounded(cx.theme().radius)
                            .hover(move |row| row.bg(hover))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_label(&clicked, cx)),
                            )
                            .child(div().size_4().flex_none().when(applied, |slot| {
                                slot.child(Icon::new(IconName::Check).small())
                            }))
                            .child(widgets::label_tag(label))
                    }))
                    .into_any_element(),
            ),
        }
    }

    pub(super) fn render_side(
        &self,
        detail: &PullRequestDetail,
        loaded: &Loaded,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let activity = v_flex()
            .gap_2()
            .child(activity_row(
                "pr-activity-approvals",
                IconName::CircleCheck,
                "Approvals",
                approvals(detail).done,
                cx,
            ))
            .child(activity_row(
                "pr-activity-comments",
                IconName::MessageSquare,
                "Comments",
                comment_count(detail),
                cx,
            ));
        let reviewers = if detail.reviewers.is_empty() {
            ui::empty_line(IconName::UserRound, "No reviewers", cx).into_any_element()
        } else {
            v_flex()
                .gap_2()
                .children(detail.reviewers.iter().map(|reviewer| {
                    let (icon, color, state) = reviewer_style(reviewer.state);
                    let name = reviewer_name(reviewer);
                    let who =
                        match &reviewer.kind {
                            ReviewerKind::User(actor) => person(&actor.avatar_url, &actor.login),
                            ReviewerKind::Team { name, .. } => {
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .items_center()
                                    .text_size(scale::body())
                                    .child(h_flex().flex_none().size_5().justify_center().child(
                                        Icon::new(IconName::Users).small().text_color(muted),
                                    ))
                                    .child(div().min_w_0().truncate().child(name.clone()))
                            }
                        };
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(who.flex_1())
                        .child(hint(
                            SharedString::from(format!("reviewer-{name}")),
                            state,
                            Icon::new(icon).small().text_color(color),
                        ))
                }))
                .into_any_element()
        };
        let applied = if detail.labels.is_empty() {
            none(cx).into_any_element()
        } else {
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(detail.labels.iter().map(widgets::label_tag))
                .into_any_element()
        };
        let (label_toggle, editor) = if detail.viewer_can_update {
            (
                Some(self.render_label_toggle(cx)),
                self.render_label_editor(detail, cx),
            )
        } else {
            (None, None)
        };
        let labels = v_flex().gap_2().child(applied).children(editor);
        let assignees = if detail.assignees.is_empty() {
            none(cx).into_any_element()
        } else {
            v_flex()
                .gap_2()
                .children(
                    detail
                        .assignees
                        .iter()
                        .map(|actor| person(&actor.avatar_url, &actor.login)),
                )
                .into_any_element()
        };
        div()
            .id("pr-side")
            .w(scale::side_width())
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.side_scroll)
            .border_l_1()
            .border_color(crate::theme::tokens::border_subtle())
            .child(v_flex().p_4().children(separated(vec![
                section("Activity", activity, cx).into_any_element(),
                section("Reviewers", reviewers, cx).into_any_element(),
                section_with("Labels", label_toggle, labels, cx).into_any_element(),
                section("Assignees", assignees, cx).into_any_element(),
                loaded.linear.clone().into_any_element(),
                loaded.t3.clone().into_any_element(),
            ])))
            .into_any_element()
    }
}
