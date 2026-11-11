//! Editing a view's filter clauses, after t3code's `kanbanFilterEdit.ts`:
//! the options each field offers, how a clause reads as a chip, and the pure
//! edits the filter bar makes (toggle a value, flip the operator, set the
//! search text, remove a clause).

use loupe_linear::filter::{
    CURRENT_CYCLE, DateField, DateOp, FilterClause, FlagField, ME, NONE, SetField, SetOp,
};
use loupe_linear::{IssueSummary, Priority, StateType, Team};

/// One value a set field can take, as the filter menus list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FieldOption {
    pub value: String,
    pub label: String,
}

fn option(value: impl Into<String>, label: impl Into<String>) -> FieldOption {
    FieldOption {
        value: value.into(),
        label: label.into(),
    }
}

const STATE_TYPES: [StateType; 6] = [
    StateType::Triage,
    StateType::Backlog,
    StateType::Unstarted,
    StateType::Started,
    StateType::Completed,
    StateType::Canceled,
];

/// The values `field` offers on `team`'s screen. Milestones come from the
/// loaded issues, since the team catalog does not list them.
pub(super) fn options(field: SetField, team: &Team, issues: &[IssueSummary]) -> Vec<FieldOption> {
    match field {
        SetField::Status => team
            .states
            .iter()
            .map(|state| option(&state.id, &state.name))
            .collect(),
        SetField::StatusType => STATE_TYPES
            .iter()
            .map(|kind| option(kind.as_str(), kind.label()))
            .collect(),
        SetField::Assignee | SetField::Creator => {
            let none = if field == SetField::Assignee {
                "No assignee"
            } else {
                "No creator"
            };
            let mut options = vec![option(ME, "Me"), option(NONE, none)];
            options.extend(
                team.members
                    .iter()
                    .map(|member| option(&member.id, &member.name)),
            );
            options
        }
        SetField::Priority => Priority::ALL
            .iter()
            .map(|priority| option(priority.to_wire().to_string(), priority.label()))
            .collect(),
        SetField::Label => {
            let mut options: Vec<FieldOption> = team
                .labels
                .iter()
                .map(|label| option(&label.id, &label.name))
                .collect();
            options.push(option(NONE, "No label"));
            options
        }
        SetField::Project => {
            let mut options: Vec<FieldOption> = team
                .projects
                .iter()
                .map(|project| option(&project.id, &project.name))
                .collect();
            options.push(option(NONE, "No project"));
            options
        }
        SetField::Milestone => {
            let mut options: Vec<FieldOption> = Vec::new();
            for milestone in issues.iter().filter_map(|issue| issue.milestone.as_ref()) {
                if options.iter().all(|known| known.value != milestone.id) {
                    options.push(option(&milestone.id, &milestone.name));
                }
            }
            options.push(option(NONE, "No milestone"));
            options
        }
        SetField::Cycle => {
            let mut options = vec![option(CURRENT_CYCLE, "Current cycle")];
            options.extend(team.cycles.iter().map(|cycle| {
                let name = cycle
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Cycle {}", cycle.number));
                option(&cycle.id, name)
            }));
            options.push(option(NONE, "No cycle"));
            options
        }
        SetField::PullRequest => vec![
            option("open", "Open"),
            option("draft", "Draft"),
            option("merged", "Merged"),
            option("closed", "Closed"),
            option(NONE, "No pull request"),
        ],
    }
}

/// The date presets the filter menu offers, after t3code's.
pub(super) const DATE_PRESETS: [(DateOp, &str); 8] = [
    (DateOp::PastDays(1), "Past day"),
    (DateOp::PastDays(7), "Past week"),
    (DateOp::PastDays(30), "Past month"),
    (DateOp::NextDays(1), "Next day"),
    (DateOp::NextDays(7), "Next week"),
    (DateOp::NextDays(30), "Next month"),
    (DateOp::IsSet, "Is set"),
    (DateOp::IsEmpty, "Is empty"),
];

fn date_text(op: DateOp) -> String {
    match op {
        DateOp::Before(day) => format!("before {}", day.format("%b %-d, %Y")),
        DateOp::After(day) => format!("after {}", day.format("%b %-d, %Y")),
        DateOp::PastDays(1) => "in the past day".into(),
        DateOp::PastDays(days) => format!("in the past {days} days"),
        DateOp::NextDays(1) => "in the next day".into(),
        DateOp::NextDays(days) => format!("in the next {days} days"),
        DateOp::IsSet => "is set".into(),
        DateOp::IsEmpty => "is empty".into(),
    }
}

/// How a clause reads on its chip, such as "Assignee is Me, Ada" or
/// "Priority is not Low".
pub(super) fn describe(clause: &FilterClause, team: &Team, issues: &[IssueSummary]) -> String {
    match clause {
        FilterClause::Set { field, op, values } => {
            let known = options(*field, team, issues);
            let names: Vec<String> = values
                .iter()
                .map(|value| {
                    known
                        .iter()
                        .find(|option| &option.value == value)
                        .map(|option| option.label.clone())
                        .unwrap_or_else(|| value.clone())
                })
                .collect();
            let verb = match (op, values.len()) {
                (SetOp::In, 1) => "is",
                (SetOp::In, _) => "is any of",
                (SetOp::NotIn, 1) => "is not",
                (SetOp::NotIn, _) => "is none of",
            };
            let shown = if names.len() > 2 {
                format!("{}, {} +{}", names[0], names[1], names.len() - 2)
            } else {
                names.join(", ")
            };
            format!("{} {verb} {shown}", field.label())
        }
        FilterClause::Date { field, op } => format!("{} {}", field.label(), date_text(*op)),
        FilterClause::Flag { field, value } => {
            if *value {
                field.label().to_owned()
            } else {
                format!("Not: {}", field.label().to_lowercase())
            }
        }
        FilterClause::Text { value } => format!("Text contains “{value}”"),
    }
}

/// Adds `value` to the `field` clause (creating an "is" clause), or removes
/// it; a clause left without values goes away.
pub(super) fn toggle_value(clauses: &mut Vec<FilterClause>, field: SetField, value: &str) {
    let position = clauses
        .iter()
        .position(|clause| matches!(clause, FilterClause::Set { field: f, .. } if *f == field));
    match position {
        Some(index) => {
            if let FilterClause::Set { values, .. } = &mut clauses[index] {
                match values.iter().position(|known| known == value) {
                    Some(at) => {
                        values.remove(at);
                    }
                    None => values.push(value.to_owned()),
                }
                if values.is_empty() {
                    clauses.remove(index);
                }
            }
        }
        None => clauses.push(FilterClause::Set {
            field,
            op: SetOp::In,
            values: vec![value.to_owned()],
        }),
    }
}

/// The values the `field` clause holds now.
pub(super) fn selected(clauses: &[FilterClause], field: SetField) -> Vec<String> {
    clauses
        .iter()
        .find_map(|clause| match clause {
            FilterClause::Set {
                field: f, values, ..
            } if *f == field => Some(values.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Switches the clause at `index` between "is" and "is not".
pub(super) fn flip_op(clauses: &mut [FilterClause], index: usize) {
    if let Some(FilterClause::Set { op, .. }) = clauses.get_mut(index) {
        *op = match op {
            SetOp::In => SetOp::NotIn,
            SetOp::NotIn => SetOp::In,
        };
    }
}

/// Sets the one date clause for `field` (replacing an earlier one).
pub(super) fn set_date(clauses: &mut Vec<FilterClause>, field: DateField, op: DateOp) {
    clauses.retain(|clause| !matches!(clause, FilterClause::Date { field: f, .. } if *f == field));
    clauses.push(FilterClause::Date { field, op });
}

/// Sets the one flag clause for `field` (replacing an earlier one).
pub(super) fn set_flag(clauses: &mut Vec<FilterClause>, field: FlagField, value: bool) {
    clauses.retain(|clause| !matches!(clause, FilterClause::Flag { field: f, .. } if *f == field));
    clauses.push(FilterClause::Flag { field, value });
}

/// Replaces the search text clause; blank text removes it.
pub(super) fn set_text(clauses: &mut Vec<FilterClause>, text: &str) {
    let text = text.trim();
    match clauses
        .iter_mut()
        .find(|clause| matches!(clause, FilterClause::Text { .. }))
    {
        Some(FilterClause::Text { value }) if !text.is_empty() => *value = text.to_owned(),
        _ => {
            clauses.retain(|clause| !matches!(clause, FilterClause::Text { .. }));
            if !text.is_empty() {
                clauses.push(FilterClause::Text {
                    value: text.to_owned(),
                });
            }
        }
    }
}

/// The search text the view's clauses hold.
pub(super) fn text_of(clauses: &[FilterClause]) -> String {
    clauses
        .iter()
        .find_map(|clause| match clause {
            FilterClause::Text { value } => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use loupe_linear::filter::{
        DateField, DateOp, FilterClause, FlagField, ME, NONE, SetField, SetOp,
    };
    use loupe_linear::{Label, StateType, Team, User, WorkflowState};

    use super::{
        describe, flip_op, options, selected, set_date, set_flag, set_text, text_of, toggle_value,
    };

    fn team() -> Team {
        let user = |id: &str, name: &str| User {
            id: id.into(),
            name: name.into(),
            display_name: name.to_lowercase(),
            avatar_url: None,
        };
        Team {
            id: "t1".into(),
            key: "ENG".into(),
            name: "Engineering".into(),
            states: vec![WorkflowState {
                id: "s1".into(),
                name: "Todo".into(),
                kind: StateType::Unstarted,
                position: 0.,
                color: "#888888".into(),
            }],
            members: vec![user("u1", "Ada"), user("u2", "Ben")],
            labels: vec![Label {
                id: "l1".into(),
                name: "Bug".into(),
                color: "#eb5757".into(),
                parent_id: None,
            }],
            projects: Vec::new(),
            cycles: Vec::new(),
            label_groups: Vec::new(),
            estimation: Default::default(),
        }
    }

    #[test]
    fn toggling_values_builds_and_removes_a_clause() {
        let mut clauses = Vec::new();
        toggle_value(&mut clauses, SetField::Assignee, ME);
        toggle_value(&mut clauses, SetField::Assignee, "u1");
        assert_eq!(selected(&clauses, SetField::Assignee), [ME, "u1"]);
        toggle_value(&mut clauses, SetField::Assignee, ME);
        toggle_value(&mut clauses, SetField::Assignee, "u1");
        assert!(clauses.is_empty());
    }

    #[test]
    fn chips_read_like_sentences() {
        let team = team();
        let mut clauses = Vec::new();
        toggle_value(&mut clauses, SetField::Assignee, ME);
        assert_eq!(describe(&clauses[0], &team, &[]), "Assignee is Me");
        toggle_value(&mut clauses, SetField::Assignee, "u1");
        toggle_value(&mut clauses, SetField::Assignee, NONE);
        assert_eq!(
            describe(&clauses[0], &team, &[]),
            "Assignee is any of Me, Ada +1"
        );
        flip_op(&mut clauses, 0);
        assert!(matches!(
            clauses[0],
            FilterClause::Set {
                op: SetOp::NotIn,
                ..
            }
        ));
        assert!(describe(&clauses[0], &team, &[]).starts_with("Assignee is none of"));
        let due = FilterClause::Date {
            field: DateField::DueDate,
            op: DateOp::NextDays(7),
        };
        assert_eq!(describe(&due, &team, &[]), "Due date in the next 7 days");
        let flag = FilterClause::Flag {
            field: FlagField::HasPullRequests,
            value: false,
        };
        assert_eq!(describe(&flag, &team, &[]), "Not: has pull requests");
    }

    #[test]
    fn a_field_has_one_date_or_flag_clause() {
        let mut clauses = Vec::new();
        set_date(&mut clauses, DateField::DueDate, DateOp::PastDays(7));
        set_date(&mut clauses, DateField::DueDate, DateOp::IsEmpty);
        set_flag(&mut clauses, FlagField::IsSubIssue, true);
        set_flag(&mut clauses, FlagField::IsSubIssue, false);
        assert_eq!(clauses.len(), 2);
        assert!(clauses.contains(&FilterClause::Date {
            field: DateField::DueDate,
            op: DateOp::IsEmpty
        }));
    }

    #[test]
    fn search_text_is_one_clause() {
        let mut clauses = Vec::new();
        set_text(&mut clauses, "  login ");
        set_text(&mut clauses, "logout");
        assert_eq!(text_of(&clauses), "logout");
        set_text(&mut clauses, "   ");
        assert!(clauses.is_empty());
    }

    #[test]
    fn people_options_start_with_me_and_none() {
        let values: Vec<String> = options(SetField::Assignee, &team(), &[])
            .into_iter()
            .map(|option| option.value)
            .collect();
        assert_eq!(values, [ME, NONE, "u1", "u2"]);
    }
}
