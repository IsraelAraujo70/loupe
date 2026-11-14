//! The side column: state, priority, assignee, and labels as pickers, then
//! the read-only project, cycle, estimate, dates, and creator. Every value
//! starts on one edge: a 16 px mark, then its text.
//!
//! A picker is a list of [`Choice`]s built from the team's catalog. The
//! mouse, the keyboard (S, P, A, L open one; the arrows and Enter pick), and
//! accessibility clients all go through [`IssueView::pick`].

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::{IssueSummary, IssueUpdate, Priority, StateRef, Team, User};

use super::optimistic::state_ref;
use super::{IssueView, Loaded};
use crate::linear::data::Loadable;
use crate::linear::widgets;
use crate::shell::svg_icon;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;

const ROW_HEIGHT: f32 = 28.;
const LABEL_WIDTH: f32 = 64.;
/// The side padding of a value: the hover fill of a picker reaches this far
/// past its mark, and a read-only value keeps the same inset.
const VALUE_INSET: f32 = 8.;
const MARK: f32 = 16.;

/// A property the side column edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::linear) enum Property {
    State,
    Priority,
    Assignee,
    Labels,
}

impl Property {
    fn label(self) -> &'static str {
        match self {
            Property::State => "State",
            Property::Priority => "Priority",
            Property::Assignee => "Assignee",
            Property::Labels => "Labels",
        }
    }

    /// The property in a sentence: "Change state", "Could not change state".
    pub(super) fn noun(self) -> &'static str {
        match self {
            Property::State => "state",
            Property::Priority => "priority",
            Property::Assignee => "assignee",
            Property::Labels => "labels",
        }
    }

    /// The key that opens the picker, as Linear binds it.
    fn key(self) -> &'static str {
        match self {
            Property::State => "S",
            Property::Priority => "P",
            Property::Assignee => "A",
            Property::Labels => "L",
        }
    }

    fn id(self, place: Place) -> &'static str {
        match (self, place) {
            (Property::State, Place::Side) => "prop-state",
            (Property::State, Place::Header) => "header-state",
            (Property::Priority, _) => "prop-priority",
            (Property::Assignee, _) => "prop-assignee",
            (Property::Labels, _) => "prop-labels",
        }
    }
}

/// Where a picker hangs: under its row of the side column, or under the
/// state chip of the page header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    Side,
    Header,
}

/// The picker on screen and the row its keyboard cursor is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OpenPicker {
    pub property: Property,
    pub place: Place,
    pub cursor: usize,
}

/// What a row of a picker shows before its name.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::linear) enum Mark {
    State(StateRef),
    Priority(Priority),
    Person(Option<User>),
    /// A label's color, as Linear's hex string.
    Swatch(String),
    /// A project's box in its color, as Linear's hex string; `None` for
    /// "No project".
    Project(Option<String>),
}

/// What picking a row asks for.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::linear) enum Pick {
    Update(Box<IssueUpdate>),
    /// Flip this label. The picker stays open: labels are a set.
    Label(String),
}

/// One row of a picker.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::linear) struct Choice {
    pub id: String,
    pub name: String,
    pub mark: Mark,
    /// The issue has this value now.
    pub selected: bool,
    pub pick: Pick,
}

/// The rows of `property`'s picker for the issue as `shown`, from `team`'s
/// states, members, and labels.
pub(in crate::linear) fn choices(
    property: Property,
    team: &Team,
    shown: &IssueSummary,
) -> Vec<Choice> {
    match property {
        Property::State => team
            .states
            .iter()
            .map(|state| Choice {
                id: format!("state-{}", state.id),
                name: state.name.clone(),
                mark: Mark::State(state_ref(state)),
                selected: state.id == shown.state.id,
                pick: Pick::Update(Box::new(IssueUpdate {
                    state_id: Some(state.id.clone()),
                    ..IssueUpdate::default()
                })),
            })
            .collect(),
        Property::Priority => Priority::ALL
            .into_iter()
            .map(|priority| Choice {
                id: format!("priority-{}", priority.to_wire()),
                name: priority.label().to_owned(),
                mark: Mark::Priority(priority),
                selected: priority == shown.priority,
                pick: Pick::Update(Box::new(IssueUpdate {
                    priority: Some(priority),
                    ..IssueUpdate::default()
                })),
            })
            .collect(),
        Property::Assignee => {
            let current = shown.assignee.as_ref().map(|user| user.id.as_str());
            let nobody = Choice {
                id: "assignee-none".into(),
                name: "No assignee".into(),
                mark: Mark::Person(None),
                selected: current.is_none(),
                pick: Pick::Update(Box::new(IssueUpdate {
                    assignee_id: Some(None),
                    ..IssueUpdate::default()
                })),
            };
            std::iter::once(nobody)
                .chain(team.members.iter().map(|member| Choice {
                    id: format!("assignee-{}", member.id),
                    name: member.name.clone(),
                    mark: Mark::Person(Some(member.clone())),
                    selected: current == Some(member.id.as_str()),
                    pick: Pick::Update(Box::new(IssueUpdate {
                        assignee_id: Some(Some(member.id.clone())),
                        ..IssueUpdate::default()
                    })),
                }))
                .collect()
        }
        Property::Labels => team
            .labels
            .iter()
            .map(|label| Choice {
                id: format!("label-{}", label.id),
                name: label.name.clone(),
                mark: Mark::Swatch(label.color.clone()),
                selected: shown.labels.iter().any(|on| on.id == label.id),
                pick: Pick::Label(label.id.clone()),
            })
            .collect(),
    }
}

/// The row the cursor starts on: the issue's current value, else the first.
pub(super) fn first_cursor(choices: &[Choice]) -> usize {
    choices
        .iter()
        .position(|choice| choice.selected)
        .unwrap_or(0)
}

/// The cursor after a key: Down and J go one row down, Up and K one up, both
/// wrapping. Any other key leaves it.
pub(super) fn step_cursor(cursor: usize, count: usize, key: &str) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match key {
        "down" | "j" => Some((cursor + 1) % count),
        "up" | "k" => Some((cursor + count - 1) % count),
        _ => None,
    }
}

pub(in crate::linear) fn mark(mark: &Mark, cx: &App) -> AnyElement {
    match mark {
        Mark::State(state) => widgets::state_icon(state, cx),
        Mark::Priority(priority) => widgets::priority_icon(*priority, cx),
        Mark::Person(user) => widgets::avatar(user.as_ref(), dp(MARK)),
        Mark::Swatch(color) => h_flex()
            .size(dp(MARK))
            .flex_none()
            .justify_center()
            .child(
                div()
                    .size_2()
                    .rounded_full()
                    .bg(widgets::hex_color(color).unwrap_or(cx.theme().muted_foreground)),
            )
            .into_any_element(),
        Mark::Project(color) => svg_icon(
            IconName::Box,
            dp(MARK),
            color
                .as_deref()
                .and_then(widgets::hex_color)
                .unwrap_or(cx.theme().muted_foreground),
        )
        .into_any_element(),
    }
}

/// A row of the side column: the label, then the value.
fn property(label: &'static str, value: impl IntoElement, cx: &App) -> Div {
    // The label sits on the value's first line, so it stays at the top
    // when the value wraps (several labels).
    h_flex()
        .min_h(dp(ROW_HEIGHT))
        .items_start()
        .child(
            h_flex()
                .w(dp(LABEL_WIDTH))
                .h(dp(ROW_HEIGHT))
                .flex_none()
                .items_center()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(scale::body())
                .child(value),
        )
}

/// A value as a mark and its text, the shape every row shares. `faint`
/// draws the text in the muted color, for an empty value.
fn value(mark: AnyElement, text: impl Into<SharedString>, faint: bool, cx: &App) -> Div {
    let text: SharedString = text.into();
    h_flex()
        .min_w_0()
        .flex_1()
        .gap_2()
        .items_center()
        .child(mark)
        .child(
            div()
                .min_w_0()
                .truncate()
                .when(faint, |text| text.text_color(cx.theme().muted_foreground))
                .child(text),
        )
}

fn icon_mark(icon: IconName) -> AnyElement {
    svg_icon(icon, dp(MARK), tokens::text_faint()).into_any_element()
}

/// A value that is not edited here, on the pickers' own inset.
fn read_only(icon: IconName, text: Option<String>, cx: &App) -> Div {
    let faint = text.is_none();
    h_flex()
        .min_h(dp(ROW_HEIGHT))
        .px(dp(VALUE_INSET))
        .child(value(
            icon_mark(icon),
            text.unwrap_or_else(|| "None".into()),
            faint,
            cx,
        ))
}

fn estimate_text(estimate: f64) -> String {
    let unit = if estimate == 1. { "point" } else { "points" };
    if estimate.fract() == 0. {
        format!("{} {unit}", estimate as i64)
    } else {
        format!("{estimate} {unit}")
    }
}

/// What a picker's trigger shows: the issue's current value.
fn current(property: Property, issue: &IssueSummary, cx: &App) -> AnyElement {
    match property {
        Property::State => value(
            widgets::state_icon(&issue.state, cx),
            issue.state.name.clone(),
            false,
            cx,
        )
        .into_any_element(),
        Property::Priority => value(
            widgets::priority_icon(issue.priority, cx),
            issue.priority.label(),
            issue.priority == Priority::None,
            cx,
        )
        .into_any_element(),
        Property::Assignee => {
            let user = issue.assignee.as_ref();
            value(
                widgets::avatar(user, dp(MARK)),
                user.map_or_else(|| "Unassigned".to_owned(), |user| user.name.clone()),
                user.is_none(),
                cx,
            )
            .into_any_element()
        }
        Property::Labels if issue.labels.is_empty() => {
            value(icon_mark(IconName::Tag), "No labels", true, cx).into_any_element()
        }
        Property::Labels => h_flex()
            .flex_1()
            .min_w_0()
            .flex_wrap()
            .gap_1()
            .children(
                issue
                    .labels
                    .iter()
                    .map(|label| widgets::label_tag(label, cx)),
            )
            .into_any_element(),
    }
}

/// The rows of an open picker. `on_pick` and `on_key` go to the issue view.
fn picker_rows(
    choices: Rc<Vec<Choice>>,
    cursor: usize,
    focus: FocusHandle,
    view: WeakEntity<IssueView>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fill, hover, foreground) = (theme.list_active, theme.accent, theme.foreground);
    let radius = theme.radius;
    let keys = view.clone();
    v_flex()
        .id("picker-rows")
        .track_focus(&focus)
        .max_h(dp(320.))
        .overflow_y_scroll()
        .gap_0p5()
        .on_key_down(move |event, window, cx| {
            keys.update(cx, |view, cx| view.picker_key(event, window, cx))
                .ok();
        })
        .children(choices.iter().enumerate().map(|(index, choice)| {
            let view = view.clone();
            let name = if choice.selected {
                format!("{}, selected", choice.name)
            } else {
                choice.name.clone()
            };
            h_flex()
                .id(SharedString::from(choice.id.clone()))
                .role(Role::MenuItem)
                .aria_label(name)
                .h(dp(ROW_HEIGHT))
                .flex_none()
                .px_2()
                .gap_2()
                .items_center()
                .rounded(radius)
                .text_size(scale::body())
                // The fill is the keyboard cursor; the pointer has its own.
                .when(index == cursor, |row| row.bg(fill))
                .hover(move |row| row.bg(hover))
                .child(mark(&choice.mark, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(choice.name.clone()),
                )
                .when(choice.selected, |row| {
                    row.child(svg_icon(IconName::Check, dp(MARK), foreground))
                })
                .on_click(move |_, window, cx| {
                    view.update(cx, |view, cx| view.pick(index, window, cx))
                        .ok();
                })
        }))
        .into_any_element()
}

impl IssueView {
    /// Opens `property`'s picker at `place` with the cursor on the current
    /// value. Nothing happens before the team's catalog has loaded.
    pub(super) fn open_picker(&mut self, property: Property, place: Place, cx: &mut Context<Self>) {
        let Some((team, shown)) = self.editable() else {
            return;
        };
        let cursor = first_cursor(&choices(property, &team, &shown));
        self.picker = Some(OpenPicker {
            property,
            place,
            cursor,
        });
        cx.notify();
    }

    /// The team's catalog and the issue as shown, once both are there.
    fn editable(&self) -> Option<(Arc<Team>, IssueSummary)> {
        let (Loadable::Ready(loaded), Loadable::Ready(team)) = (&self.load, &self.team) else {
            return None;
        };
        Some((team.clone(), loaded.model.displayed(team)))
    }

    /// Applies row `index` of the open picker. A label stays in the picker;
    /// any other value closes it.
    pub(super) fn pick(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let (Some(open), Some((team, shown))) = (self.picker, self.editable()) else {
            return;
        };
        let Some(choice) = choices(open.property, &team, &shown).into_iter().nth(index) else {
            return;
        };
        match choice.pick {
            Pick::Update(update) => {
                self.picker = None;
                // Picking the value the issue already has changes nothing.
                if !choice.selected {
                    self.edit(open.property.noun(), *update, cx);
                }
            }
            Pick::Label(label_id) => {
                self.picker = Some(OpenPicker {
                    cursor: index,
                    ..open
                });
                self.toggle_label(&label_id, cx);
            }
        }
        cx.notify();
    }

    /// The keys of an open picker: the arrows (or J and K) move the cursor,
    /// Enter and Space pick its row. Escape is the popover's.
    fn picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(open), Some((team, shown))) = (self.picker, self.editable()) else {
            return;
        };
        let count = choices(open.property, &team, &shown).len();
        let key = event.keystroke.key.as_str();
        if let Some(cursor) = step_cursor(open.cursor, count, key) {
            self.picker = Some(OpenPicker { cursor, ..open });
            cx.notify();
        } else if matches!(key, "enter" | "space") {
            self.pick(open.cursor, window, cx);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    /// Flips `label_id` as the panel shows it now; the click's intent, not a
    /// toggle, is what gets queued.
    fn toggle_label(&mut self, label_id: &str, cx: &mut Context<Self>) {
        let (Loadable::Ready(loaded), Loadable::Ready(team)) = (&mut self.load, &self.team) else {
            return;
        };
        let shown = loaded
            .model
            .displayed(team)
            .labels
            .iter()
            .any(|label| label.id == label_id);
        self.edit_error = None;
        let seq = loaded.model.begin_label(label_id.to_owned(), !shown);
        self.edit_labels.push((seq, "labels"));
        self.send_next_edit(cx);
        cx.notify();
    }

    /// `trigger` with `property`'s picker hanging from it at `place`. The
    /// popover is controlled by `IssueView::picker`, so a click (from the
    /// mouse or an accessibility client) and the property's key open it.
    pub(super) fn picker(
        &self,
        property: Property,
        place: Place,
        trigger: Button,
        team: &Team,
        shown: &IssueSummary,
        cx: &mut Context<Self>,
    ) -> Popover {
        let open = self
            .picker
            .filter(|open| open.property == property && open.place == place);
        let rows = Rc::new(choices(property, team, shown));
        let cursor = open.map_or(0, |open| open.cursor);
        let focus = self.picker_focus.clone();
        let view = cx.weak_entity();
        let (on_change, on_click) = (view.clone(), view.clone());
        Popover::new(property.id(place))
            .anchor(match place {
                Place::Side => Anchor::TopRight,
                Place::Header => Anchor::TopLeft,
            })
            .p_1()
            .w(dp(240.))
            .open(open.is_some())
            .track_focus(&self.picker_focus)
            .on_open_change(move |open, _, cx| {
                on_change
                    .update(cx, |view, cx| {
                        // Another picker may have opened since: only the one
                        // that reports closing clears the state.
                        let this = view
                            .picker
                            .is_some_and(|now| now.property == property && now.place == place);
                        if !*open && this {
                            view.picker = None;
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .trigger(
                trigger
                    .accessibility_label(format!("Change {}", property.noun()))
                    .tooltip(format!("Change {} ({})", property.noun(), property.key()))
                    .on_click(move |_, _, cx| {
                        on_click
                            .update(cx, |view, cx| view.open_picker(property, place, cx))
                            .ok();
                    }),
            )
            .content(move |_, _, cx| {
                picker_rows(rows.clone(), cursor, focus.clone(), view.clone(), cx)
            })
    }

    /// The editable rows, built on `team`'s catalog: each value is a button
    /// with a caret that opens its picker.
    fn editable_rows(
        &self,
        team: &Arc<Team>,
        shown: &IssueSummary,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        [
            Property::State,
            Property::Priority,
            Property::Assignee,
            Property::Labels,
        ]
        .into_iter()
        .map(|kind| {
            let caret = svg_icon(IconName::ChevronDown, dp(14.), tokens::text_faint());
            let trigger = Button::new(SharedString::from(format!(
                "{}-trigger",
                kind.id(Place::Side)
            )))
            .ghost()
            .compact()
            .w_full()
            // A value that wraps (several labels) grows the trigger
            // instead of spilling out of it.
            .h_auto()
            .min_h(dp(ROW_HEIGHT))
            .px(dp(VALUE_INSET))
            .py_1()
            .selected(
                self.picker
                    .is_some_and(|open| open.property == kind && open.place == Place::Side),
            )
            // The button sets its own label size; the value is set
            // inside it, at the size of the read-only values.
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .text_size(scale::body())
                    .child(current(kind, shown, cx))
                    .child(caret),
            );
            property(
                kind.label(),
                self.picker(kind, Place::Side, trigger, team, shown, cx),
                cx,
            )
            .into_any_element()
        })
        .collect()
    }

    /// The same rows as plain values, while the team catalog is not there.
    fn static_rows(&self, shown: &IssueSummary, cx: &App) -> Vec<AnyElement> {
        [
            Property::State,
            Property::Priority,
            Property::Assignee,
            Property::Labels,
        ]
        .into_iter()
        .map(|kind| {
            property(
                kind.label(),
                h_flex()
                    .min_h(dp(ROW_HEIGHT))
                    .px(dp(VALUE_INSET))
                    .py_1()
                    .child(current(kind, shown, cx)),
                cx,
            )
            .into_any_element()
        })
        .collect()
    }

    pub(super) fn render_properties(
        &self,
        loaded: &Loaded,
        shown: &IssueSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let saving = loaded.model.has_pending();

        let editable = match &self.team {
            Loadable::Ready(team) => self.editable_rows(team, shown, cx),
            _ => self.static_rows(shown, cx),
        };

        let team_notice = match &self.team {
            Loadable::Loading => Some(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Loading options…")
                    .into_any_element(),
            ),
            Loadable::Failed(message) => Some(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(format!("Could not load options: {message}")),
                    )
                    .child(
                        Button::new("retry-team")
                            .ghost()
                            .compact()
                            .label("Retry")
                            .on_click(cx.listener(|view, _, _, cx| view.retry_team(cx))),
                    )
                    .into_any_element(),
            ),
            Loadable::Ready(_) => None,
        };

        let edit_error = self.edit_error.as_ref().map(|error| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
                .child(
                    Button::new("dismiss-edit-error")
                        .ghost()
                        .compact()
                        .label("Dismiss")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.edit_error = None;
                            cx.notify();
                        })),
                )
        });

        let project = shown.project.as_ref().map(|project| project.name.clone());
        let cycle = shown.cycle.as_ref().map(|cycle| {
            cycle
                .name
                .clone()
                .unwrap_or_else(|| format!("Cycle {}", cycle.number))
        });
        let date = |value: chrono::DateTime<chrono::Utc>| value.format("%b %-d, %Y").to_string();
        let creator = loaded.creator.as_ref();
        let creator_row = h_flex()
            .min_h(dp(ROW_HEIGHT))
            .px(dp(VALUE_INSET))
            .child(value(
                widgets::avatar(creator, dp(MARK)),
                creator.map_or_else(|| "None".to_owned(), |user| user.name.clone()),
                creator.is_none(),
                cx,
            ));

        // 16 px on both sides, like the main column. The heading sits on
        // the issue title's line beside it.
        v_flex()
            .px(scale::content_inset())
            .pt_5()
            .pb_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .h_7()
                    .child(crate::ui::section_label("Properties", cx))
                    .children(saving.then(|| div().text_xs().text_color(muted).child("Saving…"))),
            )
            .children(edit_error)
            .children(team_notice)
            .children(editable)
            .child(property(
                "Project",
                read_only(IconName::Box, project, cx),
                cx,
            ))
            .child(property(
                "Cycle",
                read_only(IconName::RefreshCw, cycle, cx),
                cx,
            ))
            .child(property(
                "Estimate",
                read_only(IconName::Triangle, shown.estimate.map(estimate_text), cx),
                cx,
            ))
            .child(property(
                "Due date",
                read_only(
                    IconName::CalendarClock,
                    shown
                        .due_date
                        .map(|due| due.format("%b %-d, %Y").to_string()),
                    cx,
                ),
                cx,
            ))
            .child(property("Creator", creator_row, cx))
            .child(property(
                "Created",
                read_only(IconName::Calendar, Some(date(shown.created_at)), cx),
                cx,
            ))
            .child(property(
                "Updated",
                read_only(IconName::Clock, Some(date(shown.updated_at)), cx),
                cx,
            ))
            .into_any_element()
    }
}

/// The state chip of the page header as a picker's trigger: the state's
/// mark and name, and a caret that says it opens.
pub(super) fn state_chip_trigger(issue: &IssueSummary, selected: bool, cx: &App) -> Button {
    let theme = cx.theme();
    Button::new("header-state-trigger")
        .outline()
        .compact()
        .h_6()
        .px_2()
        .rounded(px(9999.))
        .selected(selected)
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .text_size(scale::meta())
                .text_color(theme.foreground)
                .child(widgets::state_icon(&issue.state, cx))
                .child(issue.state.name.clone())
                .child(svg_icon(
                    IconName::ChevronDown,
                    dp(12.),
                    tokens::text_faint(),
                )),
        )
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_linear::{
        IssueSummary, Label, Priority, StateRef, StateType, Team, TeamRef, User, WorkflowState,
    };

    use super::{Pick, Property, choices, first_cursor, step_cursor};

    fn team() -> Team {
        let state = |id: &str, name: &str, kind: StateType| WorkflowState {
            id: id.into(),
            name: name.into(),
            kind,
            position: 0.,
            color: "#888888".into(),
        };
        Team {
            id: "t".into(),
            key: "T".into(),
            name: "Team".into(),
            states: vec![
                state("s-todo", "Todo", StateType::Unstarted),
                state("s-doing", "In Progress", StateType::Started),
                state("s-done", "Done", StateType::Completed),
            ],
            members: vec![User {
                id: "u-ana".into(),
                name: "Ana".into(),
                display_name: "ana".into(),
                avatar_url: None,
            }],
            labels: vec![
                Label {
                    id: "l-bug".into(),
                    name: "Bug".into(),
                    color: "#ff0000".into(),
                    parent_id: None,
                },
                Label {
                    id: "l-infra".into(),
                    name: "Infra".into(),
                    color: "#00ff00".into(),
                    parent_id: None,
                },
            ],
            projects: Vec::new(),
            cycles: Vec::new(),
            label_groups: Vec::new(),
            estimation: Default::default(),
        }
    }

    fn issue() -> IssueSummary {
        let at = DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .unwrap()
            .to_utc();
        IssueSummary {
            id: "i".into(),
            identifier: "T-1".into(),
            title: "Title".into(),
            priority: Priority::None,
            sort_order: 0.,
            state: StateRef {
                id: "s-doing".into(),
                name: "In Progress".into(),
                kind: StateType::Started,
                color: "#888888".into(),
            },
            team: TeamRef {
                id: "t".into(),
                key: "T".into(),
                name: "Team".into(),
            },
            assignee: None,
            labels: vec![Label {
                id: "l-infra".into(),
                name: "Infra".into(),
                color: "#00ff00".into(),
                parent_id: None,
            }],
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: String::new(),
            branch_name: String::new(),
            created_at: at,
            updated_at: at,
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests: Vec::new(),
            pull_requests_complete: true,
        }
    }

    #[test]
    fn the_state_picker_lists_the_teams_states_and_marks_the_current_one() {
        let rows = choices(Property::State, &team(), &issue());
        let names: Vec<(&str, bool)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.selected))
            .collect();
        assert_eq!(
            names,
            [("Todo", false), ("In Progress", true), ("Done", false)]
        );
        assert_eq!(
            first_cursor(&rows),
            1,
            "the cursor starts on the current state"
        );
        let Pick::Update(update) = &rows[2].pick else {
            panic!("a state is an update");
        };
        assert_eq!(update.state_id.as_deref(), Some("s-done"));
        assert_eq!(update.priority, None, "only the state is sent");
    }

    #[test]
    fn the_assignee_picker_starts_with_no_assignee() {
        let rows = choices(Property::Assignee, &team(), &issue());
        assert_eq!(rows[0].name, "No assignee");
        assert!(rows[0].selected, "the issue is unassigned");
        let Pick::Update(update) = &rows[1].pick else {
            panic!("a member is an update");
        };
        assert_eq!(update.assignee_id, Some(Some("u-ana".into())));
    }

    #[test]
    fn labels_toggle_and_show_what_is_on_the_issue() {
        let rows = choices(Property::Labels, &team(), &issue());
        let on: Vec<(&str, bool)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.selected))
            .collect();
        assert_eq!(on, [("Bug", false), ("Infra", true)]);
        assert_eq!(rows[0].pick, Pick::Label("l-bug".into()));
    }

    #[test]
    fn the_cursor_wraps_and_ignores_other_keys() {
        assert_eq!(step_cursor(0, 3, "down"), Some(1));
        assert_eq!(step_cursor(2, 3, "j"), Some(0));
        assert_eq!(step_cursor(0, 3, "up"), Some(2));
        assert_eq!(step_cursor(1, 3, "k"), Some(0));
        assert_eq!(step_cursor(1, 3, "x"), None);
        assert_eq!(step_cursor(0, 0, "down"), None);
    }
}
