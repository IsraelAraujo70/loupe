//! Live read smoke test against the real Linear API. Reads `LINEAR_API_KEY`
//! from the environment and calls read methods only. Prints counts and issue
//! identifiers, never the key, bodies, or people's names.
//!
//!     LINEAR_API_KEY=... cargo test -p loupe-linear --test live_read -- --ignored --nocapture

use loupe_linear::{IssueQuery, Linear};

#[test]
#[ignore = "needs LINEAR_API_KEY and calls api.linear.app"]
fn live_read_smoke() {
    let Ok(key) = std::env::var("LINEAR_API_KEY") else {
        println!("LINEAR_API_KEY is not set, skipping");
        return;
    };
    let client = Linear::new(key);

    client.viewer().expect("viewer");
    println!("viewer: ok");

    let teams = client.teams().expect("teams");
    println!(
        "teams: {}",
        teams
            .iter()
            .map(|t| t.key.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let Some(first) = teams.first() else {
        println!("no teams, stopping");
        return;
    };

    let team = client.team(&first.id).expect("team");
    println!(
        "team {}: {} states, {} members, {} labels, {} projects, {} open cycles",
        team.key,
        team.states.len(),
        team.members.len(),
        team.labels.len(),
        team.projects.len(),
        team.cycles.len()
    );

    let mine = client.my_open_issues().expect("my_open_issues");
    println!("my open issues: {}", mine.len());

    let page = client
        .team_issues(&first.id, &IssueQuery::default(), None)
        .expect("team_issues");
    println!(
        "team {} first page: {} issues, more pages: {}",
        team.key,
        page.nodes.len(),
        page.next_cursor.is_some()
    );

    if let Some(issue) = page.nodes.first() {
        let detail = client.issue(&issue.identifier).expect("issue");
        let pull_requests = detail
            .attachments
            .iter()
            .filter(|a| a.pull_request.is_some())
            .count();
        println!(
            "issue {}: {} comments, {} attachments ({} pull requests)",
            detail.issue.identifier,
            detail.comments.len(),
            detail.attachments.len(),
            pull_requests
        );

        let term = issue.title.split_whitespace().next().unwrap_or("issue");
        let found = client.search_issues(term, 10).expect("search_issues");
        println!("search: {} results", found.len());

        let by_identifier = client
            .issues_by_identifiers(&[issue.identifier.clone(), "ZZZ-999999".into()])
            .expect("issues_by_identifiers");
        assert!(
            by_identifier
                .iter()
                .all(|i| i.identifier == issue.identifier),
            "issues_by_identifiers returned an issue that was not asked for"
        );
        assert_eq!(
            by_identifier.len(),
            1,
            "the real identifier must match once"
        );
        println!(
            "by identifiers: {}",
            by_identifier
                .iter()
                .map(|i| i.identifier.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    let views = client.custom_views().expect("custom_views");
    println!("custom views: {}", views.len());

    let with_pr = mine.iter().chain(page.nodes.iter()).find_map(|issue| {
        let detail = client.issue(&issue.identifier).ok()?;
        let url = detail
            .attachments
            .into_iter()
            .find(|a| a.pull_request.is_some())?
            .url;
        Some((issue.identifier.clone(), url))
    });
    match with_pr {
        Some((identifier, url)) => {
            let linked = client.issues_for_url(&url).expect("issues_for_url");
            assert!(
                linked.iter().any(|i| i.identifier == identifier),
                "issues_for_url must return the issue the PR is attached to"
            );
            println!(
                "issues for {identifier}'s pull request: {}",
                linked
                    .iter()
                    .map(|i| i.identifier.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        None => println!("issues_for_url: no issue with a pull request attachment found"),
    }
}
