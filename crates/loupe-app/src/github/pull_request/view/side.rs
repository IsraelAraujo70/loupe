//! The details column: the linked Linear issues first, then reviewers,
//! labels, and assignees as one block of properties, the T3 panel, and one
//! line for what is not connected. The approval and comment counts live in
//! the merge box beside it.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{Label, PullRequestDetail, Reviewer, ReviewerKind, ReviewerState};

use super::{Loaded, PullRequestView};
use crate::github::common::Loadable;
use crate::github::pr_row::hint;
use crate::github::widgets;
use crate::services::Services;
use crate::settings::catalog::Section;
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
            scale::avatar_md(),
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

    /// The details column: the linked Linear issues, the reviewers, labels,
    /// and assignees as one block of properties, the T3 threads, and one
    /// line for the integrations that are not connected.
    pub(super) fn render_side(
        &self,
        detail: &PullRequestDetail,
        loaded: &Loaded,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let reviewers = (!detail.reviewers.is_empty()).then(|| {
            v_flex()
                .flex_1()
                .min_w_0()
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
                        .h_7()
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
        });
        let spoken_reviewers = detail
            .reviewers
            .iter()
            .map(|reviewer| {
                format!(
                    "{} ({})",
                    reviewer_name(reviewer),
                    reviewer_style(reviewer.state).2
                )
            })
            .collect::<Vec<_>>();
        let labels = (!detail.labels.is_empty()).then(|| {
            h_flex()
                .flex_1()
                .min_w_0()
                .py_1()
                .gap_1()
                .flex_wrap()
                .children(detail.labels.iter().map(widgets::label_tag))
                .into_any_element()
        });
        let spoken_labels = detail
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<Vec<_>>();
        let (label_toggle, editor) = if detail.viewer_can_update {
            (
                Some(self.render_label_toggle(cx)),
                self.render_label_editor(detail, cx),
            )
        } else {
            (None, None)
        };
        let assignees = (!detail.assignees.is_empty()).then(|| {
            v_flex()
                .flex_1()
                .min_w_0()
                .children(detail.assignees.iter().map(|actor| {
                    h_flex()
                        .h_7()
                        .items_center()
                        .child(person(&actor.avatar_url, &actor.login))
                }))
                .into_any_element()
        });
        let spoken_assignees = detail
            .assignees
            .iter()
            .map(|actor| actor.login.clone())
            .collect::<Vec<_>>();
        let properties = v_flex()
            .id("pr-properties")
            .role(Role::Group)
            .aria_label("Details")
            .w_full()
            .child(property(
                "pr-reviewers",
                "Reviewers",
                &spoken_reviewers,
                reviewers,
                None,
                cx,
            ))
            .child(property(
                "pr-labels",
                "Labels",
                &spoken_labels,
                labels,
                label_toggle.map(IntoElement::into_any_element),
                cx,
            ))
            .children(editor.map(|editor| div().pl(PROPERTY_LANE).pb_2().child(editor)))
            .child(property(
                "pr-assignees",
                "Assignees",
                &spoken_assignees,
                assignees,
                None,
                cx,
            ));
        let services = Services::get(cx);
        let linear = services.linear.is_some();
        let t3 = services.t3.is_some();
        let mut sections = Vec::new();
        if linear {
            sections.push(loaded.linear.clone().into_any_element());
        }
        sections.push(properties.into_any_element());
        if t3 {
            sections.push(loaded.t3.clone().into_any_element());
        }
        if let Some(line) = not_connected(linear, t3, cx) {
            sections.push(line.into_any_element());
        }
        ui::side_panel("pr-side", ui::SideEdge::Trailing, scale::side_width())
            .track_scroll(&self.side_scroll)
            .child(v_flex().p_4().children(ui::side_sections(sections)))
            .into_any_element()
    }
}

/// The width of a property's name, which its values line up after.
const PROPERTY_LANE: Rems = Rems(5.5);

/// One property of the details: its name in a fixed lane, then its values
/// (or "None"), and its action at the trailing edge. Accessibility reads it
/// as one line: "Labels: bug, ui".
fn property(
    id: &'static str,
    name: &'static str,
    spoken: &[String],
    values: Option<AnyElement>,
    action: Option<AnyElement>,
    cx: &App,
) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    let spoken = if spoken.is_empty() {
        format!("{name}: none")
    } else {
        format!("{name}: {}", spoken.join(", "))
    };
    h_flex()
        .id(id)
        .role(Role::Group)
        .aria_label(spoken)
        .w_full()
        .min_h_7()
        .gap_2()
        .items_start()
        .text_size(scale::body())
        .child(
            h_flex()
                .flex_none()
                .w(PROPERTY_LANE)
                .h_7()
                .items_center()
                .text_color(muted)
                .child(name),
        )
        .child(values.unwrap_or_else(|| {
            h_flex()
                .flex_1()
                .h_7()
                .items_center()
                .text_color(tokens::text_faint())
                .child("None")
                .into_any_element()
        }))
        .children(action.map(|action| h_flex().flex_none().h_7().items_center().child(action)))
}

/// "Linear, T3 not connected · Settings": one line for every integration
/// that is not connected, with the button to Settings, where the first of
/// them opens. `None` when both are connected.
fn not_connected(linear: bool, t3: bool, cx: &App) -> Option<Stateful<Div>> {
    let missing: Vec<(&str, Section)> = [
        (!linear).then_some(("Linear", Section::Linear)),
        (!t3).then_some(("T3", Section::T3)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let section = missing.first()?.1;
    let names = missing
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    let muted = cx.theme().muted_foreground;
    let text = format!("{names} not connected");
    Some(
        h_flex()
            .id("pr-integrations")
            .role(Role::Group)
            .aria_label(text.clone())
            .w_full()
            .gap_1p5()
            .items_center()
            .text_size(scale::meta())
            .text_color(muted)
            .child(Icon::new(IconName::Unplug).xsmall().flex_none())
            .child(div().min_w_0().truncate().child(text))
            .child(div().flex_none().child("·"))
            .child(
                Button::new("pr-integrations-settings")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .label("Settings")
                    .text_color(tokens::link())
                    .tooltip(format!("Connect {names} in Settings"))
                    .accessibility_label(format!("Connect {names} in Settings"))
                    .on_click(move |_, window, cx| {
                        crate::settings::view::open_section(section, window, cx)
                    }),
            ),
    )
}
