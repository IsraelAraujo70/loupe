//! Filter clauses, after the t3code kanban's `KanbanFilterClause`: a view
//! keeps the issues that match every clause. [`matches`] evaluates a clause
//! against an issue; [`to_issue_filter`] turns a query into Linear's
//! `IssueFilter` so filtering happens on the server.
//!
//! The server filter is a superset: a clause Linear cannot express exactly is
//! left out of it (or widened), and [`matches`] runs on every returned issue,
//! so the screen never shows an issue a clause rejects. Inside an `or`, every
//! object has one field, because Linear does not AND the fields of one object
//! there.

use std::collections::HashSet;

use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::types::{ClosedIssues, IssueQuery, IssueSummary, PullRequestState, StateType};

/// The set value meaning the API key's owner (assignee, creator).
pub const ME: &str = "me";
/// The set value meaning "no value" (no assignee, no label, no project...).
pub const NONE: &str = "none";
/// The cycle value meaning the team's active cycle.
pub const CURRENT_CYCLE: &str = "current";

/// Fields matched against a set of values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetField {
    /// Workflow state ids.
    Status,
    /// State types (`StateType::as_str`).
    StatusType,
    /// User ids, [`ME`], [`NONE`].
    Assignee,
    Creator,
    /// Linear's priority numbers as strings, `"0"` (none) to `"4"` (low).
    Priority,
    /// Label ids, [`NONE`].
    Label,
    Project,
    Milestone,
    /// Cycle ids, [`CURRENT_CYCLE`], [`NONE`].
    Cycle,
    /// `open`, `draft`, `merged`, `closed`, `unknown`, [`NONE`].
    PullRequest,
}

impl SetField {
    pub const ALL: [SetField; 10] = [
        SetField::Status,
        SetField::StatusType,
        SetField::Assignee,
        SetField::Creator,
        SetField::Priority,
        SetField::Label,
        SetField::Project,
        SetField::Milestone,
        SetField::Cycle,
        SetField::PullRequest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SetField::Status => "Status",
            SetField::StatusType => "Status type",
            SetField::Assignee => "Assignee",
            SetField::Creator => "Creator",
            SetField::Priority => "Priority",
            SetField::Label => "Label",
            SetField::Project => "Project",
            SetField::Milestone => "Milestone",
            SetField::Cycle => "Cycle",
            SetField::PullRequest => "Pull request",
        }
    }
}

/// `In`: is / is any of. `NotIn`: is not / is none of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetOp {
    In,
    NotIn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DateField {
    CreatedAt,
    UpdatedAt,
    DueDate,
    /// When the issue was completed or canceled.
    ClosedAt,
}

impl DateField {
    pub const ALL: [DateField; 4] = [
        DateField::CreatedAt,
        DateField::UpdatedAt,
        DateField::DueDate,
        DateField::ClosedAt,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateField::CreatedAt => "Created",
            DateField::UpdatedAt => "Updated",
            DateField::DueDate => "Due date",
            DateField::ClosedAt => "Closed",
        }
    }
}

/// A date condition. `Before`/`After` compare with the start of that day
/// (UTC); `PastDays`/`NextDays` take a window from now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "op", content = "value", rename_all = "camelCase")]
pub enum DateOp {
    Before(NaiveDate),
    After(NaiveDate),
    PastDays(u32),
    NextDays(u32),
    IsSet,
    IsEmpty,
}

/// Yes/no facts about an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlagField {
    IsSubIssue,
    HasSubIssues,
    HasPullRequests,
    HasDueDate,
}

impl FlagField {
    pub const ALL: [FlagField; 4] = [
        FlagField::IsSubIssue,
        FlagField::HasSubIssues,
        FlagField::HasPullRequests,
        FlagField::HasDueDate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FlagField::IsSubIssue => "Is a sub-issue",
            FlagField::HasSubIssues => "Has sub-issues",
            FlagField::HasPullRequests => "Has pull requests",
            FlagField::HasDueDate => "Has a due date",
        }
    }
}

/// One condition. A view keeps the issues that match every clause.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FilterClause {
    Set {
        field: SetField,
        op: SetOp,
        values: Vec<String>,
    },
    Date {
        field: DateField,
        #[serde(flatten)]
        op: DateOp,
    },
    Flag {
        field: FlagField,
        value: bool,
    },
    /// Case-insensitive match on the identifier, the title, and label names.
    Text {
        value: String,
    },
}

/// What evaluation needs beyond the issue itself.
#[derive(Clone, Debug, Default)]
pub struct MatchContext {
    /// The API key owner's user id, for [`ME`].
    pub me: Option<String>,
    /// Ids of the cycles active now, for [`CURRENT_CYCLE`].
    pub current_cycles: HashSet<String>,
}

fn pull_request_value(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Draft => "draft",
        PullRequestState::Merged => "merged",
        PullRequestState::Closed => "closed",
        PullRequestState::Unknown => "unknown",
    }
}

/// The filter values an issue has for a set field.
fn set_values(issue: &IssueSummary, field: SetField, context: &MatchContext) -> Vec<String> {
    let person = |id: Option<&String>| match id {
        None => vec![NONE.to_owned()],
        Some(id) if context.me.as_ref() == Some(id) => vec![id.clone(), ME.to_owned()],
        Some(id) => vec![id.clone()],
    };
    let or_none = |id: Option<&String>| vec![id.cloned().unwrap_or_else(|| NONE.to_owned())];
    match field {
        SetField::Status => vec![issue.state.id.clone()],
        SetField::StatusType => vec![issue.state.kind.as_str().to_owned()],
        SetField::Assignee => person(issue.assignee.as_ref().map(|user| &user.id)),
        SetField::Creator => person(issue.creator_id.as_ref()),
        SetField::Priority => vec![issue.priority.to_wire().to_string()],
        SetField::Label if issue.labels.is_empty() => vec![NONE.to_owned()],
        SetField::Label => issue.labels.iter().map(|label| label.id.clone()).collect(),
        SetField::Project => or_none(issue.project.as_ref().map(|project| &project.id)),
        SetField::Milestone => or_none(issue.milestone.as_ref().map(|milestone| &milestone.id)),
        SetField::Cycle => match &issue.cycle {
            None => vec![NONE.to_owned()],
            Some(cycle) if context.current_cycles.contains(&cycle.id) => {
                vec![cycle.id.clone(), CURRENT_CYCLE.to_owned()]
            }
            Some(cycle) => vec![cycle.id.clone()],
        },
        SetField::PullRequest if issue.pull_requests.is_empty() => vec![NONE.to_owned()],
        SetField::PullRequest => issue
            .pull_requests
            .iter()
            .map(|pr| pull_request_value(pr.state).to_owned())
            .collect(),
    }
}

fn start_of(day: NaiveDate) -> DateTime<Utc> {
    day.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc()
}

/// Whether `issue` satisfies `clause` at `now`.
pub fn matches(
    issue: &IssueSummary,
    clause: &FilterClause,
    context: &MatchContext,
    now: DateTime<Utc>,
) -> bool {
    match clause {
        FilterClause::Set { field, op, values } => {
            let have = set_values(issue, *field, context);
            let any = values.iter().any(|value| have.contains(value));
            match op {
                SetOp::In => any,
                SetOp::NotIn => !any,
            }
        }
        FilterClause::Date { field, op } => {
            let at = match field {
                DateField::CreatedAt => Some(issue.created_at),
                DateField::UpdatedAt => Some(issue.updated_at),
                DateField::DueDate => issue.due_date.map(start_of),
                DateField::ClosedAt => issue.closed_at,
            };
            match (op, at) {
                (DateOp::IsSet, at) => at.is_some(),
                (DateOp::IsEmpty, at) => at.is_none(),
                (_, None) => false,
                (DateOp::Before(day), Some(at)) => at < start_of(*day),
                (DateOp::After(day), Some(at)) => at > start_of(*day),
                (DateOp::PastDays(days), Some(at)) => {
                    let from = if *field == DateField::DueDate {
                        start_of(now.date_naive()) - Duration::days(i64::from(*days))
                    } else {
                        now - Duration::days(i64::from(*days))
                    };
                    from <= at && at <= now
                }
                (DateOp::NextDays(days), Some(at)) => {
                    let from = if *field == DateField::DueDate {
                        start_of(now.date_naive())
                    } else {
                        now
                    };
                    from <= at && at <= now + Duration::days(i64::from(*days))
                }
            }
        }
        FilterClause::Flag { field, value } => {
            let fact = match field {
                FlagField::IsSubIssue => issue.parent_id.is_some(),
                FlagField::HasSubIssues => issue.has_sub_issues,
                FlagField::HasPullRequests => !issue.pull_requests.is_empty(),
                FlagField::HasDueDate => issue.due_date.is_some(),
            };
            fact == *value
        }
        FilterClause::Text { value } => {
            let needle = value.trim().to_lowercase();
            needle.is_empty()
                || std::iter::once(issue.identifier.as_str())
                    .chain(std::iter::once(issue.title.as_str()))
                    .chain(issue.labels.iter().map(|label| label.name.as_str()))
                    .any(|text| text.to_lowercase().contains(&needle))
        }
    }
}

/// Whether `issue` satisfies every clause of `query`, including its closed
/// window.
pub fn matches_query(
    issue: &IssueSummary,
    query: &IssueQuery,
    context: &MatchContext,
    now: DateTime<Utc>,
) -> bool {
    let in_window = match &query.closed {
        ClosedIssues::All => true,
        ClosedIssues::None => !issue.state.kind.is_closed(),
        ClosedIssues::ClosedSince(since) => {
            !issue.state.kind.is_closed() || issue.closed_at.is_some_and(|closed| closed > *since)
        }
    };
    in_window
        && query
            .clauses
            .iter()
            .all(|clause| matches(issue, clause, context, now))
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

const CLOSED_TYPES: [StateType; 3] = [
    StateType::Completed,
    StateType::Canceled,
    StateType::Duplicate,
];

fn closed_type_names() -> Vec<&'static str> {
    CLOSED_TYPES.iter().map(|kind| kind.as_str()).collect()
}

/// One filter object, or an `or` of single-field alternatives.
fn any_of(mut alternatives: Vec<Value>) -> Option<Value> {
    match alternatives.len() {
        0 => None,
        1 => alternatives.pop(),
        _ => Some(json!({ "or": alternatives })),
    }
}

/// The server form of a nullable relation matched by ids, none, and (for a
/// user relation) me.
fn relation_in(relation: &str, values: &[String], is_user: bool) -> Option<Value> {
    let ids: Vec<&String> = values
        .iter()
        .filter(|value| *value != ME && *value != NONE)
        .collect();
    let mut alternatives = Vec::new();
    if !ids.is_empty() {
        alternatives.push(json!({ relation: { "id": { "in": ids } } }));
    }
    if is_user && values.iter().any(|value| value == ME) {
        alternatives.push(json!({ relation: { "isMe": { "eq": true } } }));
    }
    if values.iter().any(|value| value == NONE) {
        alternatives.push(json!({ relation: { "null": true } }));
    }
    any_of(alternatives)
}

/// The server filter for one clause, or `None` when Linear cannot narrow it
/// without risking to drop an issue the clause keeps.
fn clause_filter(clause: &FilterClause, now: DateTime<Utc>) -> Option<Value> {
    match clause {
        FilterClause::Set { field, op, values } => {
            if values.is_empty() {
                return None;
            }
            match (field, op) {
                (SetField::Status, SetOp::In) => {
                    Some(json!({ "state": { "id": { "in": values } } }))
                }
                (SetField::Status, SetOp::NotIn) => {
                    Some(json!({ "state": { "id": { "nin": values } } }))
                }
                (SetField::StatusType, op) => {
                    let key = if *op == SetOp::In { "in" } else { "nin" };
                    Some(json!({ "state": { "type": { key: values } } }))
                }
                (SetField::Priority, op) => {
                    let numbers: Vec<i64> = values
                        .iter()
                        .filter_map(|value| value.parse().ok())
                        .collect();
                    if numbers.len() != values.len() {
                        return None;
                    }
                    let key = if *op == SetOp::In { "in" } else { "nin" };
                    Some(json!({ "priority": { key: numbers } }))
                }
                (SetField::Assignee, SetOp::In) => relation_in("assignee", values, true),
                (SetField::Creator, SetOp::In) => relation_in("creator", values, true),
                (SetField::Project, SetOp::In) => relation_in("project", values, false),
                (SetField::Milestone, SetOp::In) => relation_in("projectMilestone", values, false),
                (SetField::Label, SetOp::In) => {
                    let ids: Vec<&String> = values.iter().filter(|value| *value != NONE).collect();
                    let mut alternatives = Vec::new();
                    if !ids.is_empty() {
                        alternatives.push(json!({ "labels": { "some": { "id": { "in": ids } } } }));
                    }
                    if values.iter().any(|value| value == NONE) {
                        alternatives.push(json!({ "labels": { "length": { "eq": 0 } } }));
                    }
                    any_of(alternatives)
                }
                (SetField::Cycle, SetOp::In) => {
                    let ids: Vec<&String> = values
                        .iter()
                        .filter(|value| *value != NONE && *value != CURRENT_CYCLE)
                        .collect();
                    let mut alternatives = Vec::new();
                    if !ids.is_empty() {
                        alternatives.push(json!({ "cycle": { "id": { "in": ids } } }));
                    }
                    if values.iter().any(|value| value == CURRENT_CYCLE) {
                        alternatives.push(json!({ "cycle": { "isActive": { "eq": true } } }));
                    }
                    if values.iter().any(|value| value == NONE) {
                        alternatives.push(json!({ "cycle": { "null": true } }));
                    }
                    any_of(alternatives)
                }
                // Negated relations and pull request states stay local: how
                // Linear treats a missing relation under `nin` is not
                // documented, and pull request status lives in metadata.
                _ => None,
            }
        }
        FilterClause::Date { field, op } => date_filter(*field, *op, now),
        FilterClause::Flag { field, value } => match field {
            FlagField::IsSubIssue => Some(json!({ "parent": { "null": !value } })),
            FlagField::HasDueDate => Some(json!({ "dueDate": { "null": !value } })),
            FlagField::HasSubIssues if *value => {
                Some(json!({ "children": { "length": { "gt": 0 } } }))
            }
            FlagField::HasSubIssues => Some(json!({ "children": { "length": { "eq": 0 } } })),
            FlagField::HasPullRequests => None,
        },
        FilterClause::Text { value } => {
            let text = value.trim();
            // The identifier is not filterable on the server, and any run of
            // letters, digits, and `-` could be part of one ("ENG" matches
            // ENG-42). Only text with another character, which no identifier
            // contains, is narrowed on the server.
            if !text
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || c == '-'))
            {
                return None;
            }
            Some(json!({ "or": [
                { "title": { "containsIgnoreCase": text } },
                { "labels": { "some": { "name": { "containsIgnoreCase": text } } } },
            ] }))
        }
    }
}

fn date_filter(field: DateField, op: DateOp, now: DateTime<Utc>) -> Option<Value> {
    let day = |date: NaiveDate| date.format("%Y-%m-%d").to_string();
    let comparator = match (field, op) {
        (DateField::DueDate, DateOp::Before(date)) => json!({ "lt": day(date) }),
        (DateField::DueDate, DateOp::After(date)) => json!({ "gt": day(date) }),
        (DateField::DueDate, DateOp::PastDays(days)) => json!({
            "gte": day(now.date_naive() - Duration::days(i64::from(days))),
            "lte": day(now.date_naive()),
        }),
        (DateField::DueDate, DateOp::NextDays(days)) => json!({
            "gte": day(now.date_naive()),
            "lte": day(now.date_naive() + Duration::days(i64::from(days))),
        }),
        (_, DateOp::Before(date)) => json!({ "lt": timestamp(start_of(date)) }),
        (_, DateOp::After(date)) => json!({ "gt": timestamp(start_of(date)) }),
        (_, DateOp::PastDays(days)) => json!({
            "gte": timestamp(now - Duration::days(i64::from(days))),
        }),
        (_, DateOp::NextDays(days)) => json!({
            "gte": timestamp(now),
            "lte": timestamp(now + Duration::days(i64::from(days))),
        }),
        (DateField::DueDate, DateOp::IsSet) => json!({ "null": false }),
        (DateField::DueDate, DateOp::IsEmpty) => json!({ "null": true }),
        (DateField::ClosedAt, DateOp::IsSet) => {
            return Some(json!({ "or": [
                { "completedAt": { "null": false } },
                { "canceledAt": { "null": false } },
            ] }));
        }
        (DateField::ClosedAt, DateOp::IsEmpty) => {
            return Some(json!({ "and": [
                { "completedAt": { "null": true } },
                { "canceledAt": { "null": true } },
            ] }));
        }
        // Created and updated are always set: local evaluation decides.
        (_, DateOp::IsSet | DateOp::IsEmpty) => return None,
    };
    Some(match field {
        DateField::CreatedAt => json!({ "createdAt": comparator }),
        DateField::UpdatedAt => json!({ "updatedAt": comparator }),
        DateField::DueDate => json!({ "dueDate": comparator }),
        DateField::ClosedAt => json!({ "or": [
            { "completedAt": comparator.clone() },
            { "canceledAt": comparator },
        ] }),
    })
}

/// Linear's `IssueFilter` for one team's issues matching `query`: a superset
/// of what [`matches_query`] keeps.
pub fn to_issue_filter(team_id: &str, query: &IssueQuery, now: DateTime<Utc>) -> Value {
    issue_filter(Some(team_id), query, now)
}

/// Like [`to_issue_filter`], for one team or, with `None`, every team.
pub fn issue_filter(team_id: Option<&str>, query: &IssueQuery, now: DateTime<Utc>) -> Value {
    let scope = team_id.map(|team_id| json!({ "team": { "id": { "eq": team_id } } }));
    scoped_filter(scope, query, now)
}

/// Linear's `IssueFilter` for one project's issues matching `query`, from
/// every team the project spans: a superset of what [`matches_query`] keeps.
pub fn to_project_issue_filter(project_id: &str, query: &IssueQuery, now: DateTime<Utc>) -> Value {
    let scope = json!({ "project": { "id": { "eq": project_id } } });
    scoped_filter(Some(scope), query, now)
}

/// `query` as an `IssueFilter`, and-ed with `scope` when there is one.
fn scoped_filter(scope: Option<Value>, query: &IssueQuery, now: DateTime<Utc>) -> Value {
    let mut parts: Vec<Value> = scope.into_iter().collect();
    match &query.closed {
        ClosedIssues::All => {}
        ClosedIssues::None => {
            parts.push(json!({ "state": { "type": { "nin": closed_type_names() } } }));
        }
        ClosedIssues::ClosedSince(since) => parts.push(json!({ "or": [
            { "state": { "type": { "nin": closed_type_names() } } },
            { "completedAt": { "gt": timestamp(*since) } },
            { "canceledAt": { "gt": timestamp(*since) } },
        ] })),
    }
    parts.extend(
        query
            .clauses
            .iter()
            .filter_map(|clause| clause_filter(clause, now)),
    );
    json!({ "and": parts })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{DateTime, NaiveDate, Utc};
    use serde_json::{Value, json};

    use super::{
        CURRENT_CYCLE, DateField, DateOp, FilterClause, FlagField, ME, MatchContext, NONE,
        SetField, SetOp, matches, matches_query, to_issue_filter,
    };
    use crate::types::{
        ClosedIssues, CycleRef, IssueQuery, IssueSummary, Label, Priority, PullRequestLink,
        PullRequestState, StateRef, StateType, TeamRef, User,
    };

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .to_utc()
    }

    fn now() -> DateTime<Utc> {
        at("2026-11-10T12:00:00Z")
    }

    fn issue() -> IssueSummary {
        IssueSummary {
            id: "i1".into(),
            identifier: "ENG-42".into(),
            title: "Fix login redirect".into(),
            priority: Priority::High,
            sort_order: 0.,
            state: StateRef {
                id: "s-doing".into(),
                name: "In Progress".into(),
                kind: StateType::Started,
                color: "#f2c94c".into(),
            },
            team: TeamRef {
                id: "t1".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: Some(User {
                id: "u-me".into(),
                name: "Ada".into(),
                display_name: "ada".into(),
                avatar_url: None,
            }),
            labels: vec![Label {
                id: "l-bug".into(),
                name: "Bug".into(),
                color: "#eb5757".into(),
                parent_id: None,
            }],
            project: None,
            cycle: Some(CycleRef {
                id: "c41".into(),
                number: 41,
                name: None,
            }),
            estimate: None,
            due_date: NaiveDate::from_ymd_opt(2026, 11, 12),
            url: "https://linear.app/acme/issue/ENG-42".into(),
            branch_name: String::new(),
            created_at: at("2026-10-01T09:00:00Z"),
            updated_at: at("2026-11-09T09:00:00Z"),
            creator_id: Some("u-ben".into()),
            parent_id: None,
            has_sub_issues: true,
            milestone: None,
            closed_at: None,
            pull_requests: vec![PullRequestLink {
                owner: "acme".into(),
                repo: "web".into(),
                number: 7,
                state: PullRequestState::Open,
            }],
            pull_requests_complete: true,
        }
    }

    fn context() -> MatchContext {
        MatchContext {
            me: Some("u-me".into()),
            current_cycles: HashSet::from(["c41".to_owned()]),
        }
    }

    fn set(field: SetField, op: SetOp, values: &[&str]) -> FilterClause {
        FilterClause::Set {
            field,
            op,
            values: values.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    fn keeps(clause: FilterClause) -> bool {
        matches(&issue(), &clause, &context(), now())
    }

    #[test]
    fn set_clauses_match_ids_and_special_values() {
        assert!(keeps(set(SetField::Assignee, SetOp::In, &[ME])));
        assert!(keeps(set(SetField::Assignee, SetOp::In, &["u-me"])));
        assert!(!keeps(set(SetField::Assignee, SetOp::In, &[NONE])));
        assert!(keeps(set(SetField::Creator, SetOp::NotIn, &[ME])));
        assert!(keeps(set(SetField::Project, SetOp::In, &[NONE])));
        assert!(keeps(set(SetField::Cycle, SetOp::In, &[CURRENT_CYCLE])));
        assert!(keeps(set(SetField::Priority, SetOp::In, &["1", "2"])));
        assert!(!keeps(set(SetField::Priority, SetOp::NotIn, &["2"])));
        assert!(keeps(set(SetField::StatusType, SetOp::In, &["started"])));
        assert!(keeps(set(SetField::Label, SetOp::In, &["l-bug", "l-x"])));
        assert!(!keeps(set(SetField::Label, SetOp::NotIn, &["l-bug"])));
        assert!(keeps(set(SetField::PullRequest, SetOp::In, &["open"])));
        assert!(!keeps(set(SetField::PullRequest, SetOp::In, &[NONE])));
    }

    #[test]
    fn unlabeled_issue_matches_label_none() {
        let mut bare = issue();
        bare.labels.clear();
        let clause = set(SetField::Label, SetOp::In, &[NONE]);
        assert!(matches(&bare, &clause, &context(), now()));
    }

    #[test]
    fn date_clauses() {
        let date = |field, op| keeps(FilterClause::Date { field, op });
        let day = |d| NaiveDate::from_ymd_opt(2026, 11, d).expect("valid day");
        assert!(date(DateField::DueDate, DateOp::NextDays(3)));
        assert!(!date(DateField::DueDate, DateOp::NextDays(1)));
        assert!(date(DateField::DueDate, DateOp::Before(day(13))));
        assert!(date(DateField::UpdatedAt, DateOp::PastDays(2)));
        assert!(!date(DateField::CreatedAt, DateOp::PastDays(7)));
        assert!(date(DateField::CreatedAt, DateOp::IsSet));
        assert!(date(DateField::ClosedAt, DateOp::IsEmpty));
        assert!(!date(DateField::ClosedAt, DateOp::After(day(1))));
    }

    #[test]
    fn flag_and_text_clauses() {
        let flag = |field, value| keeps(FilterClause::Flag { field, value });
        assert!(flag(FlagField::HasSubIssues, true));
        assert!(flag(FlagField::IsSubIssue, false));
        assert!(flag(FlagField::HasPullRequests, true));
        assert!(flag(FlagField::HasDueDate, true));
        let text = |value: &str| {
            keeps(FilterClause::Text {
                value: value.into(),
            })
        };
        assert!(text("eng-4"));
        assert!(text("LOGIN"));
        assert!(text("bug"));
        assert!(!text("payments"));
    }

    #[test]
    fn closed_window_drops_old_closed_issues() {
        let mut done = issue();
        done.state.kind = StateType::Completed;
        done.closed_at = Some(at("2026-10-01T00:00:00Z"));
        let query = |closed| IssueQuery {
            clauses: Vec::new(),
            closed,
        };
        let since = ClosedIssues::ClosedSince(at("2026-11-01T00:00:00Z"));
        assert!(!matches_query(
            &done,
            &query(since.clone()),
            &context(),
            now()
        ));
        assert!(!matches_query(
            &done,
            &query(ClosedIssues::None),
            &context(),
            now()
        ));
        assert!(matches_query(
            &done,
            &query(ClosedIssues::All),
            &context(),
            now()
        ));
        assert!(matches_query(&issue(), &query(since), &context(), now()));
    }

    /// Linear ORs the fields of one object inside an `or`, so every object
    /// in an `or` array must have exactly one field.
    fn assert_single_field_ors(value: &Value) {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if key == "or" {
                        for branch in child.as_array().expect("or is an array") {
                            let fields = branch.as_object().expect("or branch is an object");
                            assert_eq!(fields.len(), 1, "multi-field or branch: {branch}");
                        }
                    }
                    assert_single_field_ors(child);
                }
            }
            Value::Array(items) => items.iter().for_each(assert_single_field_ors),
            _ => {}
        }
    }

    fn every_clause() -> Vec<FilterClause> {
        let mut clauses = Vec::new();
        for field in SetField::ALL {
            for op in [SetOp::In, SetOp::NotIn] {
                clauses.push(set(field, op, &["x1", ME, NONE, CURRENT_CYCLE, "2"]));
            }
        }
        let day = NaiveDate::from_ymd_opt(2026, 11, 1).expect("valid day");
        for field in DateField::ALL {
            for op in [
                DateOp::Before(day),
                DateOp::After(day),
                DateOp::PastDays(7),
                DateOp::NextDays(7),
                DateOp::IsSet,
                DateOp::IsEmpty,
            ] {
                clauses.push(FilterClause::Date { field, op });
            }
        }
        for field in FlagField::ALL {
            for value in [true, false] {
                clauses.push(FilterClause::Flag { field, value });
            }
        }
        clauses.push(FilterClause::Text {
            value: "login".into(),
        });
        clauses
    }

    #[test]
    fn server_filter_never_puts_two_fields_in_one_or_branch() {
        let query = IssueQuery {
            clauses: every_clause(),
            closed: ClosedIssues::ClosedSince(now()),
        };
        assert_single_field_ors(&to_issue_filter("t1", &query, now()));
    }

    #[test]
    fn server_filter_translates_what_linear_can_take() {
        let query = IssueQuery {
            clauses: vec![
                set(SetField::Assignee, SetOp::In, &[ME, NONE]),
                set(SetField::Priority, SetOp::In, &["1", "2"]),
                set(SetField::Label, SetOp::In, &["l-bug"]),
                set(SetField::Assignee, SetOp::NotIn, &["u-x"]),
                FilterClause::Flag {
                    field: FlagField::IsSubIssue,
                    value: false,
                },
                FilterClause::Text {
                    value: "eng-42".into(),
                },
            ],
            closed: ClosedIssues::None,
        };
        assert_eq!(
            to_issue_filter("t1", &query, now()),
            json!({ "and": [
                { "team": { "id": { "eq": "t1" } } },
                { "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } },
                { "or": [
                    { "assignee": { "isMe": { "eq": true } } },
                    { "assignee": { "null": true } },
                ] },
                { "priority": { "in": [1, 2] } },
                { "labels": { "some": { "id": { "in": ["l-bug"] } } } },
                { "parent": { "null": true } },
            ] })
        );
    }

    #[test]
    fn text_that_could_be_an_identifier_stays_local() {
        let only_team = |value: &str| {
            let query = IssueQuery {
                clauses: vec![FilterClause::Text {
                    value: value.into(),
                }],
                closed: ClosedIssues::All,
            };
            to_issue_filter("t1", &query, now())["and"]
                .as_array()
                .map_or(0, Vec::len)
                == 1
        };
        // ENG-42's identifier contains each of these, so the server must not
        // narrow by title or labels.
        for text in ["ENG", "eng-4", "42", "login"] {
            assert!(only_team(text), "{text} must stay local");
        }
        assert!(
            !only_team("fix login"),
            "a space never appears in an identifier"
        );
        let mut eng = issue();
        eng.title = "Fix login".into();
        assert!(matches(
            &eng,
            &FilterClause::Text {
                value: "ENG".into()
            },
            &context(),
            now()
        ));
    }

    #[test]
    fn clauses_round_trip_through_json() {
        let clauses = every_clause();
        let text = serde_json::to_string(&clauses).expect("serialize");
        let back: Vec<FilterClause> = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, clauses);
        let date: FilterClause = serde_json::from_value(json!({
            "kind": "date", "field": "dueDate", "op": "nextDays", "value": 7
        }))
        .expect("date clause");
        assert_eq!(
            date,
            FilterClause::Date {
                field: DateField::DueDate,
                op: DateOp::NextDays(7)
            }
        );
    }
}
