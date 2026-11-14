//! Live smoke test of the pull request detail query. Needs `GITHUB_TOKEN`.

use loupe_github::{GitHub, PrRef, PrState, RepoRef};

fn client() -> GitHub {
    let token = std::env::var("GITHUB_TOKEN").expect("set GITHUB_TOKEN to run live tests");
    GitHub::new(token)
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn loads_a_reviewed_merged_pull_request() {
    let github = client();
    let found = github
        .search_pull_requests(
            "repo:zed-industries/zed is:pr is:merged comments:>8",
            1,
            None,
        )
        .expect("search");
    let pr = found.items.first().expect("a reviewed PR").pr.clone();
    let detail = github.pull_request(&pr).expect("detail");
    println!(
        "{pr}: commits {}, checks {}, timeline {}, threads {}, reviewers {}, labels {}",
        detail.commits.len(),
        detail.checks.runs.len(),
        detail.timeline.len(),
        detail.threads.len(),
        detail.reviewers.len(),
        detail.labels.len()
    );
    assert_eq!(detail.pr, pr);
    assert_eq!(detail.state, PrState::Merged);
    assert!(!detail.commits.is_empty());
    assert!(!detail.timeline.is_empty());
    assert!(!detail.reviewers.is_empty());
    assert!(!detail.head_oid.is_empty());
    assert!(detail.threads.iter().all(|thread| !thread.path.is_empty()));
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn paginates_review_threads_past_one_hundred() {
    let github = client();
    let pr = PrRef::new(RepoRef::new("rust-lang", "rust"), 100_000);
    let found = github
        .search_pull_requests(
            "repo:rust-lang/rust is:pr is:merged comments:>300 sort:comments-desc",
            1,
            None,
        )
        .expect("search");
    let pr = found
        .items
        .first()
        .map(|item| item.pr.clone())
        .unwrap_or(pr);
    let detail = github.pull_request(&pr).expect("detail");
    println!(
        "{pr}: timeline {}, threads {}",
        detail.timeline.len(),
        detail.threads.len()
    );
    assert!(detail.timeline.len() > 100 || detail.threads.len() > 100);
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn loads_the_review_fixture() {
    let github = client();
    let pr = PrRef::new(RepoRef::new("IsraelAraujo70", "loupe"), 1);
    let detail = github.pull_request(&pr).expect("detail");
    println!(
        "{pr}: draft {}, state {:?}, mergeable {:?}, merge methods {:?}, files {}",
        detail.is_draft, detail.state, detail.mergeable, detail.merge_methods, detail.changed_files
    );
    assert!(detail.is_draft);
    assert_eq!(detail.state, PrState::Open);
    assert_eq!(detail.base_ref, "test/loupe-review-base");
    assert_eq!(detail.head_ref, "test/loupe-review-fixture");
    let files = github.pull_request_files(&pr).expect("files");
    let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
    println!("files: {paths:?}");
    assert!(paths.contains(&"loupe-fixture/new_name.md"));
    assert!(
        files
            .iter()
            .any(|file| file.previous_path.as_deref() == Some("loupe-fixture/old_name.md"))
    );
}
