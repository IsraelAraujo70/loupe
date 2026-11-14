//! Linear's response shapes and their conversion into the domain types. Each
//! struct mirrors one selection in `documents.rs`; nothing here leaves the
//! crate.

use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::types::{
    Attachment, Author, Comment, Cycle, CycleRef, EstimateScale, Estimation, IssueDetail,
    IssueSummary, Label, LinearCustomView, MilestoneRef, Organization, Page, Priority, Project,
    ProjectDetail, ProjectHealth, ProjectMilestone, ProjectOptions, ProjectPost, ProjectResource,
    ProjectResourceKind, ProjectStatus, ProjectStatusKind, ProjectSummary, PullRequestLink,
    PullRequestState, StateRef, StateType, Team, TeamRef, TeamSummary, User, Viewer, WorkflowState,
};
use crate::{Error, Result};

#[derive(Deserialize)]
pub(crate) struct Connection<T> {
    nodes: Vec<T>,
    #[serde(default, rename = "pageInfo")]
    page_info: Option<PageInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

impl<T> Connection<T> {
    fn next_cursor(&self) -> Option<String> {
        let info = self.page_info.as_ref()?;
        if info.has_next_page {
            info.end_cursor.clone()
        } else {
            None
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireUser {
    id: String,
    name: String,
    display_name: String,
    avatar_url: Option<String>,
    #[serde(default = "active_by_default")]
    active: bool,
}

fn active_by_default() -> bool {
    true
}

impl From<WireUser> for User {
    fn from(user: WireUser) -> Self {
        User {
            id: user.id,
            name: user.name,
            display_name: user.display_name,
            avatar_url: user.avatar_url,
        }
    }
}

#[derive(Deserialize)]
struct WireOrganization {
    id: String,
    name: String,
    #[serde(rename = "urlKey")]
    url_key: String,
    #[serde(rename = "logoUrl", default)]
    logo_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewerData {
    viewer: WireViewer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireViewer {
    id: String,
    name: String,
    display_name: String,
    avatar_url: Option<String>,
    email: String,
    organization: WireOrganization,
}

impl ViewerData {
    pub(crate) fn into_domain(self) -> Viewer {
        let viewer = self.viewer;
        Viewer {
            email: viewer.email,
            user: User {
                id: viewer.id,
                name: viewer.name,
                display_name: viewer.display_name,
                avatar_url: viewer.avatar_url,
            },
            organization: Organization {
                id: viewer.organization.id,
                name: viewer.organization.name,
                url_key: viewer.organization.url_key,
                // Linear sends null for a workspace without a logo; an
                // empty string means the same.
                logo_url: viewer
                    .organization
                    .logo_url
                    .filter(|url| !url.trim().is_empty()),
            },
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct TeamsData {
    teams: Connection<WireTeamRef>,
}

#[derive(Deserialize)]
struct WireTeamRef {
    id: String,
    key: String,
    name: String,
}

impl From<WireTeamRef> for TeamRef {
    fn from(team: WireTeamRef) -> Self {
        TeamRef {
            id: team.id,
            key: team.key,
            name: team.name,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct MyTeamsData {
    viewer: TeamsData,
}

impl MyTeamsData {
    pub(crate) fn into_domain(self) -> Vec<TeamSummary> {
        self.viewer.into_domain()
    }
}

impl TeamsData {
    pub(crate) fn into_domain(self) -> Vec<TeamSummary> {
        let mut teams: Vec<TeamSummary> = self
            .teams
            .nodes
            .into_iter()
            .map(|team| TeamSummary {
                id: team.id,
                key: team.key,
                name: team.name,
            })
            .collect();
        teams.sort_by_cached_key(|team| team.name.to_lowercase());
        teams
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TeamData {
    team: WireTeam,
    issue_labels: Connection<WireLabel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTeam {
    id: String,
    key: String,
    name: String,
    issue_estimation_type: String,
    issue_estimation_allow_zero: bool,
    issue_estimation_extended: bool,
    states: Connection<WireWorkflowState>,
    members: Connection<WireUser>,
    projects: Connection<WireProject>,
    cycles: Connection<WireCycle>,
}

#[derive(Deserialize)]
struct WireWorkflowState {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    position: f64,
    color: String,
}

#[derive(Deserialize)]
struct WireLabelParent {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLabel {
    id: String,
    name: String,
    color: String,
    /// Only the Team document asks for it.
    #[serde(default)]
    is_group: bool,
    parent: Option<WireLabelParent>,
}

impl From<WireLabel> for Label {
    fn from(label: WireLabel) -> Self {
        Label {
            id: label.id,
            name: label.name,
            color: label.color,
            parent_id: label.parent.map(|parent| parent.id),
        }
    }
}

#[derive(Deserialize)]
struct WireProject {
    id: String,
    name: String,
    color: Option<String>,
}

impl From<WireProject> for Project {
    fn from(project: WireProject) -> Self {
        Project {
            id: project.id,
            name: project.name,
            color: project.color,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCycle {
    id: String,
    number: f64,
    name: Option<String>,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

impl TeamData {
    pub(crate) fn into_domain(self) -> Team {
        let team = self.team;

        let mut states: Vec<WorkflowState> = team
            .states
            .nodes
            .into_iter()
            .map(|state| WorkflowState {
                id: state.id,
                name: state.name,
                kind: StateType::parse(&state.kind),
                position: state.position,
                color: state.color,
            })
            .collect();
        states.sort_by(|a, b| {
            a.kind
                .rank()
                .cmp(&b.kind.rank())
                .then(a.position.total_cmp(&b.position))
        });

        let mut members: Vec<User> = team
            .members
            .nodes
            .into_iter()
            .filter(|member| member.active)
            .map(User::from)
            .collect();
        members.sort_by_cached_key(|member| member.name.to_lowercase());

        let label_groups: Vec<String> = self
            .issue_labels
            .nodes
            .iter()
            .filter(|label| label.is_group)
            .map(|label| label.id.clone())
            .collect();
        let mut labels: Vec<Label> = dedupe_by(
            self.issue_labels.nodes.into_iter().map(Label::from),
            |label| label.id.clone(),
        );
        labels.sort_by_cached_key(|label| label.name.to_lowercase());

        let mut cycles: Vec<Cycle> = team
            .cycles
            .nodes
            .into_iter()
            .map(|cycle| Cycle {
                id: cycle.id,
                number: cycle.number as u32,
                name: cycle.name,
                starts_at: cycle.starts_at,
                ends_at: cycle.ends_at,
            })
            .collect();
        cycles.sort_by_key(|cycle| cycle.starts_at);

        Team {
            id: team.id,
            key: team.key,
            name: team.name,
            states,
            members,
            labels,
            projects: team.projects.nodes.into_iter().map(Project::from).collect(),
            cycles,
            label_groups,
            estimation: Estimation {
                scale: EstimateScale::parse(&team.issue_estimation_type),
                allow_zero: team.issue_estimation_allow_zero,
                extended: team.issue_estimation_extended,
            },
        }
    }
}

#[derive(Deserialize)]
struct WireStateRef {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    color: String,
}

#[derive(Deserialize)]
struct WireCycleRef {
    id: String,
    number: f64,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIssue {
    id: String,
    identifier: String,
    title: String,
    priority: f64,
    sort_order: f64,
    estimate: Option<f64>,
    due_date: Option<NaiveDate>,
    url: String,
    #[serde(default)]
    branch_name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    state: WireStateRef,
    team: WireTeamRef,
    assignee: Option<WireUser>,
    labels: Connection<WireLabel>,
    project: Option<WireProject>,
    cycle: Option<WireCycleRef>,
    creator: Option<WireId>,
    parent: Option<WireId>,
    children: Connection<WireId>,
    project_milestone: Option<WireMilestone>,
    completed_at: Option<DateTime<Utc>>,
    canceled_at: Option<DateTime<Utc>>,
    pull_requests: Connection<WireLinkAttachment>,
}

#[derive(Deserialize)]
struct WireId {
    id: String,
}

#[derive(Deserialize)]
struct WireMilestone {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct WireLinkAttachment {
    url: String,
    #[serde(default)]
    metadata: Option<Value>,
}

impl From<WireIssue> for IssueSummary {
    fn from(issue: WireIssue) -> Self {
        IssueSummary {
            id: issue.id,
            identifier: issue.identifier,
            title: issue.title,
            priority: Priority::from_wire(issue.priority),
            sort_order: issue.sort_order,
            state: StateRef {
                id: issue.state.id,
                name: issue.state.name,
                kind: StateType::parse(&issue.state.kind),
                color: issue.state.color,
            },
            team: issue.team.into(),
            assignee: issue.assignee.map(User::from),
            labels: issue.labels.nodes.into_iter().map(Label::from).collect(),
            project: issue.project.map(Project::from),
            cycle: issue.cycle.map(|cycle| CycleRef {
                id: cycle.id,
                number: cycle.number as u32,
                name: cycle.name,
            }),
            estimate: issue.estimate,
            due_date: issue.due_date,
            url: issue.url,
            branch_name: issue.branch_name,
            created_at: issue.created_at,
            updated_at: issue.updated_at,
            creator_id: issue.creator.map(|creator| creator.id),
            parent_id: issue.parent.map(|parent| parent.id),
            has_sub_issues: !issue.children.nodes.is_empty(),
            milestone: issue.project_milestone.map(|milestone| MilestoneRef {
                id: milestone.id,
                name: milestone.name,
            }),
            closed_at: issue.completed_at.or(issue.canceled_at),
            pull_requests_complete: issue.pull_requests.next_cursor().is_none(),
            pull_requests: issue
                .pull_requests
                .nodes
                .iter()
                .filter_map(|attachment| {
                    parse_pull_request(&attachment.url, attachment.metadata.as_ref())
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssuePullRequestsData {
    issue: Option<IssuePullRequestsNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuePullRequestsNode {
    pull_requests: Connection<WireLinkAttachment>,
}

impl IssuePullRequestsData {
    /// The pull requests of this page and the cursor of the next one.
    pub(crate) fn into_page(self) -> Page<PullRequestLink> {
        let Some(issue) = self.issue else {
            return Page {
                nodes: Vec::new(),
                next_cursor: None,
            };
        };
        let next_cursor = issue.pull_requests.next_cursor();
        Page {
            nodes: issue
                .pull_requests
                .nodes
                .iter()
                .filter_map(|attachment| {
                    parse_pull_request(&attachment.url, attachment.metadata.as_ref())
                })
                .collect(),
            next_cursor,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCustomView {
    id: String,
    name: String,
    team: Option<WireId>,
    filter_data: Value,
    icon: Option<String>,
    color: Option<String>,
    #[serde(default)]
    shared: bool,
    owner: Option<WireOwner>,
    #[serde(default)]
    view_preferences_values: Value,
    #[serde(default)]
    user_view_preferences: Option<WireViewPreferences>,
    #[serde(default = "issue_model")]
    model_name: String,
}

#[derive(Deserialize)]
struct WireViewPreferences {
    #[serde(default)]
    preferences: Value,
}

impl WireCustomView {
    fn is_issue_view(&self) -> bool {
        self.model_name == "Issue"
    }

    fn into_domain(self) -> LinearCustomView {
        // The values in force merge the workspace's and the owner's
        // settings, and Linear's merge keeps some workspace values over the
        // owner's (seen for `fieldLabels`). What the owner set is what the
        // owner sees, so those win here.
        let mut preferences = self.view_preferences_values;
        let own: serde_json::Map<String, Value> = self
            .user_view_preferences
            .and_then(|own| match own.preferences {
                Value::Object(own) => Some(own),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, value)| !value.is_null())
            .collect();
        if let Some(merged) = preferences.as_object_mut() {
            for (key, value) in &own {
                if merged.contains_key(key) {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
        LinearCustomView {
            id: self.id,
            name: self.name,
            team_id: self.team.map(|team| team.id),
            filter_data: self.filter_data,
            icon: self.icon,
            color: self.color,
            shared: self.shared,
            owner_name: self.owner.map(|owner| owner.name).unwrap_or_default(),
            preferences,
            own_preferences: if own.is_empty() {
                Value::Null
            } else {
                Value::Object(own)
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomViewData {
    custom_view: Option<WireCustomView>,
}

impl CustomViewData {
    pub(crate) fn into_domain(self) -> Result<LinearCustomView> {
        self.custom_view
            .map(WireCustomView::into_domain)
            .ok_or_else(|| did_not("read the view"))
    }
}

#[derive(Deserialize)]
struct CustomViewPayload {
    success: bool,
    #[serde(rename = "customView")]
    custom_view: Option<WireCustomView>,
}

impl CustomViewPayload {
    fn into_view(self, action: &str) -> Result<LinearCustomView> {
        match self {
            CustomViewPayload {
                success: true,
                custom_view: Some(view),
            } => Ok(view.into_domain()),
            _ => Err(did_not(action)),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CustomViewCreateData {
    #[serde(rename = "customViewCreate")]
    payload: CustomViewPayload,
}

impl CustomViewCreateData {
    pub(crate) fn into_domain(self) -> Result<LinearCustomView> {
        self.payload.into_view("create the view")
    }
}

#[derive(Deserialize)]
pub(crate) struct CustomViewUpdateData {
    #[serde(rename = "customViewUpdate")]
    payload: CustomViewPayload,
}

impl CustomViewUpdateData {
    pub(crate) fn into_domain(self) -> Result<LinearCustomView> {
        self.payload.into_view("update the view")
    }
}

#[derive(Deserialize)]
struct SuccessPayload {
    success: bool,
}

/// The answer of a mutation that only says whether it worked, under any
/// field name.
#[derive(Deserialize)]
pub(crate) struct SuccessData {
    #[serde(
        alias = "customViewDelete",
        alias = "viewPreferencesCreate",
        alias = "favoriteCreate"
    )]
    payload: SuccessPayload,
}

impl SuccessData {
    pub(crate) fn into_result(self, action: &str) -> Result<()> {
        if self.payload.success {
            Ok(())
        } else {
            Err(did_not(action))
        }
    }
}

fn issue_model() -> String {
    "Issue".into()
}

#[derive(Deserialize)]
struct WireOwner {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomViewsData {
    custom_views: Connection<WireCustomView>,
}

impl CustomViewsData {
    pub(crate) fn into_page(self) -> Page<LinearCustomView> {
        let next_cursor = self.custom_views.next_cursor();
        Page {
            nodes: self
                .custom_views
                .nodes
                .into_iter()
                .filter(WireCustomView::is_issue_view)
                .map(WireCustomView::into_domain)
                .collect(),
            next_cursor,
        }
    }
}

/// Keeps the first of each key, in order.
fn dedupe_by<T, K: std::hash::Hash + Eq>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K,
) -> Vec<T> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(key(item)))
        .collect()
}

fn issues_from(nodes: Vec<WireIssue>) -> Vec<IssueSummary> {
    dedupe_by(nodes.into_iter().map(IssueSummary::from), |issue| {
        issue.id.clone()
    })
}

/// `MyOpenIssues` page.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyOpenIssuesData {
    viewer: AssignedIssues,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssignedIssues {
    assigned_issues: Connection<WireIssue>,
}

impl MyOpenIssuesData {
    pub(crate) fn into_page(self) -> Page<IssueSummary> {
        let connection = self.viewer.assigned_issues;
        let next_cursor = connection.next_cursor();
        Page {
            nodes: issues_from(connection.nodes),
            next_cursor,
        }
    }
}

/// `TeamIssues` page.
#[derive(Deserialize)]
pub(crate) struct TeamIssuesData {
    issues: Connection<WireIssue>,
}

impl TeamIssuesData {
    pub(crate) fn into_page(self) -> Page<IssueSummary> {
        let next_cursor = self.issues.next_cursor();
        Page {
            nodes: issues_from(self.issues.nodes),
            next_cursor,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct IssueData {
    issue: WireIssueDetail,
}

#[derive(Deserialize)]
struct WireIssueDetail {
    #[serde(flatten)]
    issue: WireIssue,
    description: Option<String>,
    creator: Option<WireUser>,
    comments: Connection<WireComment>,
    attachments: Connection<WireAttachment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireComment {
    id: String,
    body: String,
    created_at: DateTime<Utc>,
    edited_at: Option<DateTime<Utc>>,
    user: Option<WireUser>,
    #[serde(default)]
    bot_actor: Option<WireBot>,
    parent: Option<WireLabelParent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBot {
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
    avatar_url: Option<String>,
}

impl From<WireComment> for Comment {
    fn from(comment: WireComment) -> Self {
        let author = match (comment.user, comment.bot_actor) {
            (Some(user), _) => Author::User(user.into()),
            (None, Some(bot)) => match bot.name.or(bot.kind) {
                Some(name) => Author::Bot {
                    name,
                    avatar_url: bot.avatar_url,
                },
                None => Author::Unknown,
            },
            (None, None) => Author::Unknown,
        };
        Comment {
            id: comment.id,
            body: comment.body,
            author,
            created_at: comment.created_at,
            edited_at: comment.edited_at,
            parent_id: comment.parent.map(|parent| parent.id),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireAttachment {
    id: String,
    url: String,
    title: String,
    subtitle: Option<String>,
    metadata: Option<Value>,
}

impl From<WireAttachment> for Attachment {
    fn from(attachment: WireAttachment) -> Self {
        Attachment {
            pull_request: parse_pull_request(&attachment.url, attachment.metadata.as_ref()),
            id: attachment.id,
            url: attachment.url,
            title: attachment.title,
            subtitle: attachment.subtitle.filter(|subtitle| !subtitle.is_empty()),
        }
    }
}

impl IssueData {
    pub(crate) fn into_domain(self) -> IssueDetail {
        let detail = self.issue;
        let mut comments: Vec<Comment> = detail
            .comments
            .nodes
            .into_iter()
            .map(Comment::from)
            .collect();
        comments.sort_by_key(|comment| comment.created_at);
        IssueDetail {
            issue: detail.issue.into(),
            description: detail.description.unwrap_or_default(),
            creator: detail.creator.map(User::from),
            comments,
            attachments: detail
                .attachments
                .nodes
                .into_iter()
                .map(Attachment::from)
                .collect(),
        }
    }
}

/// Reads `https://github.com/{owner}/{repo}/pull/{n}` (trailing slash, extra
/// path segments, query, and fragment allowed) and the state Linear's GitHub
/// integration stored in the attachment metadata.
fn parse_pull_request(url: &str, metadata: Option<&Value>) -> Option<PullRequestLink> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("https://www.github.com/"))?;
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut segments = path.split('/');
    let owner = segments.next().filter(|s| !s.is_empty())?;
    let repo = segments.next().filter(|s| !s.is_empty())?;
    if segments.next()? != "pull" {
        return None;
    }
    let number: u64 = segments.next()?.parse().ok()?;
    Some(PullRequestLink {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number,
        state: pull_request_state(metadata),
    })
}

fn pull_request_state(metadata: Option<&Value>) -> PullRequestState {
    let Some(metadata) = metadata.and_then(Value::as_object) else {
        return PullRequestState::Unknown;
    };
    let status = metadata.get("status").and_then(Value::as_str);
    let merged_at = metadata
        .get("mergedAt")
        .is_some_and(|value| !value.is_null());
    let draft = metadata.get("draft").and_then(Value::as_bool);
    if status.is_none() && !merged_at && draft.is_none() {
        return PullRequestState::Unknown;
    }
    let is = |expected: &str| status.is_some_and(|s| s.eq_ignore_ascii_case(expected));
    if is("merged") || merged_at {
        PullRequestState::Merged
    } else if is("closed") {
        PullRequestState::Closed
    } else if draft == Some(true) {
        PullRequestState::Draft
    } else {
        PullRequestState::Open
    }
}

#[derive(Deserialize)]
pub(crate) struct SearchIssuesData {
    #[serde(rename = "searchIssues")]
    search_issues: Connection<WireIssue>,
}

impl SearchIssuesData {
    pub(crate) fn into_domain(self) -> Vec<IssueSummary> {
        issues_from(self.search_issues.nodes)
    }
}

#[derive(Deserialize)]
pub(crate) struct IssuesForUrlData {
    #[serde(rename = "attachmentsForURL")]
    attachments_for_url: Connection<AttachmentIssue>,
}

#[derive(Deserialize)]
struct AttachmentIssue {
    issue: WireIssue,
}

impl IssuesForUrlData {
    pub(crate) fn into_domain(self) -> Vec<IssueSummary> {
        issues_from(
            self.attachments_for_url
                .nodes
                .into_iter()
                .map(|attachment| attachment.issue)
                .collect(),
        )
    }
}

#[derive(Deserialize)]
pub(crate) struct IssuesByIdentifiersData {
    issues: Connection<WireIssue>,
}

impl IssuesByIdentifiersData {
    pub(crate) fn into_domain(self) -> Vec<IssueSummary> {
        issues_from(self.issues.nodes)
    }
}

fn did_not(action: &str) -> Error {
    Error::GraphQl(vec![format!("Linear did not {action}")])
}

#[derive(Deserialize)]
pub(crate) struct IssueUpdateData {
    #[serde(rename = "issueUpdate")]
    payload: IssuePayload,
}

#[derive(Deserialize)]
pub(crate) struct IssueCreateData {
    #[serde(rename = "issueCreate")]
    payload: IssuePayload,
}

#[derive(Deserialize)]
struct IssuePayload {
    success: bool,
    issue: Option<WireIssue>,
}

impl IssuePayload {
    fn into_issue(self, action: &str) -> Result<IssueSummary> {
        match self {
            IssuePayload {
                success: true,
                issue: Some(issue),
            } => Ok(issue.into()),
            _ => Err(did_not(action)),
        }
    }
}

impl IssueUpdateData {
    pub(crate) fn into_domain(self) -> Result<IssueSummary> {
        self.payload.into_issue("update the issue")
    }
}

impl IssueCreateData {
    pub(crate) fn into_domain(self) -> Result<IssueSummary> {
        self.payload.into_issue("create the issue")
    }
}

#[derive(Deserialize)]
pub(crate) struct CommentCreateData {
    #[serde(rename = "commentCreate")]
    payload: CommentPayload,
}

#[derive(Deserialize)]
struct CommentPayload {
    success: bool,
    comment: Option<WireComment>,
}

impl CommentCreateData {
    pub(crate) fn into_domain(self) -> Result<Comment> {
        match self.payload {
            CommentPayload {
                success: true,
                comment: Some(comment),
            } => Ok(comment.into()),
            _ => Err(did_not("create the comment")),
        }
    }
}

/// Merges pages while dropping issues already seen.
pub(crate) fn merge_unique(into: &mut Vec<IssueSummary>, more: Vec<IssueSummary>) {
    let mut seen: HashSet<String> = into.iter().map(|issue| issue.id.clone()).collect();
    into.extend(
        more.into_iter()
            .filter(|issue| seen.insert(issue.id.clone())),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_pull_request, pull_request_state};
    use crate::types::PullRequestState;

    #[test]
    fn pull_request_url_variants() {
        let link = |url: &str| parse_pull_request(url, None);
        let plain = link("https://github.com/acme/web/pull/42").expect("plain");
        assert_eq!(
            (plain.owner.as_str(), plain.repo.as_str(), plain.number),
            ("acme", "web", 42)
        );
        assert!(link("https://github.com/acme/web/pull/42/").is_some());
        assert!(link("https://github.com/acme/web/pull/42/files#diff").is_some());
        assert!(link("https://github.com/acme/web/pull/42?x=1").is_some());
        assert!(link("https://github.com/acme/web/issues/42").is_none());
        assert!(link("https://github.com/acme/web/pull/abc").is_none());
        assert!(link("https://github.com/acme/web").is_none());
        assert!(link("https://example.com/acme/web/pull/42").is_none());
    }

    #[test]
    fn pull_request_state_from_metadata() {
        let state = |value| pull_request_state(Some(&value));
        assert_eq!(pull_request_state(None), PullRequestState::Unknown);
        assert_eq!(state(json!(null)), PullRequestState::Unknown);
        assert_eq!(state(json!({})), PullRequestState::Unknown);
        assert_eq!(
            state(json!({ "status": "merged" })),
            PullRequestState::Merged
        );
        assert_eq!(
            state(json!({ "status": "open", "mergedAt": "2026-11-02T10:00:00.000Z" })),
            PullRequestState::Merged
        );
        assert_eq!(
            state(json!({ "status": "closed" })),
            PullRequestState::Closed
        );
        assert_eq!(
            state(json!({ "status": "open", "draft": true })),
            PullRequestState::Draft
        );
        assert_eq!(
            state(json!({ "status": "open", "draft": false, "mergedAt": null })),
            PullRequestState::Open
        );
        assert_eq!(state(json!({ "draft": true })), PullRequestState::Draft);
    }
}

#[derive(Deserialize)]
pub(crate) struct ProjectsData {
    projects: Connection<WireProjectSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProjectSummary {
    id: String,
    name: String,
    color: String,
    url: String,
    progress: f32,
    target_date: Option<NaiveDate>,
    status: WireProjectStatus,
    lead: Option<WireUser>,
    teams: Connection<WireId>,
}

#[derive(Deserialize)]
struct WireProjectStatus {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    color: String,
}

impl From<WireProjectStatus> for ProjectStatus {
    fn from(status: WireProjectStatus) -> Self {
        ProjectStatus {
            kind: match status.kind.as_str() {
                "planned" => ProjectStatusKind::Planned,
                "started" => ProjectStatusKind::Started,
                "paused" => ProjectStatusKind::Paused,
                "completed" => ProjectStatusKind::Completed,
                "canceled" => ProjectStatusKind::Canceled,
                _ => ProjectStatusKind::Backlog,
            },
            id: status.id,
            name: status.name,
            color: status.color,
        }
    }
}

impl ProjectsData {
    pub(crate) fn into_page(self) -> Page<ProjectSummary> {
        let next_cursor = self.projects.next_cursor();
        Page {
            nodes: self
                .projects
                .nodes
                .into_iter()
                .map(|project| ProjectSummary {
                    id: project.id,
                    name: project.name,
                    color: project.color,
                    status: project.status.into(),
                    progress: project.progress.clamp(0., 1.),
                    lead: project.lead.map(User::from),
                    target_date: project.target_date,
                    url: project.url,
                    team_ids: project
                        .teams
                        .nodes
                        .into_iter()
                        .map(|team| team.id)
                        .collect(),
                })
                .collect(),
            next_cursor,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireFavorite {
    id: String,
    title: String,
    icon: Option<String>,
    color: Option<String>,
    url: Option<String>,
    sort_order: f64,
    parent: Option<WireId>,
    #[serde(rename = "type")]
    kind: String,
    custom_view: Option<WireFavoriteView>,
    predefined_view_type: Option<String>,
    predefined_view_team: Option<WireTeamRef>,
    issue: Option<WireFavoriteIssue>,
}
#[derive(Deserialize)]
struct WireFavoriteView {
    id: String,
    team: Option<WireTeamRef>,
}
#[derive(Deserialize)]
struct WireFavoriteIssue {
    identifier: String,
}
#[derive(Deserialize)]
pub(crate) struct FavoritesData {
    favorites: Connection<WireFavorite>,
}
impl FavoritesData {
    pub(crate) fn into_page(self) -> Page<WireFavorite> {
        let next_cursor = self.favorites.next_cursor();
        Page {
            nodes: self.favorites.nodes,
            next_cursor,
        }
    }
}
pub(crate) fn favorites_in_order(mut rows: Vec<WireFavorite>) -> Vec<crate::LinearFavorite> {
    rows.sort_by(|a, b| a.sort_order.total_cmp(&b.sort_order).then(a.id.cmp(&b.id)));
    fn drain(
        parent: Option<&str>,
        rows: &mut Vec<WireFavorite>,
        out: &mut Vec<crate::LinearFavorite>,
    ) {
        let mut ix = 0;
        while ix < rows.len() {
            if rows[ix].parent.as_ref().map(|p| p.id.as_str()) != parent {
                ix += 1;
                continue;
            }
            let row = rows.remove(ix);
            let id = row.id.clone();
            let target = if let Some(view) = row.custom_view {
                crate::FavoriteTarget::CustomView {
                    view_id: view.id,
                    team: view.team.map(|t| crate::TeamSummary {
                        id: t.id,
                        key: t.key,
                        name: t.name,
                    }),
                }
            } else if let Some(kind) = row.predefined_view_type {
                crate::FavoriteTarget::PredefinedView {
                    kind,
                    team: row.predefined_view_team.map(|t| crate::TeamSummary {
                        id: t.id,
                        key: t.key,
                        name: t.name,
                    }),
                }
            } else if let Some(issue) = row.issue {
                crate::FavoriteTarget::Issue {
                    identifier: issue.identifier,
                }
            } else {
                crate::FavoriteTarget::Other { kind: row.kind }
            };
            out.push(crate::LinearFavorite {
                id: row.id,
                title: row.title,
                icon: row.icon,
                color: row.color,
                url: row.url,
                target,
            });
            drain(Some(&id), rows, out);
        }
    }
    let mut out = Vec::new();
    drain(None, &mut rows, &mut out);
    // An unavailable parent must not hide its children.
    while !rows.is_empty() {
        rows[0].parent = None;
        drain(None, &mut rows, &mut out);
    }
    out
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomViewIssuesData {
    custom_view: TeamIssuesData,
}
impl CustomViewIssuesData {
    pub(crate) fn into_page(self) -> Page<IssueSummary> {
        self.custom_view.into_page()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProjectDetail {
    id: String,
    name: String,
    icon: Option<String>,
    color: String,
    description: String,
    content: Option<String>,
    url: String,
    progress: f32,
    start_date: Option<NaiveDate>,
    target_date: Option<NaiveDate>,
    status: WireProjectStatus,
    lead: Option<WireUser>,
    lead_team: Option<WireId>,
    teams: Connection<WireTeamRef>,
    members: Connection<WireUser>,
    labels: Connection<WireProjectLabel>,
    project_milestones: Connection<WireProjectMilestone>,
    project_updates: Connection<WireProjectPost>,
    documents: Connection<WireDocumentLink>,
    external_links: Connection<WireExternalLink>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProjectLabel {
    id: String,
    name: String,
    color: String,
    #[serde(default)]
    is_group: bool,
}

impl From<WireProjectLabel> for Label {
    fn from(label: WireProjectLabel) -> Self {
        Label {
            id: label.id,
            name: label.name,
            color: label.color,
            parent_id: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProjectMilestone {
    id: String,
    name: String,
    progress: f32,
    target_date: Option<NaiveDate>,
    sort_order: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProjectPost {
    id: String,
    body: String,
    health: String,
    created_at: DateTime<Utc>,
    url: String,
    user: Option<WireUser>,
}

#[derive(Deserialize)]
struct WireDocumentLink {
    id: String,
    title: String,
    url: String,
}

#[derive(Deserialize)]
struct WireExternalLink {
    id: String,
    label: String,
    url: String,
}

impl From<WireProjectDetail> for ProjectDetail {
    fn from(project: WireProjectDetail) -> Self {
        let mut milestones = project.project_milestones.nodes;
        milestones.sort_by(|a, b| a.sort_order.total_cmp(&b.sort_order));
        let mut updates: Vec<ProjectPost> = project
            .project_updates
            .nodes
            .into_iter()
            .map(|post| ProjectPost {
                id: post.id,
                body: post.body,
                health: match post.health.as_str() {
                    "atRisk" => ProjectHealth::AtRisk,
                    "offTrack" => ProjectHealth::OffTrack,
                    _ => ProjectHealth::OnTrack,
                },
                author: post.user.map(User::from),
                created_at: post.created_at,
                url: post.url,
            })
            .collect();
        updates.sort_by_key(|post| std::cmp::Reverse(post.created_at));
        let documents = project
            .documents
            .nodes
            .into_iter()
            .map(|document| ProjectResource {
                id: document.id,
                title: document.title,
                url: document.url,
                kind: ProjectResourceKind::Document,
            });
        let links = project
            .external_links
            .nodes
            .into_iter()
            .map(|link| ProjectResource {
                id: link.id,
                title: link.label,
                url: link.url,
                kind: ProjectResourceKind::Link,
            });
        let teams: Vec<TeamRef> = project.teams.nodes.into_iter().map(TeamRef::from).collect();
        ProjectDetail {
            lead_team_id: project
                .lead_team
                .map(|team| team.id)
                .or_else(|| teams.first().map(|team| team.id.clone()))
                .unwrap_or_default(),
            id: project.id,
            name: project.name,
            icon: project.icon.filter(|icon| !icon.trim().is_empty()),
            color: project.color,
            summary: project.description,
            content: project.content.filter(|content| !content.trim().is_empty()),
            status: project.status.into(),
            progress: project.progress.clamp(0., 1.),
            lead: project.lead.map(User::from),
            start_date: project.start_date,
            target_date: project.target_date,
            url: project.url,
            teams,
            members: project.members.nodes.into_iter().map(User::from).collect(),
            labels: project.labels.nodes.into_iter().map(Label::from).collect(),
            milestones: milestones
                .into_iter()
                .map(|milestone| ProjectMilestone {
                    id: milestone.id,
                    name: milestone.name,
                    progress: milestone.progress.clamp(0., 1.),
                    target_date: milestone.target_date,
                })
                .collect(),
            updates,
            resources: documents.chain(links).collect(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct ProjectData {
    project: WireProjectDetail,
}

impl ProjectData {
    pub(crate) fn into_domain(self) -> ProjectDetail {
        self.project.into()
    }
}

#[derive(Deserialize)]
pub(crate) struct ProjectUpdateData {
    #[serde(rename = "projectUpdate")]
    payload: ProjectPayload,
}

#[derive(Deserialize)]
struct ProjectPayload {
    success: bool,
    project: Option<WireProjectDetail>,
}

impl ProjectUpdateData {
    pub(crate) fn into_domain(self) -> Result<ProjectDetail> {
        match self.payload {
            ProjectPayload {
                success: true,
                project: Some(project),
            } => Ok(project.into()),
            _ => Err(did_not("update the project")),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectOptionsData {
    organization: WireProjectStatuses,
    project_labels: Connection<WireProjectLabel>,
    users: Connection<WireUser>,
    teams: Connection<WireTeamRef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProjectStatuses {
    project_statuses: Vec<WireOrderedProjectStatus>,
}

#[derive(Deserialize)]
struct WireOrderedProjectStatus {
    #[serde(flatten)]
    status: WireProjectStatus,
    position: f64,
}

impl ProjectOptionsData {
    pub(crate) fn into_domain(self) -> ProjectOptions {
        let mut statuses = self.organization.project_statuses;
        statuses.sort_by(|a, b| a.position.total_cmp(&b.position));
        let by_name = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
        let mut labels: Vec<Label> = self
            .project_labels
            .nodes
            .into_iter()
            .filter(|label| !label.is_group)
            .map(Label::from)
            .collect();
        labels.sort_by(|a, b| by_name(&a.name, &b.name));
        let mut users: Vec<User> = self
            .users
            .nodes
            .into_iter()
            .filter(|user| user.active)
            .map(User::from)
            .collect();
        users.sort_by(|a, b| by_name(&a.name, &b.name));
        let mut teams: Vec<TeamRef> = self.teams.nodes.into_iter().map(TeamRef::from).collect();
        teams.sort_by(|a, b| by_name(&a.name, &b.name));
        ProjectOptions {
            statuses: statuses
                .into_iter()
                .map(|ordered| ordered.status.into())
                .collect(),
            labels,
            users,
            teams,
        }
    }
}
