//! Live check of the asynchronous merge request against api.github.com,
//! without side effects: it asks to merge pull requests of this repository
//! that are already merged or closed, which GitHub answers right away.
//! The token needs write access to IsraelAraujo70/loupe.
//!
//! Run: `gh auth token | read -r GITHUB_TOKEN; export GITHUB_TOKEN;
//! cargo test -p loupe-github --test live_merge -- --ignored --nocapture`

use loupe_github::{Error, GitHub, MergeMethod, MergeOutcome, PrRef, RepoRef};

fn client() -> GitHub {
    let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN is set");
    GitHub::new(token)
}

fn loupe_pr(number: u64) -> PrRef {
    PrRef::new(RepoRef::new("IsraelAraujo70", "loupe"), number)
}

#[test]
#[ignore = "needs GITHUB_TOKEN with write access and network"]
fn merging_a_merged_pull_request_reports_merged() {
    // #16 was squash-merged as 4a1ad49.
    let outcome = client().merge(
        &loupe_pr(16),
        MergeMethod::Squash,
        "4a1ad49272b9be272d1c41af8e612511c25dc9b5",
    );
    println!("#16: {outcome:?}");
    assert_eq!(outcome.ok(), Some(MergeOutcome::Merged));
}

#[test]
#[ignore = "needs GITHUB_TOKEN with write access and network"]
fn merging_a_closed_pull_request_fails_with_githubs_reason() {
    // #1 was closed without merging; c4417c4 is its head.
    let outcome = client().merge(
        &loupe_pr(1),
        MergeMethod::Squash,
        "c4417c496fddaf397be9ea1495bd4041054fe769",
    );
    println!("#1: {outcome:?}");
    assert!(
        matches!(&outcome, Err(Error::MergeFailed(message)) if message.contains("closed")),
        "{outcome:?}"
    );
}
