//! "Add Filter…" on the Reviews screen, after the Linear board's menu: a
//! search field over the filter fields, where a field opens its values and
//! each value toggles in place. On the first page, typing also finds the
//! values of every field, so "approved" finds Review: Approved.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::reviews::{self, Clause, Field, FieldOption};

use super::InboxView;
use crate::shell::svg_icon;

/// Values listed for a search on the first page, at most.
const MAX_VALUE_MATCHES: usize = 30;

/// The first page; `None` separates the people fields from the state ones.
const FIELDS: [Option<Field>; 11] = [
    Some(Field::Org),
    Some(Field::Repo),
    Some(Field::Author),
    Some(Field::Assignee),
    Some(Field::Reviewer),
    Some(Field::Label),
    None,
    Some(Field::Status),
    Some(Field::Review),
    Some(Field::Checks),
    Some(Field::Merge),
];

pub(super) fn field_icon(field: Field) -> IconName {
    match field {
        Field::Org => IconName::Building2,
        Field::Repo => IconName::BookOpen,
        Field::Author => IconName::UserPen,
        Field::Assignee => IconName::UserRound,
        Field::Reviewer => IconName::Eye,
        Field::Label => IconName::Tag,
        Field::Status => IconName::GitPullRequest,
        Field::Review => IconName::CircleCheck,
        Field::Checks => IconName::CircleDot,
        Field::Merge => IconName::GitMerge,
    }
}

/// A page of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Fields,
    Field(Field),
}

/// What a row does when picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    Open(Field),
    Toggle(Field, String),
}

/// One row of the menu.
#[derive(Clone, Debug)]
pub(super) struct Choice {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub checked: bool,
    /// The values a field row already has in the filter.
    pub detail: Option<SharedString>,
    /// A thin rule above the row, between groups.
    pub separated: bool,
    pub pick: Pick,
}

/// The options of every field, computed once per render.
pub(super) struct FilterOptions {
    by_field: Vec<(Field, Vec<FieldOption>)>,
}

impl FilterOptions {
    pub(super) fn new<'a>(
        prs: impl Iterator<Item = &'a loupe_github::PrSummary> + Clone,
        clauses: &[Clause],
    ) -> Self {
        Self {
            by_field: Field::ALL
                .iter()
                .map(|field| (*field, reviews::options(*field, prs.clone(), clauses)))
                .collect(),
        }
    }

    pub(super) fn of(&self, field: Field) -> &[FieldOption] {
        self.by_field
            .iter()
            .find(|(known, _)| *known == field)
            .map(|(_, options)| options.as_slice())
            .unwrap_or_default()
    }
}

fn open(field: Field, separated: bool, clauses: &[Clause]) -> Choice {
    // A field that is part of the filter says so: a check, and its values.
    let applied: Vec<&str> = clauses
        .iter()
        .filter(|clause| clause.field == field)
        .flat_map(|clause| clause.values.iter().map(String::as_str))
        .collect();
    Choice {
        id: format!("reviews-filter-field-{field:?}").into(),
        label: field.label().into(),
        icon: Some(field_icon(field)),
        checked: !applied.is_empty(),
        detail: (!applied.is_empty()).then(|| applied.join(", ").into()),
        separated,
        pick: Pick::Open(field),
    }
}

fn value(field: Field, option: &FieldOption, clauses: &[Clause], qualified: bool) -> Choice {
    let label = if qualified {
        format!("{}: {}", field.label(), option.label)
    } else {
        option.label.clone()
    };
    Choice {
        id: format!("reviews-filter-value-{field:?}-{}", option.value).into(),
        label: label.into(),
        icon: qualified.then(|| field_icon(field)),
        checked: reviews::selected(clauses, field).contains(&option.value),
        detail: None,
        separated: false,
        pick: Pick::Toggle(field, option.value.clone()),
    }
}

/// The rows `page` shows for `query`, matched case-insensitively against
/// each label.
pub(super) fn choices(
    page: Page,
    query: &str,
    options: &FilterOptions,
    clauses: &[Clause],
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
                        rows.push(open(field, gap && !rows.is_empty(), clauses));
                        gap = false;
                    }
                    Some(_) => {}
                }
            }
            if !query.is_empty() {
                let values: Vec<Choice> = Field::ALL
                    .iter()
                    .flat_map(|field| {
                        options
                            .of(*field)
                            .iter()
                            .filter(|option| matches(&option.label))
                            .map(|option| value(*field, option, clauses, true))
                    })
                    .take(MAX_VALUE_MATCHES)
                    .collect();
                let first = rows.is_empty();
                rows.extend(values.into_iter().enumerate().map(|(ix, choice)| Choice {
                    separated: ix == 0 && !first,
                    ..choice
                }));
            }
            rows
        }
        Page::Field(field) => options
            .of(field)
            .iter()
            .filter(|option| matches(&option.label))
            .map(|option| value(field, option, clauses, false))
            .collect(),
    }
}

/// The menu's view: the search field and the rows of the current page. The
/// Reviews view owns it and shows it in the filter button's popover.
pub(super) struct FilterMenu {
    inbox: WeakEntity<InboxView>,
    query: Entity<InputState>,
    page: Page,
    highlighted: usize,
    /// Focus the search field on the next render, once the popover is up.
    focus_query: bool,
    _query_events: Subscription,
}

impl FilterMenu {
    pub(super) fn new(
        inbox: WeakEntity<InboxView>,
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
            inbox,
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

    fn current_choices(&self, cx: &App) -> Vec<Choice> {
        let Some(inbox) = self.inbox.upgrade() else {
            return Vec::new();
        };
        let inbox = inbox.read(cx);
        let options = inbox.filter_options();
        let query = self.query.read(cx).value();
        choices(self.page, &query, &options, &inbox.page.draft)
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.current_choices(cx).into_iter().nth(index) else {
            return;
        };
        match choice.pick {
            Pick::Open(field) => self.go_to(Page::Field(field), window, cx),
            Pick::Toggle(field, value) => {
                self.inbox
                    .update(cx, |inbox, cx| {
                        inbox.edit_draft(|draft| reviews::toggle_value(draft, field, &value), cx)
                    })
                    .ok();
                self.highlighted = index;
                cx.notify();
            }
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.current_choices(cx).len();
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
        let rows = self.current_choices(cx);
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
                                detail: choice.detail,
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
            .debug_selector(|| "reviews-filter-menu".into())
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
                        row.child(svg_icon(IconName::Search, px(16.), muted))
                    })
                    .when_some(back, |row, field| {
                        row.child(
                            div()
                                .id("reviews-filter-back")
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
                                .child(svg_icon(IconName::ChevronLeft, px(16.), muted)),
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
                    .id("reviews-filter-rows")
                    .max_h(px(420.))
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
    use loupe_github::reviews::{Clause, Field, FieldOption, Op};

    use super::{Choice, FilterOptions, Page, Pick, choices};

    fn options() -> FilterOptions {
        let option = |value: &str| FieldOption {
            value: value.into(),
            label: value.into(),
        };
        FilterOptions {
            by_field: vec![
                (Field::Org, vec![option("acme"), option("hooli-inc")]),
                (
                    Field::Review,
                    vec![FieldOption {
                        value: "approved".into(),
                        label: "Approved".into(),
                    }],
                ),
            ],
        }
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
    fn the_first_page_lists_people_then_state_fields() {
        let rows = choices(Page::Fields, "", &options(), &[]);
        assert_eq!(
            labels(&rows),
            [
                "Organization",
                "Repository",
                "Author",
                "Assignee",
                "Reviewer",
                "Label",
                "-- Status",
                "Review",
                "Checks",
                "Merge",
            ]
        );
    }

    #[test]
    fn typing_finds_values_of_every_field_and_marks_picked_ones() {
        let clauses = [Clause {
            field: Field::Review,
            op: Op::In,
            values: vec!["approved".into()],
        }];
        let rows = choices(Page::Fields, "appro", &options(), &clauses);
        assert_eq!(labels(&rows), ["Review: Approved ✓"]);
        assert_eq!(rows[0].pick, Pick::Toggle(Field::Review, "approved".into()));
        let rows = choices(Page::Field(Field::Org), "hoo", &options(), &[]);
        assert_eq!(labels(&rows), ["hooli-inc"]);
    }
}
