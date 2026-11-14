//! A local stand-in for Linear's GraphQL endpoint, for driving the app
//! without touching a real workspace. It serves the decoding fixtures in
//! `tests/fixtures`, keyed by `operationName`, and keeps one in-memory copy of
//! the issues so mutations show up in later reads.
//!
//!     cargo run -p loupe-linear --example stub_server -- [port]   (default 8787)
//!     LOUPE_LINEAR_ENDPOINT=http://127.0.0.1:8787/graphql ./target/debug/loupe
//!
//! `POST /fail-on` makes every mutation fail until `POST /fail-off`, to show
//! that the app rolls back optimistic edits. Single-threaded and HTTP/1.1
//! only; it ignores the API key.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use serde_json::{Map, Value, json};

const CLOSED_TYPES: [&str; 3] = ["completed", "canceled", "duplicate"];

fn main() {
    let port = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<u16>().ok())
        .unwrap_or(8787);
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind the stub port");
    let mut stub = Stub::load();
    eprintln!("linear stub listening on http://127.0.0.1:{port}/graphql");
    for stream in listener.incoming().flatten() {
        if let Err(error) = serve(&mut stub, stream) {
            eprintln!("stub: {error}");
        }
    }
}

fn serve(stub: &mut Stub, stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let response = match path {
        "/fail-on" => {
            stub.fail_mutations = true;
            json!({ "failMutations": true })
        }
        "/fail-off" => {
            stub.fail_mutations = false;
            json!({ "failMutations": false })
        }
        _ => match serde_json::from_slice::<Value>(&body) {
            Ok(request) => stub.handle(&request),
            Err(error) => graphql_error(&format!("bad request body: {error}"), "BAD_REQUEST"),
        },
    };
    let text = response.to_string();
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        text.len(),
        text
    )?;
    stream.flush()
}

fn graphql_error(message: &str, code: &str) -> Value {
    json!({ "errors": [{ "message": message, "extensions": { "code": code } }] })
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

struct Stub {
    viewer: Value,
    teams: Value,
    team: Value,
    /// Every known issue summary by id, in fixture order.
    issues: BTreeMap<String, Value>,
    order: Vec<String>,
    /// Detail-only fields (description, creator, comments, attachments) by id.
    details: BTreeMap<String, Map<String, Value>>,
    fail_mutations: bool,
    next_number: u64,
}

impl Stub {
    fn load() -> Stub {
        let mut stub = Stub {
            viewer: fixture("Viewer")["data"]["viewer"].clone(),
            teams: fixture("Teams")["data"].clone(),
            team: fixture("Team")["data"].clone(),
            issues: BTreeMap::new(),
            order: Vec::new(),
            details: BTreeMap::new(),
            fail_mutations: false,
            next_number: 900,
        };
        let lists = [
            fixture("TeamIssues")["data"]["issues"]["nodes"].clone(),
            fixture("MyOpenIssues")["data"]["viewer"]["assignedIssues"]["nodes"].clone(),
            fixture("IssuesByIdentifiers")["data"]["issues"]["nodes"].clone(),
            fixture("SearchIssues")["data"]["searchIssues"]["nodes"].clone(),
        ];
        for list in lists {
            for issue in list.as_array().into_iter().flatten() {
                stub.remember(issue.clone());
            }
        }
        let detail = fixture("Issue")["data"]["issue"].clone();
        let mut summary = detail.clone();
        let mut extra = Map::new();
        if let Some(object) = summary.as_object_mut() {
            for field in ["description", "creator", "comments", "attachments"] {
                if let Some(value) = object.remove(field) {
                    extra.insert(field.into(), value);
                }
            }
        }
        let id = summary["id"].as_str().unwrap_or_default().to_owned();
        stub.remember(summary);
        stub.details.insert(id, extra);
        stub
    }

    fn remember(&mut self, issue: Value) {
        let Some(id) = issue["id"].as_str().map(str::to_owned) else {
            return;
        };
        if !self.issues.contains_key(&id) {
            self.order.push(id.clone());
        }
        self.issues.insert(id, issue);
    }

    fn all_issues(&self) -> impl Iterator<Item = &Value> {
        self.order.iter().filter_map(|id| self.issues.get(id))
    }

    fn find(&self, id_or_identifier: &str) -> Option<&Value> {
        self.issues.get(id_or_identifier).or_else(|| {
            self.all_issues()
                .find(|issue| issue["identifier"] == id_or_identifier)
        })
    }

    fn handle(&mut self, request: &Value) -> Value {
        let operation = request["operationName"].as_str().unwrap_or_default();
        let variables = &request["variables"];
        let is_mutation = matches!(
            operation,
            "IssueUpdate"
                | "CommentCreate"
                | "IssueCreate"
                | "CustomViewCreate"
                | "CustomViewUpdate"
                | "CustomViewDelete"
                | "ViewPreferencesSave"
                | "FavoriteCreate"
        );
        if is_mutation && self.fail_mutations {
            return graphql_error("The stub refused this change (fail-on).", "INVALID_INPUT");
        }
        match operation {
            "Viewer" => json!({ "data": { "viewer": self.viewer } }),
            "CustomViews"
            | "CustomView"
            | "CustomViewCreate"
            | "CustomViewUpdate"
            | "CustomViewDelete"
            | "ViewPreferencesSave"
            | "FavoriteCreate"
            | "MyTeams"
            | "Favorites"
            | "RecentIssues"
            | "CustomViewIssues"
            | "Projects"
            | "Project"
            | "ProjectOptions" => fixture(operation),
            "IssuePullRequests" => {
                let key = variables["id"].as_str().unwrap_or_default();
                let pull_requests = self
                    .find(key)
                    .map(|issue| issue["pullRequests"]["nodes"].clone())
                    .unwrap_or_else(|| json!([]));
                json!({ "data": { "issue": { "pullRequests": {
                    "nodes": pull_requests,
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                } } } })
            }
            "Teams" => json!({ "data": self.teams }),
            "Team" => self.team_data(variables["id"].as_str().unwrap_or_default()),
            "MyOpenIssues" => {
                let me = self.viewer["id"].clone();
                let nodes: Vec<Value> = self
                    .newest_first()
                    .into_iter()
                    .filter(|issue| issue["assignee"]["id"] == me && !is_closed(issue))
                    .collect();
                json!({ "data": { "viewer": { "assignedIssues": {
                    "nodes": nodes,
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                } } } })
            }
            "TeamIssues" => {
                let filter = &variables["filter"];
                let nodes: Vec<Value> = self
                    .newest_first()
                    .into_iter()
                    .filter(|issue| self.matches(issue, filter))
                    .collect();
                json!({ "data": { "issues": {
                    "nodes": nodes,
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                } } })
            }
            "IssuesByIdentifiers" => {
                let filter = &variables["filter"];
                let nodes: Vec<Value> = self
                    .all_issues()
                    .filter(|issue| self.matches(issue, filter))
                    .cloned()
                    .collect();
                json!({ "data": { "issues": { "nodes": nodes } } })
            }
            "SearchIssues" => {
                let term = variables["term"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase();
                let first = variables["first"].as_u64().unwrap_or(10) as usize;
                let nodes: Vec<Value> = self
                    .all_issues()
                    .filter(|issue| {
                        let title = issue["title"].as_str().unwrap_or_default().to_lowercase();
                        let identifier = issue["identifier"]
                            .as_str()
                            .unwrap_or_default()
                            .to_lowercase();
                        title.contains(&term) || identifier.contains(&term)
                    })
                    .take(first)
                    .cloned()
                    .collect();
                json!({ "data": { "searchIssues": { "nodes": nodes } } })
            }
            "IssuesForUrl" => {
                let url = variables["url"].as_str().unwrap_or_default();
                let nodes: Vec<Value> = self
                    .details
                    .iter()
                    .filter(|(_, detail)| {
                        detail["attachments"]["nodes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|attachment| {
                                attachment["url"].as_str().map(|u| u.trim_end_matches('/'))
                                    == Some(url.trim_end_matches('/'))
                            })
                    })
                    .filter_map(|(id, _)| self.issues.get(id))
                    .map(|issue| json!({ "issue": issue }))
                    .collect();
                json!({ "data": { "attachmentsForURL": { "nodes": nodes } } })
            }
            "Issue" => {
                let key = variables["id"].as_str().unwrap_or_default();
                match self.find(key).cloned() {
                    Some(mut issue) => {
                        let id = issue["id"].as_str().unwrap_or_default().to_owned();
                        let detail = self.details.get(&id).cloned().unwrap_or_default();
                        if let Some(object) = issue.as_object_mut() {
                            object.insert(
                                "description".into(),
                                detail.get("description").cloned().unwrap_or(Value::Null),
                            );
                            object.insert(
                                "creator".into(),
                                detail.get("creator").cloned().unwrap_or(Value::Null),
                            );
                            for field in ["comments", "attachments"] {
                                object.insert(
                                    field.into(),
                                    detail
                                        .get(field)
                                        .cloned()
                                        .unwrap_or_else(|| json!({ "nodes": [] })),
                                );
                            }
                        }
                        json!({ "data": { "issue": issue } })
                    }
                    None => json!({
                        "data": null,
                        "errors": [{ "message": "Entity not found: Issue",
                                     "extensions": { "code": "INVALID_INPUT" } }],
                    }),
                }
            }
            "IssueUpdate" => self.update_issue(
                variables["id"].as_str().unwrap_or_default(),
                &variables["input"],
            ),
            "CommentCreate" => self.create_comment(&variables["input"]),
            "IssueCreate" => self.create_issue(&variables["input"]),
            other => graphql_error(&format!("the stub does not know {other:?}"), "BAD_REQUEST"),
        }
    }

    fn newest_first(&self) -> Vec<Value> {
        let mut issues: Vec<Value> = self.all_issues().cloned().collect();
        issues.sort_by(|a, b| {
            b["updatedAt"]
                .as_str()
                .unwrap_or_default()
                .cmp(a["updatedAt"].as_str().unwrap_or_default())
        });
        issues
    }

    /// The fixture team, renamed for any other team the key can see, so every
    /// team has a catalog.
    fn team_data(&self, id: &str) -> Value {
        let mut data = self.team.clone();
        let known = self.teams["teams"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|team| team["id"] == id || team["key"] == id)
            .cloned();
        if let Some(team) = known {
            data["team"]["id"] = team["id"].clone();
            data["team"]["key"] = team["key"].clone();
            data["team"]["name"] = team["name"].clone();
        }
        json!({ "data": data })
    }

    fn states(&self) -> Vec<Value> {
        self.team["team"]["states"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Evaluates the subset of Linear's `IssueFilter` the client sends.
    fn matches(&self, issue: &Value, filter: &Value) -> bool {
        let Some(filter) = filter.as_object() else {
            return true;
        };
        filter
            .iter()
            .all(|(field, condition)| match field.as_str() {
                "and" => condition
                    .as_array()
                    .into_iter()
                    .flatten()
                    .all(|clause| self.matches(issue, clause)),
                "or" => condition
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|clause| self.matches(issue, clause)),
                "team" => {
                    compare(&issue["team"]["id"], &condition["id"])
                        && compare(&issue["team"]["key"], &condition["key"])
                }
                "number" => {
                    let number = issue["identifier"]
                        .as_str()
                        .and_then(|identifier| identifier.rsplit_once('-'))
                        .and_then(|(_, number)| number.parse::<u64>().ok())
                        .map(Value::from)
                        .unwrap_or(Value::Null);
                    compare(&number, condition)
                }
                "state" => {
                    compare(&issue["state"]["id"], &condition["id"])
                        && compare(&issue["state"]["type"], &condition["type"])
                }
                "assignee" => {
                    let assignee = &issue["assignee"];
                    if condition["null"] == true {
                        return assignee.is_null();
                    }
                    if condition["isMe"]["eq"] == true {
                        return assignee["id"] == self.viewer["id"];
                    }
                    compare(&assignee["id"], &condition["id"])
                }
                "labels" if !condition["length"].is_null() => {
                    let count = issue["labels"]["nodes"].as_array().map_or(0, Vec::len);
                    compare(&Value::from(count), &condition["length"])
                }
                "labels" => {
                    let wanted = &condition["some"]["id"];
                    issue["labels"]["nodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|label| compare(&label["id"], wanted))
                }
                "project" => compare(&issue["project"]["id"], &condition["id"]),
                "priority" => compare(&issue["priority"], condition),
                "updatedAt" => compare(&issue["updatedAt"], condition),
                "title" => compare(&issue["title"], condition),
                "description" => {
                    let id = issue["id"].as_str().unwrap_or_default();
                    let description = self
                        .details
                        .get(id)
                        .and_then(|detail| detail.get("description"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    compare(&description, condition)
                }
                _ => true,
            })
    }

    fn update_issue(&mut self, id: &str, input: &Value) -> Value {
        let states = self.states();
        let members = self.team["team"]["members"]["nodes"].clone();
        let labels = self.team["issueLabels"]["nodes"].clone();
        let projects = self.team["team"]["projects"]["nodes"].clone();
        let cycles = self.team["team"]["cycles"]["nodes"].clone();
        let Some(issue) = self.issues.get_mut(id) else {
            return graphql_error("Entity not found: Issue", "INVALID_INPUT");
        };
        let by_id = |list: &Value, wanted: &Value| -> Option<Value> {
            list.as_array()?
                .iter()
                .find(|item| item["id"] == *wanted)
                .cloned()
        };
        let Some(input) = input.as_object() else {
            return graphql_error("input must be an object", "BAD_REQUEST");
        };
        for (field, value) in input {
            match field.as_str() {
                "stateId" => {
                    if let Some(state) = states.iter().find(|state| state["id"] == *value) {
                        issue["state"] = json!({
                            "id": state["id"], "name": state["name"],
                            "type": state["type"], "color": state["color"],
                        });
                    }
                }
                "priority" => issue["priority"] = value.clone(),
                "sortOrder" => issue["sortOrder"] = value.clone(),
                "assigneeId" => {
                    issue["assignee"] = by_id(&members, value)
                        .map(|member| {
                            json!({
                                "id": member["id"], "name": member["name"],
                                "displayName": member["displayName"],
                                "avatarUrl": member["avatarUrl"],
                            })
                        })
                        .unwrap_or(Value::Null);
                }
                "labelIds" => {
                    let nodes: Vec<Value> = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|wanted| by_id(&labels, wanted))
                        .collect();
                    issue["labels"] = json!({ "nodes": nodes });
                }
                "projectId" => {
                    issue["project"] = by_id(&projects, value).unwrap_or(Value::Null);
                }
                "cycleId" => {
                    issue["cycle"] = by_id(&cycles, value)
                        .map(|cycle| {
                            json!({ "id": cycle["id"], "number": cycle["number"], "name": cycle["name"] })
                        })
                        .unwrap_or(Value::Null);
                }
                "estimate" => issue["estimate"] = value.clone(),
                "dueDate" => issue["dueDate"] = value.clone(),
                "title" => issue["title"] = value.clone(),
                _ => {}
            }
        }
        issue["updatedAt"] = json!(now());
        let issue = issue.clone();
        if let Some(description) = input.get("description") {
            self.details
                .entry(id.to_owned())
                .or_default()
                .insert("description".into(), description.clone());
        }
        json!({ "data": { "issueUpdate": { "success": true, "issue": issue } } })
    }

    fn create_comment(&mut self, input: &Value) -> Value {
        let Some(issue_id) = input["issueId"].as_str() else {
            return graphql_error("issueId is required", "INVALID_INPUT");
        };
        if !self.issues.contains_key(issue_id) {
            return graphql_error("Entity not found: Issue", "INVALID_INPUT");
        }
        let comment = json!({
            "id": format!("stub-comment-{}", self.next_id()),
            "body": input["body"],
            "createdAt": now(),
            "editedAt": null,
            "user": {
                "id": self.viewer["id"], "name": self.viewer["name"],
                "displayName": self.viewer["displayName"], "avatarUrl": self.viewer["avatarUrl"],
            },
            "parent": null,
        });
        let detail = self.details.entry(issue_id.to_owned()).or_default();
        let comments = detail
            .entry("comments")
            .or_insert_with(|| json!({ "nodes": [] }));
        if let Some(nodes) = comments["nodes"].as_array_mut() {
            nodes.push(comment.clone());
        }
        json!({ "data": { "commentCreate": { "success": true, "comment": comment } } })
    }

    fn create_issue(&mut self, input: &Value) -> Value {
        let title = input["title"].as_str().unwrap_or_default();
        if title.trim().is_empty() {
            return graphql_error("title is required", "INVALID_INPUT");
        }
        let team =
            self.team_data(input["teamId"].as_str().unwrap_or_default())["data"]["team"].clone();
        let states = self.states();
        let state = input["stateId"]
            .as_str()
            .and_then(|id| states.iter().find(|state| state["id"] == id))
            .or_else(|| states.iter().find(|state| state["type"] == "unstarted"))
            .cloned()
            .unwrap_or(Value::Null);
        let number = self.next_id();
        let identifier = format!("{}-{number}", team["key"].as_str().unwrap_or("STUB"));
        let id = format!("stub-issue-{number}");
        let created = now();
        let issue = json!({
            "id": id,
            "identifier": identifier,
            "title": title,
            "priority": input["priority"].as_i64().unwrap_or(0),
            "sortOrder": -(number as f64),
            "estimate": null,
            "dueDate": null,
            "url": format!("https://linear.app/stub/issue/{identifier}"),
            "createdAt": created,
            "updatedAt": created,
            "state": { "id": state["id"], "name": state["name"], "type": state["type"], "color": state["color"] },
            "team": { "id": team["id"], "key": team["key"], "name": team["name"] },
            "assignee": null,
            "labels": { "nodes": [] },
            "project": null,
            "cycle": null,
            "creator": { "id": self.viewer["id"] },
            "parent": null,
            "children": { "nodes": [] },
            "projectMilestone": null,
            "completedAt": null,
            "canceledAt": null,
            "pullRequests": { "nodes": [] },
        });
        self.remember(issue.clone());
        let assignee = input["assigneeId"].clone();
        let mut extra = json!({});
        if !assignee.is_null() {
            extra["assigneeId"] = assignee;
        }
        if let Some(labels) = input.get("labelIds") {
            extra["labelIds"] = labels.clone();
        }
        let issue = if extra.as_object().is_some_and(|object| !object.is_empty()) {
            self.update_issue(&id, &extra)["data"]["issueUpdate"]["issue"].clone()
        } else {
            issue
        };
        let mut detail = Map::new();
        detail.insert(
            "description".into(),
            input.get("description").cloned().unwrap_or(Value::Null),
        );
        self.details.insert(id, detail);
        json!({ "data": { "issueCreate": { "success": true, "issue": issue } } })
    }

    fn next_id(&mut self) -> u64 {
        self.next_number += 1;
        self.next_number
    }
}

fn is_closed(issue: &Value) -> bool {
    CLOSED_TYPES.contains(&issue["state"]["type"].as_str().unwrap_or_default())
}

/// Applies one Linear comparator (`eq`, `neq`, `in`, `nin`, `gt`, `lt`,
/// `containsIgnoreCase`, `null`) to `value`. A missing comparator matches.
fn compare(value: &Value, comparator: &Value) -> bool {
    let Some(comparator) = comparator.as_object() else {
        return true;
    };
    comparator.iter().all(|(op, operand)| match op.as_str() {
        "eq" => value == operand,
        "neq" => value != operand,
        "in" => operand
            .as_array()
            .is_some_and(|items| items.contains(value)),
        "nin" => operand
            .as_array()
            .is_none_or(|items| !items.contains(value)),
        "gt" => order(value, operand).is_some_and(|ordering| ordering.is_gt()),
        "lt" => order(value, operand).is_some_and(|ordering| ordering.is_lt()),
        "null" => value.is_null() == (operand == true),
        "containsIgnoreCase" => match (value.as_str(), operand.as_str()) {
            (Some(text), Some(needle)) => text.to_lowercase().contains(&needle.to_lowercase()),
            _ => false,
        },
        _ => true,
    })
}

fn order(value: &Value, operand: &Value) -> Option<std::cmp::Ordering> {
    match (value, operand) {
        (Value::String(a), Value::String(b)) => Some(a.as_str().cmp(b.as_str())),
        (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
        _ => None,
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
