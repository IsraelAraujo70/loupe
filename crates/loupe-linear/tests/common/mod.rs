//! Sample variables for every document, shared by the offline schema gate and
//! the live validation gate so both send what the client sends.

use chrono::{NaiveDate, TimeZone, Utc};
use loupe_linear::documents::Document;
use loupe_linear::filter::{DateField, DateOp, FilterClause, FlagField, SetField, SetOp};
use loupe_linear::variables;
use loupe_linear::{
    ClosedIssues, CustomViewUpdate, IssueQuery, IssueUpdate, NewCustomView, NewIssue, Priority,
    ProjectChange,
};
use serde_json::{Value, json};

/// One clause of every kind a Linear view can keep, with every kind of
/// value, so the filter a saved view sends is checked against the schema.
fn exportable_clauses() -> Vec<FilterClause> {
    let set = |field, op, values: &[&str]| FilterClause::Set {
        field,
        op,
        values: values.iter().map(|v| (*v).to_owned()).collect(),
    };
    let day = NaiveDate::from_ymd_opt(2026, 11, 1).expect("valid day");
    let mut clauses = vec![
        set(SetField::Status, SetOp::In, &["s1", "s2"]),
        set(SetField::Status, SetOp::NotIn, &["s1"]),
        set(SetField::StatusType, SetOp::In, &["unstarted", "started"]),
        set(SetField::StatusType, SetOp::NotIn, &["canceled"]),
        set(SetField::Priority, SetOp::In, &["1", "2"]),
        set(SetField::Priority, SetOp::NotIn, &["0"]),
        set(SetField::Assignee, SetOp::In, &["me", "u1", "none"]),
        set(SetField::Assignee, SetOp::NotIn, &["none"]),
        set(SetField::Creator, SetOp::In, &["me"]),
        set(SetField::Project, SetOp::In, &["p1", "none"]),
        set(SetField::Milestone, SetOp::In, &["m1", "none"]),
        set(SetField::Cycle, SetOp::In, &["current", "c1", "none"]),
        set(SetField::Label, SetOp::In, &["l1", "none"]),
    ];
    for field in [
        DateField::CreatedAt,
        DateField::UpdatedAt,
        DateField::DueDate,
    ] {
        for op in [DateOp::Before(day), DateOp::After(day)] {
            clauses.push(FilterClause::Date { field, op });
        }
    }
    for field in [
        FlagField::IsSubIssue,
        FlagField::HasSubIssues,
        FlagField::HasDueDate,
    ] {
        for value in [true, false] {
            clauses.push(FilterClause::Flag { field, value });
        }
    }
    clauses
}

/// Plausible variables for `document`, one set per interesting shape. Panics
/// for a document without samples, so a new document cannot skip the gates.
pub fn sample_variables(document: &Document) -> Vec<Value> {
    let now = Utc
        .with_ymd_and_hms(2026, 11, 10, 12, 0, 0)
        .single()
        .expect("valid date");
    match document.name {
        "Viewer" | "Teams" | "MyTeams" => vec![json!({})],
        "Team" => vec![variables::team("0e2f9c1a-6b43-4d7e-9a51-3c8d2f7b1e04", now)],
        "MyOpenIssues" => vec![
            variables::my_open_issues(None),
            variables::my_open_issues(Some("0e2f9c1a-6b43-4d7e-9a51-3c8d2f7b1e04")),
        ],
        "TeamIssues" => {
            let team = "0e2f9c1a-6b43-4d7e-9a51-3c8d2f7b1e04";
            let set = |field, op, values: &[&str]| FilterClause::Set {
                field,
                op,
                values: values.iter().map(|v| (*v).to_owned()).collect(),
            };
            let day = NaiveDate::from_ymd_opt(2026, 11, 1).expect("valid day");
            let mut everything = Vec::new();
            for field in SetField::ALL {
                for op in [SetOp::In, SetOp::NotIn] {
                    everything.push(set(field, op, &["a1", "me", "none", "current", "2"]));
                }
            }
            for field in DateField::ALL {
                for op in [
                    DateOp::Before(day),
                    DateOp::After(day),
                    DateOp::PastDays(7),
                    DateOp::NextDays(7),
                    DateOp::IsSet,
                    DateOp::IsEmpty,
                ] {
                    everything.push(FilterClause::Date { field, op });
                }
            }
            for field in FlagField::ALL {
                for value in [true, false] {
                    everything.push(FilterClause::Flag { field, value });
                }
            }
            everything.push(FilterClause::Text {
                value: "login".into(),
            });
            let query = |clauses: Vec<FilterClause>, closed| IssueQuery { clauses, closed };
            vec![
                variables::team_issues(team, &IssueQuery::default(), None, now),
                variables::team_issues(
                    team,
                    &query(everything, ClosedIssues::ClosedSince(now)),
                    Some("cursor"),
                    now,
                ),
                variables::team_issues(team, &query(Vec::new(), ClosedIssues::None), None, now),
                variables::project_issues(
                    "5a0c3b8e-2f71-4c9d-8e6b-1d4f7a2c9e30",
                    &query(Vec::new(), ClosedIssues::All),
                    Some("cursor"),
                    now,
                ),
            ]
        }
        "Issue" => vec![json!({ "id": "ENG-1" })],
        "SearchIssues" => vec![
            json!({ "term": "login", "first": 20 }),
            json!({ "term": "login", "first": 20, "filter": variables::team_key_filter("eng") }),
        ],
        "RecentIssues" => vec![json!({ "filter": variables::team_key_filter("eng"), "first": 10 })],
        "IssuesForUrl" => vec![json!({ "url": "https://github.com/acme/web/pull/1" })],
        "IssuesByIdentifiers" => {
            vec![variables::issues_by_identifiers(&[
                ("ENG".into(), 1),
                ("DES".into(), 2),
            ])]
        }
        "IssueUpdate" => {
            let everything = IssueUpdate {
                state_id: Some("s1".into()),
                priority: Some(Priority::High),
                assignee_id: Some(Some("u1".into())),
                label_ids: Some(vec!["l1".into()]),
                added_label_ids: vec!["l2".into()],
                removed_label_ids: vec!["l3".into()],
                estimate: Some(Some(3.0)),
                due_date: Some(NaiveDate::from_ymd_opt(2026, 12, 5)),
                project_id: Some(Some("p1".into())),
                cycle_id: Some(Some("c1".into())),
                title: Some("Title".into()),
                description: Some("Body".into()),
                sort_order: Some(-3.5),
            };
            let cleared = IssueUpdate {
                assignee_id: Some(None),
                estimate: Some(None),
                due_date: Some(None),
                project_id: Some(None),
                cycle_id: Some(None),
                ..IssueUpdate::default()
            };
            vec![
                json!({ "id": "ENG-1", "input": variables::issue_update_input(&everything) }),
                json!({ "id": "ENG-1", "input": variables::issue_update_input(&cleared) }),
            ]
        }
        "CommentCreate" => vec![json!({ "input": { "issueId": "ENG-1", "body": "Looks good." } })],
        "IssueCreate" => {
            let everything = NewIssue {
                team_id: "t1".into(),
                title: "Title".into(),
                description: Some("Body".into()),
                state_id: Some("s1".into()),
                priority: Some(Priority::Low),
                assignee_id: Some("u1".into()),
                label_ids: vec!["l1".into()],
                project_id: Some("p1".into()),
                cycle_id: Some("c1".into()),
                estimate: Some(3),
                due_date: NaiveDate::from_ymd_opt(2026, 12, 5),
            };
            let minimal = NewIssue {
                team_id: "t1".into(),
                title: "Title".into(),
                ..NewIssue::default()
            };
            vec![
                json!({ "input": variables::issue_create_input(&everything) }),
                json!({ "input": variables::issue_create_input(&minimal) }),
            ]
        }
        "IssuePullRequests" => vec![
            json!({ "id": "ENG-1", "after": null }),
            json!({ "id": "ENG-1", "after": "cursor" }),
        ],
        "CustomViews" => vec![
            variables::custom_views(None),
            variables::custom_views(Some("cursor")),
        ],
        "CustomViewIssues" => vec![serde_json::json!({"id":"view-id", "after":null})],
        "CustomView" | "CustomViewDelete" => vec![json!({ "id": "view-id" })],
        "CustomViewCreate" => {
            let filter = |clauses: &[FilterClause]| {
                loupe_linear::views::export_linear_filter(clauses).expect("an exportable filter")
            };
            let view = |team_id: Option<&str>, clauses: &[FilterClause]| NewCustomView {
                name: "Urgent bugs".into(),
                team_id: team_id.map(str::to_owned),
                shared: false,
                filter_data: filter(clauses),
            };
            vec![
                json!({ "input": variables::custom_view_create_input(&view(None, &[])) }),
                json!({ "input": variables::custom_view_create_input(&view(
                    Some("0e2f9c1a-6b43-4d7e-9a51-3c8d2f7b1e04"),
                    &exportable_clauses(),
                )) }),
            ]
        }
        "CustomViewUpdate" => {
            let everything = CustomViewUpdate {
                name: Some("Renamed".into()),
                filter_data: Some(
                    loupe_linear::views::export_linear_filter(&exportable_clauses())
                        .expect("an exportable filter"),
                ),
            };
            vec![
                json!({ "id": "view-id", "input": variables::custom_view_update_input(&everything) }),
                json!({ "id": "view-id", "input": variables::custom_view_update_input(&CustomViewUpdate {
                    name: Some("Renamed".into()),
                    ..CustomViewUpdate::default()
                }) }),
            ]
        }
        "ViewPreferencesSave" => {
            let config = loupe_linear::views::ViewConfig::default();
            let own = json!({ "issueSubGrouping": "priority" });
            vec![
                variables::view_preferences_save(
                    "view-id",
                    &loupe_linear::views::export_preferences(&config, None, &Value::Null),
                ),
                variables::view_preferences_save(
                    "view-id",
                    &loupe_linear::views::export_preferences(&config, None, &own),
                ),
            ]
        }
        "FavoriteCreate" => vec![json!({ "input": { "customViewId": "view-id" } })],
        "Favorites" => vec![
            variables::favorites(None),
            variables::favorites(Some("cursor")),
        ],
        "Projects" => vec![
            variables::projects(None, None),
            variables::projects(Some("0e2f9c1a-6b43-4d7e-9a51-3c8d2f7b1e04"), Some("cursor")),
        ],
        "Project" => vec![json!({ "id": "5a0c3b8e-2f71-4c9d-8e6b-1d4f7a2c9e30" })],
        "ProjectOptions" => vec![json!({})],
        "ProjectUpdate" => {
            let day = NaiveDate::from_ymd_opt(2026, 11, 20);
            let everything = ProjectChange {
                status_id: Some("s1".into()),
                lead_id: Some(Some("u1".into())),
                member_ids: Some(vec!["u1".into(), "u2".into()]),
                team_ids: Some(vec!["t1".into()]),
                label_ids: Some(vec!["l1".into()]),
                start_date: Some(day),
                target_date: Some(day),
            };
            let cleared = ProjectChange {
                lead_id: Some(None),
                start_date: Some(None),
                target_date: Some(None),
                ..ProjectChange::default()
            };
            vec![
                json!({ "id": "p1", "input": variables::project_update_input(&everything) }),
                json!({ "id": "p1", "input": variables::project_update_input(&cleared) }),
            ]
        }
        other => panic!("tests/common: no sample variables for document {other}"),
    }
}
