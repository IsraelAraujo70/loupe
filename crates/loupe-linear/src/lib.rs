//! Linear API client for Loupe. Pure Rust, no GPUI: every call is blocking and
//! is meant to run on a background thread. Authenticates with a personal API
//! key, which Linear takes bare in `Authorization` (no `Bearer`).
//!
//! In debug builds, `LOUPE_LINEAR_ENDPOINT`, read once per process, replaces
//! the GraphQL endpoint; the local stub server (`examples/stub_server.rs`)
//! uses it. Release builds ignore it, so the environment cannot send the API
//! key to another host.

pub mod documents;
pub mod filter;
mod identifiers;
mod types;
#[doc(hidden)]
pub mod variables;
pub mod views;
mod wire;

use std::sync::OnceLock;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub use documents::Document;
pub use identifiers::extract_identifiers;
pub use types::*;

const GRAPHQL_URL: &str = "https://api.linear.app/graphql";
const USER_AGENT: &str = concat!("Loupe/", env!("CARGO_PKG_VERSION"));
const MY_ISSUES_MAX_PAGES: usize = 5;

/// Every way a Linear call can fail, already classified at the boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Linear rejected the API key")]
    Unauthorized,
    #[error("Linear returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("Linear GraphQL error: {}", .0.join("; "))]
    GraphQl(Vec<String>),
    #[error("network error: {0}")]
    Transport(String),
    #[error("unexpected Linear response: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// An authenticated Linear API client. Cheap to clone; share it behind `Arc`.
#[derive(Clone)]
pub struct Linear {
    api_key: String,
    endpoint: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Linear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Linear")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// The endpoint every client built with [`Linear::new`] uses.
fn default_endpoint() -> &'static str {
    static ENDPOINT: OnceLock<String> = OnceLock::new();
    ENDPOINT.get_or_init(|| {
        std::env::var("LOUPE_LINEAR_ENDPOINT")
            .ok()
            .filter(|value| cfg!(debug_assertions) && !value.trim().is_empty())
            .unwrap_or_else(|| GRAPHQL_URL.to_owned())
    })
}

impl Linear {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_endpoint(api_key, default_endpoint())
    }

    /// A client for another GraphQL endpoint, such as a test stub.
    pub fn with_endpoint(api_key: impl Into<String>, endpoint: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            api_key: api_key.into(),
            endpoint: endpoint.into(),
            agent,
        }
    }

    pub fn viewer(&self) -> Result<Viewer> {
        let data: wire::ViewerData = self.graphql(&documents::VIEWER, json!({}))?;
        Ok(data.into_domain())
    }

    /// Every team the key can see, sorted by name.
    pub fn teams(&self) -> Result<Vec<TeamSummary>> {
        let data: wire::TeamsData = self.graphql(&documents::TEAMS, json!({}))?;
        Ok(data.into_domain())
    }

    /// The teams the key's owner is a member of, sorted by name.
    pub fn my_teams(&self) -> Result<Vec<TeamSummary>> {
        let data: wire::MyTeamsData = self.graphql(&documents::MY_TEAMS, json!({}))?;
        Ok(data.into_domain())
    }

    /// One team with its states, members, labels, projects, and open cycles.
    pub fn team(&self, team_id: &str) -> Result<Team> {
        let variables = variables::team(team_id, chrono::Utc::now());
        let data: wire::TeamData = self.graphql(&documents::TEAM, variables)?;
        Ok(data.into_domain())
    }

    /// Open issues assigned to the key's owner across all teams, newest
    /// update first.
    pub fn my_open_issues(&self) -> Result<Vec<IssueSummary>> {
        let mut issues = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MY_ISSUES_MAX_PAGES {
            let variables = variables::my_open_issues(after.as_deref());
            let data: wire::MyOpenIssuesData =
                self.graphql(&documents::MY_OPEN_ISSUES, variables)?;
            let page = data.into_page();
            wire::merge_unique(&mut issues, page.nodes);
            match page.next_cursor {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        self.complete_pull_requests(&mut issues)?;
        Ok(issues)
    }

    /// One page of a team's issues, newest update first. Linear filters with
    /// a superset of `query` (see [`filter::to_issue_filter`]); run
    /// [`filter::matches_query`] on the result for the exact set.
    pub fn team_issues(
        &self,
        team_id: &str,
        query: &IssueQuery,
        after: Option<&str>,
    ) -> Result<Page<IssueSummary>> {
        self.issues(Some(team_id), query, after)
    }

    /// Like [`Self::team_issues`], for one team or, with `None`, for every
    /// team the key can see.
    pub fn issues(
        &self,
        team_id: Option<&str>,
        query: &IssueQuery,
        after: Option<&str>,
    ) -> Result<Page<IssueSummary>> {
        let variables = variables::issues(team_id, query, after, chrono::Utc::now());
        let data: wire::TeamIssuesData = self.graphql(&documents::TEAM_ISSUES, variables)?;
        let mut page = data.into_page();
        self.complete_pull_requests(&mut page.nodes)?;
        Ok(page)
    }

    /// One issue by id or identifier (`ENG-123`).
    pub fn issue(&self, id_or_identifier: &str) -> Result<IssueDetail> {
        let data: wire::IssueData =
            self.graphql(&documents::ISSUE, json!({ "id": id_or_identifier }))?;
        let mut detail = data.into_domain();
        self.complete_pull_requests(std::slice::from_mut(&mut detail.issue))?;
        Ok(detail)
    }

    /// Full-text search across the workspace, best match first.
    pub fn search_issues(&self, term: &str, first: usize) -> Result<Vec<IssueSummary>> {
        let variables = json!({ "term": term, "first": first.clamp(1, 100) });
        let data: wire::SearchIssuesData = self.graphql(&documents::SEARCH_ISSUES, variables)?;
        let mut issues = data.into_domain();
        self.complete_pull_requests(&mut issues)?;
        Ok(issues)
    }

    /// Issues of the team whose key is `team_key` (`ENG`): the best matches
    /// for `term`, or the most recently updated when `term` is blank.
    pub fn team_key_issues(
        &self,
        team_key: &str,
        term: &str,
        first: usize,
    ) -> Result<Vec<IssueSummary>> {
        let filter = variables::team_key_filter(team_key);
        let first = first.clamp(1, 100);
        let term = term.trim();
        let mut issues = if term.is_empty() {
            let data: wire::IssuesByIdentifiersData = self.graphql(
                &documents::RECENT_ISSUES,
                json!({ "filter": filter, "first": first }),
            )?;
            data.into_domain()
        } else {
            let data: wire::SearchIssuesData = self.graphql(
                &documents::SEARCH_ISSUES,
                json!({ "term": term, "first": first, "filter": filter }),
            )?;
            data.into_domain()
        };
        self.complete_pull_requests(&mut issues)?;
        Ok(issues)
    }

    /// Issues that have an attachment for `url`, such as a pull request URL.
    pub fn issues_for_url(&self, url: &str) -> Result<Vec<IssueSummary>> {
        let data: wire::IssuesForUrlData =
            self.graphql(&documents::ISSUES_FOR_URL, json!({ "url": url }))?;
        let mut issues = data.into_domain();
        self.complete_pull_requests(&mut issues)?;
        Ok(issues)
    }

    /// The issues these identifiers name. Identifiers that match no issue are
    /// skipped, not errors.
    pub fn issues_by_identifiers(&self, identifiers: &[String]) -> Result<Vec<IssueSummary>> {
        let mut pairs: Vec<(String, u64)> = Vec::new();
        for identifier in identifiers {
            if let Some(pair) = variables::parse_identifier(identifier)
                && !pairs.contains(&pair)
            {
                pairs.push(pair);
            }
        }
        let mut issues = Vec::new();
        for chunk in pairs.chunks(variables::IDENTIFIER_CHUNK) {
            let data: wire::IssuesByIdentifiersData = self.graphql(
                &documents::ISSUES_BY_IDENTIFIERS,
                variables::issues_by_identifiers(chunk),
            )?;
            wire::merge_unique(&mut issues, data.into_domain());
        }
        self.complete_pull_requests(&mut issues)?;
        Ok(issues)
    }

    /// Every custom view the key can see, paginated until complete.
    pub fn custom_views(&self) -> Result<Vec<LinearCustomView>> {
        let mut views = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let data: wire::CustomViewsData = self.graphql(
                &documents::CUSTOM_VIEWS,
                variables::custom_views(after.as_deref()),
            )?;
            let page = data.into_page();
            views.extend(page.nodes);
            match page.next_cursor {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        Ok(views)
    }

    /// Saves a new view in Linear and returns it as Linear stored it.
    pub fn create_custom_view(&self, view: &NewCustomView) -> Result<LinearCustomView> {
        let variables = json!({ "input": variables::custom_view_create_input(view) });
        let data: wire::CustomViewCreateData =
            self.graphql(&documents::CUSTOM_VIEW_CREATE, variables)?;
        data.into_domain()
    }

    /// Changes a saved view's name or filter and returns it as Linear
    /// stored it.
    pub fn update_custom_view(
        &self,
        id: &str,
        update: &CustomViewUpdate,
    ) -> Result<LinearCustomView> {
        let variables = json!({
            "id": id,
            "input": variables::custom_view_update_input(update),
        });
        let data: wire::CustomViewUpdateData =
            self.graphql(&documents::CUSTOM_VIEW_UPDATE, variables)?;
        data.into_domain()
    }

    pub fn delete_custom_view(&self, id: &str) -> Result<()> {
        let data: wire::SuccessData =
            self.graphql(&documents::CUSTOM_VIEW_DELETE, json!({ "id": id }))?;
        data.into_result("delete the view")
    }

    /// Writes the key owner's display settings for a saved view, replacing
    /// the ones that owner had, and returns the view read again.
    /// `preferences` comes from [`views::export_preferences`].
    pub fn save_view_preferences(&self, id: &str, preferences: &Value) -> Result<LinearCustomView> {
        let variables = variables::view_preferences_save(id, preferences);
        let data: wire::SuccessData = self.graphql(&documents::VIEW_PREFERENCES_SAVE, variables)?;
        data.into_result("save the view's display settings")?;
        let data: wire::CustomViewData =
            self.graphql(&documents::CUSTOM_VIEW, json!({ "id": id }))?;
        data.into_domain()
    }

    /// Adds a saved view to the key owner's favorites.
    pub fn favorite_custom_view(&self, id: &str) -> Result<()> {
        let variables = json!({ "input": { "customViewId": id } });
        let data: wire::SuccessData = self.graphql(&documents::FAVORITE_CREATE, variables)?;
        data.into_result("add the favorite")
    }

    /// Personal favorites in sidebar order, including nested folders.
    pub fn favorites(&self) -> Result<Vec<LinearFavorite>> {
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let data: wire::FavoritesData = self.graphql(
                &documents::FAVORITES,
                variables::favorites(after.as_deref()),
            )?;
            let page = data.into_page();
            rows.extend(page.nodes);
            match page.next_cursor {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        Ok(wire::favorites_in_order(rows))
    }

    /// One page of the saved view's exact issue set, evaluated by Linear.
    pub fn custom_view_issues(&self, id: &str, after: Option<&str>) -> Result<Page<IssueSummary>> {
        let data: wire::CustomViewIssuesData = self.graphql(
            &documents::CUSTOM_VIEW_ISSUES,
            json!({"id": id, "after": after}),
        )?;
        let mut page = data.into_page();
        self.complete_pull_requests(&mut page.nodes)?;
        Ok(page)
    }

    /// Complete saved-view issue set, so client-side ordering never sorts a
    /// recent page while omitting higher-ranked issues on later pages.
    pub fn custom_view_all_issues(&self, id: &str) -> Result<Vec<IssueSummary>> {
        let mut issues = Vec::new();
        let mut after = None;
        loop {
            let page = self.custom_view_issues(id, after.as_deref())?;
            wire::merge_unique(&mut issues, page.nodes);
            match page.next_cursor {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        Ok(issues)
    }

    /// Projects, newest update first, up to five pages of 50; with `team_id`,
    /// only the projects that team can see.
    pub fn projects(&self, team_id: Option<&str>) -> Result<Vec<ProjectSummary>> {
        let mut projects = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..5 {
            let data: wire::ProjectsData = self.graphql(
                &documents::PROJECTS,
                variables::projects(team_id, after.as_deref()),
            )?;
            let page = data.into_page();
            projects.extend(page.nodes);
            match page.next_cursor {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        Ok(projects)
    }

    /// One project with its milestones, latest updates, and resources.
    pub fn project(&self, id: &str) -> Result<ProjectDetail> {
        let data: wire::ProjectData = self.graphql(&documents::PROJECT, json!({ "id": id }))?;
        Ok(data.into_domain())
    }

    /// What a project's status, labels, lead, members, and teams can be.
    pub fn project_options(&self) -> Result<ProjectOptions> {
        let data: wire::ProjectOptionsData =
            self.graphql(&documents::PROJECT_OPTIONS, json!({}))?;
        Ok(data.into_domain())
    }

    /// One page of a project's issues across its teams, newest update first.
    /// Like [`Self::team_issues`], Linear filters with a superset of `query`.
    pub fn project_issues(
        &self,
        project_id: &str,
        query: &IssueQuery,
        after: Option<&str>,
    ) -> Result<Page<IssueSummary>> {
        let variables = variables::project_issues(project_id, query, after, chrono::Utc::now());
        let data: wire::TeamIssuesData = self.graphql(&documents::TEAM_ISSUES, variables)?;
        let mut page = data.into_page();
        self.complete_pull_requests(&mut page.nodes)?;
        Ok(page)
    }

    /// Changes the fields `change` sets and returns the project as Linear
    /// now has it.
    pub fn update_project(&self, id: &str, change: &ProjectChange) -> Result<ProjectDetail> {
        let variables = json!({ "id": id, "input": variables::project_update_input(change) });
        let data: wire::ProjectUpdateData = self.graphql(&documents::PROJECT_UPDATE, variables)?;
        data.into_domain()
    }

    pub fn update_issue(&self, issue_id: &str, update: &IssueUpdate) -> Result<IssueSummary> {
        let variables = json!({
            "id": issue_id,
            "input": variables::issue_update_input(update),
        });
        let data: wire::IssueUpdateData = self.graphql(&documents::ISSUE_UPDATE, variables)?;
        let mut issue = data.into_domain()?;
        self.complete_pull_requests(std::slice::from_mut(&mut issue))?;
        Ok(issue)
    }

    pub fn create_comment(&self, issue_id: &str, body: &str) -> Result<Comment> {
        let variables = json!({ "input": { "issueId": issue_id, "body": body } });
        let data: wire::CommentCreateData = self.graphql(&documents::COMMENT_CREATE, variables)?;
        data.into_domain()
    }

    pub fn create_issue(&self, input: &NewIssue) -> Result<IssueSummary> {
        let variables = json!({ "input": variables::issue_create_input(input) });
        let data: wire::IssueCreateData = self.graphql(&documents::ISSUE_CREATE, variables)?;
        let mut issue = data.into_domain()?;
        self.complete_pull_requests(std::slice::from_mut(&mut issue))?;
        Ok(issue)
    }

    /// Fetches every pull request attachment of the issues whose summary page
    /// was full, so the pull request filters see the whole set.
    fn complete_pull_requests(&self, issues: &mut [IssueSummary]) -> Result<()> {
        for issue in issues
            .iter_mut()
            .filter(|issue| !issue.pull_requests_complete)
        {
            let mut pull_requests = Vec::new();
            let mut after: Option<String> = None;
            loop {
                let data: wire::IssuePullRequestsData = self.graphql(
                    &documents::ISSUE_PULL_REQUESTS,
                    json!({ "id": issue.id, "after": after }),
                )?;
                let page = data.into_page();
                pull_requests.extend(page.nodes);
                match page.next_cursor {
                    Some(cursor) => after = Some(cursor),
                    None => break,
                }
            }
            issue.pull_requests = pull_requests;
            issue.pull_requests_complete = true;
        }
        Ok(())
    }

    /// Runs one GraphQL document and decodes `data` into `T`.
    pub(crate) fn graphql<T: DeserializeOwned>(
        &self,
        document: &Document,
        variables: Value,
    ) -> Result<T> {
        if std::env::var_os("LOUPE_OFFLINE").is_some() {
            return Err(Error::Transport("Offline mode".into()));
        }
        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", &self.api_key)
            .send_json(json!({
                "query": document.text,
                "operationName": document.name,
                "variables": variables,
            }))
            .map_err(|error| Error::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let body: Value = response
            .body_mut()
            .read_json()
            .map_err(|error| Error::Decode(error.to_string()))?;
        if let Some(errors) = body["errors"]
            .as_array()
            .filter(|errors| !errors.is_empty())
        {
            let unauthenticated = errors
                .iter()
                .any(|error| error["extensions"]["code"] == "AUTHENTICATION_ERROR");
            if unauthenticated || status == 401 {
                return Err(Error::Unauthorized);
            }
            let messages = errors
                .iter()
                .map(|error| {
                    error["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                        .to_owned()
                })
                .collect();
            return Err(Error::GraphQl(messages));
        }
        if !(200..300).contains(&status) {
            let message = body["message"].as_str().unwrap_or_default().to_owned();
            return Err(Error::Http { status, message });
        }
        serde_json::from_value(body["data"].clone())
            .map_err(|error| Error::Decode(error.to_string()))
    }
}
