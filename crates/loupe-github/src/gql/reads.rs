//! Search-backed lists: the inbox, repository PR lists, and repositories.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    Actor, ChecksState, Error, GitHub, Inbox, Label, Mergeable, Page, PrListFilter, PrRef, PrState,
    PrSummary, RepoRef, RepoSummary, Result, ReviewDecision, StackMember, StackSummary,
};

const REPO_PR_PAGE_SIZE: u32 = 30;
const MAX_PAGE_SIZE: u32 = 100;

const INBOX_REVIEW_REQUESTED: &str =
    "is:open is:pr archived:false review-requested:@me sort:updated-desc";
const INBOX_AUTHORED: &str = "is:open is:pr archived:false author:@me sort:updated-desc";
const INBOX_INVOLVED: &str =
    "is:open is:pr archived:false involves:@me -author:@me -review-requested:@me sort:updated-desc";

macro_rules! pr_fragments {
    () => {
        "
fragment PrRow on PullRequest {
  id
  number
  title
  url
  state
  isDraft
  createdAt
  updatedAt
  additions
  deletions
  headRefName
  baseRefName
  isCrossRepository
  repository { owner { login } name }
  author { login avatarUrl }
  comments { totalCount }
  reviewDecision
  mergeable
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
  labels(first: 20) { nodes { id name color } }
  assignees(first: 10) { nodes { login avatarUrl } }
  reviewRequests(first: 10) { nodes { requestedReviewer { ... on User { login avatarUrl } } } }
  latestReviews(first: 20) { nodes { state author { login avatarUrl } } }
  stack {
    number
    entries(first: 50) { nodes { position pullRequest { number title state } } }
  }
}
fragment PrPage on SearchResultItemConnection {
  issueCount
  pageInfo { hasNextPage endCursor }
  nodes { ... on PullRequest { ...PrRow } }
}
"
    };
}

macro_rules! repo_fragment {
    () => {
        "
fragment RepoRow on Repository {
  name
  owner { login }
  description
  isPrivate
  pushedAt
}
"
    };
}

const INBOX_QUERY: &str = concat!(
    "query Inbox($reviewRequested: String!, $authored: String!, $involved: String!) {
  reviewRequested: search(type: ISSUE, first: 100, query: $reviewRequested) { ...PrPage }
  authored: search(type: ISSUE, first: 100, query: $authored) { ...PrPage }
  involved: search(type: ISSUE, first: 100, query: $involved) { ...PrPage }
}",
    pr_fragments!()
);

const SEARCH_PULL_REQUESTS_QUERY: &str = concat!(
    "query SearchPullRequests($query: String!, $first: Int!, $after: String) {
  search(type: ISSUE, first: $first, after: $after, query: $query) { ...PrPage }
}",
    pr_fragments!()
);

const VIEWER_REPOSITORIES_QUERY: &str = concat!(
    "query ViewerRepositories {
  viewer {
    repositories(
      first: 100
      affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER]
      ownerAffiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER]
      orderBy: { field: PUSHED_AT, direction: DESC }
    ) {
      nodes { ...RepoRow }
    }
  }
}",
    repo_fragment!()
);

const SEARCH_REPOSITORIES_QUERY: &str = concat!(
    "query SearchRepositories($query: String!) {
  search(type: REPOSITORY, first: 20, query: $query) {
    nodes { ... on Repository { ...RepoRow } }
  }
}",
    repo_fragment!()
);

const REPOSITORY_LABELS_QUERY: &str = "query RepositoryLabels($owner: String!, $name: String!, \
    $cursor: String) { repository(owner: $owner, name: $name) { \
    labels(first: 100, after: $cursor, orderBy: {field: NAME, direction: ASC}) { \
    pageInfo { hasNextPage endCursor } nodes { id name color } } } }";

/// An "All open" search reads 50 pull requests a request, up to 500. A page
/// of 100 costs 5 rate-limit points against 3 for 50, whatever the search
/// holds, and on a busy repository it nears GitHub's 10 s limit (zed: 8 s,
/// and some runs fail with 502).
const ALL_OPEN_PAGE_SIZE: u32 = 50;
const MAX_ALL_OPEN_PAGES: usize = 10;

/// Branches counted per request by [`GitHub::open_pull_requests_by_base`].
const BASES_PER_REQUEST: usize = 50;

/// One field per branch asking how many open pull requests target it, with
/// the repository and the branch as variables, never spliced into the text.
pub(crate) fn open_by_base_query(count: usize) -> String {
    let variables: Vec<String> = (0..count)
        .map(|at| format!("$o{at}: String!, $n{at}: String!, $b{at}: String!"))
        .collect();
    let fields: Vec<String> = (0..count)
        .map(|at| {
            format!(
                "  b{at}: repository(owner: $o{at}, name: $n{at}) {{ \
                 pullRequests(baseRefName: $b{at}, states: OPEN) {{ totalCount }} }}"
            )
        })
        .collect();
    format!(
        "query OpenPullRequestsByBase({}) {{\n{}\n}}",
        variables.join(", "),
        fields.join("\n")
    )
}

/// Pages of labels read at most; GitHub repositories rarely have more than a
/// few dozen.
const MAX_LABEL_PAGES: usize = 5;

pub(crate) const DOCUMENTS: &[(&str, &str)] = &[
    ("inbox", INBOX_QUERY),
    ("search_pull_requests", SEARCH_PULL_REQUESTS_QUERY),
    ("viewer_repositories", VIEWER_REPOSITORIES_QUERY),
    ("search_repositories", SEARCH_REPOSITORIES_QUERY),
    ("repository_labels", REPOSITORY_LABELS_QUERY),
];

impl GitHub {
    /// The dashboard's three searches in one request, 100 items each (GitHub's
    /// page maximum), so the Reviews filters see every pull request a section
    /// usually holds.
    pub fn inbox(&self) -> Result<Inbox> {
        let data: InboxData = self.graphql(
            INBOX_QUERY,
            json!({
                "reviewRequested": INBOX_REVIEW_REQUESTED,
                "authored": INBOX_AUTHORED,
                "involved": INBOX_INVOLVED,
            }),
        )?;
        data.into_inbox()
    }

    /// Runs a raw search query (GitHub search syntax). `is:pr` is added when
    /// the query does not already restrict the type.
    pub fn search_pull_requests(
        &self,
        query: &str,
        first: u32,
        after: Option<&str>,
    ) -> Result<Page<PrSummary>> {
        self.search_page(&with_pr_type(query), first.clamp(1, MAX_PAGE_SIZE), after)
    }

    /// Every pull request `query` finds, 50 a request, up to 500. The page
    /// keeps the search's total and the cursor after the last one read, so
    /// a caller can say when it holds only part of the search. A failure
    /// after the first page keeps the pages already read.
    pub fn search_all_pull_requests(&self, query: &str) -> Result<Page<PrSummary>> {
        let query = with_pr_type(query);
        // GitHub answers a search that runs past its time limit with a 502
        // or 504 that usually passes on a second try.
        let read = |after: Option<&str>| match self.search_page(&query, ALL_OPEN_PAGE_SIZE, after) {
            Err(Error::Http { status, .. }) if status >= 500 => {
                self.search_page(&query, ALL_OPEN_PAGE_SIZE, after)
            }
            other => other,
        };
        let mut page = read(None)?;
        for _ in 1..MAX_ALL_OPEN_PAGES {
            let Some(after) = page.next_cursor.clone() else {
                break;
            };
            let Ok(next) = read(Some(&after)) else {
                break;
            };
            page.items.extend(next.items);
            page.next_cursor = next.next_cursor;
        }
        Ok(page)
    }

    /// Pull requests of one repository, most recently updated first,
    /// optionally narrowed by free text.
    pub fn repo_pull_requests(
        &self,
        repo: &RepoRef,
        filter: PrListFilter,
        text: Option<&str>,
        after: Option<&str>,
    ) -> Result<Page<PrSummary>> {
        self.search_page(
            &repo_search_query(repo, filter, text),
            REPO_PR_PAGE_SIZE,
            after,
        )
    }

    /// Repositories the viewer owns, collaborates on, or reaches through an
    /// organization, most recently pushed first (up to 100).
    pub fn viewer_repositories(&self) -> Result<Vec<RepoSummary>> {
        let data: ViewerRepositoriesData = self.graphql(VIEWER_REPOSITORIES_QUERY, json!({}))?;
        repositories_from_nodes(data.viewer.repositories.nodes.unwrap_or_default())
    }

    /// Every label of `repo`, by name.
    pub fn repository_labels(&self, repo: &RepoRef) -> Result<Vec<Label>> {
        let mut labels = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_LABEL_PAGES {
            let data: RepositoryLabelsData = self.graphql(
                REPOSITORY_LABELS_QUERY,
                json!({ "owner": repo.owner, "name": repo.name, "cursor": cursor }),
            )?;
            let page = data
                .repository
                .ok_or_else(|| Error::Decode(format!("repository {repo} not found")))?
                .labels;
            labels.extend(
                page.nodes
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .map(|label| Label {
                        id: label.id,
                        name: label.name,
                        color: label.color,
                    }),
            );
            match page.page_info.end_cursor {
                Some(next) if page.page_info.has_next_page => cursor = Some(next),
                _ => break,
            }
        }
        Ok(labels)
    }

    /// How many open pull requests target each `(repository, branch)` of
    /// `bases`, in the same order; `None` for a repository GitHub did not
    /// return. A pull request is stacked on another only when it is the one
    /// open pull request on that one's head branch, which the loaded pages
    /// alone cannot tell: a long-lived branch (`staging`) has others.
    pub fn open_pull_requests_by_base(
        &self,
        bases: &[(RepoRef, String)],
    ) -> Result<Vec<Option<u32>>> {
        let mut counts = Vec::with_capacity(bases.len());
        for chunk in bases.chunks(BASES_PER_REQUEST) {
            let mut variables = serde_json::Map::new();
            for (at, (repo, branch)) in chunk.iter().enumerate() {
                variables.insert(format!("o{at}"), json!(repo.owner));
                variables.insert(format!("n{at}"), json!(repo.name));
                variables.insert(format!("b{at}"), json!(branch));
            }
            let data: HashMap<String, Option<WireBaseCount>> =
                self.graphql(&open_by_base_query(chunk.len()), Value::Object(variables))?;
            counts.extend((0..chunk.len()).map(|at| {
                data.get(&format!("b{at}"))
                    .and_then(Option::as_ref)
                    .map(|repo| repo.pull_requests.total_count)
            }));
        }
        Ok(counts)
    }

    /// Repository search (GitHub search syntax), up to 20 results.
    pub fn search_repositories(&self, query: &str) -> Result<Vec<RepoSummary>> {
        let data: SearchData =
            self.graphql(SEARCH_REPOSITORIES_QUERY, json!({ "query": query }))?;
        repositories_from_nodes(data.search.nodes.unwrap_or_default())
    }

    fn search_page(&self, query: &str, first: u32, after: Option<&str>) -> Result<Page<PrSummary>> {
        let data: SearchData = self.graphql(
            SEARCH_PULL_REQUESTS_QUERY,
            json!({ "query": query, "first": first, "after": after }),
        )?;
        data.search.into_page()
    }
}

/// Appends `is:pr` unless the query already pins the type.
fn with_pr_type(query: &str) -> String {
    let pinned = query
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("is:pr") || token.eq_ignore_ascii_case("type:pr"));
    if pinned {
        query.trim().to_owned()
    } else {
        format!("{} is:pr", query.trim()).trim().to_owned()
    }
}

fn repo_search_query(repo: &RepoRef, filter: PrListFilter, text: Option<&str>) -> String {
    let mut query = format!("repo:{repo} is:pr");
    match filter {
        PrListFilter::Open => query.push_str(" is:open"),
        PrListFilter::Closed => query.push_str(" is:closed is:unmerged"),
        PrListFilter::Merged => query.push_str(" is:merged"),
        PrListFilter::All => {}
    }
    if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
        query.push(' ');
        query.push_str(text);
    }
    query.push_str(" sort:updated-desc");
    query
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBaseCount {
    pull_requests: WireCount,
}

#[derive(Deserialize)]
struct SearchData {
    search: WireConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboxData {
    review_requested: WireConnection,
    authored: WireConnection,
    involved: WireConnection,
}

impl InboxData {
    fn into_inbox(self) -> Result<Inbox> {
        Ok(Inbox {
            review_requested: self.review_requested.into_page()?,
            authored: self.authored.into_page()?,
            involved: self.involved.into_page()?,
        })
    }
}

#[derive(Deserialize)]
struct ViewerRepositoriesData {
    viewer: WireViewer,
}

#[derive(Deserialize)]
struct WireViewer {
    repositories: WireNodes,
}

#[derive(Deserialize)]
struct WireNodes {
    nodes: Option<Vec<Value>>,
}

/// A search connection. Nodes stay raw because search returns an empty object
/// for every result that is not the requested type.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireConnection {
    #[serde(default)]
    issue_count: u32,
    page_info: Option<WirePageInfo>,
    nodes: Option<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct RepositoryLabelsData {
    repository: Option<WireLabelsRepository>,
}

#[derive(Deserialize)]
struct WireLabelsRepository {
    labels: WireLabelConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLabelConnection {
    page_info: WirePageInfo,
    nodes: Option<Vec<Option<WireRepositoryLabel>>>,
}

#[derive(Deserialize)]
struct WireRepositoryLabel {
    id: String,
    name: String,
    color: String,
}

impl WireConnection {
    fn into_page(self) -> Result<Page<PrSummary>> {
        let items = decode_nodes::<WirePullRequest>(self.nodes.unwrap_or_default())?
            .into_iter()
            .map(WirePullRequest::into_summary)
            .collect();
        let next_cursor = self
            .page_info
            .filter(|info| info.has_next_page)
            .and_then(|info| info.end_cursor);
        Ok(Page {
            items,
            total_count: self.issue_count,
            next_cursor,
        })
    }
}

/// Decodes search nodes, skipping `null` and the empty objects search returns
/// for results of another type. A non-empty node that does not decode is an
/// error, so a changed wire shape never turns into silently missing rows.
fn decode_nodes<T: DeserializeOwned>(nodes: Vec<Value>) -> Result<Vec<T>> {
    nodes
        .into_iter()
        .filter(|node| node.as_object().is_some_and(|fields| !fields.is_empty()))
        .map(|node| serde_json::from_value(node).map_err(|error| Error::Decode(error.to_string())))
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePullRequest {
    id: String,
    number: u64,
    title: String,
    url: String,
    state: WirePrState,
    is_draft: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    additions: u32,
    deletions: u32,
    head_ref_name: String,
    base_ref_name: String,
    is_cross_repository: bool,
    repository: WireRepoRef,
    author: Option<WireActor>,
    comments: WireCount,
    review_decision: Option<String>,
    mergeable: Option<String>,
    commits: WireCommits,
    labels: WireList<WireRepositoryLabel>,
    assignees: WireList<WireActor>,
    review_requests: WireList<WireReviewRequest>,
    latest_reviews: WireList<WireReview>,
    /// Null outside a stack.
    #[serde(default)]
    stack: Option<WireRowStack>,
}

#[derive(Deserialize)]
struct WireRowStack {
    number: u64,
    entries: WireList<WireRowStackEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRowStackEntry {
    position: u32,
    /// Null when the viewer cannot see the pull request.
    pull_request: Option<WireRowStackPull>,
}

#[derive(Deserialize)]
struct WireRowStackPull {
    number: u64,
    title: String,
    state: WirePrState,
}

impl WireRowStack {
    fn into_summary(self) -> StackSummary {
        let mut open: Vec<StackMember> = self
            .entries
            .into_items()
            .filter_map(|entry| {
                let pr = entry.pull_request?;
                matches!(pr.state, WirePrState::Open).then_some(StackMember {
                    position: entry.position,
                    number: pr.number,
                    title: pr.title,
                })
            })
            .collect();
        open.sort_by_key(|member| member.position);
        StackSummary {
            number: self.number,
            open,
        }
    }
}

/// A connection read only for its first page of nodes.
#[derive(Deserialize)]
struct WireList<T> {
    nodes: Option<Vec<Option<T>>>,
}

impl<T> WireList<T> {
    fn into_items(self) -> impl Iterator<Item = T> {
        self.nodes.into_iter().flatten().flatten()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireReviewRequest {
    /// `None` for a deleted reviewer; a team or bot decodes to `None` too,
    /// since only the user fragment selects fields.
    requested_reviewer: Option<Value>,
}

#[derive(Deserialize)]
struct WireReview {
    author: Option<WireActor>,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, …
    #[serde(default)]
    state: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum WirePrState {
    Open,
    Closed,
    Merged,
}

#[derive(Deserialize)]
struct WireRepoRef {
    owner: WireLogin,
    name: String,
}

#[derive(Deserialize)]
struct WireLogin {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireActor {
    login: String,
    avatar_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCount {
    total_count: u32,
}

#[derive(Deserialize)]
struct WireCommits {
    nodes: Option<Vec<Option<WireCommitNode>>>,
}

#[derive(Deserialize)]
struct WireCommitNode {
    commit: WireCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCommit {
    status_check_rollup: Option<WireRollup>,
}

#[derive(Deserialize)]
struct WireRollup {
    state: String,
}

impl WirePullRequest {
    fn into_summary(self) -> PrSummary {
        let checks = self
            .commits
            .nodes
            .into_iter()
            .flatten()
            .flatten()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .and_then(|rollup| checks_state(&rollup.state));
        let actor = |wire: WireActor| Actor {
            login: wire.login,
            avatar_url: wire.avatar_url,
        };
        let mut reviewers: Vec<Actor> = Vec::new();
        let requested = self
            .review_requests
            .into_items()
            .filter_map(|request| request.requested_reviewer)
            .filter_map(|reviewer| serde_json::from_value::<WireActor>(reviewer).ok());
        let latest: Vec<WireReview> = self.latest_reviews.into_items().collect();
        // GitHub sends no decision where the branch requires no review; the
        // reviewers' latest reviews still say whether it was approved.
        let decision = self
            .review_decision
            .as_deref()
            .and_then(review_decision)
            .or_else(|| decision_from_reviews(&latest));
        let reviewed = latest.into_iter().filter_map(|review| review.author);
        for reviewer in requested.chain(reviewed).map(actor) {
            if reviewers.iter().all(|known| known.login != reviewer.login) {
                reviewers.push(reviewer);
            }
        }
        PrSummary {
            id: self.id,
            pr: PrRef::new(
                RepoRef::new(self.repository.owner.login, self.repository.name),
                self.number,
            ),
            title: self.title,
            url: self.url,
            state: match self.state {
                WirePrState::Open => PrState::Open,
                WirePrState::Closed => PrState::Closed,
                WirePrState::Merged => PrState::Merged,
            },
            is_draft: self.is_draft,
            author: self.author.map(|actor| Actor {
                login: actor.login,
                avatar_url: actor.avatar_url,
            }),
            created_at: self.created_at,
            updated_at: self.updated_at,
            comment_count: self.comments.total_count,
            additions: self.additions,
            deletions: self.deletions,
            checks,
            review_decision: decision,
            head_ref: self.head_ref_name,
            base_ref: self.base_ref_name,
            cross_repository: self.is_cross_repository,
            stack: self.stack.map(WireRowStack::into_summary),
            labels: self
                .labels
                .into_items()
                .map(|label| Label {
                    id: label.id,
                    name: label.name,
                    color: label.color,
                })
                .collect(),
            assignees: self.assignees.into_items().map(actor).collect(),
            reviewers,
            mergeable: match self.mergeable.as_deref() {
                Some("MERGEABLE") => Mergeable::Mergeable,
                Some("CONFLICTING") => Mergeable::Conflicting,
                _ => Mergeable::Unknown,
            },
        }
    }
}

fn checks_state(rollup: &str) -> Option<ChecksState> {
    match rollup {
        "SUCCESS" => Some(ChecksState::Passing),
        "FAILURE" | "ERROR" => Some(ChecksState::Failing),
        "PENDING" | "EXPECTED" => Some(ChecksState::Pending),
        _ => None,
    }
}

/// The decision the reviewers' latest reviews add up to when GitHub gives
/// none: changes requested by anyone, else approved by anyone, else none.
fn decision_from_reviews(latest: &[WireReview]) -> Option<ReviewDecision> {
    let any = |wanted: &str| {
        latest
            .iter()
            .any(|review| review.state.as_deref() == Some(wanted))
    };
    if any("CHANGES_REQUESTED") {
        Some(ReviewDecision::ChangesRequested)
    } else if any("APPROVED") {
        Some(ReviewDecision::Approved)
    } else {
        None
    }
}

fn review_decision(decision: &str) -> Option<ReviewDecision> {
    match decision {
        "APPROVED" => Some(ReviewDecision::Approved),
        "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
        "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequired),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRepository {
    name: String,
    owner: WireLogin,
    description: Option<String>,
    is_private: bool,
    pushed_at: Option<DateTime<Utc>>,
}

fn repositories_from_nodes(nodes: Vec<Value>) -> Result<Vec<RepoSummary>> {
    Ok(decode_nodes::<WireRepository>(nodes)?
        .into_iter()
        .map(|repository| RepoSummary {
            repo: RepoRef::new(repository.owner.login, repository.name),
            description: repository
                .description
                .filter(|description| !description.trim().is_empty()),
            is_private: repository.is_private,
            pushed_at: repository.pushed_at,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::{Value, json};

    use super::{DOCUMENTS, repositories_from_nodes};
    use super::{InboxData, SearchData, ViewerRepositoriesData, repo_search_query, with_pr_type};
    use crate::{
        Actor, ChecksState, Error, Mergeable, PrListFilter, PrRef, PrState, RepoRef, ReviewDecision,
    };

    fn pr_node(number: u64) -> Value {
        json!({
            "id": format!("PR_{number}"),
            "number": number,
            "title": "Add diff view",
            "url": format!("https://github.com/acme/web/pull/{number}"),
            "state": "OPEN",
            "isDraft": true,
            "createdAt": "2026-11-01T10:00:00Z",
            "updatedAt": "2026-11-02T11:30:00Z",
            "additions": 12,
            "deletions": 3,
            "headRefName": "feat/diff",
            "baseRefName": "main",
            "isCrossRepository": false,
            "repository": { "owner": { "login": "acme" }, "name": "web" },
            "author": { "login": "ana", "avatarUrl": "https://avatars.example/ana" },
            "comments": { "totalCount": 4 },
            "reviewDecision": "CHANGES_REQUESTED",
            "mergeable": "CONFLICTING",
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "FAILURE" } } }] },
            "labels": { "nodes": [{ "id": "LA_1", "name": "bug", "color": "d73a4a" }] },
            "assignees": { "nodes": [{ "login": "bo", "avatarUrl": "https://avatars.example/bo" }] },
            "reviewRequests": { "nodes": [
                { "requestedReviewer": { "login": "cy", "avatarUrl": "https://avatars.example/cy" } },
                { "requestedReviewer": {} },
                { "requestedReviewer": null }
            ] },
            "latestReviews": { "nodes": [
                { "author": { "login": "di", "avatarUrl": "https://avatars.example/di" } },
                { "author": { "login": "cy", "avatarUrl": "https://avatars.example/cy" } },
                { "author": null }
            ] }
        })
    }

    fn connection(nodes: Vec<Value>, has_next_page: bool) -> Value {
        json!({
            "issueCount": 42,
            "pageInfo": {
                "hasNextPage": has_next_page,
                "endCursor": "Y3Vyc29yOjI1"
            },
            "nodes": nodes
        })
    }

    fn page_from(search: Value) -> crate::Result<crate::Page<crate::PrSummary>> {
        let data: SearchData = serde_json::from_value(json!({ "search": search })).unwrap();
        data.search.into_page()
    }

    #[test]
    fn a_row_reads_its_base_its_fork_and_the_open_layers_of_its_stack() {
        let mut stacked = pr_node(31);
        stacked["baseRefName"] = json!("layer-a");
        stacked["stack"] = json!({
            "number": 4,
            "entries": { "nodes": [
                { "position": 3, "pullRequest": { "number": 32, "title": "Layer C", "state": "OPEN" } },
                { "position": 1, "pullRequest": { "number": 29, "title": "Layer A", "state": "MERGED" } },
                { "position": 2, "pullRequest": { "number": 31, "title": "Layer B", "state": "OPEN" } },
                { "position": 4, "pullRequest": null },
                null
            ] }
        });
        let mut fork = pr_node(40);
        fork["isCrossRepository"] = json!(true);
        fork["stack"] = Value::Null;

        let page = page_from(connection(vec![stacked, fork, pr_node(41)], false)).unwrap();

        let stacked = &page.items[0];
        assert_eq!(
            (stacked.base_ref.as_str(), stacked.cross_repository),
            ("layer-a", false)
        );
        let stack = stacked.stack.as_ref().expect("a stack");
        assert_eq!(stack.number, 4);
        let open: Vec<(u32, u64, &str)> = stack
            .open
            .iter()
            .map(|member| (member.position, member.number, member.title.as_str()))
            .collect();
        assert_eq!(open, vec![(2, 31, "Layer B"), (3, 32, "Layer C")]);
        assert!(page.items[1].cross_repository);
        assert_eq!(page.items[1].stack, None);
        assert_eq!(
            page.items[2].stack, None,
            "a node without the field is outside a stack"
        );
    }

    #[test]
    fn parses_a_search_page_skipping_non_pull_request_nodes() {
        let mut no_author = pr_node(8);
        no_author["author"] = Value::Null;
        no_author["reviewDecision"] = Value::Null;
        no_author["state"] = json!("MERGED");
        no_author["commits"] = json!({ "nodes": [{ "commit": { "statusCheckRollup": null } }] });

        let page = page_from(connection(
            vec![pr_node(7), json!({}), Value::Null, no_author],
            true,
        ))
        .unwrap();

        assert_eq!(page.total_count, 42);
        assert_eq!(page.next_cursor.as_deref(), Some("Y3Vyc29yOjI1"));
        assert_eq!(page.items.len(), 2);

        let first = &page.items[0];
        assert_eq!(first.id, "PR_7");
        assert_eq!(first.pr, PrRef::new(RepoRef::new("acme", "web"), 7));
        assert_eq!(first.state, PrState::Open);
        assert!(first.is_draft);
        assert_eq!(
            first.author,
            Some(Actor {
                login: "ana".to_owned(),
                avatar_url: "https://avatars.example/ana".to_owned(),
            })
        );
        assert_eq!(
            first.created_at,
            chrono::Utc.with_ymd_and_hms(2026, 11, 1, 10, 0, 0).unwrap()
        );
        assert_eq!(first.comment_count, 4);
        assert_eq!((first.additions, first.deletions), (12, 3));
        assert_eq!(first.checks, Some(ChecksState::Failing));
        assert_eq!(
            first.review_decision,
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(first.head_ref, "feat/diff");
        assert_eq!(first.mergeable, Mergeable::Conflicting);
        let logins = |actors: &[Actor]| -> Vec<String> {
            actors.iter().map(|actor| actor.login.clone()).collect()
        };
        assert_eq!(
            first
                .labels
                .iter()
                .map(|label| label.name.as_str())
                .collect::<Vec<_>>(),
            ["bug"]
        );
        assert_eq!(logins(&first.assignees), ["bo"]);
        assert_eq!(
            logins(&first.reviewers),
            ["cy", "di"],
            "requested first, reviewed next, teams and repeats left out"
        );

        let second = &page.items[1];
        assert_eq!(second.state, PrState::Merged);
        assert_eq!(second.author, None);
        assert_eq!(second.checks, None);
        assert_eq!(second.review_decision, None);
    }

    #[test]
    fn next_cursor_is_only_set_while_more_pages_exist() {
        let page = page_from(connection(vec![pr_node(1)], false)).unwrap();
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn maps_every_rollup_state() {
        let cases = [
            ("SUCCESS", Some(ChecksState::Passing)),
            ("FAILURE", Some(ChecksState::Failing)),
            ("ERROR", Some(ChecksState::Failing)),
            ("PENDING", Some(ChecksState::Pending)),
            ("EXPECTED", Some(ChecksState::Pending)),
            ("SOMETHING_NEW", None),
        ];
        for (rollup, expected) in cases {
            let mut node = pr_node(1);
            node["commits"] =
                json!({ "nodes": [{ "commit": { "statusCheckRollup": { "state": rollup } } }] });
            let page = page_from(connection(vec![node], false)).unwrap();
            assert_eq!(page.items[0].checks, expected, "rollup {rollup}");
        }
    }

    #[test]
    fn maps_every_review_decision_and_tolerates_unknown_ones() {
        let cases = [
            ("APPROVED", Some(ReviewDecision::Approved)),
            ("CHANGES_REQUESTED", Some(ReviewDecision::ChangesRequested)),
            ("REVIEW_REQUIRED", Some(ReviewDecision::ReviewRequired)),
            ("SOMETHING_NEW", None),
        ];
        for (decision, expected) in cases {
            let mut node = pr_node(1);
            node["reviewDecision"] = json!(decision);
            let page = page_from(connection(vec![node], false)).unwrap();
            assert_eq!(
                page.items[0].review_decision, expected,
                "decision {decision}"
            );
        }
    }

    #[test]
    fn without_a_decision_the_latest_reviews_decide() {
        let review =
            |state: &str| json!({ "state": state, "author": { "login": "x", "avatarUrl": "" } });
        let cases = [
            (vec![review("COMMENTED")], None),
            (
                vec![review("APPROVED"), review("COMMENTED")],
                Some(ReviewDecision::Approved),
            ),
            (
                vec![review("APPROVED"), review("CHANGES_REQUESTED")],
                Some(ReviewDecision::ChangesRequested),
            ),
            (Vec::new(), None),
        ];
        for (reviews, expected) in cases {
            let mut node = pr_node(1);
            node["reviewDecision"] = Value::Null;
            node["latestReviews"] = json!({ "nodes": reviews });
            let page = page_from(connection(vec![node], false)).unwrap();
            assert_eq!(page.items[0].review_decision, expected);
        }
        // GitHub's own decision wins when it has one.
        let mut node = pr_node(1);
        node["reviewDecision"] = json!("REVIEW_REQUIRED");
        node["latestReviews"] = json!({ "nodes": [review("APPROVED")] });
        let page = page_from(connection(vec![node], false)).unwrap();
        assert_eq!(
            page.items[0].review_decision,
            Some(ReviewDecision::ReviewRequired)
        );
    }

    #[test]
    fn missing_commit_nodes_mean_no_checks() {
        let mut node = pr_node(1);
        node["commits"] = json!({ "nodes": [] });
        let page = page_from(connection(vec![node], false)).unwrap();
        assert_eq!(page.items[0].checks, None);
    }

    #[test]
    fn an_unknown_pull_request_state_is_a_decode_error() {
        let mut node = pr_node(1);
        node["state"] = json!("ARCHIVED");
        let error = page_from(connection(vec![node], false)).unwrap_err();
        assert!(matches!(error, Error::Decode(_)), "{error:?}");
    }

    #[test]
    fn parses_the_three_inbox_searches() {
        let data: InboxData = serde_json::from_value(json!({
            "reviewRequested": connection(vec![pr_node(1)], false),
            "authored": connection(vec![pr_node(2), pr_node(3)], true),
            "involved": connection(vec![], false),
        }))
        .unwrap();
        let inbox = data.into_inbox().unwrap();
        assert_eq!(inbox.review_requested.items.len(), 1);
        assert_eq!(inbox.authored.items.len(), 2);
        assert!(inbox.authored.next_cursor.is_some());
        assert!(inbox.involved.items.is_empty());
    }

    #[test]
    fn adds_the_pull_request_type_only_when_missing() {
        assert_eq!(
            with_pr_type("repo:a/b is:merged"),
            "repo:a/b is:merged is:pr"
        );
        assert_eq!(with_pr_type("repo:a/b is:pr"), "repo:a/b is:pr");
        assert_eq!(with_pr_type("type:pr login"), "type:pr login");
        assert_eq!(with_pr_type("is:pro"), "is:pro is:pr");
        assert_eq!(with_pr_type("  "), "is:pr");
    }

    #[test]
    fn builds_repository_queries_per_filter() {
        let repo = RepoRef::new("zed-industries", "zed");
        let query = |filter, text| repo_search_query(&repo, filter, text);
        assert_eq!(
            query(PrListFilter::Open, None),
            "repo:zed-industries/zed is:pr is:open sort:updated-desc"
        );
        assert_eq!(
            query(PrListFilter::Closed, None),
            "repo:zed-industries/zed is:pr is:closed is:unmerged sort:updated-desc"
        );
        assert_eq!(
            query(PrListFilter::Merged, Some("  ")),
            "repo:zed-industries/zed is:pr is:merged sort:updated-desc"
        );
        assert_eq!(
            query(PrListFilter::All, Some(" diff ")),
            "repo:zed-industries/zed is:pr diff sort:updated-desc"
        );
    }

    #[test]
    fn parses_repositories_from_a_viewer_listing_and_a_search() {
        let repo = |owner: &str, name: &str, description: Value, pushed_at: Value| {
            json!({
                "name": name,
                "owner": { "login": owner },
                "description": description,
                "isPrivate": true,
                "pushedAt": pushed_at
            })
        };
        let viewer: ViewerRepositoriesData = serde_json::from_value(json!({
            "viewer": { "repositories": { "nodes": [
                repo("acme", "web", json!("The site"), json!("2026-11-02T11:30:00Z")),
                repo("acme", "empty", json!(""), Value::Null),
            ] } }
        }))
        .unwrap();
        let repos =
            repositories_from_nodes(viewer.viewer.repositories.nodes.unwrap_or_default()).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].repo, RepoRef::new("acme", "web"));
        assert_eq!(repos[0].description.as_deref(), Some("The site"));
        assert!(repos[0].is_private);
        assert!(repos[0].pushed_at.is_some());
        assert_eq!(repos[1].description, None);
        assert_eq!(repos[1].pushed_at, None);

        let search: SearchData = serde_json::from_value(json!({
            "search": { "nodes": [json!({}), repo("acme", "web", Value::Null, Value::Null)] }
        }))
        .unwrap();
        let found = repositories_from_nodes(search.search.nodes.unwrap_or_default()).unwrap();
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn registers_every_document_once() {
        let names: Vec<_> = DOCUMENTS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "inbox",
                "search_pull_requests",
                "viewer_repositories",
                "search_repositories",
                "repository_labels"
            ]
        );
    }
}
