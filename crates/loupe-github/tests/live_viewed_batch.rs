//! Counts the requests behind marking files viewed on the test fixture's
//! writable pull request (docs/TEST_FIXTURE.md). It WRITES to GitHub: it sets
//! the viewed mark of the fixture's README.md and puts it back. Needs
//! `GITHUB_TOKEN` and `LOUPE_E2E=1`; run each test alone with
//! `LOUPE_TRACE_GITHUB=1 --nocapture` and count the `github-call` lines.

use loupe_github::{GitHub, PrRef, ViewedState};

mod fixture;

const FILE: &str = "README.md";
const FILES: usize = 120;

fn connect() -> (GitHub, PrRef, String) {
    assert_eq!(
        std::env::var("LOUPE_E2E").as_deref(),
        Ok("1"),
        "set LOUPE_E2E=1"
    );
    let github = GitHub::new(std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN"));
    let pr = fixture::pr(fixture::STACK_1);
    let id = github.pull_request(&pr).expect("detail").id.to_string();
    (github, pr, id)
}

fn viewed(github: &GitHub, pr: &PrRef) -> ViewedState {
    let files = github.pull_request_files(pr).expect("files");
    files
        .iter()
        .find(|file| file.path == FILE)
        .expect("the fixture's file")
        .viewed
}

#[test]
#[ignore = "writes to the fixture PR; needs GITHUB_TOKEN and LOUPE_E2E=1"]
fn one_request_per_file() {
    let (github, pr, id) = connect();
    let before = viewed(&github, &pr);
    assert_ne!(before, ViewedState::Dismissed, "cannot restore Dismissed");
    for _ in 0..FILES {
        github.set_file_viewed(&id, FILE, true).expect("mark");
    }
    github
        .set_file_viewed(&id, FILE, before == ViewedState::Viewed)
        .expect("restore");
    assert_eq!(viewed(&github, &pr), before);
}

#[test]
#[ignore = "writes to the fixture PR; needs GITHUB_TOKEN and LOUPE_E2E=1"]
fn batched_requests() {
    let (github, pr, id) = connect();
    let before = viewed(&github, &pr);
    assert_ne!(before, ViewedState::Dismissed, "cannot restore Dismissed");
    let paths = vec![FILE.to_owned(); FILES];
    assert!(github.set_files_viewed(&id, &paths, true).is_empty());
    assert_eq!(viewed(&github, &pr), ViewedState::Viewed);
    assert!(github.set_files_viewed(&id, &paths[..1], false).is_empty());
    assert_eq!(viewed(&github, &pr), ViewedState::Unviewed);
    let restore = before == ViewedState::Viewed;
    assert!(
        github
            .set_files_viewed(&id, &paths[..1], restore)
            .is_empty()
    );
    assert_eq!(viewed(&github, &pr), before);
}

#[test]
#[ignore = "writes to the fixture PR; needs GITHUB_TOKEN and LOUPE_E2E=1"]
fn a_refused_file_fails_alone() {
    let (github, pr, id) = connect();
    let before = viewed(&github, &pr);
    assert_ne!(before, ViewedState::Dismissed, "cannot restore Dismissed");
    let paths = vec![
        FILE.to_owned(),
        "no/such/file.rs".to_owned(),
        FILE.to_owned(),
    ];
    let failures = github.set_files_viewed(&id, &paths, true);
    println!("failures: {failures:?}");
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].path, "no/such/file.rs");
    assert_eq!(viewed(&github, &pr), ViewedState::Viewed);
    let restore = before == ViewedState::Viewed;
    assert!(
        github
            .set_files_viewed(&id, &paths[..1], restore)
            .is_empty()
    );
    assert_eq!(viewed(&github, &pr), before);
}
