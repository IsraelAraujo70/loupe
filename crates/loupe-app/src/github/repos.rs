//! The repositories the sidebar lists and the Actions page covers: the
//! pinned ones, the watched ones, then those of the open pull requests that
//! involve you, each once.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, Global, Task};
use loupe_github::{GitHub, RepoRef};

use crate::services::Services;
use crate::store::Settings;

/// How often the repositories of your pull requests are read again.
const RELOAD_EVERY: Duration = Duration::from_secs(10 * 60);

/// Why a repository is in the list, which decides its place and what its
/// menu offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoSource {
    /// Pinned to the top; reorders among the pinned.
    Pinned,
    /// Watched from its page; reorders among the watched.
    Watched,
    /// One of your open pull requests is there; sorted by name.
    PullRequests,
}

impl RepoSource {
    /// Whether the user put the repository in the list, so they can move it
    /// and take it out.
    pub fn is_added(self) -> bool {
        !matches!(self, RepoSource::PullRequests)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedRepo {
    pub repo: RepoRef,
    pub source: RepoSource,
}

/// The list in its order: the pinned repositories and then the watched
/// ones in the order the user gave them, then those of your pull requests
/// by name. A repository shows once, in the first place it qualifies for;
/// names differing only in case are the same repository.
pub fn list(pinned: &[RepoRef], watched: &[RepoRef], from_prs: &[RepoRef]) -> Vec<ListedRepo> {
    let mut listed: Vec<ListedRepo> = Vec::new();
    let mut add = |repo: &RepoRef, source: RepoSource| {
        let key = repo.to_string();
        if !listed
            .iter()
            .any(|item| item.repo.to_string().eq_ignore_ascii_case(&key))
        {
            listed.push(ListedRepo {
                repo: repo.clone(),
                source,
            });
        }
    };
    for repo in pinned {
        add(repo, RepoSource::Pinned);
    }
    for repo in watched {
        add(repo, RepoSource::Watched);
    }
    let mut from_prs: Vec<&RepoRef> = from_prs.iter().collect();
    from_prs.sort_by_key(|repo| repo.to_string().to_lowercase());
    for repo in from_prs {
        add(repo, RepoSource::PullRequests);
    }
    listed
}

/// [`list`] for the current settings and the repositories of your pull
/// requests read so far.
pub fn listed(cx: &App) -> Vec<ListedRepo> {
    let settings = Settings::get(cx);
    let from_prs = cx
        .try_global::<PullRequestRepos>()
        .map(|global| global.repos.clone())
        .unwrap_or_default();
    list(&settings.pinned_repos, &settings.watched_repos, &from_prs)
}

/// The repositories of the open pull requests that involve you, read in
/// the background while GitHub is connected.
#[derive(Default)]
pub struct PullRequestRepos {
    repos: Arc<Vec<RepoRef>>,
    client: Option<Arc<GitHub>>,
    _load: Option<Task<()>>,
}

impl Global for PullRequestRepos {}

/// Reads the repositories of your pull requests now and every
/// [`RELOAD_EVERY`], and again whenever the GitHub account changes. A
/// failed read keeps the last list.
pub fn start_pull_request_repos(cx: &mut App) {
    if cx.has_global::<PullRequestRepos>() {
        return;
    }
    cx.set_global(PullRequestRepos::default());
    sync(cx);
    cx.observe_global::<Services>(sync).detach();
}

fn sync(cx: &mut App) {
    let github = Services::get(cx).github.clone();
    let current = &cx.global::<PullRequestRepos>().client;
    let same = match (current, &github) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    let load = github.clone().map(|client| {
        cx.spawn(async move |cx| {
            loop {
                let reader = client.clone();
                let read = cx
                    .background_executor()
                    .spawn(async move { reader.pull_request_repositories() })
                    .await;
                if let Ok(repos) = read {
                    cx.update(|cx| {
                        cx.global_mut::<PullRequestRepos>().repos = Arc::new(repos);
                    });
                }
                cx.background_executor().timer(RELOAD_EVERY).await;
            }
        })
    });
    cx.set_global(PullRequestRepos {
        repos: Arc::default(),
        client: github,
        _load: load,
    });
}

#[cfg(test)]
mod tests {
    use loupe_github::RepoRef;

    use super::{RepoSource, list};

    fn repo(slug: &str) -> RepoRef {
        RepoRef::parse(slug).expect("slug")
    }

    #[test]
    fn lists_watched_and_pull_request_repositories_once_each() {
        let listed = list(
            &[repo("acme/zeta")],
            &[repo("acme/web"), repo("acme/zeta"), repo("acme/api")],
            &[
                repo("other/tools"),
                repo("ACME/Web"),
                repo("acme/billing"),
                repo("acme/api"),
            ],
        );
        let lines: Vec<(String, RepoSource)> = listed
            .iter()
            .map(|item| (item.repo.to_string(), item.source))
            .collect();
        assert_eq!(
            lines,
            [
                ("acme/zeta".into(), RepoSource::Pinned),
                ("acme/web".into(), RepoSource::Watched),
                ("acme/api".into(), RepoSource::Watched),
                ("acme/billing".into(), RepoSource::PullRequests),
                ("other/tools".into(), RepoSource::PullRequests),
            ]
        );
    }
}
