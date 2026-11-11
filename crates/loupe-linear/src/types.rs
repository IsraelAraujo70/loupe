//! Domain types the client returns. Wire shapes never leave the crate: every
//! response is parsed into these at the boundary.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// The user who owns the API key, with their workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    pub user: User,
    pub organization: Organization,
    /// The account's email. One person has a user per workspace, and the
    /// email tells which of their accounts a workspace belongs to.
    #[serde(default)]
    pub email: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub name: String,
    /// The workspace slug in `https://linear.app/<url_key>/...`.
    pub url_key: String,
    /// The workspace's logo image; `None` when none was uploaded. Linear
    /// serves it from its upload host, which answers only with the API key.
    #[serde(default)]
    pub logo_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    /// Full name.
    pub name: String,
    /// The short handle Linear shows in mentions.
    pub display_name: String,
    pub avatar_url: Option<String>,
}

/// A team as the team picker lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSummary {
    pub id: String,
    /// Issue prefix, such as `ENG`.
    pub key: String,
    pub name: String,
}

/// A team with everything the board, the filters, and the property pickers
/// need.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub key: String,
    pub name: String,
    /// Sorted by state type (see [`StateType::rank`]) then position.
    pub states: Vec<WorkflowState>,
    /// Active members, sorted by name.
    pub members: Vec<User>,
    /// Team labels plus workspace labels, sorted by name.
    pub labels: Vec<Label>,
    pub projects: Vec<Project>,
    /// Cycles that have not ended yet, soonest first.
    pub cycles: Vec<Cycle>,
    /// The labels that are groups. Linear puts a group's labels on an
    /// issue, never the group itself.
    #[serde(default)]
    pub label_groups: Vec<String>,
    /// The estimate scale. Older cached catalogs lack it and read as
    /// estimates not used.
    #[serde(default)]
    pub estimation: Estimation,
}

/// The scale a team estimates issues on (Team settings > Estimates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstimateScale {
    /// The team does not estimate, or uses a scale this client does not
    /// know yet.
    #[default]
    NotUsed,
    Exponential,
    Fibonacci,
    Linear,
    TShirt,
}

impl EstimateScale {
    pub fn parse(value: &str) -> EstimateScale {
        match value {
            "exponential" => EstimateScale::Exponential,
            "fibonacci" => EstimateScale::Fibonacci,
            "linear" => EstimateScale::Linear,
            "tShirt" => EstimateScale::TShirt,
            _ => EstimateScale::NotUsed,
        }
    }
}

/// A team's estimate settings: the scale, whether 0 is a value, and whether
/// the scale has its two extended steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Estimation {
    pub scale: EstimateScale,
    pub allow_zero: bool,
    pub extended: bool,
}

impl Estimation {
    /// The point values an issue of the team can take, smallest first.
    /// Empty when the team does not estimate.
    pub fn points(self) -> Vec<u32> {
        let (base, extra): (&[u32], &[u32]) = match self.scale {
            EstimateScale::NotUsed => return Vec::new(),
            EstimateScale::Exponential => (&[1, 2, 4, 8, 16], &[32, 64]),
            EstimateScale::Fibonacci => (&[1, 2, 3, 5, 8], &[13, 21]),
            EstimateScale::Linear => (&[1, 2, 3, 4, 5], &[6, 7]),
            EstimateScale::TShirt => (&[1, 2, 3, 5, 8], &[13, 21]),
        };
        let zero = self.allow_zero.then_some(0);
        let extra = if self.extended { extra } else { &[] };
        zero.into_iter()
            .chain(base.iter().copied())
            .chain(extra.iter().copied())
            .collect()
    }

    /// How Linear names `points` on this scale: "3 points", or a T-shirt
    /// size.
    pub fn label(self, points: u32) -> String {
        if self.scale == EstimateScale::TShirt {
            let size = match points {
                1 => Some("XS"),
                2 => Some("S"),
                3 => Some("M"),
                5 => Some("L"),
                8 => Some("XL"),
                13 => Some("XXL"),
                21 => Some("XXXL"),
                _ => None,
            };
            if let Some(size) = size {
                return size.to_owned();
            }
        }
        match points {
            1 => "1 point".to_owned(),
            points => format!("{points} points"),
        }
    }
}

/// Linear's workflow state categories. A category Linear adds later parses as
/// [`StateType::Backlog`], so it still gets a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StateType {
    Triage,
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
    Duplicate,
}

impl StateType {
    pub fn parse(value: &str) -> StateType {
        match value {
            "triage" => StateType::Triage,
            "unstarted" => StateType::Unstarted,
            "started" => StateType::Started,
            "completed" => StateType::Completed,
            "canceled" => StateType::Canceled,
            "duplicate" => StateType::Duplicate,
            _ => StateType::Backlog,
        }
    }

    /// The wire name, as Linear's filters take it.
    pub fn as_str(self) -> &'static str {
        match self {
            StateType::Triage => "triage",
            StateType::Backlog => "backlog",
            StateType::Unstarted => "unstarted",
            StateType::Started => "started",
            StateType::Completed => "completed",
            StateType::Canceled => "canceled",
            StateType::Duplicate => "duplicate",
        }
    }

    /// Display order for groups and board columns: in progress first, then
    /// todo, backlog, triage, and the closed categories last.
    pub fn rank(self) -> u8 {
        match self {
            StateType::Started => 0,
            StateType::Unstarted => 1,
            StateType::Backlog => 2,
            StateType::Triage => 3,
            StateType::Completed => 4,
            StateType::Canceled => 5,
            StateType::Duplicate => 6,
        }
    }

    /// Completed, canceled, and duplicate issues are closed.
    pub fn is_closed(self) -> bool {
        matches!(
            self,
            StateType::Completed | StateType::Canceled | StateType::Duplicate
        )
    }

    /// Group heading for a category, such as "In Progress" or "Todo".
    pub fn label(self) -> &'static str {
        match self {
            StateType::Triage => "Triage",
            StateType::Backlog => "Backlog",
            StateType::Unstarted => "Todo",
            StateType::Started => "In Progress",
            StateType::Completed => "Done",
            StateType::Canceled => "Canceled",
            StateType::Duplicate => "Duplicate",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowState {
    pub id: String,
    pub name: String,
    pub kind: StateType,
    pub position: f64,
    /// Hex color such as `#f2c94c`.
    pub color: String,
}

/// The state an issue is in, as embedded in the issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRef {
    pub id: String,
    pub name: String,
    pub kind: StateType,
    pub color: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRef {
    pub id: String,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
    /// The label group this label belongs to, if any.
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

/// A project as the projects list shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    pub status: ProjectStatus,
    /// Share of the project's issues done, 0 to 1.
    pub progress: f32,
    pub lead: Option<User>,
    pub target_date: Option<NaiveDate>,
    pub url: String,
    pub team_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectStatus {
    pub id: String,
    pub name: String,
    pub kind: ProjectStatusKind,
    /// `#rrggbb`.
    pub color: String,
}

/// Linear's project status categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectStatusKind {
    Backlog,
    Planned,
    Started,
    Paused,
    Completed,
    Canceled,
}

/// A project as its own page shows it: the header, the overview, and the
/// properties.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectDetail {
    pub id: String,
    pub name: String,
    /// Linear's icon name (`Shield`) or an emoji, when the project has one.
    pub icon: Option<String>,
    /// `#rrggbb`.
    pub color: String,
    /// The one-line summary under the name.
    pub summary: String,
    /// The long description, as Markdown.
    pub content: Option<String>,
    pub status: ProjectStatus,
    /// Linear's own figure: the share of the scope done, 0 to 1.
    pub progress: f32,
    pub lead: Option<User>,
    pub start_date: Option<NaiveDate>,
    pub target_date: Option<NaiveDate>,
    pub url: String,
    /// The team the project's issues are created in first.
    pub lead_team_id: String,
    pub teams: Vec<TeamRef>,
    pub members: Vec<User>,
    pub labels: Vec<Label>,
    /// In Linear's order.
    pub milestones: Vec<ProjectMilestone>,
    /// Newest first.
    pub updates: Vec<ProjectPost>,
    /// Documents, then links, as Linear lists them under Resources.
    pub resources: Vec<ProjectResource>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectMilestone {
    pub id: String,
    pub name: String,
    /// Linear's own figure, 0 to 1.
    pub progress: f32,
    pub target_date: Option<NaiveDate>,
}

/// A project update, as Linear posts it on the project's page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectPost {
    pub id: String,
    pub body: String,
    pub health: ProjectHealth,
    pub author: Option<User>,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectHealth {
    OnTrack,
    AtRisk,
    OffTrack,
}

impl ProjectHealth {
    pub fn label(self) -> &'static str {
        match self {
            ProjectHealth::OnTrack => "On track",
            ProjectHealth::AtRisk => "At risk",
            ProjectHealth::OffTrack => "Off track",
        }
    }
}

/// A document or a link the project keeps under Resources.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResource {
    pub id: String,
    pub title: String,
    pub url: String,
    pub kind: ProjectResourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectResourceKind {
    Document,
    Link,
}

/// What the properties of a project can be set to: the workspace's project
/// statuses, project labels, active users, and teams.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectOptions {
    /// In Linear's order.
    pub statuses: Vec<ProjectStatus>,
    /// Sorted by name; label groups are left out.
    pub labels: Vec<Label>,
    /// Sorted by name.
    pub users: Vec<User>,
    /// Sorted by name.
    pub teams: Vec<TeamRef>,
}

/// Fields to change on a project. `None` leaves a field alone; for clearable
/// fields, `Some(None)` clears it. A set replaces the whole set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectChange {
    pub status_id: Option<String>,
    pub lead_id: Option<Option<String>>,
    pub member_ids: Option<Vec<String>>,
    pub team_ids: Option<Vec<String>>,
    pub label_ids: Option<Vec<String>>,
    pub start_date: Option<Option<NaiveDate>>,
    pub target_date: Option<Option<NaiveDate>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cycle {
    pub id: String,
    pub number: u32,
    pub name: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// The cycle an issue is in, as embedded in the issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleRef {
    pub id: String,
    pub number: u32,
    pub name: Option<String>,
}

/// Linear's priority scale. The wire value is 0 (none) to 4 (low), with 1 the
/// most urgent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Priority {
    #[default]
    None,
    Urgent,
    High,
    Medium,
    Low,
}

impl Priority {
    /// Every priority in the order pickers list them.
    pub const ALL: [Priority; 5] = [
        Priority::Urgent,
        Priority::High,
        Priority::Medium,
        Priority::Low,
        Priority::None,
    ];

    /// Parses Linear's number; anything outside 1..=4 is no priority.
    pub fn from_wire(value: f64) -> Priority {
        match value as i64 {
            1 => Priority::Urgent,
            2 => Priority::High,
            3 => Priority::Medium,
            4 => Priority::Low,
            _ => Priority::None,
        }
    }

    pub fn to_wire(self) -> i64 {
        match self {
            Priority::None => 0,
            Priority::Urgent => 1,
            Priority::High => 2,
            Priority::Medium => 3,
            Priority::Low => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Priority::None => "No priority",
            Priority::Urgent => "Urgent",
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
        }
    }
}

/// An issue as lists, boards, the palette, and the PR panel show it. Also what
/// mutations return.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueSummary {
    pub id: String,
    /// Such as `ENG-123`.
    pub identifier: String,
    pub title: String,
    pub priority: Priority,
    /// Linear's manual order: lower sorts first.
    pub sort_order: f64,
    pub state: StateRef,
    pub team: TeamRef,
    pub assignee: Option<User>,
    pub labels: Vec<Label>,
    pub project: Option<Project>,
    pub cycle: Option<CycleRef>,
    pub estimate: Option<f64>,
    pub due_date: Option<NaiveDate>,
    /// The issue's page on linear.app.
    pub url: String,
    /// The git branch name Linear suggests for the issue, in the
    /// workspace's format (such as `alice/eng-123-fix-login`). Empty when
    /// Linear sent none.
    #[serde(default)]
    pub branch_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub creator_id: Option<String>,
    /// The parent issue's id, when this is a sub-issue.
    pub parent_id: Option<String>,
    pub has_sub_issues: bool,
    pub milestone: Option<MilestoneRef>,
    /// When the issue was completed or canceled.
    pub closed_at: Option<DateTime<Utc>>,
    /// GitHub pull requests attached to the issue: Linear returns only
    /// attachments whose URL has `/pull/`, up to 20, so other attachments
    /// never crowd them out.
    pub pull_requests: Vec<PullRequestLink>,
    /// False when Linear had more pull request attachments than one page.
    /// Every `Linear` method fetches the rest before it returns, so callers
    /// always see true.
    pub pull_requests_complete: bool,
}

/// The project milestone an issue is in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneRef {
    pub id: String,
    pub name: String,
}

/// An issue view saved in Linear, as `customViews` returns it.
/// [`crate::views::linear_preferences`] turns it into Loupe's view settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearCustomView {
    pub id: String,
    pub name: String,
    /// Linear's icon name, such as `Bolt`; `None` for the default icon.
    pub icon: Option<String>,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// Visible to the workspace, not only to its owner.
    pub shared: bool,
    pub owner_name: String,
    /// The team the view belongs to; `None` for a workspace view.
    pub team_id: Option<String>,
    /// Linear's `IssueFilter` JSON.
    pub filter_data: serde_json::Value,
    /// The view's display settings (layout, grouping, ordering, shown
    /// properties) as JSON with Linear's names: the ones in force, with the
    /// key owner's own on top. `Null` when Linear sent none.
    pub preferences: serde_json::Value,
    /// What the key owner set for this view, without the workspace's
    /// defaults: the base [`crate::views::export_preferences`] writes on,
    /// because Linear replaces the owner's whole object on a save. `Null`
    /// when the owner set nothing.
    #[serde(default)]
    pub own_preferences: serde_json::Value,
}

/// A view to save in Linear.
#[derive(Clone, Debug, PartialEq)]
pub struct NewCustomView {
    pub name: String,
    /// The team whose screen lists the view; `None` for a workspace view.
    pub team_id: Option<String>,
    /// Visible to the workspace. A view made in Loupe starts private.
    pub shared: bool,
    /// Linear's `IssueFilter`, from [`crate::views::export_linear_filter`].
    pub filter_data: serde_json::Value,
}

/// What to change in a saved view; `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomViewUpdate {
    pub name: Option<String>,
    pub filter_data: Option<serde_json::Value>,
}

/// One item of Linear's Favorites, in the order Linear's sidebar shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinearFavorite {
    pub id: String,
    pub title: String,
    /// Linear's icon name; `None` for the item's default icon.
    pub icon: Option<String>,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// The item in Linear's web app.
    pub url: Option<String>,
    pub target: FavoriteTarget,
}

/// What a favorite opens.
#[derive(Clone, Debug, PartialEq)]
pub enum FavoriteTarget {
    /// A custom view; `team` is `None` for a workspace view.
    CustomView {
        view_id: String,
        team: Option<TeamSummary>,
    },
    /// One of a team's built-in views, such as `allIssues` or `backlog`.
    PredefinedView {
        kind: String,
        team: Option<TeamSummary>,
    },
    Issue {
        identifier: String,
    },
    /// Anything else (project, cycle, document, …), by Linear's type name.
    Other {
        kind: String,
    },
}

/// One issue with its description, discussion, and attachments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub issue: IssueSummary,
    /// Markdown; empty when the issue has none.
    pub description: String,
    pub creator: Option<User>,
    /// Oldest first.
    pub comments: Vec<Comment>,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    /// Markdown.
    pub body: String,
    pub author: Author,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    /// The comment this one replies to.
    pub parent_id: Option<String>,
}

/// Who wrote a comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Author {
    User(User),
    /// An integration, such as GitHub or Slack.
    Bot {
        name: String,
        avatar_url: Option<String>,
    },
    /// A deleted user or an author Linear does not report.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub url: String,
    pub title: String,
    pub subtitle: Option<String>,
    /// Set when `url` is a GitHub pull request.
    pub pull_request: Option<PullRequestLink>,
}

/// A GitHub pull request attached to an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestLink {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub state: PullRequestState,
}

/// What Linear's GitHub integration last reported about a pull request.
/// `Unknown` when the attachment carries no status metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullRequestState {
    Open,
    Draft,
    Merged,
    Closed,
    Unknown,
}

/// One page of a paginated list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub nodes: Vec<T>,
    /// Pass back as `after` to read the next page; `None` on the last page.
    pub next_cursor: Option<String>,
}

/// Which closed (completed, canceled, duplicate) issues a query includes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClosedIssues {
    #[default]
    All,
    None,
    /// Closed issues that closed after this instant.
    ClosedSince(DateTime<Utc>),
}

/// What [`crate::Linear::team_issues`] asks for: issues matching every clause,
/// within the closed-issue window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IssueQuery {
    pub clauses: Vec<crate::filter::FilterClause>,
    pub closed: ClosedIssues,
}

/// Fields to change on an issue. `None` leaves a field alone; for clearable
/// fields, `Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IssueUpdate {
    pub state_id: Option<String>,
    pub priority: Option<Priority>,
    pub assignee_id: Option<Option<String>>,
    /// Replaces the whole label set.
    pub label_ids: Option<Vec<String>>,
    /// Labels to put on the issue, keeping the others. Unlike `label_ids`,
    /// two edits that each add or remove one label never undo each other.
    #[serde(default)]
    pub added_label_ids: Vec<String>,
    /// Labels to take off the issue, keeping the others.
    #[serde(default)]
    pub removed_label_ids: Vec<String>,
    pub estimate: Option<Option<f64>>,
    pub due_date: Option<Option<NaiveDate>>,
    pub project_id: Option<Option<String>>,
    pub cycle_id: Option<Option<String>>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// The manual order (see [`IssueSummary::sort_order`]).
    pub sort_order: Option<f64>,
}

/// A new issue. Unset optional fields take the team's defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewIssue {
    pub team_id: String,
    pub title: String,
    pub description: Option<String>,
    pub state_id: Option<String>,
    pub priority: Option<Priority>,
    pub assignee_id: Option<String>,
    pub label_ids: Vec<String>,
    pub project_id: Option<String>,
    pub cycle_id: Option<String>,
    /// Points on the team's [`Estimation`] scale.
    pub estimate: Option<u32>,
    pub due_date: Option<NaiveDate>,
}
