//! The Overview tab's side column: the project's status, lead, members,
//! dates, teams, and labels, each a control that changes it in Linear. Every
//! value starts on one edge: a 16 px mark, then its text, as on an issue.

use std::rc::Rc;

use chrono::NaiveDate;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::calendar::Calendar;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::{ProjectDetail, ProjectOptions};

use super::model::{self, Choice, Field, Mark, step_cursor};
use super::{DateField, OpenPicker, ProjectView};
use crate::linear::data::Loadable;
use crate::linear::widgets;
use crate::shell::svg_icon;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;

const ROW_HEIGHT: f32 = 28.;
const LABEL_WIDTH: f32 = 88.;
const VALUE_INSET: f32 = 8.;
const MARK: f32 = 16.;

fn mark(mark: &Mark, cx: &App) -> AnyElement {
    let swatch = |color: Hsla| {
        h_flex()
            .size(dp(MARK))
            .flex_none()
            .justify_center()
            .child(div().size_2().rounded_full().bg(color))
            .into_any_element()
    };
    let muted = cx.theme().muted_foreground;
    match mark {
        Mark::Status(status) => swatch(widgets::hex_color(&status.color).unwrap_or(muted)),
        Mark::Person(user) => widgets::avatar(user.as_ref(), dp(MARK)),
        Mark::Swatch(color) => swatch(widgets::hex_color(color).unwrap_or(muted)),
        Mark::Team => svg_icon(IconName::Users, dp(MARK), tokens::text_faint()).into_any_element(),
    }
}

/// A row of the side column: the label, then the value.
fn property(label: &'static str, value: impl IntoElement, cx: &App) -> Div {
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

/// A value as a mark and its text. `faint` mutes the text, for "None".
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

/// A date with its calendar mark, or a faint "None".
fn date_value(day: Option<NaiveDate>, cx: &App) -> Div {
    value(
        svg_icon(IconName::Calendar, dp(MARK), tokens::text_faint()).into_any_element(),
        day.map_or_else(
            || "None".to_owned(),
            |day| day.format("%b %-d, %Y").to_string(),
        ),
        day.is_none(),
        cx,
    )
}

/// What `field`'s trigger shows: the project's current value.
fn current(field: Field, project: &ProjectDetail, cx: &App) -> AnyElement {
    let faint_icon = |icon| svg_icon(icon, dp(MARK), tokens::text_faint()).into_any_element();
    let chips = |chips: Vec<AnyElement>| {
        h_flex()
            .flex_1()
            .min_w_0()
            .flex_wrap()
            .gap_1()
            .children(chips)
            .into_any_element()
    };
    match field {
        Field::Status => {
            let status = &project.status;
            value(
                mark(&Mark::Status(status.clone()), cx),
                status.name.clone(),
                false,
                cx,
            )
            .into_any_element()
        }
        Field::Lead => {
            let lead = project.lead.as_ref();
            value(
                widgets::avatar(lead, dp(MARK)),
                lead.map_or_else(|| "No lead".to_owned(), |lead| lead.name.clone()),
                lead.is_none(),
                cx,
            )
            .into_any_element()
        }
        Field::Members if project.members.is_empty() => {
            value(faint_icon(IconName::Users), "No members", true, cx).into_any_element()
        }
        Field::Members => h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .items_center()
            .child(
                h_flex().flex_none().gap_0p5().children(
                    project
                        .members
                        .iter()
                        .take(5)
                        .map(|member| widgets::avatar(Some(member), dp(MARK))),
                ),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(match project.members.as_slice() {
                        [only] => only.name.clone(),
                        members => format!("{} members", members.len()),
                    }),
            )
            .into_any_element(),
        Field::Teams if project.teams.is_empty() => {
            value(faint_icon(IconName::Users), "No teams", true, cx).into_any_element()
        }
        Field::Teams => chips(
            project
                .teams
                .iter()
                .map(|team| {
                    crate::ui::chip()
                        .child(team.name.clone())
                        .into_any_element()
                })
                .collect(),
        ),
        Field::Labels if project.labels.is_empty() => {
            value(faint_icon(IconName::Tag), "No labels", true, cx).into_any_element()
        }
        Field::Labels => chips(
            project
                .labels
                .iter()
                .map(|label| widgets::label_tag(label, cx))
                .collect(),
        ),
    }
}

/// The rows of an open picker.
fn picker_rows(
    choices: Rc<Vec<Choice>>,
    cursor: usize,
    focus: FocusHandle,
    view: WeakEntity<ProjectView>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fill, hover, foreground) = (theme.list_active, theme.accent, theme.foreground);
    let radius = theme.radius;
    let keys = view.clone();
    v_flex()
        .id("project-picker-rows")
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
                .id(SharedString::from(format!("choice-{index}")))
                .role(Role::MenuItem)
                .aria_label(name)
                .h(dp(ROW_HEIGHT))
                .flex_none()
                .px_2()
                .gap_2()
                .items_center()
                .rounded(radius)
                .text_size(scale::body())
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

impl ProjectView {
    /// The project as shown and the workspace's options, once both are there.
    fn editable(&self) -> Option<(ProjectDetail, std::sync::Arc<ProjectOptions>)> {
        let (Loadable::Ready(project), Loadable::Ready(options)) = (&self.project, &self.options)
        else {
            return None;
        };
        Some((project.clone(), options.clone()))
    }

    fn open_picker(&mut self, field: Field, cx: &mut Context<Self>) {
        let Some((project, options)) = self.editable() else {
            return;
        };
        let rows = model::choices(field, &project, &options);
        let cursor = rows.iter().position(|choice| choice.selected).unwrap_or(0);
        self.picker = Some(OpenPicker { field, cursor });
        self.date_open = None;
        cx.notify();
    }

    /// Applies row `index` of the open picker. A set keeps its picker open.
    fn pick(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let (Some(open), Some((project, options))) = (self.picker, self.editable()) else {
            return;
        };
        let rows = model::choices(open.field, &project, &options);
        let Some(choice) = rows.get(index) else {
            return;
        };
        let change = model::change_for(open.field, choice, &project);
        self.picker = open.field.is_set().then_some(OpenPicker {
            cursor: index,
            ..open
        });
        match change {
            Some(change) => self.edit(open.field.noun(), change, cx),
            None if open.field == Field::Teams => {
                self.edit_error = Some("A project stays in at least one team.".into());
            }
            None => {}
        }
        cx.notify();
    }

    /// The arrows (or J and K) move the cursor; Enter and Space pick.
    fn picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(open), Some((project, options))) = (self.picker, self.editable()) else {
            return;
        };
        let count = model::choices(open.field, &project, &options).len();
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

    /// `field`'s value as a button with a caret, and its picker under it.
    fn picker(
        &self,
        field: Field,
        project: &ProjectDetail,
        options: &ProjectOptions,
        cx: &mut Context<Self>,
    ) -> Popover {
        let open = self.picker.filter(|open| open.field == field);
        let rows = Rc::new(model::choices(field, project, options));
        let cursor = open.map_or(0, |open| open.cursor);
        let focus = self.picker_focus.clone();
        let view = cx.weak_entity();
        let (on_change, on_click) = (view.clone(), view.clone());
        let trigger = Button::new(SharedString::from(format!("{}-trigger", field.id())))
            .ghost()
            .compact()
            .w_full()
            .h_auto()
            .min_h(dp(ROW_HEIGHT))
            .px(dp(VALUE_INSET))
            .py_1()
            .selected(open.is_some())
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .text_size(scale::body())
                    .child(current(field, project, cx))
                    .child(svg_icon(
                        IconName::ChevronDown,
                        dp(14.),
                        tokens::text_faint(),
                    )),
            )
            .accessibility_label(format!("Change {}", field.noun()))
            .tooltip(format!("Change {}", field.noun()))
            .on_click(move |_, _, cx| {
                on_click
                    .update(cx, |view, cx| view.open_picker(field, cx))
                    .ok();
            });
        Popover::new(field.id())
            .anchor(Anchor::TopRight)
            .p_1()
            .w(dp(240.))
            .open(open.is_some())
            .track_focus(&self.picker_focus)
            // The popover opens on the press and says so here; the click
            // that follows opens it too. Taking both keeps a render between
            // the two from closing it again.
            .on_open_change(move |open, _, cx| {
                on_change
                    .update(cx, |view, cx| {
                        let this = view.picker.is_some_and(|now| now.field == field);
                        if *open && !this {
                            view.open_picker(field, cx);
                        } else if !*open && this {
                            view.picker = None;
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .trigger(trigger)
            .content(move |_, _, cx| {
                picker_rows(rows.clone(), cursor, focus.clone(), view.clone(), cx)
            })
    }

    /// A date as a value with a caret, like the pickers; it opens a
    /// calendar, with Clear once the project has the date.
    fn date_row(&self, field: DateField, day: Option<NaiveDate>, cx: &mut Context<Self>) -> Div {
        let (label, noun, id, calendar) = match field {
            DateField::Start => (
                "Start date",
                "start date",
                "project-start-date",
                &self.start_calendar,
            ),
            DateField::Target => (
                "Target date",
                "target date",
                "project-target-date",
                &self.target_calendar,
            ),
        };
        let open = self.date_open == Some(field);
        let view = cx.weak_entity();
        let (on_change, on_click) = (view.clone(), view.clone());
        let trigger = Button::new(SharedString::from(format!("{id}-trigger")))
            .ghost()
            .compact()
            .w_full()
            .h(dp(ROW_HEIGHT))
            .px(dp(VALUE_INSET))
            .selected(open)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .text_size(scale::body())
                    .child(date_value(day, cx))
                    .child(svg_icon(
                        IconName::ChevronDown,
                        dp(14.),
                        tokens::text_faint(),
                    )),
            )
            .accessibility_label(format!("Change {noun}"))
            .tooltip(format!("Change {noun}"))
            .on_click(move |_, _, cx| {
                on_click
                    .update(cx, |view, cx| {
                        view.picker = None;
                        view.date_open = Some(field);
                        cx.notify();
                    })
                    .ok();
            });
        let calendar = calendar.clone();
        let calendar_focus = calendar.read(cx).focus_handle.clone();
        let popover = Popover::new(id)
            .anchor(Anchor::TopRight)
            .p_2()
            .open(open)
            .track_focus(&calendar_focus)
            .on_open_change(move |open, _, cx| {
                on_change
                    .update(cx, |view, cx| {
                        if *open && view.date_open != Some(field) {
                            view.picker = None;
                            view.date_open = Some(field);
                            cx.notify();
                        } else if !*open && view.date_open == Some(field) {
                            view.date_open = None;
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .trigger(trigger)
            .content(move |_, _, _| {
                let clear = view.clone();
                v_flex()
                    .gap_1()
                    .child(Calendar::new(&calendar).small())
                    .when(day.is_some(), |column| {
                        column.child(
                            Button::new(SharedString::from(format!("{id}-clear")))
                                .ghost()
                                .small()
                                .label("Clear")
                                .accessibility_label(format!("Clear {noun}"))
                                .on_click(move |_, _, cx| {
                                    clear
                                        .update(cx, |view, cx| view.set_date(field, None, cx))
                                        .ok();
                                }),
                        )
                    })
            });
        property(label, popover, cx)
    }

    pub(super) fn render_properties(
        &self,
        project: &ProjectDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = match &self.options {
            Loadable::Ready(options) => {
                let options = options.clone();
                Field::ALL
                    .into_iter()
                    .map(|field| {
                        property(field.label(), self.picker(field, project, &options, cx), cx)
                            .into_any_element()
                    })
                    .collect()
            }
            _ => Field::ALL
                .into_iter()
                .map(|field| {
                    property(
                        field.label(),
                        h_flex()
                            .min_h(dp(ROW_HEIGHT))
                            .px(dp(VALUE_INSET))
                            .py_1()
                            .child(current(field, project, cx)),
                        cx,
                    )
                    .into_any_element()
                })
                .collect(),
        };
        // Status, lead, and members, then the dates, then teams and labels:
        // Linear's order.
        let mut rows = rows.into_iter();
        let head: Vec<AnyElement> = rows.by_ref().take(3).collect();
        let tail: Vec<AnyElement> = rows.collect();

        let options_notice = match &self.options {
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
                        Button::new("retry-project-options")
                            .ghost()
                            .compact()
                            .label("Retry")
                            .on_click(cx.listener(|view, _, _, cx| view.load_options(cx))),
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
                    Button::new("dismiss-project-error")
                        .ghost()
                        .compact()
                        .label("Dismiss")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.edit_error = None;
                            cx.notify();
                        })),
                )
        });
        let editable = matches!(self.options, Loadable::Ready(_));
        let (start, target) = if editable {
            (
                self.date_row(DateField::Start, project.start_date, cx),
                self.date_row(DateField::Target, project.target_date, cx),
            )
        } else {
            let date = |label, day| {
                property(
                    label,
                    h_flex()
                        .min_h(dp(ROW_HEIGHT))
                        .px(dp(VALUE_INSET))
                        .child(date_value(day, cx)),
                    cx,
                )
            };
            (
                date("Start date", project.start_date),
                date("Target date", project.target_date),
            )
        };
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
                    .children(
                        (self.saving > 0)
                            .then(|| div().text_xs().text_color(muted).child("Saving…")),
                    ),
            )
            .children(edit_error)
            .children(options_notice)
            .children(head)
            .child(start)
            .child(target)
            .children(tail)
            .into_any_element()
    }
}
