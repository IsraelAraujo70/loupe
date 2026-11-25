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

/// zed-industries/zed#3174 has 769 commits, more than the 250 GitHub lists
/// of a pull request.
#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn lists_every_commit_of_a_pull_request_past_the_250_github_lists() {
    let github = client();
    let pr = PrRef::new(RepoRef::new("zed-industries", "zed"), 3174);
    let detail = github.pull_request(&pr).expect("detail");
    assert_eq!(detail.commits.len(), 100, "the detail holds the first page");
    assert_eq!(detail.commits_total, 769);
    assert!(!detail.has_every_commit());

    let commits = github.pull_request_commits(&detail).expect("commits");
    assert_eq!(commits.len(), 769);
    assert_eq!(
        commits.last().map(|commit| commit.oid.as_str()),
        Some(detail.head_oid.as_str()),
        "oldest first, the head last"
    );
    let unique: std::collections::HashSet<_> = commits.iter().map(|c| &c.oid).collect();
    assert_eq!(unique.len(), 769);

    let head = github.pull_request_head(&pr).expect("head");
    assert_eq!(head.head_oid, detail.head_oid);
    assert_eq!(head.base_oid, detail.base_oid);
    assert_eq!(head.base_ref, detail.base_ref);
}

/// zed-industries/zed#34374 has 122 commits: the list pages on from the
/// detail's first page in GraphQL.
#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn pages_on_from_the_first_commit_page_under_250() {
    let github = client();
    let pr = PrRef::new(RepoRef::new("zed-industries", "zed"), 34374);
    let detail = github.pull_request(&pr).expect("detail");
    assert_eq!(detail.commits_total, 122);
    let commits = github.pull_request_commits(&detail).expect("commits");
    assert_eq!(commits.len(), detail.commits_total as usize);
    assert_eq!(commits[..100], detail.commits[..]);
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
