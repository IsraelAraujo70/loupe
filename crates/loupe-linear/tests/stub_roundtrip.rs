//! Drives every client method against a running stub server
//! (`examples/stub_server.rs`), including the mutations and the failure
//! switch. Start the stub first, then:
//!
//!     LOUPE_STUB_URL=http://127.0.0.1:8787 cargo test -p loupe-linear --test stub_roundtrip -- --ignored --nocapture

use loupe_linear::filter::{FilterClause, ME, SetField, SetOp};
use loupe_linear::{ClosedIssues, Error, IssueQuery, IssueUpdate, Linear, NewIssue, Priority};

fn post(base: &str, path: &str) {
    ureq::post(&format!("{base}{path}"))
        .send_empty()
        .expect("toggle the stub");
}

#[test]
#[ignore = "needs a running stub server at LOUPE_STUB_URL"]
fn every_method_round_trips_through_the_stub() {
    let Ok(base) = std::env::var("LOUPE_STUB_URL") else {
        println!("LOUPE_STUB_URL is not set, skipping");
        return;
    };
    let client = Linear::with_endpoint("stub-key", format!("{base}/graphql"));
    post(&base, "/fail-off");

    let viewer = client.viewer().expect("viewer");
    let teams = client.teams().expect("teams");
    let eng = teams.iter().find(|t| t.key == "ENG").expect("ENG team");
    let team = client.team(&eng.id).expect("team");
    println!("teams {}, ENG states {}", teams.len(), team.states.len());

    let mine = client.my_open_issues().expect("my issues");
    assert!(!mine.is_empty());
    assert!(
        mine.iter()
            .all(|i| i.assignee.as_ref().map(|u| &u.id) == Some(&viewer.user.id))
    );

    let all = client
        .team_issues(&eng.id, &IssueQuery::default(), None)
        .expect("team issues");
    let urgent = client
        .team_issues(
            &eng.id,
            &IssueQuery {
                clauses: vec![
                    FilterClause::Set {
                        field: SetField::Priority,
                        op: SetOp::In,
                        values: vec!["1".into()],
                    },
                    FilterClause::Set {
                        field: SetField::Assignee,
                        op: SetOp::In,
                        values: vec![ME.into()],
                    },
                ],
                closed: ClosedIssues::None,
            },
            None,
        )
        .expect("filtered issues");
    assert!(urgent.nodes.len() < all.nodes.len());
    assert!(urgent.nodes.iter().all(|i| i.priority == Priority::Urgent));
    println!(
        "ENG issues {}, urgent+mine {}",
        all.nodes.len(),
        urgent.nodes.len()
    );

    let detail = client.issue("ENG-101").expect("issue");
    let pr = detail
        .attachments
        .iter()
        .find(|a| a.pull_request.is_some())
        .expect("a PR attachment");
    let linked = client.issues_for_url(&pr.url).expect("issues for url");
    assert!(linked.iter().any(|i| i.identifier == "ENG-101"));
    let by_id = client
        .issues_by_identifiers(&["ENG-101".into(), "ENG-102".into(), "ZZZ-1".into()])
        .expect("by identifiers");
    let mut found: Vec<_> = by_id.iter().map(|i| i.identifier.as_str()).collect();
    found.sort();
    assert_eq!(found, ["ENG-101", "ENG-102"]);
    assert!(!client.search_issues("crash", 5).expect("search").is_empty());
    assert!(!client.custom_views().expect("custom views").is_empty());

    let target = team
        .states
        .iter()
        .find(|s| s.id != detail.issue.state.id)
        .expect("another state");
    let updated = client
        .update_issue(
            &detail.issue.id,
            &IssueUpdate {
                state_id: Some(target.id.clone()),
                priority: Some(Priority::Low),
                ..IssueUpdate::default()
            },
        )
        .expect("update");
    assert_eq!(updated.state.id, target.id);
    assert_eq!(updated.priority, Priority::Low);
    let reread = client.issue(&detail.issue.identifier).expect("reread");
    assert_eq!(reread.issue.state.id, target.id);

    let comment = client
        .create_comment(&detail.issue.id, "From the stub round trip")
        .expect("comment");
    let reread = client.issue(&detail.issue.identifier).expect("reread");
    assert!(reread.comments.iter().any(|c| c.id == comment.id));

    let created = client
        .create_issue(&NewIssue {
            team_id: eng.id.clone(),
            title: "Created by the stub round trip".into(),
            priority: Some(Priority::High),
            ..NewIssue::default()
        })
        .expect("create");
    assert_eq!(created.team.key, "ENG");
    assert_eq!(created.priority, Priority::High);

    post(&base, "/fail-on");
    let refused = client.update_issue(
        &detail.issue.id,
        &IssueUpdate {
            priority: Some(Priority::Urgent),
            ..IssueUpdate::default()
        },
    );
    post(&base, "/fail-off");
    assert!(matches!(refused, Err(Error::GraphQl(_))), "{refused:?}");
    println!(
        "mutations ok: update, comment, create {}, refused when failing",
        created.identifier
    );
}
