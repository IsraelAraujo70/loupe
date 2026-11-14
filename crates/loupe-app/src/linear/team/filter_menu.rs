//! "Add Filter…", after Linear's filter menu: a search field over the filter
//! fields, where a field opens its values and each value toggles in place.
//! On the first page, typing also finds the values of every field, so "bug"
//! finds the Bug label.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::filter::{DateField, FilterClause, FlagField, SetField};

use super::TeamView;
use super::filter::{self, DATE_PRESETS, FieldOption};
use super::toolbar::FilterOptions;
use crate::linear::data::Loadable;
use crate::shell::svg_icon;
use crate::theme::scale::dp;

/// Values listed for a search on the first page, at most.
const MAX_VALUE_MATCHES: usize = 30;

/// A page of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Fields,
    Field(Field),
}

/// An entry of the first page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Set(SetField),
    /// The sub-issue flags.
    Relations,
    /// The date fields and the due date flag.
    Dates,
    Date(DateField),
    /// Pull request status and the has-pull-requests flag.
    PullRequests,
}

/// The first page in Linear's order; `None` separates groups.
const FIELDS: [Option<Field>; 14] = [
    Some(Field::Set(SetField::Status)),
    Some(Field::Set(SetField::StatusType)),
    Some(Field::Set(SetField::Assignee)),
    Some(Field::Set(SetField::Creator)),
    Some(Field::Set(SetField::Priority)),
    Some(Field::Set(SetField::Label)),
    Some(Field::Relations),
    Some(Field::Dates),
    None,
    Some(Field::Set(SetField::Project)),
    Some(Field::Set(SetField::Milestone)),
    Some(Field::Set(SetField::Cycle)),
    None,
    Some(Field::PullRequests),
];

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Set(SetField::Label) => "Labels",
            Field::Set(SetField::Milestone) => "Project milestone",
            Field::Set(field) => field.label(),
            Field::Relations => "Relations",
            Field::Dates => "Dates",
            Field::Date(field) => field.label(),
            Field::PullRequests => "Pull requests",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Field::Set(SetField::Status) => IconName::CircleDashed,
            Field::Set(SetField::StatusType) => IconName::CircleDot,
            Field::Set(SetField::Assignee) => IconName::UserRound,
            Field::Set(SetField::Creator) => IconName::UserPen,
            Field::Set(SetField::Priority) => IconName::ChartNoAxesColumn,
            Field::Set(SetField::Label) => IconName::Tag,
            Field::Set(SetField::Project) => IconName::Box,
            Field::Set(SetField::Milestone) => IconName::Milestone,
            Field::Set(SetField::Cycle) => IconName::RefreshCw,
            Field::Set(SetField::PullRequest) | Field::PullRequests => IconName::GitPullRequest,
            Field::Relations => IconName::Network,
            Field::Dates | Field::Date(_) => IconName::Calendar,
        }
    }
}

type Edit = Rc<dyn Fn(&mut Vec<FilterClause>)>;

/// What a row does when picked.
#[derive(Clone)]
pub(super) enum Pick {
    Open(Field),
    Edit(Edit),
}

/// One row of the menu.
#[derive(Clone)]
pub(super) struct Choice {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub checked: bool,
    /// A thin rule above the row, between groups.
    pub separated: bool,
    pub pick: Pick,
}

fn open(field: Field, separated: bool) -> Choice {
    Choice {
        id: format!("filter-field-{field:?}").into(),
        label: field.label().into(),
        icon: Some(field.icon()),
        checked: false,
        separated,
        pick: Pick::Open(field),
    }
}

fn set_value(
    field: SetField,
    option: &FieldOption,
    clauses: &[FilterClause],
    qualified: bool,
) -> Choice {
    let value = option.value.clone();
    let label = if qualified {
        format!("{}: {}", Field::Set(field).label(), option.label)
    } else {
        option.label.clone()
    };
    Choice {
        id: format!("filter-value-{field:?}-{value}").into(),
        label: label.into(),
        icon: qualified.then(|| Field::Set(field).icon()),
        checked: filter::selected(clauses, field).contains(&option.value),
        separated: false,
        pick: Pick::Edit(Rc::new(move |clauses| {
            filter::toggle_value(clauses, field, &value)
        })),
    }
}

fn flag(field: FlagField, value: bool, label: &str, clauses: &[FilterClause]) -> Choice {
    let checked = clauses.iter().any(|clause| {
        matches!(clause, FilterClause::Flag { field: f, value: v } if *f == field && *v == value)
    });
    Choice {
        id: format!("filter-flag-{field:?}-{value}").into(),
        label: label.to_owned().into(),
        icon: None,
        checked,
        separated: false,
        pick: Pick::Edit(Rc::new(move |clauses| {
            filter::set_flag(clauses, field, value)
        })),
    }
}

/// The rows of a field's page.
fn field_choices(field: Field, options: &FilterOptions, clauses: &[FilterClause]) -> Vec<Choice> {
    match field {
        Field::Set(set) => options
            .of(set)
            .iter()
            .map(|option| set_value(set, option, clauses, false))
            .collect(),
        Field::Relations => vec![
            flag(FlagField::IsSubIssue, true, "Is a sub-issue", clauses),
            flag(FlagField::IsSubIssue, false, "Is not a sub-issue", clauses),
            flag(FlagField::HasSubIssues, true, "Has sub-issues", clauses),
            flag(FlagField::HasSubIssues, false, "Has no sub-issues", clauses),
        ],
        Field::Dates => {
            let mut rows: Vec<Choice> = DateField::ALL
                .iter()
                .map(|date| open(Field::Date(*date), false))
                .collect();
            let mut due = flag(FlagField::HasDueDate, true, "Has a due date", clauses);
            due.separated = true;
            rows.push(due);
            rows.push(flag(FlagField::HasDueDate, false, "Has no due date", clauses));
            rows
        }
        Field::Date(date) => DATE_PRESETS
            .iter()
            .map(|(preset, label)| {
                let preset = *preset;
                Choice {
                    id: format!("filter-date-{date:?}-{label}").into(),
                    label: (*label).into(),
                    icon: None,
                    checked: clauses.iter().any(|clause| {
                        matches!(clause, FilterClause::Date { field, op } if *field == date && *op == preset)
                    }),
                    separated: false,
                    pick: Pick::Edit(Rc::new(move |clauses| {
                        filter::set_date(clauses, date, preset)
                    })),
                }
            })
            .collect(),
        Field::PullRequests => {
            let mut rows = vec![
                flag(FlagField::HasPullRequests, true, "Has pull requests", clauses),
                flag(FlagField::HasPullRequests, false, "Has no pull requests", clauses),
            ];
            rows.extend(
                options
                    .of(SetField::PullRequest)
                    .iter()
                    .enumerate()
                    .map(|(ix, option)| Choice {
                        separated: ix == 0,
                        ..set_value(SetField::PullRequest, option, clauses, false)
                    }),
            );
            rows
        }
    }
}

/// The rows `page` shows for `query`, matched case-insensitively against
/// each label.
pub(super) fn choices(
    page: Page,
    query: &str,
    options: &FilterOptions,
    clauses: &[FilterClause],
) -> Vec<Choice> {
    let query = query.trim().to_lowercase();
    let matches = |label: &str| query.is_empty() || label.to_lowercase().contains(&query);
    match page {
        Page::Fields => {
            let mut rows: Vec<Choice> = Vec::new();
            let mut gap = false;
            for entry in FIELDS {
                match entry {
                    None => gap = true,
                    Some(field) if matches(field.label()) => {
                        rows.push(open(field, gap && !rows.is_empty()));
                        gap = false;
                    }
                    Some(_) => {}
                }
            }
            if !query.is_empty() {
                let values = SetField::ALL
                    .iter()
                    .flat_map(|field| {
                        options
                            .of(*field)
                            .iter()
                            .filter(|option| matches(&option.label))
                            .map(|option| set_value(*field, option, clauses, true))
                    })
                    .take(MAX_VALUE_MATCHES)
                    .enumerate()
                    .map(|(ix, choice)| Choice {
                        separated: ix == 0 && !rows.is_empty(),
                        ..choice
                    });
                rows.extend(values.collect::<Vec<_>>());
            }
            rows
        }
        Page::Field(field) => field_choices(field, options, clauses)
            .into_iter()
            .filter(|choice| matches(&choice.label))
            .collect(),
    }
}

/// The menu's view: the search field and the rows of the current page. The
/// team view owns it and shows it in the filter button's popover.
pub(super) struct FilterMenu {
    team_view: WeakEntity<TeamView>,
    query: Entity<InputState>,
    page: Page,
    highlighted: usize,
    /// Focus the search field on the next render, once the popover is up.
    focus_query: bool,
    _query_events: Subscription,
}

impl FilterMenu {
    pub(super) fn new(
        team_view: WeakEntity<TeamView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Add Filter…"));
        let events =
            cx.subscribe_in(
                &query,
                window,
                |menu, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        menu.highlighted = 0;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => menu.pick(menu.highlighted, window, cx),
                    _ => {}
                },
            );
        FilterMenu {
            team_view,
            query,
            page: Page::Fields,
            highlighted: 0,
            focus_query: false,
            _query_events: events,
        }
    }

    /// Back to the first page with an empty search, for a fresh open.
    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to(Page::Fields, window, cx);
    }

    fn go_to(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.highlighted = 0;
        self.focus_query = true;
        self.query
            .update(cx, |query, cx| query.set_value("", window, cx));
        cx.notify();
    }

    /// The options and clauses of the team view, or `None` before its team
    /// catalog loads.
    fn current_choices(&self, cx: &App) -> Option<Vec<Choice>> {
        let team_view = self.team_view.upgrade()?;
        let team_view = team_view.read(cx);
        let Loadable::Ready(team) = &team_view.team else {
            return None;
        };
        let listed = match &team_view.issues {
            Loadable::Ready(listed) => listed.as_slice(),
            _ => &[],
        };
        let options = FilterOptions::new(team, listed);
        let query = self.query.read(cx).value();
        Some(choices(self.page, &query, &options, team_view.filters()))
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self
            .current_choices(cx)
            .and_then(|choices| choices.into_iter().nth(index))
        else {
            return;
        };
        match choice.pick {
            Pick::Open(field) => self.go_to(Page::Field(field), window, cx),
            Pick::Edit(edit) => {
                self.team_view
                    .update(cx, |team_view, cx| {
                        team_view.edit_filters(|clauses| edit(clauses), cx)
                    })
                    .ok();
                self.highlighted = index;
                cx.notify();
            }
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.current_choices(cx).map_or(0, |choices| choices.len());
        let empty_query = self.query.read(cx).value().is_empty();
        match event.keystroke.key.as_str() {
            "down" if count > 0 => self.highlighted = (self.highlighted + 1) % count,
            "up" if count > 0 => self.highlighted = (self.highlighted + count - 1) % count,
            "escape" if self.page != Page::Fields => self.go_to(Page::Fields, window, cx),
            "backspace" if self.page != Page::Fields && empty_query => {
                self.go_to(Page::Fields, window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for FilterMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_query) {
            self.query.focus_handle(cx).focus(window, cx);
        }
        let rows = self.current_choices(cx).unwrap_or_default();
        let theme = cx.theme();
        let (muted, hover, border) = (theme.muted_foreground, theme.list_hover, theme.border);
        let back = match self.page {
            Page::Fields => None,
            Page::Field(field) => Some(field),
        };
        let highlighted = self.highlighted.min(rows.len().saturating_sub(1));
        let row_elements: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, choice)| {
                let opens = matches!(choice.pick, Pick::Open(_));
                let name: SharedString = if choice.checked {
                    format!("{}, selected", choice.label).into()
                } else {
                    choice.label.clone()
                };
                v_flex()
                    .when(choice.separated, |row| {
                        row.child(div().my_1().h(px(1.)).bg(border))
                    })
                    .child(
                        crate::ui::menu_row(
                            choice.id,
                            name,
                            choice.label,
                            crate::ui::MenuRow {
                                icon: choice.icon,
                                checked: choice.checked,
                                detail: None,
                                opens,
                                focused: ix == highlighted,
                            },
                            cx,
                        )
                        .on_click(
                            cx.listener(move |menu, _, window, cx| menu.pick(ix, window, cx)),
                        ),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = row_elements.is_empty();
        v_flex()
            .w_full()
            .capture_key_down(cx.listener(Self::on_key))
            .child(
                h_flex()
                    // The rows' own grid: 12 px, a 16 px icon, 8 px, then
                    // the text, so the field's text sits over the options'.
                    .pl_3()
                    .pr_2()
                    .pb_1()
                    .gap_2()
                    // The 24 px back button takes the icon's place: 8 + 24
                    // + 4 keeps the text on the same 36 px.
                    .when(back.is_some(), |row| row.pl_2().gap_1())
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .when(back.is_none(), |row| {
                        row.child(svg_icon(IconName::Search, dp(16.), muted))
                    })
                    .when_some(back, |row, field| {
                        row.child(
                            div()
                                .id("filter-back")
                                .role(Role::Button)
                                .aria_label(format!("Back from {}", field.label()))
                                .flex_none()
                                .size_6()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .hover(move |button| button.bg(hover))
                                .on_click(cx.listener(|menu, _, window, cx| {
                                    menu.go_to(Page::Fields, window, cx)
                                }))
                                .child(svg_icon(IconName::ChevronLeft, dp(16.), muted)),
                        )
                    })
                    .child(
                        Input::new(&self.query)
                            .small()
                            .appearance(false)
                            .pl_0()
                            .flex_1(),
                    )
                    .when(back.is_none(), |row| {
                        row.child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .rounded_sm()
                                .border_1()
                                .border_color(border)
                                .text_xs()
                                .text_color(muted)
                                .child("F"),
                        )
                    }),
            )
            .child(
                div()
                    .id("filter-menu-rows")
                    .max_h(dp(420.))
                    .overflow_y_scroll()
                    .pt_1()
                    .when(empty, |list| {
                        list.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_sm()
                                .text_color(muted)
                                .child("No matches"),
                        )
                    })
                    .children(row_elements),
            )
    }
}

#[cfg(test)]
mod tests {
    use loupe_linear::filter::{FilterClause, FlagField, SetField, SetOp};

    use super::{Choice, Field, Page, Pick, choices};
    use crate::linear::team::filter::FieldOption;
    use crate::linear::team::toolbar::FilterOptions;

    fn options() -> FilterOptions {
        let option = |value: &str, label: &str| FieldOption {
            value: value.into(),
            label: label.into(),
        };
        FilterOptions::from_fields(vec![
            (
                SetField::Label,
                vec![option("l-bug", "Bug"), option("l-feat", "Feature")],
            ),
            (SetField::Project, vec![option("p-debug", "Debugger")]),
        ])
    }

    fn labels(rows: &[Choice]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                format!(
                    "{}{}{}",
                    if row.separated { "-- " } else { "" },
                    row.label,
                    if row.checked { " ✓" } else { "" }
                )
            })
            .collect()
    }

    #[test]
    fn the_first_page_lists_fields_in_groups_like_linear() {
        let rows = choices(Page::Fields, "", &options(), &[]);
        assert_eq!(
            labels(&rows),
            [
                "Status",
                "Status type",
                "Assignee",
                "Creator",
                "Priority",
                "Labels",
                "Relations",
                "Dates",
                "-- Project",
                "Project milestone",
                "Cycle",
                "-- Pull requests",
            ]
        );
    }

    #[test]
    fn typing_finds_fields_and_the_values_of_every_field() {
        let rows = choices(Page::Fields, "bu", &options(), &[]);
        assert_eq!(labels(&rows), ["Labels: Bug", "Project: Debugger"]);
        let rows = choices(Page::Fields, "lab", &options(), &[]);
        assert_eq!(labels(&rows), ["Labels"]);
        assert!(matches!(
            rows[0].pick,
            Pick::Open(Field::Set(SetField::Label))
        ));
    }

    #[test]
    fn picking_a_value_toggles_its_clause_and_marks_it() {
        let rows = choices(
            Page::Field(Field::Set(SetField::Label)),
            "",
            &options(),
            &[],
        );
        let Pick::Edit(edit) = &rows[0].pick else {
            panic!("a value edits")
        };
        let mut clauses = Vec::new();
        edit(&mut clauses);
        assert_eq!(
            clauses,
            [FilterClause::Set {
                field: SetField::Label,
                op: SetOp::In,
                values: vec!["l-bug".into()],
            }]
        );
        let rows = choices(
            Page::Field(Field::Set(SetField::Label)),
            "",
            &options(),
            &clauses,
        );
        assert_eq!(labels(&rows), ["Bug ✓", "Feature"]);
        edit(&mut clauses);
        assert!(clauses.is_empty(), "a second pick removes the value");
    }

    #[test]
    fn relations_and_dates_set_their_flags() {
        let rows = choices(Page::Field(Field::Relations), "no", &options(), &[]);
        assert_eq!(labels(&rows), ["Is not a sub-issue", "Has no sub-issues"]);
        let Pick::Edit(edit) = &rows[1].pick else {
            panic!("a flag edits")
        };
        let mut clauses = Vec::new();
        edit(&mut clauses);
        assert_eq!(
            clauses,
            [FilterClause::Flag {
                field: FlagField::HasSubIssues,
                value: false,
            }]
        );
        let dates = choices(Page::Field(Field::Dates), "", &options(), &clauses);
        assert_eq!(
            labels(&dates),
            [
                "Created",
                "Updated",
                "Due date",
                "Closed",
                "-- Has a due date",
                "Has no due date",
            ]
        );
    }
}
