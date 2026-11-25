//! Live smoke test of the pull request detail query. Needs `GITHUB_TOKEN`.

use loupe_github::diff::parse_patch;
use loupe_github::{
    CheckStatus, ChecksState, FileStatus, GitHub, PrRef, PrState, RepoRef, StackShape,
};

mod fixture;

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

/// IsraelAraujo70/loupe#1 is a closed README pull request, not the draft
/// with the `loupe-fixture/` files this test once pinned. Its state belongs to
/// the repository, so the test reads it and checks that the detail and the
/// file list agree, whatever state the pull request is in.
#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn loads_the_review_fixture() {
    let github = client();
    let pr = fixture::pr(fixture::STACK_1);
    let detail = github.pull_request(&pr).expect("detail");
    println!(
        "{pr}: draft {}, state {:?}, mergeable {:?}, files {}, threads {}",
        detail.is_draft,
        detail.state,
        detail.mergeable,
        detail.changed_files,
        detail.threads.len()
    );
    assert!(detail.is_draft, "the fixture stays a draft");
    assert_eq!(detail.state, PrState::Open);
    assert_eq!(detail.base_ref, fixture::BASE_BRANCH);
    assert_eq!(detail.head_ref, fixture::STACK_1_BRANCH);
    assert!(
        detail.body.contains("BACK-293"),
        "the Linear ticket in the body"
    );
    assert!(
        detail
            .threads
            .iter()
            .any(|thread| thread.path == "src/units.rs" && !thread.is_resolved),
        "the seeded open thread"
    );
    assert!(
        detail
            .threads
            .iter()
            .any(|thread| thread.path == "src/pantry/stock_report.rs" && thread.is_resolved),
        "the seeded resolved thread"
    );

    let files = github.pull_request_files(&pr).expect("files");
    assert_eq!(files.len(), detail.changed_files as usize);
    let file = |path: &str| {
        files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} is in the fixture"))
    };
    let renamed = file("docs/guide/quickstart.md");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(
        renamed.previous_path.as_deref(),
        Some("docs/guide/getting-started.md")
    );
    assert!(renamed.patch.is_some(), "renamed with edits");
    let moved = file("src/recipe/parser.rs");
    assert_eq!(moved.status, FileStatus::Renamed);
    assert!(moved.patch.is_none(), "renamed without edits");
    assert_eq!(
        file("src/pantry/legacy_import.rs").status,
        FileStatus::Removed
    );
    for binary in ["assets/badge.png", "assets/icon.png"] {
        assert!(file(binary).patch.is_none(), "{binary} is binary");
    }
    assert!(
        file("scripts/seed.sh").patch.is_none(),
        "a mode change only"
    );
    assert!(file("web/src/features/planner/.gitkeep").patch.is_none());
    file("web/src/features/planner/week/day/MealSlot.tsx");
    file("web/src/features/planner/week/day/MealSlots.tsx");
    file("src/pantry/stock_report.rs");
    file("src/pantry/stock_reports.rs");
    file("src/generated/units_table.rs");
    let long = parse_patch(
        file("src/catalog/ingredients.rs")
            .patch
            .as_deref()
            .expect("patch"),
    )
    .expect("hunks");
    assert!(long.len() >= 3, "edits far apart in a long file");
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn reads_the_fixture_checks() {
    let detail = client()
        .pull_request(&fixture::pr(fixture::STACK_1))
        .expect("detail");
    let run = |name: &str| {
        detail
            .checks
            .runs
            .iter()
            .find(|run| run.name == name)
            .unwrap_or_else(|| panic!("the {name} check"))
    };
    assert_eq!(run("passes").status, CheckStatus::Passed);
    assert_eq!(run("fails").status, CheckStatus::Failed);
    assert_eq!(run("skipped").status, CheckStatus::Skipped);
    let status = run("loupe-fixture/status");
    assert_eq!(status.status, CheckStatus::Passed);
    assert!(
        status.workflow.is_none(),
        "a commit status, not a check run"
    );
    assert!(status.description.is_some());
    assert_eq!(detail.checks.state, Some(ChecksState::Failing));
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn the_fixture_stack_chains_three_pull_requests() {
    let github = client();
    let detail = github
        .pull_request(&fixture::pr(fixture::STACK_2))
        .expect("detail");
    let graph = github
        .pull_request_stack(&detail.pr, &detail.base_ref, &detail.head_ref)
        .expect("stack")
        .expect("the fixture is a stack");
    let numbers: Vec<u64> = graph.nodes.iter().map(|node| node.node.pr.number).collect();
    println!("{:?} {numbers:?} on {}", graph.shape, graph.base_ref);
    assert_eq!(graph.shape, StackShape::Chain);
    assert_eq!(graph.base_ref, fixture::BASE_BRANCH);
    assert_eq!(
        numbers,
        [fixture::STACK_1, fixture::STACK_2, fixture::STACK_3]
    );
    assert!(graph.nodes.iter().all(|node| node.node.is_draft));
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn lists_every_commit_of_the_fixture_past_250() {
    let github = client();
    let detail = github
        .pull_request(&fixture::pr(fixture::MANY_COMMITS))
        .expect("detail");
    assert_eq!(detail.commits_total, fixture::MANY_COMMITS_TOTAL);
    assert!(!detail.has_every_commit());
    let commits = github.pull_request_commits(&detail).expect("commits");
    assert_eq!(commits.len(), fixture::MANY_COMMITS_TOTAL as usize);
    assert_eq!(
        commits.last().map(|commit| commit.oid.as_str()),
        Some(detail.head_oid.as_str()),
        "oldest first, the head last"
    );
}
