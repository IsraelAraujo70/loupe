//! Decoding tests. Each fixture in `tests/fixtures/<Document>.json` is a full
//! GraphQL response body. A throwaway local HTTP server hands them to the real
//! client, so every test goes through the same path production does: request
//! building, `graphql()`, wire decoding, and conversion into domain types.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::NaiveDate;
use loupe_linear::documents::ALL;
use loupe_linear::{
    Author, CustomViewUpdate, Error, EstimateScale, IssueQuery, IssueUpdate, Linear, NewCustomView,
    NewIssue, Priority, PullRequestState, StateType,
};
use serde_json::{Value, json};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixtures_dir().join(format!("{name}.json")))
        .unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

fn fixture(name: &str) -> Value {
    serde_json::from_str(&fixture_text(name)).expect("fixture is JSON")
}

#[derive(Clone, Debug)]
struct Recorded {
    operation: String,
    variables: Value,
    authorization: Option<String>,
}

struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Stub {
    /// Serves fixtures by operation name. `queued` bodies replace the fixture
    /// for an operation, one per request, before it falls back to the file.
    fn start(queued: Vec<(&str, Vec<String>)>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/graphql", listener.local_addr().expect("addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let queues: Arc<Mutex<HashMap<String, VecDeque<String>>>> = Arc::new(Mutex::new(
            queued
                .into_iter()
                .map(|(name, bodies)| (name.to_owned(), bodies.into()))
                .collect(),
        ));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &seen, &queues);
            }
        });
        Stub { url, requests }
    }

    fn client(&self) -> Linear {
        Linear::with_endpoint("test-key", &self.url)
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("lock").clone()
    }

    fn only_request(&self) -> Recorded {
        let mut requests = self.requests();
        assert_eq!(
            requests.len(),
            1,
            "expected exactly one request: {requests:?}"
        );
        requests.remove(0)
    }
}

fn serve(
    stream: std::net::TcpStream,
    seen: &Mutex<Vec<Recorded>>,
    queues: &Mutex<HashMap<String, VecDeque<String>>>,
) {
    let mut reader = BufReader::new(stream);
    let mut length = 0usize;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let operation = request["operationName"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    seen.lock().expect("lock").push(Recorded {
        operation: operation.clone(),
        variables: request["variables"].clone(),
        authorization,
    });
    let queued = queues
        .lock()
        .expect("lock")
        .get_mut(&operation)
        .and_then(VecDeque::pop_front);
    let response = queued.unwrap_or_else(|| fixture_text(&operation));
    let mut stream = reader.into_inner();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.len(),
        response
    );
}

fn date(value: &str) -> NaiveDate {
    value.parse().expect("date")
}

#[test]
fn every_document_has_exactly_one_fixture() {
    let mut expected: Vec<String> = ALL.iter().map(|d| format!("{}.json", d.name)).collect();
    expected.sort();
    let mut found: Vec<String> = std::fs::read_dir(fixtures_dir())
        .expect("fixtures dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    assert_eq!(found, expected);
    for document in ALL {
        assert!(
            fixture(document.name)["data"].is_object(),
            "{} has no data",
            document.name
        );
    }
}

#[test]
fn viewer() {
    let stub = Stub::start(vec![]);
    let viewer = stub.client().viewer().expect("viewer");
    assert_eq!(viewer.user.display_name, "ada");
    assert_eq!(viewer.user.name, "Ada Okafor");
    assert!(viewer.user.avatar_url.is_some());
    assert_eq!(viewer.organization.url_key, "acme");
    assert_eq!(viewer.email, "ada@acme.test");
    assert_eq!(
        viewer.organization.logo_url.as_deref(),
        Some("https://uploads.linear.app/de85dab6/0c9f7e60/256x256/logo")
    );
    let request = stub.only_request();
    assert_eq!(request.operation, "Viewer");
    assert_eq!(request.authorization.as_deref(), Some("test-key"));
}

#[test]
fn an_identity_saved_before_logos_existed_still_loads() {
    // The app keeps the viewer on disk to start offline. One saved by an
    // earlier build has no logo field.
    let stub = Stub::start(vec![]);
    let viewer = stub.client().viewer().expect("viewer");
    let mut saved = serde_json::to_value(&viewer).expect("serialize");
    saved["organization"]
        .as_object_mut()
        .expect("organization")
        .remove("logo_url")
        .expect("the logo is saved with the identity");
    let loaded: loupe_linear::Viewer =
        serde_json::from_value(saved).expect("an older snapshot loads");
    assert_eq!(loaded.organization.logo_url, None);
    assert_eq!(loaded.organization.name, viewer.organization.name);
}

#[test]
fn a_workspace_without_a_logo_has_none() {
    for missing in [json!(null), json!(""), json!("   ")] {
        let mut response = fixture("Viewer");
        response["data"]["viewer"]["organization"]["logoUrl"] = missing.clone();
        let stub = Stub::start(vec![("Viewer", vec![response.to_string()])]);
        let viewer = stub.client().viewer().expect("viewer");
        assert_eq!(viewer.organization.logo_url, None, "logoUrl {missing}");
    }
    // A stub or an older answer that leaves the field out reads the same.
    let mut response = fixture("Viewer");
    response["data"]["viewer"]["organization"]
        .as_object_mut()
        .expect("organization")
        .remove("logoUrl");
    let stub = Stub::start(vec![("Viewer", vec![response.to_string()])]);
    let viewer = stub.client().viewer().expect("viewer");
    assert_eq!(viewer.organization.logo_url, None);
}

#[test]
fn teams_sorted_by_name() {
    let stub = Stub::start(vec![]);
    let teams = stub.client().teams().expect("teams");
    let names: Vec<_> = teams
        .iter()
        .map(|t| (t.key.as_str(), t.name.as_str()))
        .collect();
    assert_eq!(names, [("DES", "Design"), ("ENG", "Engineering")]);
}

#[test]
fn my_teams_lists_the_viewer_memberships_by_name() {
    let stub = Stub::start(vec![]);
    let teams = stub.client().my_teams().expect("my teams");
    let keys: Vec<_> = teams.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["BE", "ENG"]);
    assert_eq!(stub.only_request().operation, "MyTeams");
}

#[test]
fn team_pickers() {
    let stub = Stub::start(vec![]);
    let team = stub.client().team("team-1").expect("team");
    assert_eq!(
        (team.key.as_str(), team.name.as_str()),
        ("ENG", "Engineering")
    );

    let states: Vec<_> = team.states.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        states,
        [
            "In Progress",
            "In Review",
            "Todo",
            "Backlog",
            "Triage",
            "Done",
            "Canceled",
            "Duplicate"
        ]
    );
    let kinds: Vec<_> = team.states.iter().map(|s| s.kind).collect();
    for kind in [
        StateType::Triage,
        StateType::Backlog,
        StateType::Unstarted,
        StateType::Started,
        StateType::Completed,
        StateType::Canceled,
        StateType::Duplicate,
    ] {
        assert!(kinds.contains(&kind), "no {kind:?} state");
    }

    let members: Vec<_> = team
        .members
        .iter()
        .map(|m| m.display_name.as_str())
        .collect();
    assert_eq!(
        members,
        ["ada", "ben", "chloe", "dmitri"],
        "inactive member excluded, sorted by name"
    );

    let labels: Vec<_> = team.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Accessibility",
            "API",
            "Area",
            "Branding",
            "Bug",
            "Customer",
            "Feature",
            "Improvement",
            "Needs design",
            "Tech debt"
        ]
    );
    let area = team.labels.iter().find(|l| l.name == "Area").expect("Area");
    let api = team.labels.iter().find(|l| l.name == "API").expect("API");
    assert_eq!(api.parent_id.as_deref(), Some(area.id.as_str()));
    assert_eq!(area.parent_id, None);
    assert_eq!(team.label_groups, std::slice::from_ref(&area.id));

    let projects: Vec<_> = team.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(projects, ["Atlas", "Beacon"]);
    assert_eq!(team.projects[0].color.as_deref(), Some("#bb87fc"));

    assert_eq!(team.cycles.len(), 2);
    assert_eq!(team.cycles[0].number, 41);
    assert_eq!(team.cycles[0].name, None);
    assert_eq!(team.cycles[1].name.as_deref(), Some("Polish sprint"));
    assert!(team.cycles[0].starts_at < team.cycles[1].starts_at);
    assert_eq!(team.estimation.scale, EstimateScale::Fibonacci);
    assert_eq!(team.estimation.points(), [1, 2, 3, 5, 8, 13, 21]);

    let request = stub.only_request();
    assert_eq!(request.operation, "Team");
    assert_eq!(request.variables["id"], "team-1");
    assert_eq!(request.variables["teamId"], "team-1");
    assert!(
        request.variables["now"]
            .as_str()
            .is_some_and(|now| now.ends_with('Z'))
    );
}

#[test]
fn my_open_issues() {
    let stub = Stub::start(vec![]);
    let issues = stub.client().my_open_issues().expect("my_open_issues");
    assert_eq!(issues.len(), 8);
    assert!(issues.iter().all(|i| !i.state.kind.is_closed()));
    let eng_101 = issues
        .iter()
        .find(|i| i.identifier == "ENG-101")
        .expect("ENG-101");
    assert_eq!(
        eng_101.branch_name, "ada/eng-101-crash-when-the-sync-worker-restarts",
        "the branch name Linear suggests"
    );
    assert!(
        issues
            .iter()
            .filter(|i| i.identifier != "ENG-101")
            .all(|i| i.branch_name.is_empty()),
        "an issue without one decodes with none"
    );
    assert!(
        issues
            .windows(2)
            .all(|pair| pair[0].updated_at >= pair[1].updated_at)
    );
    let teams: std::collections::BTreeSet<_> = issues.iter().map(|i| i.team.key.as_str()).collect();
    assert_eq!(teams.into_iter().collect::<Vec<_>>(), ["DES", "ENG"]);
    assert!(
        issues
            .iter()
            .all(|i| i.assignee.as_ref().is_some_and(|u| u.display_name == "ada"))
    );

    let request = stub.only_request();
    assert_eq!(request.operation, "MyOpenIssues");
    assert_eq!(
        request.variables["filter"],
        json!({ "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } })
    );
}

fn page_of(cursor: Option<&str>, ids: &[&str]) -> String {
    let nodes: Vec<Value> = ids
        .iter()
        .map(|id| {
            let mut node =
                fixture("MyOpenIssues")["data"]["viewer"]["assignedIssues"]["nodes"][0].clone();
            node["id"] = json!(id);
            node
        })
        .collect();
    json!({ "data": { "viewer": { "assignedIssues": {
        "nodes": nodes,
        "pageInfo": { "hasNextPage": cursor.is_some(), "endCursor": cursor },
    } } } })
    .to_string()
}

#[test]
fn my_open_issues_follows_cursors() {
    let stub = Stub::start(vec![(
        "MyOpenIssues",
        vec![page_of(Some("c1"), &["a", "b"]), page_of(None, &["b", "c"])],
    )]);
    let issues = stub.client().my_open_issues().expect("my_open_issues");
    let ids: Vec<_> = issues.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        ["a", "b", "c"],
        "second page merged without duplicates"
    );
    let requests = stub.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].variables["after"], Value::Null);
    assert_eq!(requests[1].variables["after"], "c1");
}

#[test]
fn my_open_issues_stops_after_five_pages() {
    let pages = (0..8)
        .map(|n| page_of(Some("more"), &[&format!("p{n}")]))
        .collect();
    let stub = Stub::start(vec![("MyOpenIssues", pages)]);
    let issues = stub.client().my_open_issues().expect("my_open_issues");
    assert_eq!(issues.len(), 5);
    assert_eq!(stub.requests().len(), 5);
}

#[test]
fn team_issues_page() {
    let stub = Stub::start(vec![]);
    let page = stub
        .client()
        .team_issues("team-1", &IssueQuery::default(), Some("cur"))
        .expect("team_issues");
    assert_eq!(page.nodes.len(), 15);
    assert_eq!(page.next_cursor, None);

    let crash = page
        .nodes
        .iter()
        .find(|i| i.identifier == "ENG-101")
        .expect("ENG-101");
    assert_eq!(crash.title, "Crash when the sync worker restarts mid-batch");
    assert_eq!(crash.priority, Priority::Urgent);
    assert_eq!(crash.state.name, "In Progress");
    assert_eq!(crash.state.kind, StateType::Started);
    assert_eq!(crash.state.color, "#f2c94c");
    assert_eq!(crash.team.key, "ENG");
    assert_eq!(crash.estimate, Some(5.0));
    assert_eq!(crash.due_date, Some(date("2026-11-14")));
    assert_eq!(
        crash.assignee.as_ref().map(|u| u.display_name.as_str()),
        Some("ada")
    );
    assert_eq!(crash.labels.len(), 1);
    assert_eq!(crash.labels[0].name, "Bug");
    assert_eq!(
        crash.project.as_ref().map(|p| p.name.as_str()),
        Some("Atlas")
    );
    let cycle = crash.cycle.as_ref().expect("cycle");
    assert_eq!((cycle.number, cycle.name.as_deref()), (41, None));
    assert_eq!(
        crash.created_at.to_rfc3339(),
        "2026-10-21T09:12:44.318+00:00"
    );
    assert!(
        crash
            .url
            .starts_with("https://linear.app/acme/issue/ENG-101/")
    );

    let triage = page
        .nodes
        .iter()
        .find(|i| i.identifier == "ENG-105")
        .expect("ENG-105");
    assert_eq!(triage.state.kind, StateType::Triage);
    assert!(triage.assignee.is_none() && triage.project.is_none() && triage.cycle.is_none());
    assert!(triage.estimate.is_none() && triage.due_date.is_none());

    let none = page
        .nodes
        .iter()
        .find(|i| i.identifier == "ENG-107")
        .expect("ENG-107");
    assert_eq!(none.priority, Priority::None);
    assert_eq!(none.labels.len(), 2);

    let kinds: std::collections::BTreeSet<_> = page.nodes.iter().map(|i| i.state.kind).collect();
    assert_eq!(kinds.len(), 7, "every state type appears");

    let request = stub.only_request();
    assert_eq!(request.operation, "TeamIssues");
    assert_eq!(request.variables["after"], "cur");
    assert_eq!(
        request.variables["filter"]["and"][0]["team"],
        json!({ "id": { "eq": "team-1" } })
    );
}

#[test]
fn team_issues_reports_the_next_cursor() {
    let mut body = fixture("TeamIssues");
    body["data"]["issues"]["pageInfo"] = json!({ "hasNextPage": true, "endCursor": "next" });
    let stub = Stub::start(vec![("TeamIssues", vec![body.to_string()])]);
    let page = stub
        .client()
        .team_issues("t", &IssueQuery::default(), None)
        .expect("team_issues");
    assert_eq!(page.next_cursor.as_deref(), Some("next"));
}

#[test]
fn issue_detail() {
    let stub = Stub::start(vec![]);
    let detail = stub.client().issue("ENG-101").expect("issue");
    assert_eq!(detail.issue.identifier, "ENG-101");
    assert!(detail.description.starts_with("## Problem"));
    assert!(detail.description.contains("```rust"));
    assert!(detail.description.contains("1. Start the worker"));
    assert_eq!(
        detail.creator.as_ref().map(|u| u.display_name.as_str()),
        Some("chloe")
    );

    assert_eq!(detail.comments.len(), 5);
    assert!(
        detail
            .comments
            .windows(2)
            .all(|pair| pair[0].created_at <= pair[1].created_at)
    );
    let by = |n: usize| &detail.comments[n];
    assert!(matches!(&by(0).author, Author::User(user) if user.display_name == "ben"));
    assert!(by(1).edited_at.is_some());
    assert_eq!(by(2).parent_id.as_deref(), Some(by(1).id.as_str()));
    assert!(
        matches!(&by(3).author, Author::Bot { name, avatar_url } if name == "GitHub" && avatar_url.is_some())
    );
    assert!(by(0).edited_at.is_none() && by(0).parent_id.is_none());

    assert_eq!(detail.attachments.len(), 3);
    let merged = detail.attachments[0]
        .pull_request
        .as_ref()
        .expect("merged pr");
    assert_eq!(
        (
            merged.owner.as_str(),
            merged.repo.as_str(),
            merged.number,
            merged.state
        ),
        ("acme", "sync-worker", 412, PullRequestState::Merged)
    );
    assert_eq!(
        detail.attachments[0].subtitle.as_deref(),
        Some("Merged · ada")
    );
    let open = detail.attachments[1]
        .pull_request
        .as_ref()
        .expect("open pr");
    assert_eq!(
        (open.repo.as_str(), open.number, open.state),
        ("web", 1187, PullRequestState::Open)
    );
    assert!(detail.attachments[2].pull_request.is_none());
    assert_eq!(detail.attachments[2].subtitle, None);

    let request = stub.only_request();
    assert_eq!(request.operation, "Issue");
    assert_eq!(request.variables, json!({ "id": "ENG-101" }));
}

#[test]
fn null_description_becomes_empty() {
    let mut body = fixture("Issue");
    body["data"]["issue"]["description"] = Value::Null;
    body["data"]["issue"]["creator"] = Value::Null;
    body["data"]["issue"]["comments"]["nodes"][0]["user"] = Value::Null;
    let stub = Stub::start(vec![("Issue", vec![body.to_string()])]);
    let detail = stub.client().issue("ENG-101").expect("issue");
    assert_eq!(detail.description, "");
    assert!(detail.creator.is_none());
    assert_eq!(detail.comments[0].author, Author::Unknown);
}

#[test]
fn search_issues() {
    let stub = Stub::start(vec![]);
    let found = stub.client().search_issues("crash", 20).expect("search");
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].identifier, "ENG-101");
    let request = stub.only_request();
    assert_eq!(request.operation, "SearchIssues");
    assert_eq!(request.variables, json!({ "term": "crash", "first": 20 }));
}

#[test]
fn team_key_issues_search_inside_the_team() {
    let stub = Stub::start(vec![]);
    let found = stub
        .client()
        .team_key_issues("eng", " crash ", 20)
        .expect("search");
    assert_eq!(found.len(), 3);
    let request = stub.only_request();
    assert_eq!(request.operation, "SearchIssues");
    assert_eq!(
        request.variables,
        json!({
            "term": "crash",
            "first": 20,
            "filter": { "team": { "key": { "eq": "ENG" } } },
        })
    );
}

#[test]
fn team_key_issues_without_a_term_lists_the_latest() {
    let stub = Stub::start(vec![]);
    let found = stub
        .client()
        .team_key_issues("Eng", "  ", 10)
        .expect("recent");
    assert_eq!(found.len(), 3);
    let request = stub.only_request();
    assert_eq!(request.operation, "RecentIssues");
    assert_eq!(
        request.variables,
        json!({ "filter": { "team": { "key": { "eq": "ENG" } } }, "first": 10 })
    );
}

#[test]
fn issues_for_url_dedupes() {
    let stub = Stub::start(vec![]);
    let issues = stub
        .client()
        .issues_for_url("https://github.com/acme/sync-worker/pull/412")
        .expect("issues_for_url");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].identifier, "ENG-101");
    let request = stub.only_request();
    assert_eq!(request.operation, "IssuesForUrl");
    assert_eq!(
        request.variables["url"],
        "https://github.com/acme/sync-worker/pull/412"
    );
}

#[test]
fn issues_by_identifiers_sends_one_or_clause_per_unique_identifier() {
    let stub = Stub::start(vec![]);
    let identifiers: Vec<String> = ["eng-101", "ENG-102", "ENG-101", "not an id", "DES-21"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let issues = stub
        .client()
        .issues_by_identifiers(&identifiers)
        .expect("by identifiers");
    assert_eq!(issues.len(), 3);
    let request = stub.only_request();
    assert_eq!(request.operation, "IssuesByIdentifiers");
    assert_eq!(
        request.variables["filter"]["or"],
        json!([
            { "and": [{ "team": { "key": { "eq": "ENG" } } }, { "number": { "eq": 101 } }] },
            { "and": [{ "team": { "key": { "eq": "ENG" } } }, { "number": { "eq": 102 } }] },
            { "and": [{ "team": { "key": { "eq": "DES" } } }, { "number": { "eq": 21 } }] },
        ])
    );
}

#[test]
fn issues_by_identifiers_skips_the_request_when_nothing_parses() {
    let stub = Stub::start(vec![]);
    let issues = stub
        .client()
        .issues_by_identifiers(&["nope".to_owned(), String::new()])
        .expect("by identifiers");
    assert!(issues.is_empty());
    assert!(stub.requests().is_empty());
}

#[test]
fn update_issue() {
    let stub = Stub::start(vec![]);
    let update = IssueUpdate {
        state_id: Some("state-review".into()),
        priority: Some(Priority::High),
        assignee_id: Some(None),
        due_date: Some(Some(date("2026-12-01"))),
        ..IssueUpdate::default()
    };
    let issue = stub
        .client()
        .update_issue("ENG-101", &update)
        .expect("update");
    assert_eq!(issue.identifier, "ENG-101");
    assert_eq!(issue.priority, Priority::High);
    assert_eq!(issue.state.name, "In Review");
    let request = stub.only_request();
    assert_eq!(request.operation, "IssueUpdate");
    assert_eq!(request.variables["id"], "ENG-101");
    assert_eq!(
        request.variables["input"],
        json!({ "stateId": "state-review", "priority": 2, "assigneeId": null, "dueDate": "2026-12-01" })
    );
}

#[test]
fn create_comment() {
    let stub = Stub::start(vec![]);
    let comment = stub
        .client()
        .create_comment("issue-1", "Looks good to me.")
        .expect("comment");
    assert_eq!(comment.body, "Looks good to me.");
    assert!(matches!(&comment.author, Author::User(user) if user.display_name == "ada"));
    assert!(comment.edited_at.is_none() && comment.parent_id.is_none());
    let request = stub.only_request();
    assert_eq!(request.operation, "CommentCreate");
    assert_eq!(
        request.variables,
        json!({ "input": { "issueId": "issue-1", "body": "Looks good to me." } })
    );
}

#[test]
fn create_issue() {
    let stub = Stub::start(vec![]);
    let new = NewIssue {
        team_id: "team-1".into(),
        title: "New issue from Loupe".into(),
        ..NewIssue::default()
    };
    let issue = stub.client().create_issue(&new).expect("create");
    assert_eq!(issue.identifier, "ENG-118");
    assert_eq!(issue.state.kind, StateType::Backlog);
    let request = stub.only_request();
    assert_eq!(request.operation, "IssueCreate");
    assert_eq!(
        request.variables,
        json!({ "input": { "teamId": "team-1", "title": "New issue from Loupe" } })
    );
}

fn expect_did_not(result: Result<impl std::fmt::Debug, Error>, message: &str) {
    match result {
        Err(Error::GraphQl(messages)) => assert_eq!(messages, [message]),
        other => panic!("expected a GraphQl error, got {other:?}"),
    }
}

#[test]
fn mutations_without_success_or_entity_are_errors() {
    let failed = json!({ "data": { "issueUpdate": { "success": false, "issue": null } } });
    let stub = Stub::start(vec![("IssueUpdate", vec![failed.to_string()])]);
    expect_did_not(
        stub.client().update_issue("ENG-1", &IssueUpdate::default()),
        "Linear did not update the issue",
    );

    let no_comment = json!({ "data": { "commentCreate": { "success": true, "comment": null } } });
    let stub = Stub::start(vec![("CommentCreate", vec![no_comment.to_string()])]);
    expect_did_not(
        stub.client().create_comment("i", "b"),
        "Linear did not create the comment",
    );

    let no_issue = json!({ "data": { "issueCreate": { "success": true, "issue": null } } });
    let stub = Stub::start(vec![("IssueCreate", vec![no_issue.to_string()])]);
    expect_did_not(
        stub.client().create_issue(&NewIssue::default()),
        "Linear did not create the issue",
    );
}

#[test]
fn errors_are_classified() {
    let unauthenticated = json!({ "errors": [{
        "message": "Authentication required, not authenticated",
        "extensions": { "code": "AUTHENTICATION_ERROR" },
    }] });
    let stub = Stub::start(vec![("Viewer", vec![unauthenticated.to_string()])]);
    assert!(matches!(stub.client().viewer(), Err(Error::Unauthorized)));

    let invalid = json!({ "errors": [{ "message": "Entity not found: Issue" }] });
    let stub = Stub::start(vec![("Issue", vec![invalid.to_string()])]);
    assert!(
        matches!(stub.client().issue("ENG-1"), Err(Error::GraphQl(messages)) if messages == ["Entity not found: Issue"])
    );

    let wrong_shape = json!({ "data": { "viewer": { "id": 1 } } });
    let stub = Stub::start(vec![("Viewer", vec![wrong_shape.to_string()])]);
    assert!(matches!(stub.client().viewer(), Err(Error::Decode(_))));
}

#[test]
fn issue_summaries_carry_filter_facts() {
    let stub = Stub::start(vec![]);
    let page = stub
        .client()
        .team_issues("team-1", &IssueQuery::default(), None)
        .expect("team_issues");
    let crash = page
        .nodes
        .iter()
        .find(|i| i.identifier == "ENG-101")
        .expect("ENG-101");
    assert!(crash.has_sub_issues);
    assert!(crash.creator_id.is_some());
    assert_eq!(crash.pull_requests.len(), 2);
    assert!(page.nodes.iter().any(|i| i.parent_id.is_some()));
    assert!(
        page.nodes
            .iter()
            .filter(|i| i.state.kind.is_closed())
            .all(|i| i.closed_at.is_some())
    );
}

#[test]
fn custom_views_import_into_loupe_views() {
    use loupe_linear::filter::{FilterClause, ME, SetField, SetOp};
    use loupe_linear::views::{ViewsFile, import_views};

    let stub = Stub::start(vec![]);
    let client = stub.client();
    let linear_views = client.custom_views().expect("custom views");
    assert_eq!(linear_views.len(), 4);
    let team = client.team("team-1").expect("team");
    let now = chrono::Utc::now();
    let mut file = ViewsFile::seeded(now);
    let seeded = file.views.len();

    let report = import_views(&mut file, &linear_views, &team, now);
    assert_eq!(report.imported, ["My urgent bugs", "In review"]);
    // An `or` across two fields has no exact clause form, so the view is
    // skipped rather than imported wider than it is in Linear.
    let skipped: Vec<&str> = report
        .skipped
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(skipped, ["Unassigned or todo"]);
    assert_eq!(file.views.len(), seeded + 2);

    let urgent = file
        .views
        .iter()
        .find(|view| view.name == "My urgent bugs")
        .expect("imported view");
    let bug = team
        .labels
        .iter()
        .find(|label| label.name == "Bug")
        .expect("Bug label");
    assert_eq!(
        urgent.config.filters,
        vec![
            FilterClause::Set {
                field: SetField::Assignee,
                op: SetOp::In,
                values: vec![ME.into()],
            },
            FilterClause::Set {
                field: SetField::Priority,
                op: SetOp::In,
                values: vec!["1".into(), "2".into()],
            },
            FilterClause::Set {
                field: SetField::Label,
                op: SetOp::In,
                values: vec![bug.id.clone()],
            },
        ]
    );
    assert_eq!(urgent.team_id.as_deref(), Some(team.id.as_str()));

    let again = import_views(&mut file, &linear_views, &team, now);
    assert!(again.imported.is_empty());
    assert_eq!(again.already.len(), 2);
    let operations: Vec<String> = stub.requests().into_iter().map(|r| r.operation).collect();
    assert_eq!(operations, ["CustomViews", "Team"], "import only reads");
}

#[test]
fn a_full_pull_request_page_is_completed_before_returning() {
    use loupe_linear::PullRequestState;

    let mut body = fixture("TeamIssues");
    let nodes = body["data"]["issues"]["nodes"]
        .as_array_mut()
        .expect("issue nodes");
    let crash = nodes
        .iter_mut()
        .find(|node| node["identifier"] == "ENG-101")
        .expect("ENG-101");
    crash["pullRequests"] = json!({
        "nodes": [{ "url": "https://github.com/acme/web/pull/1187", "metadata": { "status": "open" } }],
        "pageInfo": { "hasNextPage": true, "endCursor": "more" },
    });
    let stub = Stub::start(vec![("TeamIssues", vec![body.to_string()])]);
    let page = stub
        .client()
        .team_issues("team-1", &IssueQuery::default(), None)
        .expect("team_issues");
    let crash = page
        .nodes
        .iter()
        .find(|issue| issue.identifier == "ENG-101")
        .expect("ENG-101");
    assert!(crash.pull_requests_complete);
    let states: Vec<PullRequestState> = crash.pull_requests.iter().map(|pr| pr.state).collect();
    assert_eq!(
        states,
        [
            PullRequestState::Merged,
            PullRequestState::Open,
            PullRequestState::Closed
        ]
    );
    let operations: Vec<String> = stub.requests().into_iter().map(|r| r.operation).collect();
    assert_eq!(operations, ["TeamIssues", "IssuePullRequests"]);
    assert!(page.nodes.iter().all(|issue| issue.pull_requests_complete));
}

#[test]
fn projects_decode_status_lead_progress_and_teams() {
    use loupe_linear::ProjectStatusKind;

    let stub = Stub::start(vec![]);
    let projects = stub.client().projects(None).expect("projects");
    assert_eq!(projects.len(), 2);
    let security = &projects[0];
    assert_eq!(security.name, "Security Module");
    assert_eq!(security.status.kind, ProjectStatusKind::Started);
    assert_eq!(security.status.name, "In Progress");
    assert!((security.progress - 0.42).abs() < f32::EPSILON);
    assert_eq!(
        security.lead.as_ref().map(|lead| lead.name.as_str()),
        Some("Israel Araujo")
    );
    assert_eq!(security.target_date, NaiveDate::from_ymd_opt(2026, 12, 15));
    assert_eq!(security.team_ids, ["team-1"]);
    let backlog = &projects[1];
    assert_eq!(backlog.status.kind, ProjectStatusKind::Backlog);
    assert_eq!(backlog.lead, None);
    assert_eq!(backlog.target_date, None);
}

#[test]
fn a_project_page_decodes_in_linears_order() {
    use loupe_linear::{ProjectHealth, ProjectResourceKind, ProjectStatusKind};

    let stub = Stub::start(vec![]);
    let project = stub.client().project("project-1").expect("project");
    assert_eq!(project.name, "Vendor Integrations");
    assert_eq!(project.icon.as_deref(), Some("Shield"));
    assert_eq!(project.status.id, "status-started");
    assert_eq!(project.status.kind, ProjectStatusKind::Started);
    assert!((project.progress - 0.375).abs() < f32::EPSILON);
    assert_eq!(project.start_date, NaiveDate::from_ymd_opt(2026, 11, 6));
    assert_eq!(project.target_date, NaiveDate::from_ymd_opt(2026, 11, 20));
    assert_eq!(project.lead_team_id, "team-1");
    assert_eq!(project.summary, "Bureau checks before onboarding.");
    // Milestones by Linear's sort order, not the response's.
    let milestones: Vec<_> = project.milestones.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(milestones, ["ms-1", "ms-2"]);
    assert!((project.milestones[1].progress - 0.5).abs() < f32::EPSILON);
    // The newest update first.
    assert_eq!(project.updates[0].id, "pu-2");
    assert_eq!(project.updates[0].health, ProjectHealth::AtRisk);
    // Documents, then links.
    let kinds: Vec<_> = project.resources.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [ProjectResourceKind::Document, ProjectResourceKind::Link]
    );
    assert_eq!(project.resources[1].title, "Vendor A API");
    assert_eq!(stub.only_request().variables, json!({ "id": "project-1" }));
}

#[test]
fn project_options_leave_out_label_groups_and_follow_linears_order() {
    let stub = Stub::start(vec![]);
    let options = stub.client().project_options().expect("options");
    let statuses: Vec<_> = options.statuses.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        statuses,
        ["status-backlog", "status-started", "status-done"]
    );
    let labels: Vec<_> = options.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(labels, ["identity-check", "vendor-a"]);
    let users: Vec<_> = options.users.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(users, ["Israel Araujo", "Maya Moreau"]);
    let teams: Vec<_> = options.teams.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(teams, ["API", "WEB"]);
}

#[test]
fn project_issues_ask_for_the_project_not_a_team() {
    let stub = Stub::start(vec![]);
    stub.client()
        .project_issues("project-1", &IssueQuery::default(), None)
        .expect("issues");
    let request = stub.only_request();
    assert_eq!(request.operation, "TeamIssues");
    assert_eq!(
        request.variables["filter"],
        json!({ "and": [{ "project": { "id": { "eq": "project-1" } } }] })
    );
}

#[test]
fn update_project_sends_only_what_changed() {
    use loupe_linear::ProjectChange;

    let stub = Stub::start(vec![]);
    let change = ProjectChange {
        status_id: Some("status-done".into()),
        lead_id: Some(None),
        member_ids: Some(vec!["user-1".into()]),
        target_date: Some(Some(date("2026-12-01"))),
        ..ProjectChange::default()
    };
    let project = stub
        .client()
        .update_project("project-1", &change)
        .expect("update");
    assert_eq!(project.id, "project-1");
    let request = stub.only_request();
    assert_eq!(request.operation, "ProjectUpdate");
    assert_eq!(
        request.variables["input"],
        json!({ "statusId": "status-done", "leadId": null, "memberIds": ["user-1"], "targetDate": "2026-12-01" })
    );

    let failed = json!({ "data": { "projectUpdate": { "success": false, "project": null } } });
    let stub = Stub::start(vec![("ProjectUpdate", vec![failed.to_string()])]);
    expect_did_not(
        stub.client().update_project("project-1", &change),
        "Linear did not update the project",
    );
}

#[test]
fn favorites_preserve_user_order_and_destination() {
    let stub = Stub::start(vec![]);
    let rows = stub.client().favorites().expect("favorites");
    assert_eq!(
        rows.iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second", "Project", "Cycle"]
    );
    assert!(
        matches!(&rows[0].target, loupe_linear::FavoriteTarget::CustomView { view_id, .. } if view_id == "view-1")
    );
    assert!(
        matches!(&rows[1].target, loupe_linear::FavoriteTarget::Issue { identifier } if identifier == "ENG-101")
    );
    assert_eq!(rows[0].icon.as_deref(), Some("Bolt"));
    assert_eq!(rows[0].color.as_deref(), Some("#4cb782"));
}

#[test]
fn opening_a_remote_view_uses_linears_own_filter_and_cursor() {
    let stub = Stub::start(vec![]);
    let page = stub
        .client()
        .custom_view_issues("view-1", Some("next"))
        .expect("view issues");
    assert!(!page.nodes.is_empty());
    let request = stub
        .requests()
        .into_iter()
        .find(|r| r.operation == "CustomViewIssues")
        .expect("native view query");
    assert_eq!(request.variables, json!({"id":"view-1", "after":"next"}));
    assert!(
        request.variables.get("filter").is_none(),
        "Linear evaluates its saved filter"
    );
}

#[test]
fn remote_view_metadata_and_unsupported_filters_survive_decoding() {
    let mut response = fixture("CustomViews");
    let row = &mut response["data"]["customViews"]["nodes"][0];
    row["icon"] = json!("Bolt");
    row["color"] = json!("#4cb782");
    row["shared"] = json!(true);
    row["owner"] = json!({"name":"Owner"});
    row["filterData"] = json!({"estimate":{"gt":2}});
    row["viewPreferencesValues"] =
        json!({"layout":"list", "issueGrouping":"none", "viewOrdering":"dueDate"});
    let stub = Stub::start(vec![("CustomViews", vec![response.to_string()])]);
    let views = stub.client().custom_views().expect("views");
    assert_eq!(views[0].icon.as_deref(), Some("Bolt"));
    assert_eq!(views[0].color.as_deref(), Some("#4cb782"));
    assert_eq!(views[0].owner_name, "Owner");
    assert!(views[0].shared);
    assert_eq!(views[0].filter_data, json!({"estimate":{"gt":2}}));
    assert_eq!(views[0].preferences["layout"], "list");
}

#[test]
fn a_view_is_created_in_linear_with_its_filter_and_comes_back_as_stored() {
    let stub = Stub::start(vec![]);
    let filter = json!({ "and": [{ "priority": { "in": [1, 2] } }] });
    let view = stub
        .client()
        .create_custom_view(&NewCustomView {
            name: "Urgent bugs".into(),
            team_id: Some("team-1".into()),
            shared: false,
            filter_data: filter.clone(),
        })
        .expect("create");
    assert_eq!(view.name, "Urgent bugs");
    assert!(!view.shared);
    let request = stub.only_request();
    assert_eq!(request.operation, "CustomViewCreate");
    assert_eq!(
        request.variables["input"],
        json!({ "name": "Urgent bugs", "shared": false, "teamId": "team-1", "filterData": filter })
    );
}

#[test]
fn the_owners_own_display_settings_win_over_the_ones_in_force() {
    let stub = Stub::start(vec![]);
    let view = stub
        .client()
        .update_custom_view(
            "view-1",
            &CustomViewUpdate {
                name: Some("Renamed".into()),
                ..CustomViewUpdate::default()
            },
        )
        .expect("update");
    let request = stub.only_request();
    assert_eq!(request.operation, "CustomViewUpdate");
    assert_eq!(
        request.variables,
        json!({ "id": "view-1", "input": { "name": "Renamed" } })
    );
    // In force: a board with labels. The owner chose a list without them.
    assert_eq!(view.preferences["layout"], "list");
    assert_eq!(view.preferences["fieldLabels"], false);
    assert_eq!(view.preferences["issueGrouping"], "workflowState");
    // What the owner set, for the next save to write back; nulls are unset.
    assert_eq!(
        view.own_preferences,
        json!({ "layout": "list", "fieldLabels": false, "issueSubGrouping": "priority" })
    );
}

#[test]
fn display_settings_are_saved_as_the_owners_and_the_view_is_read_again() {
    let stub = Stub::start(vec![]);
    let preferences = json!({ "layout": "list", "issueSubGrouping": "priority" });
    let view = stub
        .client()
        .save_view_preferences("view-1", &preferences)
        .expect("save");
    assert_eq!(view.preferences["layout"], "list");
    let requests = stub.requests();
    let operations: Vec<&str> = requests.iter().map(|r| r.operation.as_str()).collect();
    assert_eq!(operations, ["ViewPreferencesSave", "CustomView"]);
    assert_eq!(
        requests[0].variables["input"],
        json!({
            "type": "user",
            "viewType": "customView",
            "customViewId": "view-1",
            "preferences": preferences,
        })
    );
    assert_eq!(requests[1].variables, json!({ "id": "view-1" }));
}

#[test]
fn deleting_a_view_and_adding_a_favorite_report_what_linear_answered() {
    let stub = Stub::start(vec![(
        "CustomViewDelete",
        vec![
            fixture_text("CustomViewDelete"),
            json!({ "data": { "customViewDelete": { "success": false } } }).to_string(),
        ],
    )]);
    stub.client().delete_custom_view("view-1").expect("delete");
    let refused = stub.client().delete_custom_view("view-1");
    assert!(matches!(refused, Err(Error::GraphQl(_))), "{refused:?}");
    stub.client()
        .favorite_custom_view("view-1")
        .expect("favorite");
    let last = stub.requests().pop().expect("a request");
    assert_eq!(last.operation, "FavoriteCreate");
    assert_eq!(
        last.variables,
        json!({ "input": { "customViewId": "view-1" } })
    );
}

#[test]
fn favorites_sort_across_pages_not_in_api_page_order() {
    let mut first = fixture("Favorites");
    let mut second = first.clone();
    first["data"]["favorites"]["nodes"] = json!([first["data"]["favorites"]["nodes"][0].clone()]);
    first["data"]["favorites"]["pageInfo"] = json!({"hasNextPage":true,"endCursor":"page-two"});
    second["data"]["favorites"]["nodes"] = json!([second["data"]["favorites"]["nodes"][1].clone()]);
    let stub = Stub::start(vec![(
        "Favorites",
        vec![first.to_string(), second.to_string()],
    )]);
    let rows = stub.client().favorites().expect("favorites");
    assert_eq!(
        rows.iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(stub.requests()[1].variables["after"], "page-two");
}

#[test]
fn complete_remote_view_does_not_omit_a_higher_priority_issue_on_a_later_page() {
    let mut first = fixture("CustomViewIssues");
    let mut second = first.clone();
    first["data"]["customView"]["issues"]["pageInfo"] =
        json!({"hasNextPage":true,"endCursor":"older"});
    let mut urgent = first["data"]["customView"]["issues"]["nodes"][0].clone();
    urgent["id"] = json!("older-urgent");
    urgent["identifier"] = json!("ENG-999");
    urgent["priority"] = json!(1);
    second["data"]["customView"]["issues"]["nodes"] = json!([urgent]);
    second["data"]["customView"]["issues"]["pageInfo"] =
        json!({"hasNextPage":false,"endCursor":null});
    let stub = Stub::start(vec![(
        "CustomViewIssues",
        vec![first.to_string(), second.to_string()],
    )]);
    let issues = stub
        .client()
        .custom_view_all_issues("view-1")
        .expect("complete view");
    assert!(issues.iter().any(|issue| issue.identifier == "ENG-999"));
    assert_eq!(
        stub.requests()
            .iter()
            .filter(|r| r.operation == "CustomViewIssues")
            .count(),
        2
    );
}
