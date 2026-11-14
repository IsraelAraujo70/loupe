//! Command palette search over pull requests, and the repositories its
//! `repo:` and `org:` filters match against. The shell builds the search
//! query; this module runs it and shapes the rows.

use gpui_kit::{App, SharedString, Task};
use loupe_github::{PrState, PrSummary, RepoRef};

use super::common::client;
use crate::route::Route;
use crate::services::background;
use crate::shell::PaletteItem;

fn subtitle(summary: &PrSummary) -> String {
    let author = summary
        .author
        .as_ref()
        .map_or("ghost", |author| author.login.as_str());
    let state = match (summary.state, summary.is_draft) {
        (PrState::Open, true) => "draft",
        (PrState::Open, false) => "open",
        (PrState::Closed, _) => "closed",
        (PrState::Merged, _) => "merged",
    };
    format!("{} · {author} · {state}", summary.pr)
}

fn palette_item(summary: &PrSummary) -> PaletteItem {
    PaletteItem {
        title: SharedString::from(summary.title.clone()),
        subtitle: SharedString::from(subtitle(summary)),
        route: Route::PullRequest(summary.pr.clone()),
        comments: summary.comment_count,
    }
}

/// The first `first` pull requests GitHub finds for `query`, in GitHub search
/// syntax. Empty for a blank query, without a GitHub client, or when the
/// search fails.
pub fn search(query: &str, first: u32, cx: &mut App) -> Task<Vec<PaletteItem>> {
    let query = query.trim().to_owned();
    if query.is_empty() {
        return Task::ready(Vec::new());
    }
    let Some(github) = client(cx) else {
        return Task::ready(Vec::new());
    };
    background(cx, move || {
        match github.search_pull_requests(&query, first, None) {
            Ok(page) => page.items.iter().map(palette_item).collect(),
            Err(error) => {
                eprintln!("github palette search failed: {error}");
                Vec::new()
            }
        }
    })
}

/// The repositories the viewer owns, collaborates on, or reaches through an
/// organization, most recently pushed first. Empty without a client or when
/// the request fails.
pub fn repositories(cx: &mut App) -> Task<Vec<RepoRef>> {
    let Some(github) = client(cx) else {
        return Task::ready(Vec::new());
    };
    background(cx, move || match github.viewer_repositories() {
        Ok(repos) => repos.into_iter().map(|summary| summary.repo).collect(),
        Err(error) => {
            eprintln!("github palette repositories failed: {error}");
            Vec::new()
        }
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_github::{Actor, PrRef, PrState, PrSummary, RepoRef};

    use super::palette_item;

    #[test]
    fn row_names_repo_author_state_and_comments() {
        let at = DateTime::from_timestamp(0, 0).expect("epoch");
        let summary = PrSummary {
            id: "PR_1".into(),
            pr: PrRef::new(RepoRef::new("acme", "web"), 12),
            title: "Fix".into(),
            url: String::new(),
            state: PrState::Open,
            is_draft: true,
            author: Some(Actor {
                login: "ana".into(),
                avatar_url: String::new(),
            }),
            created_at: at,
            updated_at: at,
            comment_count: 4,
            additions: 0,
            deletions: 0,
            checks: None,
            review_decision: None,
            head_ref: "fix".into(),
            base_ref: String::new(),
            cross_repository: false,
            stack: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: loupe_github::Mergeable::Unknown,
        };
        let item = palette_item(&summary);
        assert_eq!(item.title.as_ref(), "Fix");
        assert_eq!(item.subtitle.as_ref(), "acme/web#12 · ana · draft");
        assert_eq!(item.comments, 4);
    }
}
