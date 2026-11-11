use loupe_github::{PrRef, RepoRef};
use serde::{Deserialize, Serialize};

/// Every screen the content area can show. Each tab keeps a history of these;
/// `shell::router` maps each variant to the view its front owns. Open tabs are
/// saved in `Settings`, so a route must stay deserializable across versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Route {
    /// GitHub dashboard, shown as "Reviews": PRs waiting on my review, my
    /// open PRs, involved PRs.
    Inbox,
    /// GitHub notifications grouped by repository, shown as "Inbox" as in
    /// Linear.
    Notifications,
    /// Pull requests of one repository.
    Repo(RepoRef),
    /// One pull request: conversation, files, commits, checks.
    PullRequest(PrRef),
    /// GitHub Actions runs of one repository, of one branch when given (a
    /// pull request's).
    Actions {
        repo: RepoRef,
        #[serde(default)]
        branch: Option<String>,
    },
    /// One workflow run: its graph, jobs, and logs, with job `job_id` open
    /// when given. `title` names the tab, such as `Release #57`.
    WorkflowRun {
        repo: RepoRef,
        run_id: u64,
        #[serde(default)]
        job_id: Option<u64>,
        #[serde(default)]
        title: String,
    },
    /// Linear issues assigned to me.
    LinearMyIssues,
    /// Issues of one Linear team, as a board or a list, on saved view
    /// `view_id` when given.
    LinearTeam {
        team_id: String,
        name: String,
        #[serde(default)]
        view_id: Option<String>,
    },
    /// Linear projects: the workspace's, or one team's.
    LinearProjects { team_id: Option<String> },
    /// One Linear project's page: its overview and its issues. `name` labels
    /// the tab until the project loads.
    LinearProject { id: String, name: String },
    /// Saved views: every team's, or one team's.
    LinearViews { team_id: Option<String> },
    /// One Linear issue, by identifier such as `ENG-123`.
    LinearIssue { identifier: String },
    /// Accounts and preferences.
    Settings,
}

impl Route {
    /// Whether the page shows one Linear workspace's own things (a team, a
    /// project, an issue), which another workspace does not have.
    pub fn is_in_linear_workspace(&self) -> bool {
        matches!(
            self,
            Route::LinearTeam { .. }
                | Route::LinearProjects { team_id: Some(_) }
                | Route::LinearProject { .. }
                | Route::LinearViews { team_id: Some(_) }
                | Route::LinearIssue { .. }
        )
    }
}

/// A Linear issue identifier such as `ENG-278`, in any case.
fn issue(identifier: &str) -> Option<Route> {
    let identifier = identifier.to_ascii_uppercase();
    let (team, number) = identifier.split_once('-')?;
    let valid = !team.is_empty()
        && team.chars().all(|c| c.is_ascii_alphanumeric())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit());
    valid.then_some(Route::LinearIssue { identifier })
}

impl Route {
    /// The page Loupe shows for a web link, or `None` when the link belongs
    /// in the browser: a GitHub pull request
    /// (`https://github.com/<owner>/<repo>/pull/<n>`), or an issue of the
    /// connected Linear workspace (`https://linear.app/<url_key>/issue/<id>`).
    /// An issue of another workspace stays in the browser, since Loupe could
    /// not load it.
    pub fn for_link(url: &str, linear_workspace: Option<&str>) -> Option<Route> {
        let url = url.trim();
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let rest = rest.strip_prefix("www.").unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        let path = path.split(['?', '#']).next()?;
        match host.to_ascii_lowercase().as_str() {
            "github.com" => {
                PrRef::parse(&format!("https://github.com/{path}")).map(Route::PullRequest)
            }
            "linear.app" => {
                let mut parts = path.split('/');
                let workspace = parts.next()?;
                if parts.next()? != "issue"
                    || !linear_workspace.is_some_and(|key| key.eq_ignore_ascii_case(workspace))
                {
                    return None;
                }
                issue(parts.next()?)
            }
            _ => None,
        }
    }

    /// The page for a URL the system hands to Loupe: `loupe://pr/<owner>/<repo>/<n>`,
    /// `loupe://issue/<id>`, or a web link that [`Route::for_link`] opens in
    /// Loupe (what Finicky sends).
    pub fn for_system_url(url: &str, linear_workspace: Option<&str>) -> Option<Route> {
        let url = url.trim();
        let Some(rest) = url.strip_prefix("loupe://") else {
            return Self::for_link(url, linear_workspace);
        };
        let rest = rest.split(['?', '#']).next()?.trim_end_matches('/');
        let parts: Vec<&str> = rest.split('/').collect();
        match parts.as_slice() {
            ["pr", owner, repo, number] if !owner.is_empty() && !repo.is_empty() => {
                Some(Route::PullRequest(PrRef::new(
                    RepoRef::new(*owner, *repo),
                    number.parse().ok()?,
                )))
            }
            ["issue", identifier] => issue(identifier),
            _ => None,
        }
    }

    /// What a tab showing this route is called.
    pub fn label(&self) -> String {
        match self {
            Route::Inbox => "Reviews".into(),
            Route::Notifications => "Inbox".into(),
            Route::Repo(repo) => repo.to_string(),
            Route::PullRequest(pr) => format!("{}#{}", pr.repo.name, pr.number),
            Route::Actions { repo, .. } => format!("{} actions", repo.name),
            Route::WorkflowRun { title, .. } if !title.is_empty() => title.clone(),
            Route::WorkflowRun { repo, .. } => format!("{} run", repo.name),
            Route::LinearMyIssues => "My issues".into(),
            Route::LinearTeam { name, .. } => name.clone(),
            Route::LinearProjects { .. } => "Projects".into(),
            Route::LinearProject { name, .. } => name.clone(),
            Route::LinearViews { .. } => "Views".into(),
            Route::LinearIssue { identifier } => identifier.clone(),
            Route::Settings => "Settings".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Route;
    use loupe_github::{PrRef, RepoRef};

    #[test]
    fn routes_survive_a_round_trip_through_settings() {
        let routes = vec![
            Route::Inbox,
            Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 7)),
            Route::LinearTeam {
                team_id: "t1".into(),
                name: "Back-end".into(),
                view_id: Some("v1".into()),
            },
            Route::LinearProjects { team_id: None },
            Route::Actions {
                repo: RepoRef::new("acme", "web"),
                branch: Some("feat/x".into()),
            },
            Route::WorkflowRun {
                repo: RepoRef::new("acme", "web"),
                run_id: 43012008138,
                job_id: Some(7),
                title: "Release #57".into(),
            },
            Route::LinearProject {
                id: "p1".into(),
                name: "Integrations".into(),
            },
            Route::LinearIssue {
                identifier: "ENG-1".into(),
            },
        ];
        let json = serde_json::to_string(&routes).expect("serialize");
        let back: Vec<Route> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, routes);
        // Tabs saved before `view_id` existed still load.
        let old = r#"{"LinearTeam":{"team_id":"t1","name":"Back-end"}}"#;
        let route: Route = serde_json::from_str(old).expect("an older saved tab");
        assert!(matches!(route, Route::LinearTeam { view_id: None, .. }));
    }

    #[test]
    fn pull_request_and_issue_links_open_in_loupe() {
        let pr = Some(Route::PullRequest(PrRef::new(
            RepoRef::new("hooli-inc", "hooli-api"),
            306,
        )));
        for url in [
            "https://github.com/hooli-inc/hooli-api/pull/306",
            "https://github.com/hooli-inc/hooli-api/pull/306/files?w=1#diff-1",
            "http://www.github.com/hooli-inc/hooli-api/pull/306",
        ] {
            assert_eq!(Route::for_link(url, None), pr, "{url}");
        }
        let issue = Some(Route::LinearIssue {
            identifier: "ENG-278".into(),
        });
        for url in [
            "https://linear.app/hooli/issue/ENG-278",
            "https://linear.app/hooli/issue/ENG-278/integrate-vendor-b",
            "https://linear.app/Hooli/issue/eng-278#comment-1",
        ] {
            assert_eq!(Route::for_link(url, Some("hooli")), issue, "{url}");
        }
    }

    #[test]
    fn other_links_stay_in_the_browser() {
        for url in [
            "https://github.com/acme/web/issues/42",
            "https://github.com/acme/web",
            "https://github.com/acme/web/pull/abc",
            "https://gist.github.com/acme/web/pull/1",
            "https://example.com/acme/web/pull/1",
            "acme/web#4",
            "https://linear.app/hooli/project/loupe-1",
            "https://linear.app/hooli/issue/BACK",
            "https://linear.app/hooli/issue/ENG-12x",
            "mailto:someone@example.com",
        ] {
            assert_eq!(Route::for_link(url, Some("hooli")), None, "{url}");
        }
        // An issue of another workspace, or with Linear signed out.
        let url = "https://linear.app/globex/issue/OPS-86";
        assert_eq!(Route::for_link(url, Some("hooli")), None);
        assert_eq!(Route::for_link(url, None), None);
    }

    #[test]
    fn system_urls_name_a_page() {
        let pr = Some(Route::PullRequest(PrRef::new(
            RepoRef::new("acme", "web"),
            7,
        )));
        let issue = Some(Route::LinearIssue {
            identifier: "ENG-1".into(),
        });
        assert_eq!(Route::for_system_url("loupe://pr/acme/web/7", None), pr);
        assert_eq!(Route::for_system_url("loupe://pr/acme/web/7/", None), pr);
        assert_eq!(Route::for_system_url("loupe://issue/eng-1", None), issue);
        // Finicky hands over the web link itself.
        assert_eq!(
            Route::for_system_url("https://github.com/acme/web/pull/7", None),
            pr
        );
        assert_eq!(
            Route::for_system_url("https://linear.app/acme/issue/ENG-1/title", Some("acme")),
            issue
        );
        for url in [
            "loupe://pr/acme/web",
            "loupe://pr/acme/web/seven",
            "loupe://pr/acme/web/7/files",
            "loupe://pr//web/7",
            "loupe://issue/ENG",
            "loupe://settings",
            "https://example.com/acme/web/pull/7",
        ] {
            assert_eq!(Route::for_system_url(url, Some("acme")), None, "{url}");
        }
    }

    #[test]
    fn labels_name_the_page() {
        let pr = Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 7));
        assert_eq!(pr.label(), "web#7");
        assert_eq!(
            Route::LinearIssue {
                identifier: "ENG-1".into()
            }
            .label(),
            "ENG-1"
        );
    }
}
