//! Live reads against api.github.com. Run with `--ignored` and `GITHUB_TOKEN`
//! in the environment. Output is limited to counts and titles.

use loupe_github::{GitHub, Page, PrListFilter, PrState, PrSummary, RepoRef};

fn client() -> GitHub {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .expect("GITHUB_TOKEN is not set: pipe `gh auth token` into the run to enable live tests");
    GitHub::new(token.trim())
}

fn zed() -> RepoRef {
    RepoRef::new("zed-industries", "zed")
}

fn show(label: &str, page: &Page<PrSummary>) {
    println!(
        "{label}: {} items of {}, next page: {}",
        page.items.len(),
        page.total_count,
        page.next_cursor.is_some()
    );
    for item in &page.items {
        println!("  {} {}", item.pr, item.title);
    }
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn inbox_returns_three_pages() {
    let inbox = client().inbox().expect("inbox");
    show("review requested", &inbox.review_requested);
    show("authored", &inbox.authored);
    show("involved", &inbox.involved);
    for page in [&inbox.review_requested, &inbox.authored, &inbox.involved] {
        assert!(page.items.len() <= 25);
        assert!(page.items.iter().all(|pr| pr.state == PrState::Open));
    }
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn search_pages_through_merged_pull_requests() {
    let gh = client();
    let first = gh
        .search_pull_requests("repo:zed-industries/zed is:merged", 5, None)
        .expect("first page");
    show("first page", &first);
    assert_eq!(first.items.len(), 5);
    assert!(first.items.iter().all(|pr| pr.state == PrState::Merged));
    let cursor = first.next_cursor.clone().expect("a next cursor");

    let second = gh
        .search_pull_requests("repo:zed-industries/zed is:merged", 5, Some(&cursor))
        .expect("second page");
    show("second page", &second);
    assert_eq!(second.items.len(), 5);
    assert!(second.items.iter().all(|pr| pr.state == PrState::Merged));
    assert!(
        second
            .items
            .iter()
            .all(|pr| first.items.iter().all(|seen| seen.id != pr.id)),
        "the second page repeats the first"
    );
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn repository_list_filters_by_state() {
    let gh = client();
    let closed = gh
        .repo_pull_requests(&zed(), PrListFilter::Closed, None, None)
        .expect("closed");
    show("closed", &closed);
    assert!(!closed.items.is_empty());
    assert!(closed.items.iter().all(|pr| pr.state == PrState::Closed));

    let open = gh
        .repo_pull_requests(&zed(), PrListFilter::Open, Some("diff"), None)
        .expect("open with text");
    show("open matching diff", &open);
    assert!(open.items.iter().all(|pr| pr.state == PrState::Open));
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn lists_viewer_repositories() {
    let repos = client().viewer_repositories().expect("viewer repositories");
    println!("viewer repositories: {}", repos.len());
    for repo in repos.iter().take(5) {
        println!("  {}", repo.repo);
    }
    assert!(!repos.is_empty());
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn searches_repositories() {
    let repos = client()
        .search_repositories("gpui-component")
        .expect("repository search");
    println!("repository search: {}", repos.len());
    for repo in &repos {
        println!("  {}", repo.repo);
    }
    assert!(!repos.is_empty());
    assert!(repos.len() <= 20);
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn all_open_reads_every_page_of_a_search() {
    let page = client()
        .search_all_pull_requests(
            "is:pr is:open archived:false repo:rust-lang/rust-analyzer repo:tokio-rs/tokio \
             sort:updated-desc",
        )
        .expect("all open");
    println!(
        "all open: {} items of {}, more: {}",
        page.items.len(),
        page.total_count,
        page.next_cursor.is_some()
    );
    assert!(page.total_count > 100, "the two keep several pages open");
    assert_eq!(page.items.len(), (page.total_count as usize).min(500));
    assert!(page.items.iter().all(|pr| pr.state == PrState::Open));
    let mut ids: Vec<&str> = page.items.iter().map(|pr| pr.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), page.items.len(), "a page repeats another");
}
