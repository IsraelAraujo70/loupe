//! Builders for the GraphQL variables each document takes. Pure functions, so
//! the schema gate and the live validation gate can build the same variables
//! the client sends.

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::types::{
    CustomViewUpdate, IssueQuery, IssueUpdate, NewCustomView, NewIssue, ProjectChange, StateType,
};

/// How many identifiers one `IssuesByIdentifiers` request names.
pub const IDENTIFIER_CHUNK: usize = 50;

const CLOSED_TYPES: [StateType; 3] = [
    StateType::Completed,
    StateType::Canceled,
    StateType::Duplicate,
];

fn closed_type_names() -> Vec<&'static str> {
    CLOSED_TYPES.iter().map(|kind| kind.as_str()).collect()
}

fn open_states() -> Value {
    json!({ "type": { "nin": closed_type_names() } })
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Variables for the `Team` document.
pub fn team(team_id: &str, now: DateTime<Utc>) -> Value {
    json!({ "id": team_id, "teamId": team_id, "now": timestamp(now) })
}

/// The `IssueFilter` for `MyOpenIssues`: everything not closed.
pub fn my_open_issues_filter() -> Value {
    json!({ "state": open_states() })
}

/// Variables for `MyOpenIssues`.
pub fn my_open_issues(after: Option<&str>) -> Value {
    json!({ "filter": my_open_issues_filter(), "after": after })
}

/// Variables for `TeamIssues`.
pub fn team_issues(
    team_id: &str,
    query: &IssueQuery,
    after: Option<&str>,
    now: DateTime<Utc>,
) -> Value {
    issues(Some(team_id), query, after, now)
}

/// Variables for `TeamIssues` over one team or, with `None`, every team.
pub fn issues(
    team_id: Option<&str>,
    query: &IssueQuery,
    after: Option<&str>,
    now: DateTime<Utc>,
) -> Value {
    json!({ "filter": crate::filter::issue_filter(team_id, query, now), "after": after })
}

/// The `input` of `CustomViewCreate`.
pub fn custom_view_create_input(view: &NewCustomView) -> Value {
    let mut input = json!({
        "name": view.name,
        "shared": view.shared,
        "filterData": view.filter_data,
    });
    if let Some(team_id) = &view.team_id {
        input["teamId"] = json!(team_id);
    }
    input
}

/// The `input` of `CustomViewUpdate`: only what changes.
pub fn custom_view_update_input(update: &CustomViewUpdate) -> Value {
    let mut input = json!({});
    if let Some(name) = &update.name {
        input["name"] = json!(name);
    }
    if let Some(filter_data) = &update.filter_data {
        input["filterData"] = filter_data.clone();
    }
    input
}

/// Variables for `ViewPreferencesSave`: the key owner's settings for one
/// custom view.
pub fn view_preferences_save(view_id: &str, preferences: &Value) -> Value {
    json!({ "input": {
        "type": "user",
        "viewType": "customView",
        "customViewId": view_id,
        "preferences": preferences,
    } })
}

/// Variables for `TeamIssues` when it reads one project's issues.
pub fn project_issues(
    project_id: &str,
    query: &IssueQuery,
    after: Option<&str>,
    now: DateTime<Utc>,
) -> Value {
    json!({
        "filter": crate::filter::to_project_issue_filter(project_id, query, now),
        "after": after,
    })
}

/// The `ProjectUpdateInput` for `change`: only the fields it sets.
pub fn project_update_input(change: &ProjectChange) -> Value {
    let day =
        |date: &Option<NaiveDate>| json!(date.map(|date| date.format("%Y-%m-%d").to_string()));
    let mut input = Map::new();
    if let Some(status_id) = &change.status_id {
        input.insert("statusId".into(), json!(status_id));
    }
    if let Some(lead_id) = &change.lead_id {
        input.insert("leadId".into(), json!(lead_id));
    }
    if let Some(member_ids) = &change.member_ids {
        input.insert("memberIds".into(), json!(member_ids));
    }
    if let Some(team_ids) = &change.team_ids {
        input.insert("teamIds".into(), json!(team_ids));
    }
    if let Some(label_ids) = &change.label_ids {
        input.insert("labelIds".into(), json!(label_ids));
    }
    if let Some(start_date) = &change.start_date {
        input.insert("startDate".into(), day(start_date));
    }
    if let Some(target_date) = &change.target_date {
        input.insert("targetDate".into(), day(target_date));
    }
    Value::Object(input)
}

/// Variables for `CustomViews`.
pub fn custom_views(after: Option<&str>) -> Value {
    json!({ "after": after })
}

/// Variables for `Favorites`.
pub fn favorites(after: Option<&str>) -> Value {
    json!({ "after": after })
}

/// Variables for `Projects`: with `team_id`, only the projects that team can
/// see.
pub fn projects(team_id: Option<&str>, after: Option<&str>) -> Value {
    let filter = team_id.map(|id| json!({ "accessibleTeams": { "id": { "eq": id } } }));
    json!({ "after": after, "filter": filter })
}

/// The `IssueFilter` for the issues of the team whose key is `key`, in any
/// case (`eng` names `ENG`).
pub fn team_key_filter(key: &str) -> Value {
    json!({ "team": { "key": { "eq": key.trim().to_ascii_uppercase() } } })
}

/// Splits `ENG-123` into its team key and number; `None` when it is not an
/// identifier.
pub fn parse_identifier(identifier: &str) -> Option<(String, u64)> {
    let (key, number) = identifier.trim().rsplit_once('-')?;
    let valid_key = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric());
    if !valid_key {
        return None;
    }
    let number: u64 = number.parse().ok()?;
    Some((key.to_ascii_uppercase(), number))
}

/// Variables for `IssuesByIdentifiers`: an `or` over `(team key, number)`
/// pairs. Each pair is an explicit `and`: inside an `or`, Linear does not AND
/// the fields of one object, so `{ team, number }` there matches every issue
/// of the team (seen live on 2026-11-10).
pub fn issues_by_identifiers(pairs: &[(String, u64)]) -> Value {
    let any: Vec<Value> = pairs
        .iter()
        .map(|(key, number)| {
            json!({ "and": [
                { "team": { "key": { "eq": key } } },
                { "number": { "eq": number } },
            ] })
        })
        .collect();
    json!({ "filter": { "or": any }, "first": pairs.len().max(1) })
}

/// `IssueUpdateInput`: unset fields are left out, cleared fields are `null`.
pub fn issue_update_input(update: &IssueUpdate) -> Value {
    let mut input = Map::new();
    if let Some(state_id) = &update.state_id {
        input.insert("stateId".into(), json!(state_id));
    }
    if let Some(priority) = update.priority {
        input.insert("priority".into(), json!(priority.to_wire()));
    }
    if let Some(assignee_id) = &update.assignee_id {
        input.insert("assigneeId".into(), json!(assignee_id));
    }
    if let Some(label_ids) = &update.label_ids {
        input.insert("labelIds".into(), json!(label_ids));
    }
    if !update.added_label_ids.is_empty() {
        input.insert("addedLabelIds".into(), json!(update.added_label_ids));
    }
    if !update.removed_label_ids.is_empty() {
        input.insert("removedLabelIds".into(), json!(update.removed_label_ids));
    }
    if let Some(estimate) = update.estimate {
        input.insert("estimate".into(), json!(estimate.map(|e| e.round() as i64)));
    }
    if let Some(due_date) = update.due_date {
        input.insert(
            "dueDate".into(),
            json!(due_date.map(|date| date.format("%Y-%m-%d").to_string())),
        );
    }
    if let Some(project_id) = &update.project_id {
        input.insert("projectId".into(), json!(project_id));
    }
    if let Some(cycle_id) = &update.cycle_id {
        input.insert("cycleId".into(), json!(cycle_id));
    }
    if let Some(sort_order) = update.sort_order {
        input.insert("sortOrder".into(), json!(sort_order));
    }
    if let Some(title) = &update.title {
        input.insert("title".into(), json!(title));
    }
    if let Some(description) = &update.description {
        input.insert("description".into(), json!(description));
    }
    Value::Object(input)
}

/// `IssueCreateInput`: unset fields are left out.
pub fn issue_create_input(new: &NewIssue) -> Value {
    let mut input = Map::new();
    input.insert("teamId".into(), json!(new.team_id));
    input.insert("title".into(), json!(new.title));
    if let Some(description) = &new.description {
        input.insert("description".into(), json!(description));
    }
    if let Some(state_id) = &new.state_id {
        input.insert("stateId".into(), json!(state_id));
    }
    if let Some(priority) = new.priority {
        input.insert("priority".into(), json!(priority.to_wire()));
    }
    if let Some(assignee_id) = &new.assignee_id {
        input.insert("assigneeId".into(), json!(assignee_id));
    }
    if !new.label_ids.is_empty() {
        input.insert("labelIds".into(), json!(new.label_ids));
    }
    if let Some(project_id) = &new.project_id {
        input.insert("projectId".into(), json!(project_id));
    }
    if let Some(cycle_id) = &new.cycle_id {
        input.insert("cycleId".into(), json!(cycle_id));
    }
    if let Some(estimate) = new.estimate {
        input.insert("estimate".into(), json!(estimate));
    }
    if let Some(due_date) = new.due_date {
        input.insert(
            "dueDate".into(),
            json!(due_date.format("%Y-%m-%d").to_string()),
        );
    }
    Value::Object(input)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use serde_json::json;

    use super::{
        issue_create_input, issue_update_input, issues_by_identifiers, my_open_issues_filter,
        parse_identifier,
    };
    use crate::types::{IssueUpdate, NewIssue, Priority};

    #[test]
    fn my_open_issues_excludes_closed_types() {
        assert_eq!(
            my_open_issues_filter(),
            json!({ "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } })
        );
    }

    #[test]
    fn parses_identifiers() {
        assert_eq!(parse_identifier("eng-12"), Some(("ENG".into(), 12)));
        assert_eq!(parse_identifier(" DES-7 "), Some(("DES".into(), 7)));
        assert_eq!(parse_identifier("ENG-"), None);
        assert_eq!(parse_identifier("ENG-x"), None);
        assert_eq!(parse_identifier("123"), None);
        assert_eq!(parse_identifier("-5"), None);
    }

    #[test]
    fn identifiers_become_an_or_of_key_and_number() {
        let variables = issues_by_identifiers(&[("ENG".into(), 1), ("DES".into(), 2)]);
        assert_eq!(
            variables,
            json!({
                "filter": { "or": [
                    { "and": [{ "team": { "key": { "eq": "ENG" } } }, { "number": { "eq": 1 } }] },
                    { "and": [{ "team": { "key": { "eq": "DES" } } }, { "number": { "eq": 2 } }] },
                ] },
                "first": 2,
            })
        );
    }

    #[test]
    fn update_omits_unset_and_nulls_cleared_fields() {
        let update = IssueUpdate {
            state_id: Some("s1".into()),
            priority: Some(Priority::High),
            assignee_id: Some(None),
            label_ids: Some(vec![]),
            added_label_ids: Vec::new(),
            removed_label_ids: Vec::new(),
            estimate: Some(Some(3.0)),
            due_date: Some(Some(NaiveDate::from_ymd_opt(2026, 12, 5).unwrap())),
            project_id: Some(None),
            cycle_id: Some(Some("c1".into())),
            title: None,
            description: Some("body".into()),
            sort_order: Some(-12.5),
        };
        assert_eq!(
            issue_update_input(&update),
            json!({
                "stateId": "s1",
                "priority": 2,
                "assigneeId": null,
                "labelIds": [],
                "estimate": 3,
                "dueDate": "2026-12-05",
                "projectId": null,
                "cycleId": "c1",
                "sortOrder": -12.5,
                "description": "body",
            })
        );
    }

    #[test]
    fn label_deltas_are_sent_only_when_set() {
        let update = IssueUpdate {
            added_label_ids: vec!["l1".into()],
            removed_label_ids: vec!["l2".into()],
            ..IssueUpdate::default()
        };
        assert_eq!(
            issue_update_input(&update),
            json!({ "addedLabelIds": ["l1"], "removedLabelIds": ["l2"] })
        );
    }

    #[test]
    fn update_clears_estimate_and_due_date_with_null() {
        let update = IssueUpdate {
            estimate: Some(None),
            due_date: Some(None),
            ..IssueUpdate::default()
        };
        assert_eq!(
            issue_update_input(&update),
            json!({ "estimate": null, "dueDate": null })
        );
        assert_eq!(issue_update_input(&IssueUpdate::default()), json!({}));
    }

    #[test]
    fn new_issue_omits_unset_fields() {
        let new = NewIssue {
            team_id: "t1".into(),
            title: "Title".into(),
            ..NewIssue::default()
        };
        assert_eq!(
            issue_create_input(&new),
            json!({ "teamId": "t1", "title": "Title" })
        );
        let full = NewIssue {
            description: Some("d".into()),
            state_id: Some("s".into()),
            priority: Some(Priority::Low),
            assignee_id: Some("u".into()),
            label_ids: vec!["l".into()],
            project_id: Some("p".into()),
            cycle_id: Some("c".into()),
            estimate: Some(0),
            due_date: Some(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()),
            ..new
        };
        assert_eq!(
            issue_create_input(&full),
            json!({
                "teamId": "t1", "title": "Title", "description": "d", "stateId": "s",
                "priority": 4, "assigneeId": "u", "labelIds": ["l"], "projectId": "p",
                "cycleId": "c", "estimate": 0, "dueDate": "2026-11-20",
            })
        );
    }
}
