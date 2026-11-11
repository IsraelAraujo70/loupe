//! Command palette search over Linear issues.

use gpui_kit::{App, Task};
use loupe_linear::IssueSummary;

use crate::linear::data;
use crate::route::Route;
use crate::shell::PaletteItem;

const MIN_QUERY_CHARS: usize = 2;
const MAX_RESULTS: usize = 10;

/// Issues matching `query`, best match first. With `team_key` (`ENG`), only
/// that team's issues, and its latest ones for a blank query. Without a team,
/// empty for a query shorter than two characters. Empty when Linear is not
/// connected or the search fails.
pub fn search(query: &str, team_key: Option<&str>, cx: &mut App) -> Task<Vec<PaletteItem>> {
    let term = query.trim().to_owned();
    if team_key.is_none() && term.chars().count() < MIN_QUERY_CHARS {
        return Task::ready(Vec::new());
    }
    let Some(client) = data::client(cx) else {
        return Task::ready(Vec::new());
    };
    let team_key = team_key.map(str::to_owned);
    crate::services::background(cx, move || {
        let found = match &team_key {
            Some(key) => client.team_key_issues(key, &term, MAX_RESULTS),
            None => client.search_issues(&term, MAX_RESULTS),
        };
        match found {
            Ok(issues) => palette_items(&issues),
            Err(error) => {
                eprintln!("linear palette search failed: {}", data::error_text(&error));
                Vec::new()
            }
        }
    })
}

fn palette_items(issues: &[IssueSummary]) -> Vec<PaletteItem> {
    issues.iter().map(palette_item).collect()
}

fn palette_item(issue: &IssueSummary) -> PaletteItem {
    let mut subtitle = format!("{} · {}", issue.state.name, issue.team.name);
    if let Some(assignee) = &issue.assignee {
        subtitle.push_str(" · ");
        subtitle.push_str(&assignee.display_name);
    }
    PaletteItem {
        title: format!("{} {}", issue.identifier, issue.title).into(),
        subtitle: subtitle.into(),
        route: Route::LinearIssue {
            identifier: issue.identifier.clone(),
        },
        comments: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::palette_items;
    use crate::route::Route;
    use chrono::DateTime;
    use loupe_linear::{IssueSummary, Priority, StateRef, StateType, TeamRef, User};

    fn issue(identifier: &str, title: &str, assignee: Option<&str>) -> IssueSummary {
        let at = DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .to_utc();
        IssueSummary {
            id: format!("id-{identifier}"),
            identifier: identifier.into(),
            title: title.into(),
            priority: Priority::None,
            sort_order: 0.,
            state: StateRef {
                id: "s1".into(),
                name: "In Progress".into(),
                kind: StateType::Started,
                color: "#f2c94c".into(),
            },
            team: TeamRef {
                id: "t1".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: assignee.map(|name| User {
                id: format!("u-{name}"),
                name: format!("{name} Full"),
                display_name: name.into(),
                avatar_url: None,
            }),
            labels: Vec::new(),
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: format!("https://linear.app/acme/issue/{identifier}"),
            branch_name: String::new(),
            created_at: at,
            updated_at: at,
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests: Vec::new(),
            pull_requests_complete: true,
        }
    }

    #[test]
    fn maps_unassigned_issue() {
        let items = palette_items(&[issue("ENG-12", "Fix the login redirect", None)]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title.to_string(), "ENG-12 Fix the login redirect");
        assert_eq!(items[0].subtitle.to_string(), "In Progress · Engineering");
        assert!(matches!(
            &items[0].route,
            Route::LinearIssue { identifier } if identifier == "ENG-12"
        ));
    }

    #[test]
    fn subtitle_names_the_assignee() {
        let items = palette_items(&[issue("ENG-7", "Ship it", Some("israel"))]);
        assert_eq!(
            items[0].subtitle.to_string(),
            "In Progress · Engineering · israel"
        );
    }

    #[test]
    fn keeps_result_order() {
        let items = palette_items(&[issue("ENG-2", "b", None), issue("ENG-1", "a", None)]);
        let titles: Vec<String> = items.iter().map(|item| item.title.to_string()).collect();
        assert_eq!(titles, ["ENG-2 b", "ENG-1 a"]);
    }
}
