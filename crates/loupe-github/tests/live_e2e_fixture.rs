//! End-to-end review flow against the scratch fixture PR
//! (IsraelAraujo70/loupe#1), through the same methods the app
//! calls. It WRITES to GitHub: run it on purpose, once per verification.
//! Needs `GITHUB_TOKEN` and `LOUPE_E2E=1`.

use loupe_github::diff::{LineKind, anchor_for_range, parse_patch};
use loupe_github::{GitHub, NewThread, PrRef, RepoRef, ReviewEvent, Side, ViewedState};

const PREFIX: &str = "🤖 Loupe test:";
const FILE: &str = "loupe-fixture/calculator.rs";
const VIEWED_FILE: &str = "loupe-fixture/added.ts";

#[test]
#[ignore = "writes to the fixture PR; needs GITHUB_TOKEN and LOUPE_E2E=1"]
fn review_flow_on_the_fixture() {
    assert_eq!(
        std::env::var("LOUPE_E2E").as_deref(),
        Ok("1"),
        "set LOUPE_E2E=1"
    );
    let github = GitHub::new(std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN"));
    let pr = PrRef::new(RepoRef::new("IsraelAraujo70", "loupe"), 1);
    let detail = github.pull_request(&pr).expect("detail");
    assert!(detail.is_draft, "the fixture must stay a draft");

    let files = github.pull_request_files(&pr).expect("files");
    let calculator = files
        .iter()
        .find(|f| f.path == FILE)
        .expect("calculator.rs");
    let hunks = parse_patch(calculator.patch.as_deref().expect("patch")).expect("hunks");

    let added = hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .find(|line| line.kind == LineKind::Added && line.text.contains("checked_div"))
        .expect("added checked_div line");
    let single = anchor_for_range(&hunks, added.anchor(), added.anchor()).expect("single anchor");

    let removed: Vec<_> = hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == LineKind::Removed)
        .take(3)
        .collect();
    assert_eq!(removed.len(), 3);
    let first = (removed[0].old_line.expect("old"), Side::Left);
    let last = (removed[2].old_line.expect("old"), Side::Left);
    let range = anchor_for_range(&hunks, last, first).expect("left range anchor");
    assert_eq!(range.side, Side::Left);
    assert_eq!(range.start, Some((first.0, Side::Left)));
    println!("single anchor {single:?}; left range anchor {range:?}");

    let already_reviewed = detail.threads.iter().any(|thread| {
        thread
            .comments
            .first()
            .is_some_and(|comment| comment.body.starts_with(PREFIX))
    });
    if already_reviewed {
        println!("threads already posted by an earlier run; continuing from the review");
    } else {
        post_comments_and_review(&github, &detail.id, &pr, &single, &range);
    }
    continue_flow(&github, &pr, &detail.id, &single, &range, first);
}

fn post_comments_and_review(
    github: &GitHub,
    pull_request_id: &str,
    pr: &PrRef,
    single: &loupe_github::LineAnchor,
    range: &loupe_github::LineAnchor,
) {
    let detail_id = pull_request_id.to_owned();
    github
        .add_comment(&detail_id, &format!("{PREFIX} conversation comment"))
        .expect("conversation comment");
    github
        .add_review_thread(&NewThread {
            pull_request_id: detail_id.clone(),
            path: FILE.into(),
            body: format!("{PREFIX} single-line comment on checked_div"),
            anchor: single.clone(),
        })
        .expect("single-line thread");
    github
        .add_review_thread(&NewThread {
            pull_request_id: detail_id.clone(),
            path: FILE.into(),
            body: format!("{PREFIX} multi-line comment on the removed loop (left side)"),
            anchor: range.clone(),
        })
        .expect("multi-line left thread");

    let pending = github
        .pull_request(pr)
        .expect("detail with pending review")
        .pending_review
        .expect("pending review created by add_review_thread");
    assert_eq!(pending.comment_count, 2);
    github
        .submit_review(
            &detail_id,
            Some(&pending.id),
            ReviewEvent::Comment,
            &format!("{PREFIX} review summary"),
        )
        .expect("submit COMMENT review");
}

fn continue_flow(
    github: &GitHub,
    pr: &PrRef,
    pull_request_id: &str,
    single: &loupe_github::LineAnchor,
    range: &loupe_github::LineAnchor,
    first: (u32, Side),
) {
    let after = github.pull_request(pr).expect("detail after review");
    let mine: Vec<_> = after
        .threads
        .iter()
        .filter(|thread| {
            thread
                .comments
                .first()
                .is_some_and(|comment| comment.body.starts_with(PREFIX))
        })
        .collect();
    let single_thread = mine
        .iter()
        .rev()
        .find(|t| t.side == Side::Right && t.start_line.is_none())
        .expect("single-line thread on GitHub");
    let range_thread = mine
        .iter()
        .rev()
        .find(|t| t.side == Side::Left && t.start_line.is_some())
        .expect("multi-line left thread on GitHub");
    assert_eq!(single_thread.line, Some(single.line));
    assert_eq!(range_thread.line, Some(range.line));
    assert_eq!(range_thread.start_line, Some(first.0));
    assert_eq!(range_thread.start_side, Some(Side::Left));

    github
        .reply_to_thread(&single_thread.id, &format!("{PREFIX} reply"))
        .expect("reply");
    github.resolve_thread(&range_thread.id).expect("resolve");
    github
        .set_file_viewed(pull_request_id, VIEWED_FILE, true)
        .expect("mark viewed");

    let last = github.pull_request(pr).expect("final detail");
    let replied = last
        .threads
        .iter()
        .find(|t| t.id == single_thread.id)
        .expect("single thread");
    assert!(
        replied
            .comments
            .iter()
            .any(|c| c.body == format!("{PREFIX} reply"))
    );
    assert!(
        last.threads
            .iter()
            .find(|t| t.id == range_thread.id)
            .expect("range thread")
            .is_resolved
    );
    let viewed = github
        .pull_request_files(pr)
        .expect("files")
        .into_iter()
        .find(|f| f.path == VIEWED_FILE)
        .expect("viewed file")
        .viewed;
    assert_eq!(viewed, ViewedState::Viewed);
    println!(
        "single thread {} line {:?}; range thread {} lines {:?}-{:?} {:?}",
        single_thread.id,
        single_thread.line,
        range_thread.id,
        range_thread.start_line,
        range_thread.line,
        range_thread.side
    );
}
