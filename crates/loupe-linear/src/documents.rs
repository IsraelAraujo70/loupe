//! Every GraphQL document the client sends. Each is a named operation whose
//! name is also the stub server's dispatch key and its fixture file name
//! (`tests/fixtures/<name>.json`). The schema gate and the live validation
//! gate iterate [`ALL`], so a document missing from it is never checked.

/// One named GraphQL operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Document {
    /// The operation name, such as `Viewer`.
    pub name: &'static str,
    pub text: &'static str,
    pub kind: DocumentKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Query,
    Mutation,
}

/// The fields every issue list selects, so all of them decode into one wire
/// type. Linear's search results are a distinct GraphQL type with the same
/// fields, hence the two fragments.
macro_rules! summary_fields {
    () => {
        "id identifier title priority sortOrder estimate dueDate url branchName createdAt updatedAt \
         state { id name type color } \
         team { id key name } \
         assignee { id name displayName avatarUrl } \
         labels(first: 50) { nodes { id name color parent { id } } } \
         project { id name color } \
         cycle { id number name } \
         creator { id } \
         parent { id } \
         children(first: 1) { nodes { id } } \
         projectMilestone { id name } \
         completedAt canceledAt \
         pullRequests: attachments(first: 20, filter: { and: [ \
             { url: { contains: \"github.com/\" } }, { url: { contains: \"/pull/\" } } ] }) { \
             nodes { url metadata } pageInfo { hasNextPage endCursor } }"
    };
}

macro_rules! summary_fragment {
    () => {
        concat!(
            " fragment IssueSummary on Issue { ",
            summary_fields!(),
            " }"
        )
    };
}

macro_rules! search_summary_fragment {
    () => {
        concat!(
            " fragment IssueSearchSummary on IssueSearchResult { ",
            summary_fields!(),
            " }"
        )
    };
}

pub const VIEWER: Document = Document {
    name: "Viewer",
    text: "query Viewer { viewer { id name displayName avatarUrl email organization { id name urlKey logoUrl } } }",
    kind: DocumentKind::Query,
};

pub const TEAMS: Document = Document {
    name: "Teams",
    text: "query Teams { teams(first: 250) { nodes { id key name } } }",
    kind: DocumentKind::Query,
};

/// The teams the key's owner is a member of: Linear's "Your teams".
pub const MY_TEAMS: Document = Document {
    name: "MyTeams",
    text: "query MyTeams { viewer { teams(first: 100) { nodes { id key name } } } }",
    kind: DocumentKind::Query,
};

/// One team's pickers. Linear's complexity limit is 10,000 points; this
/// selection is about 1,000 even when every connection is full. The labels
/// come from the root `issueLabels` so workspace labels (team null) are
/// included next to the team's own. `$id` feeds `team(id:)` (a `String`) and
/// `$teamId` the label filter (an `ID`), because GraphQL does not coerce
/// between the two in variable positions.
pub const TEAM: Document = Document {
    name: "Team",
    text: "query Team($id: String!, $teamId: ID!, $now: DateTimeOrDuration!) { \
        team(id: $id) { \
            id key name \
            issueEstimationType issueEstimationAllowZero issueEstimationExtended \
            states(first: 100) { nodes { id name type position color } } \
            members(first: 250) { nodes { id name displayName avatarUrl active } } \
            projects(first: 100) { nodes { id name color } } \
            cycles(first: 25, filter: { endsAt: { gt: $now } }) { \
                nodes { id number name startsAt endsAt } } \
        } \
        issueLabels(first: 250, filter: { or: [ \
            { team: { id: { eq: $teamId } } }, { team: { null: true } } ] }) { \
            nodes { id name color isGroup parent { id } } } \
    }",
    kind: DocumentKind::Query,
};

pub const MY_OPEN_ISSUES: Document = Document {
    name: "MyOpenIssues",
    text: concat!(
        "query MyOpenIssues($filter: IssueFilter, $after: String) { viewer { \
            assignedIssues(filter: $filter, first: 100, after: $after, orderBy: updatedAt) { \
                nodes { ...IssueSummary } pageInfo { hasNextPage endCursor } } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const TEAM_ISSUES: Document = Document {
    name: "TeamIssues",
    text: concat!(
        "query TeamIssues($filter: IssueFilter, $after: String) { \
            issues(filter: $filter, first: 50, after: $after, orderBy: updatedAt) { \
                nodes { ...IssueSummary } pageInfo { hasNextPage endCursor } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

/// Issues filtered by Linear itself, including filters Loupe cannot edit.
pub const CUSTOM_VIEW_ISSUES: Document = Document {
    name: "CustomViewIssues",
    text: concat!(
        "query CustomViewIssues($id: String!, $after: String) { customView(id: $id) { issues(first: 100, after: $after, orderBy: updatedAt) { nodes { ...IssueSummary } pageInfo { hasNextPage endCursor } } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const ISSUE: Document = Document {
    name: "Issue",
    text: concat!(
        "query Issue($id: String!) { issue(id: $id) { \
            ...IssueSummary \
            description \
            creator { id name displayName avatarUrl } \
            comments(first: 100) { nodes { \
                id body createdAt editedAt \
                user { id name displayName avatarUrl } \
                botActor { type name avatarUrl } \
                parent { id } } } \
            attachments(first: 50) { nodes { \
                id url title subtitle sourceType metadata } } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const SEARCH_ISSUES: Document = Document {
    name: "SearchIssues",
    text: concat!(
        "query SearchIssues($term: String!, $first: Int!, $filter: IssueFilter) { \
            searchIssues(term: $term, first: $first, filter: $filter) { \
                nodes { ...IssueSearchSummary } } }",
        search_summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const ISSUES_FOR_URL: Document = Document {
    name: "IssuesForUrl",
    text: concat!(
        "query IssuesForUrl($url: String!) { \
            attachmentsForURL(url: $url, first: 50) { nodes { issue { ...IssueSummary } } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const ISSUES_BY_IDENTIFIERS: Document = Document {
    name: "IssuesByIdentifiers",
    text: concat!(
        "query IssuesByIdentifiers($filter: IssueFilter!, $first: Int!) { \
            issues(filter: $filter, first: $first) { nodes { ...IssueSummary } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const RECENT_ISSUES: Document = Document {
    name: "RecentIssues",
    text: concat!(
        "query RecentIssues($filter: IssueFilter!, $first: Int!) { \
            issues(filter: $filter, first: $first, orderBy: updatedAt) { \
                nodes { ...IssueSummary } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const ISSUE_UPDATE: Document = Document {
    name: "IssueUpdate",
    text: concat!(
        "mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) { \
            issueUpdate(id: $id, input: $input) { success issue { ...IssueSummary } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Mutation,
};

pub const COMMENT_CREATE: Document = Document {
    name: "CommentCreate",
    text: "mutation CommentCreate($input: CommentCreateInput!) { \
        commentCreate(input: $input) { success comment { \
            id body createdAt editedAt \
            user { id name displayName avatarUrl } \
            parent { id } } } }",
    kind: DocumentKind::Mutation,
};

pub const ISSUE_CREATE: Document = Document {
    name: "IssueCreate",
    text: concat!(
        "mutation IssueCreate($input: IssueCreateInput!) { \
            issueCreate(input: $input) { success issue { ...IssueSummary } } }",
        summary_fragment!()
    ),
    kind: DocumentKind::Mutation,
};

/// The rest of an issue's pull request attachments, when the summary's first
/// page was full.
pub const ISSUE_PULL_REQUESTS: Document = Document {
    name: "IssuePullRequests",
    text: "query IssuePullRequests($id: String!, $after: String) { issue(id: $id) { \
        pullRequests: attachments(first: 50, after: $after, filter: { and: [ \
            { url: { contains: \"github.com/\" } }, { url: { contains: \"/pull/\" } } ] }) { \
            nodes { url metadata } pageInfo { hasNextPage endCursor } } } }",
    kind: DocumentKind::Query,
};

/// The display settings Loupe reads from and writes to a view's
/// preferences.
macro_rules! preference_fields {
    () => {
        "layout issueGrouping viewOrdering viewOrderingDirection showCompletedIssues \
         showEmptyGroups showEmptyGroupsBoard showEmptyGroupsList showSubIssues \
         hiddenColumns hiddenGroupsList columnOrderBoard \
         fieldId fieldStatus fieldAssignee fieldPriority fieldProject fieldDueDate \
         fieldCycle fieldLabels fieldEstimate fieldPullRequests fieldDateCreated \
         fieldDateUpdated"
    };
}

/// The other settings of an issue view's preferences. Loupe does not use
/// them; it reads the key owner's so that saving display settings, which
/// replaces that owner's whole preferences object, writes them back.
macro_rules! kept_preference_fields {
    () => {
        "closedIssuesOrderedByRecency columnOrderList \
         fieldCustomerCount fieldCustomerRevenue fieldDateArchived fieldDateMyActivity \
         fieldLinkCount fieldMilestone fieldPreviewLinks fieldRelease fieldSentryIssues \
         fieldSla fieldTimeInCurrentStatus fieldUserPresence groupOrderingMode hiddenRows \
         issueGroupingLabelGroupId issueNesting issueSubGrouping issueSubGroupingLabelGroupId \
         openInSplitView showArchivedItems showEmptySubGroups showEmptySubGroupsBoard \
         showEmptySubGroupsList showParents showSubTeamIssues showSupervisedIssues \
         showTriageIssues"
    };
}

/// A custom view with its filter and display settings: the settings in
/// force (`viewPreferencesValues`) and the key owner's own, which are the
/// ones Loupe writes.
macro_rules! custom_view_fragment {
    () => {
        concat!(
            " fragment CustomViewFields on CustomView { \
                id name icon color shared modelName team { id } owner { name } filterData \
                viewPreferencesValues { ",
            preference_fields!(),
            " } userViewPreferences { preferences { ",
            preference_fields!(),
            " ",
            kept_preference_fields!(),
            " } } }"
        )
    };
}

/// Linear's saved views with their filters and display settings.
pub const CUSTOM_VIEWS: Document = Document {
    name: "CustomViews",
    text: concat!(
        "query CustomViews($after: String) { \
            customViews(first: 50, after: $after) { \
                nodes { ...CustomViewFields } \
                pageInfo { hasNextPage endCursor } } }",
        custom_view_fragment!()
    ),
    kind: DocumentKind::Query,
};

/// One saved view, read again after its display settings were written.
pub const CUSTOM_VIEW: Document = Document {
    name: "CustomView",
    text: concat!(
        "query CustomView($id: String!) { customView(id: $id) { ...CustomViewFields } }",
        custom_view_fragment!()
    ),
    kind: DocumentKind::Query,
};

pub const CUSTOM_VIEW_CREATE: Document = Document {
    name: "CustomViewCreate",
    text: concat!(
        "mutation CustomViewCreate($input: CustomViewCreateInput!) { \
            customViewCreate(input: $input) { success customView { ...CustomViewFields } } }",
        custom_view_fragment!()
    ),
    kind: DocumentKind::Mutation,
};

pub const CUSTOM_VIEW_UPDATE: Document = Document {
    name: "CustomViewUpdate",
    text: concat!(
        "mutation CustomViewUpdate($id: String!, $input: CustomViewUpdateInput!) { \
            customViewUpdate(id: $id, input: $input) { success customView { ...CustomViewFields } } }",
        custom_view_fragment!()
    ),
    kind: DocumentKind::Mutation,
};

pub const CUSTOM_VIEW_DELETE: Document = Document {
    name: "CustomViewDelete",
    text: "mutation CustomViewDelete($id: String!) { customViewDelete(id: $id) { success } }",
    kind: DocumentKind::Mutation,
};

/// Writes the key owner's display settings for a view. Linear replaces the
/// settings that owner already had for it.
pub const VIEW_PREFERENCES_SAVE: Document = Document {
    name: "ViewPreferencesSave",
    text: "mutation ViewPreferencesSave($input: ViewPreferencesCreateInput!) { \
        viewPreferencesCreate(input: $input) { success } }",
    kind: DocumentKind::Mutation,
};

/// Adds a custom view to the key owner's favorites.
pub const FAVORITE_CREATE: Document = Document {
    name: "FavoriteCreate",
    text: "mutation FavoriteCreate($input: FavoriteCreateInput!) { \
        favoriteCreate(input: $input) { success } }",
    kind: DocumentKind::Mutation,
};

/// The key owner's favorites, as Linear's sidebar lists them. Read-only.
pub const FAVORITES: Document = Document {
    name: "Favorites",
    text: "query Favorites($after: String) { \
        favorites(first: 100, after: $after) { \
            nodes { id type title icon color url sortOrder parent { id } \
                customView { id team { id key name } } \
                predefinedViewType predefinedViewTeam { id key name } \
                issue { identifier } } \
            pageInfo { hasNextPage endCursor } } }",
    kind: DocumentKind::Query,
};

/// Projects, newest update first; `$filter` narrows to a team's.
pub const PROJECTS: Document = Document {
    name: "Projects",
    text: "query Projects($after: String, $filter: ProjectFilter) { \
        projects(first: 50, after: $after, filter: $filter, orderBy: updatedAt) { \
            nodes { id name color url progress targetDate \
                status { id name type color } \
                lead { id name displayName avatarUrl } \
                teams { nodes { id } } } \
            pageInfo { hasNextPage endCursor } } }",
    kind: DocumentKind::Query,
};

/// The fields of a project's page: its header, overview, and properties.
/// Every connection is bounded, so the query stays far under Linear's
/// complexity limit.
macro_rules! project_fragment {
    () => {
        " fragment ProjectDetail on Project { \
            id name icon color description content url progress startDate targetDate \
            status { id name type color } \
            lead { id name displayName avatarUrl } \
            leadTeam { id } \
            teams(first: 50) { nodes { id key name } } \
            members(first: 100) { nodes { id name displayName avatarUrl } } \
            labels(first: 50) { nodes { id name color } } \
            projectMilestones(first: 50) { nodes { id name progress targetDate sortOrder } } \
            projectUpdates(first: 10) { nodes { \
                id body health createdAt url user { id name displayName avatarUrl } } } \
            documents(first: 50) { nodes { id title url } } \
            externalLinks(first: 50) { nodes { id label url } } }"
    };
}

pub const PROJECT: Document = Document {
    name: "Project",
    text: concat!(
        "query Project($id: String!) { project(id: $id) { ...ProjectDetail } }",
        project_fragment!()
    ),
    kind: DocumentKind::Query,
};

/// What a project's properties can be set to. Apps (the "Linear" agent and
/// integrations) are left out of the people: Linear refuses them as lead or
/// member ("Invalid project users", seen live on 2026-11-11).
pub const PROJECT_OPTIONS: Document = Document {
    name: "ProjectOptions",
    text: "query ProjectOptions { \
        organization { projectStatuses { id name type color position } } \
        projectLabels(first: 250) { nodes { id name color isGroup } } \
        users(first: 250, filter: { active: { eq: true }, app: { eq: false } }) { \
            nodes { id name displayName avatarUrl } } \
        teams(first: 250) { nodes { id key name } } }",
    kind: DocumentKind::Query,
};

pub const PROJECT_UPDATE: Document = Document {
    name: "ProjectUpdate",
    text: concat!(
        "mutation ProjectUpdate($id: String!, $input: ProjectUpdateInput!) { \
            projectUpdate(id: $id, input: $input) { success project { ...ProjectDetail } } }",
        project_fragment!()
    ),
    kind: DocumentKind::Mutation,
};

/// Every document, for the schema and live validation gates.
pub const ALL: &[Document] = &[
    VIEWER,
    TEAMS,
    MY_TEAMS,
    TEAM,
    MY_OPEN_ISSUES,
    TEAM_ISSUES,
    ISSUE,
    SEARCH_ISSUES,
    ISSUES_FOR_URL,
    ISSUES_BY_IDENTIFIERS,
    RECENT_ISSUES,
    ISSUE_UPDATE,
    COMMENT_CREATE,
    ISSUE_CREATE,
    CUSTOM_VIEWS,
    CUSTOM_VIEW,
    CUSTOM_VIEW_CREATE,
    CUSTOM_VIEW_UPDATE,
    CUSTOM_VIEW_DELETE,
    VIEW_PREFERENCES_SAVE,
    FAVORITE_CREATE,
    FAVORITES,
    CUSTOM_VIEW_ISSUES,
    ISSUE_PULL_REQUESTS,
    PROJECTS,
    PROJECT,
    PROJECT_OPTIONS,
    PROJECT_UPDATE,
];
