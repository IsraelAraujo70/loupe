//! Live check of every mutation document against GitHub's real validator,
//! without side effects: each call targets a node id that does not exist, so
//! a valid document fails with "Could not resolve to a node" and a broken one
//! fails with a validation error.
//!
//! Run: `gh auth token | read -r GITHUB_TOKEN; export GITHUB_TOKEN;
//! cargo test -p loupe-github --test live_mutations -- --ignored --nocapture`

use loupe_github::{Error, GitHub, LineAnchor, NewThread, Result, ReviewEvent, Side};

const BOGUS_PR: &str = "PR_kwDOBogusBogus";
const BOGUS_THREAD: &str = "PRRT_kwDOBogus";
const BOGUS_REVIEW: &str = "PRR_kwDOBogus";
const BOGUS_ISSUE_COMMENT: &str = "IC_kwDOBogus";
const BOGUS_REVIEW_COMMENT: &str = "PRRC_kwDOBogus";
const BOGUS_USER: &str = "U_kgDOBogus";
const BOGUS_LABEL: &str = "LA_kwDOBogus";

fn thread(anchor: LineAnchor) -> NewThread {
    NewThread {
        pull_request_id: BOGUS_PR.into(),
        path: "README.md".into(),
        body: "bogus".into(),
        anchor,
    }
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn every_mutation_reaches_node_resolution() {
    let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN is set");
    let github = GitHub::new(token);
    let user_ids = [BOGUS_USER.to_owned()];
    let label_ids = [BOGUS_LABEL.to_owned()];
    let events = [
        ("comment", ReviewEvent::Comment),
        ("approve", ReviewEvent::Approve),
        ("request_changes", ReviewEvent::RequestChanges),
    ];

    let mut calls: Vec<(String, Result<()>)> = vec![
        ("add_comment".into(), github.add_comment(BOGUS_PR, "bogus")),
        (
            "edit_issue_comment".into(),
            github.edit_issue_comment(BOGUS_ISSUE_COMMENT, "bogus"),
        ),
        (
            "edit_review_comment".into(),
            github.edit_review_comment(BOGUS_REVIEW_COMMENT, "bogus"),
        ),
        (
            "add_review_thread (single line)".into(),
            github.add_review_thread(&thread(LineAnchor {
                line: 1,
                side: Side::Right,
                start: None,
            })),
        ),
        (
            "add_review_thread (range)".into(),
            github.add_review_thread(&thread(LineAnchor {
                line: 3,
                side: Side::Left,
                start: Some((1, Side::Left)),
            })),
        ),
        (
            "reply_to_thread".into(),
            github.reply_to_thread(BOGUS_THREAD, "bogus"),
        ),
        ("resolve_thread".into(), github.resolve_thread(BOGUS_THREAD)),
        (
            "unresolve_thread".into(),
            github.unresolve_thread(BOGUS_THREAD),
        ),
        ("discard_review".into(), github.discard_review(BOGUS_REVIEW)),
        (
            "set_file_viewed (true)".into(),
            github.set_file_viewed(BOGUS_PR, "README.md", true),
        ),
        (
            "set_file_viewed (false)".into(),
            github.set_file_viewed(BOGUS_PR, "README.md", false),
        ),
        (
            "mark_ready_for_review".into(),
            github.mark_ready_for_review(BOGUS_PR),
        ),
        ("convert_to_draft".into(), github.convert_to_draft(BOGUS_PR)),
        (
            "close_pull_request".into(),
            github.close_pull_request(BOGUS_PR),
        ),
        (
            "reopen_pull_request".into(),
            github.reopen_pull_request(BOGUS_PR),
        ),
        (
            "request_reviewers".into(),
            github.request_reviewers(BOGUS_PR, &user_ids),
        ),
        (
            "update_pull_request".into(),
            github.update_pull_request(BOGUS_PR, Some("bogus"), Some("bogus")),
        ),
        ("add_labels".into(), github.add_labels(BOGUS_PR, &label_ids)),
        (
            "remove_labels".into(),
            github.remove_labels(BOGUS_PR, &label_ids),
        ),
    ];
    for (label, event) in events {
        calls.push((
            format!("submit_review (Some, {label})"),
            github.submit_review(BOGUS_PR, Some(BOGUS_REVIEW), event, "bogus"),
        ));
        calls.push((
            format!("submit_review (None, {label})"),
            github.submit_review(BOGUS_PR, None, event, "bogus"),
        ));
    }

    let mut failures = Vec::new();
    for (name, result) in calls {
        match result {
            Err(Error::GraphQl(messages)) => {
                let first = messages.first().map_or("", String::as_str);
                println!("{name}: {first}");
                if !messages
                    .iter()
                    .any(|message| message.contains("Could not resolve to a node"))
                {
                    failures.push(format!("{name}: {}", messages.join("; ")));
                }
            }
            Err(other) => {
                println!("{name}: {other}");
                failures.push(format!("{name}: {other}"));
            }
            Ok(()) => {
                println!("{name}: unexpectedly succeeded");
                failures.push(format!("{name}: unexpectedly succeeded"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "mutations that did not reach node resolution:\n{}",
        failures.join("\n")
    );
}
