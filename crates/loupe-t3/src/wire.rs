//! Private wire types for the T3 HTTP API, their conversion into the public
//! domain types, and the desktop deep link. Nothing here is exported.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer};
use serde_json::Value;

use crate::{
    Environment, Error, LinkedThread, OPERATE_SCOPE, Project, PullRequestKey, ReviewFacts, Session,
    ThreadStatus,
};

/// Decodes a list one element at a time and drops the elements that do not
/// fit, so one item from a newer server never fails the whole response.
fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Vec::<Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvironmentWire {
    environment_id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    server_version: String,
}

impl From<EnvironmentWire> for Environment {
    fn from(wire: EnvironmentWire) -> Self {
        Environment {
            id: wire.environment_id,
            label: wire.label,
            server_version: wire.server_version,
        }
    }
}

/// `GET /api/auth/session`. Newer servers list the exact grant in
/// `permissions`; older ones only have `scopes`.
#[derive(Deserialize)]
pub(crate) struct SessionWire {
    #[serde(default)]
    authenticated: bool,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    permissions: Option<Vec<String>>,
}

impl SessionWire {
    pub(crate) fn into_session(self) -> Result<Session, Error> {
        if !self.authenticated {
            return Err(Error::Unauthorized);
        }
        let granted = self.permissions.unwrap_or(self.scopes);
        Ok(Session {
            can_operate: granted.iter().any(|scope| scope == OPERATE_SCOPE),
        })
    }
}

/// `GET /api/projects`.
#[derive(Deserialize)]
pub(crate) struct ProjectsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    projects: Vec<ProjectDetailWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDetailWire {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    repository_identity: Option<RepositoryIdentityWire>,
    #[serde(default)]
    archived_at: Option<Value>,
    #[serde(default)]
    deleted_at: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryIdentityWire {
    #[serde(default)]
    canonical_key: Option<String>,
}

impl ProjectsWire {
    pub(crate) fn into_projects(self) -> Vec<Project> {
        self.projects
            .into_iter()
            .filter(|project| project.archived_at.is_none() && project.deleted_at.is_none())
            .map(|project| Project {
                repository: project
                    .repository_identity
                    .and_then(|identity| identity.canonical_key)
                    .filter(|key| !key.is_empty()),
                id: project.id,
                title: project.title,
            })
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    threads: Vec<ThreadShellWire>,
    #[serde(default, deserialize_with = "lenient_vec")]
    archived_threads: Vec<ThreadShellWire>,
    #[serde(default, deserialize_with = "lenient_vec")]
    projects: Vec<ProjectWire>,
}

#[derive(Deserialize)]
struct ProjectWire {
    id: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadShellWire {
    id: String,
    project_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    settled_override: Option<String>,
    #[serde(default)]
    settled_at: Option<Value>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    deleted_at: Option<Value>,
    #[serde(default)]
    model_selection: Option<ModelSelectionWire>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pull_requests: Vec<PullRequestLinkWire>,
    #[serde(default)]
    linked_pull_request: Option<ThreadPullRequestWire>,
    #[serde(default)]
    branch_pull_request: Option<ThreadPullRequestWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelSelectionWire {
    #[serde(default)]
    instance_id: String,
    #[serde(default)]
    model: String,
}

#[derive(Deserialize)]
struct PullRequestLinkWire {
    host: String,
    repository: String,
    number: u64,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct ThreadPullRequestWire {
    repository: String,
    number: u64,
}

const GITHUB_HOST: &str = "github.com";
const DISMISSED_SOURCE: &str = "stack-dismissed";

impl ThreadShellWire {
    fn links(&self, pr: &PullRequestKey) -> bool {
        let listed = self.pull_requests.iter().any(|link| {
            link.source.as_deref() != Some(DISMISSED_SOURCE)
                && pr.matches(&link.host, &link.repository, link.number)
        });
        let legacy = [&self.linked_pull_request, &self.branch_pull_request]
            .into_iter()
            .flatten()
            .any(|link| pr.matches(GITHUB_HOST, &link.repository, link.number));
        listed || legacy
    }

    /// The fork's own rule for a settled thread: the user settled it, or
    /// auto-settlement stamped it.
    fn is_settled(&self) -> bool {
        self.settled_override.as_deref() == Some("settled") || self.settled_at.is_some()
    }
}

fn thread_status(status: &str) -> ThreadStatus {
    match status {
        "idle" => ThreadStatus::Idle,
        "queued" => ThreadStatus::Queued,
        "preparing" => ThreadStatus::Preparing,
        "starting" => ThreadStatus::Starting,
        "running" => ThreadStatus::Running,
        "waiting" => ThreadStatus::Waiting,
        "completed" => ThreadStatus::Completed,
        "interrupted" => ThreadStatus::Interrupted,
        "failed" => ThreadStatus::Failed,
        "cancelled" => ThreadStatus::Cancelled,
        "rolled_back" => ThreadStatus::RolledBack,
        other => ThreadStatus::Unknown(other.to_owned()),
    }
}

pub(crate) fn linked_threads(shell: ShellWire, pr: &PullRequestKey) -> Vec<LinkedThread> {
    let ShellWire {
        threads,
        archived_threads,
        projects,
    } = shell;
    let mut seen = HashSet::new();
    let mut linked: Vec<LinkedThread> = threads
        .into_iter()
        .chain(archived_threads)
        .filter(|thread| thread.deleted_at.is_none() && thread.links(pr))
        .filter(|thread| seen.insert(thread.id.clone()))
        .map(|thread| {
            let project = projects
                .iter()
                .find(|project| project.id == thread.project_id)
                .map(|project| project.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or(&thread.project_id)
                .to_owned();
            LinkedThread {
                settled: thread.is_settled(),
                status: thread_status(&thread.status),
                id: thread.id,
                title: thread.title,
                project,
                updated_at: thread.updated_at,
            }
        })
        .collect();
    linked.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    linked
}

/// What a review launch needs from the shell: the provider instance of the
/// most recently updated active thread on each of `models` (an archived or
/// deleted thread may have run on an instance that is now off), and the worktree
/// branches each project's threads hold, archived ones included.
pub(crate) fn review_facts(shell: ShellWire, models: &[&str]) -> ReviewFacts {
    let mut branches: HashMap<String, Vec<String>> = HashMap::new();
    for thread in shell.threads.iter().chain(&shell.archived_threads) {
        if let Some(branch) = thread.branch.as_ref().filter(|branch| !branch.is_empty()) {
            branches
                .entry(thread.project_id.clone())
                .or_default()
                .push(branch.clone());
        }
    }
    let mut latest: HashMap<String, (DateTime<Utc>, String)> = HashMap::new();
    for thread in shell.threads {
        if thread.deleted_at.is_some() {
            continue;
        }
        let Some(selection) = thread.model_selection else {
            continue;
        };
        if selection.instance_id.is_empty() || !models.contains(&selection.model.as_str()) {
            continue;
        }
        let newer = latest
            .get(&selection.model)
            .is_none_or(|(at, _)| thread.updated_at > *at);
        if newer {
            latest.insert(selection.model, (thread.updated_at, selection.instance_id));
        }
    }
    ReviewFacts {
        instances: latest
            .into_iter()
            .map(|(model, (_, instance))| (model, instance))
            .collect(),
        branches,
    }
}

fn is_pchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
        )
}

fn percent_encode(text: &str, keep: fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if keep(byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) fn thread_deep_link(environment_id: &str, thread_id: &str) -> String {
    format!(
        "t3code://threads/{}/{}",
        percent_encode(environment_id, is_pchar),
        percent_encode(thread_id, is_pchar)
    )
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};

    use super::{EnvironmentWire, ProjectsWire, SessionWire, ShellWire, thread_deep_link};
    use crate::{Environment, Error, LinkedThread, PullRequestKey, Session, ThreadStatus};

    const SHELL: &str = include_str!("../tests/fixtures/shell.json");
    const ENVIRONMENT: &str = include_str!("../tests/fixtures/environment.json");

    const THREAD: &str = "mcp:7161af08-315d-402d-a20e-3711a7c1de88";
    const ENVIRONMENT_ID: &str = "ca59678f-8a71-478a-aa7f-b6b167b81412";

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn pr(number: u64) -> PullRequestKey {
        PullRequestKey::github("IsraelAraujo70/t3code", number)
    }

    fn shell_json() -> Value {
        serde_json::from_str(SHELL).unwrap()
    }

    fn linked(shell: Value, number: u64) -> Vec<LinkedThread> {
        let shell: ShellWire = serde_json::from_value(shell).unwrap();
        super::linked_threads(shell, &pr(number))
    }

    /// The shape the real server answered on 2026-11-11, cut to what Loupe
    /// reads.
    #[test]
    fn a_session_can_operate_only_with_the_scope() {
        let session = |body: Value| {
            serde_json::from_value::<SessionWire>(body)
                .unwrap()
                .into_session()
        };
        let full = json!({"authenticated": true,
            "scopes": ["orchestration:read", "orchestration:operate"],
            "permissions": ["orchestration:read", "orchestration:operate"]});
        assert_eq!(session(full).unwrap(), Session { can_operate: true });
        let read = json!({"authenticated": true, "scopes": ["orchestration:read"],
            "permissions": ["orchestration:read"]});
        assert_eq!(session(read).unwrap(), Session { can_operate: false });
        // The exact grant wins over the legacy names when both are there.
        let narrowed = json!({"authenticated": true,
            "scopes": ["orchestration:read", "orchestration:operate"],
            "permissions": ["orchestration:read"]});
        assert_eq!(session(narrowed).unwrap(), Session { can_operate: false });
        // An older server has no `permissions`.
        let legacy = json!({"authenticated": true, "scopes": ["orchestration:operate"]});
        assert_eq!(session(legacy).unwrap(), Session { can_operate: true });
        assert!(matches!(
            session(json!({"authenticated": false})),
            Err(Error::Unauthorized)
        ));
    }

    #[test]
    fn a_project_is_found_by_its_repository_whatever_the_case() {
        let wire: ProjectsWire = serde_json::from_value(json!({"projects": [
            {"id": "p-api", "title": "hooli-api", "repositoryIdentity":
                {"canonicalKey": "github.com/hooli-inc/hooli-api"},
             "archivedAt": null, "deletedAt": null},
            {"id": "p-old", "title": "hooli-api (old)", "repositoryIdentity":
                {"canonicalKey": "github.com/hooli-inc/hooli-api"},
             "archivedAt": "2026-11-01T00:00:00.000Z"},
            {"id": "p-scratch", "title": "Scratch", "repositoryIdentity": null},
            {"title": "no id: skipped"},
        ], "updatedAt": "2026-11-11T00:00:00.000Z"}))
        .unwrap();
        let projects = wire.into_projects();
        let ids: Vec<&str> = projects.iter().map(|project| project.id.as_str()).collect();
        assert_eq!(ids, ["p-api", "p-scratch"]);

        let pr = PullRequestKey::github("hooli-inc/hooli-api", 374);
        let found: Vec<&str> = projects
            .iter()
            .filter(|project| project.holds(&pr))
            .map(|project| project.id.as_str())
            .collect();
        assert_eq!(found, ["p-api"]);
        // A longer name is another repository.
        let other = PullRequestKey::github("hooli-inc/hooli-api-docs", 1);
        assert!(!projects.iter().any(|project| project.holds(&other)));
    }

    #[test]
    fn a_model_runs_on_the_instance_of_its_latest_active_thread() {
        let thread = |id: &str, at: &str, instance: &str, model: &str| {
            json!({"id": id, "projectId": "p", "updatedAt": at,
                   "modelSelection": {"instanceId": instance, "model": model}})
        };
        let shell: ShellWire = serde_json::from_value(json!({
            "threads": [
                thread("a", "2026-11-01T00:00:00Z", "claudeAgent", "claude-opus-5-5"),
                thread("b", "2026-11-10T00:00:00Z", "claude_pool", "claude-opus-5-5"),
                thread("c", "2026-11-05T00:00:00Z", "codex", "gpt-6"),
                thread("f", "2026-11-08T00:00:00Z", "claude_pool", "claude-fable-5-2"),
                {"id": "g", "projectId": "p", "updatedAt": "2026-11-11T00:00:00Z",
                 "deletedAt": "2026-11-11T00:00:00Z",
                 "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5-5"}},
                {"id": "d", "projectId": "p", "updatedAt": "2026-11-11T00:00:00Z"},
            ],
            "archivedThreads": [
                // Archived today, on an instance that is off now.
                thread("e", "2026-11-11T00:00:00Z", "claudeAgent", "claude-fable-5-2"),
            ],
        }))
        .unwrap();
        let found =
            super::review_facts(shell, &["claude-opus-5-5", "claude-fable-5-2", "none"]).instances;
        assert_eq!(found.len(), 2);
        assert_eq!(found["claude-opus-5-5"], "claude_pool");
        assert_eq!(found["claude-fable-5-2"], "claude_pool");
    }

    #[test]
    fn every_thread_of_a_project_holds_its_branch() {
        let thread = |id: &str, project: &str, branch: Option<&str>| {
            json!({"id": id, "projectId": project, "updatedAt": "2026-11-11T00:00:00Z",
                   "branch": branch})
        };
        let shell: ShellWire = serde_json::from_value(json!({
            "threads": [
                thread("a", "api", Some("review/pr-374")),
                thread("b", "api", None),
                thread("c", "web", Some("review/pr-9")),
            ],
            "archivedThreads": [thread("d", "api", Some("review/pr-374-2"))],
        }))
        .unwrap();
        let branches = super::review_facts(shell, &[]).branches;
        assert_eq!(branches["api"], ["review/pr-374", "review/pr-374-2"]);
        assert_eq!(branches["web"], ["review/pr-9"]);
    }

    #[test]
    fn environment_descriptor_names_the_server() {
        let wire: EnvironmentWire = serde_json::from_str(ENVIRONMENT).unwrap();
        assert_eq!(
            Environment::from(wire),
            Environment {
                id: ENVIRONMENT_ID.into(),
                label: "srv2106147".into(),
                server_version: "0.1.7".into(),
            }
        );
    }

    #[test]
    fn shell_thread_matches_its_linked_pull_request() {
        let threads = linked(shell_json(), 23);
        assert_eq!(
            threads,
            vec![LinkedThread {
                id: THREAD.into(),
                title: "Scratch: idle compaction fix review".into(),
                project: "t3code scratch".into(),
                status: ThreadStatus::Completed,
                settled: false,
                updated_at: at("2026-11-10T14:26:16.588Z"),
            }]
        );
        assert_eq!(linked(shell_json(), 22).len(), 1);
    }

    #[test]
    fn shell_pull_request_without_a_link_matches_nothing() {
        assert!(linked(shell_json(), 21).is_empty());
    }

    #[test]
    fn a_dismissed_stack_link_does_not_count() {
        let mut shell = shell_json();
        shell["threads"][0]["pullRequests"][0]["source"] = json!("stack-dismissed");
        assert!(linked(shell.clone(), 23).is_empty());
        assert_eq!(linked(shell, 22).len(), 1);
    }

    #[test]
    fn the_branch_pull_request_matches_as_github() {
        let mut shell = shell_json();
        shell["threads"][0]["pullRequests"] = json!([]);
        shell["threads"][0]["branchPullRequest"] = json!({
            "projectId": "6da0bfb8-e3d0-47f7-a4b3-092a01c07b56",
            "repository": "israelaraujo70/t3code",
            "number": 31,
            "url": "https://github.com/IsraelAraujo70/t3code/pull/31",
        });
        assert_eq!(linked(shell.clone(), 31).len(), 1);
        assert!(linked(shell, 23).is_empty());
    }

    #[test]
    fn the_legacy_linked_pull_request_matches() {
        let mut shell = shell_json();
        shell["threads"][0]["pullRequests"] = json!([]);
        shell["threads"][0]["linkedPullRequest"] = json!({
            "projectId": "6da0bfb8-e3d0-47f7-a4b3-092a01c07b56",
            "repository": "israelaraujo70/t3code",
            "number": 40,
            "url": "https://github.com/IsraelAraujo70/t3code/pull/40",
        });
        assert_eq!(linked(shell, 40).len(), 1);
    }

    #[test]
    fn a_link_on_another_host_does_not_match_a_github_pull_request() {
        let mut shell = shell_json();
        shell["threads"][0]["pullRequests"][0]["host"] = json!("github.example.com");
        assert!(linked(shell, 23).is_empty());
    }

    #[test]
    fn a_deleted_thread_is_skipped() {
        let mut shell = shell_json();
        shell["threads"][0]["deletedAt"] = json!("2026-11-10T15:00:00.000Z");
        assert!(linked(shell, 23).is_empty());
    }

    #[test]
    fn an_unknown_status_is_kept_verbatim() {
        let mut shell = shell_json();
        shell["threads"][0]["status"] = json!("hibernating");
        let threads = linked(shell, 23);
        assert_eq!(
            threads[0].status,
            ThreadStatus::Unknown("hibernating".into())
        );
        assert_eq!(threads[0].status.label(), "hibernating");
    }

    #[test]
    fn every_known_status_maps() {
        let expected = [
            ("idle", ThreadStatus::Idle),
            ("queued", ThreadStatus::Queued),
            ("preparing", ThreadStatus::Preparing),
            ("starting", ThreadStatus::Starting),
            ("running", ThreadStatus::Running),
            ("waiting", ThreadStatus::Waiting),
            ("completed", ThreadStatus::Completed),
            ("interrupted", ThreadStatus::Interrupted),
            ("failed", ThreadStatus::Failed),
            ("cancelled", ThreadStatus::Cancelled),
            ("rolled_back", ThreadStatus::RolledBack),
        ];
        for (wire, status) in expected {
            assert_eq!(super::thread_status(wire), status, "{wire}");
        }
    }

    #[test]
    fn a_thread_is_settled_by_override_or_by_timestamp() {
        let settled = |override_: Value, settled_at: Value| {
            let mut shell = shell_json();
            shell["threads"][0]["settledOverride"] = override_;
            shell["threads"][0]["settledAt"] = settled_at;
            linked(shell, 23)[0].settled
        };
        assert!(settled(json!("settled"), Value::Null));
        assert!(settled(Value::Null, json!("2026-11-10T15:00:00.000Z")));
        assert!(!settled(json!("active"), Value::Null));
        assert!(!settled(Value::Null, Value::Null));
    }

    #[test]
    fn threads_sort_newest_first_and_dedupe_by_id_across_archive() {
        let mut shell = shell_json();
        let base = shell["threads"][0].clone();
        let with = |id: &str, updated: &str| {
            let mut thread = base.clone();
            thread["id"] = json!(id);
            thread["updatedAt"] = json!(updated);
            thread
        };
        shell["threads"] = json!([
            with("older", "2026-11-09T10:00:00.000Z"),
            base,
            with("newest", "2026-11-11T10:00:00.000Z"),
        ]);
        shell["archivedThreads"] = json!([
            with("oldest", "2026-11-01T10:00:00.000Z"),
            with("older", "2026-11-12T10:00:00.000Z"),
        ]);
        let ids: Vec<String> = linked(shell, 23)
            .into_iter()
            .map(|thread| thread.id)
            .collect();
        assert_eq!(ids, ["newest", THREAD, "older", "oldest"]);
    }

    #[test]
    fn the_project_id_stands_in_for_a_missing_project() {
        let mut shell = shell_json();
        shell["projects"] = json!([]);
        assert_eq!(
            linked(shell, 23)[0].project,
            "6da0bfb8-e3d0-47f7-a4b3-092a01c07b56"
        );
    }

    #[test]
    fn a_malformed_thread_does_not_fail_the_snapshot() {
        let mut shell = shell_json();
        shell["threads"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 5}));
        assert_eq!(linked(shell, 23).len(), 1);
    }

    #[test]
    fn deep_link_keeps_colons_and_encodes_the_rest() {
        assert_eq!(
            thread_deep_link(ENVIRONMENT_ID, THREAD),
            format!("t3code://threads/{ENVIRONMENT_ID}/{THREAD}")
        );
        assert_eq!(
            thread_deep_link("env id", "a/b c#d?\u{e9}"),
            "t3code://threads/env%20id/a%2Fb%20c%23d%3F%C3%A9"
        );
    }
}
