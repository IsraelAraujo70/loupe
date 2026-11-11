//! Live checks against api.github.com. Run with `GITHUB_TOKEN` set:
//! `cargo test -p loupe-github --test live_rest -- --ignored --nocapture`.
//! Nothing here changes real notifications.

use loupe_github::{Error, GitHub, PrRef, RepoRef};

fn client() -> GitHub {
    let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN is set");
    GitHub::new(token)
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn pull_request_files_pages_past_one_hundred_files() {
    // rust-lang/rust#165493 is merged, so its file list is fixed. `gh api
    // repos/rust-lang/rust/pulls/165493` reports changed_files = 126.
    let pr = PrRef::new(RepoRef::new("rust-lang", "rust"), 165493);
    let files = client().pull_request_files(&pr).expect("files load");
    println!("files: {}", files.len());
    assert_eq!(files.len(), 126);
    assert!(files.iter().any(|file| file.patch.is_some()));
    let mut paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(paths.len(), files.len(), "pages overlapped");
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn notifications_load_with_and_without_read_ones() {
    let github = client();
    let unread = github.notifications(false).expect("unread notifications");
    let all = github.notifications(true).expect("all notifications");
    println!("unread: {}, all: {}", unread.len(), all.len());
    assert!(all.len() >= unread.len());
    assert!(unread.iter().all(|notification| notification.unread));
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn marking_a_bogus_thread_read_is_refused_by_the_endpoint() {
    let result = client().mark_notification_read("1");
    assert!(
        matches!(
            result,
            Err(Error::Http {
                status: 403 | 404,
                ..
            })
        ),
        "unexpected result: {result:?}"
    );
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn branch_ci_finds_the_pull_request_and_the_failed_run_of_a_branch() {
    // IsraelAraujo70/loupe#19 was merged from
    // feat/comment-order after CI runs 97 and 99 failed on the `check` job.
    let repo = RepoRef::new("IsraelAraujo70", "loupe");
    let ci = client()
        .branch_ci(&repo, "feat/comment-order")
        .expect("branch ci loads");
    let pull = ci.pull_request.expect("the branch has a pull request");
    println!(
        "{} {:?}, {} failed runs",
        pull.pr,
        pull.state,
        ci.failed_runs.len()
    );
    assert_eq!(pull.pr, PrRef::new(repo, 19));
    let run = ci
        .failed_runs
        .iter()
        .find(|run| run.run_number == 99)
        .expect("run 99 failed");
    assert_eq!(run.workflow, "CI");
    assert_eq!(run.failed_jobs, ["check"]);
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn polling_the_checks_of_an_unchanged_commit_answers_unchanged() {
    // The fixture pull request #1's head carries the failing `fixture` check.
    let github = client();
    let repo = RepoRef::new("IsraelAraujo70", "loupe");
    let pr = github
        .pull_request(&PrRef::new(repo.clone(), 1))
        .expect("fixture loads");
    let first = github
        .poll_checks(&repo, &pr.head_oid, None)
        .expect("first poll");
    let loupe_github::ChecksPoll::Changed(seen) = first else {
        panic!("a first poll has nothing to compare with: {first:?}");
    };
    let second = github
        .poll_checks(&repo, &pr.head_oid, Some(&seen))
        .expect("second poll");
    assert_eq!(second, loupe_github::ChecksPoll::Unchanged);
    let core = github
        .rate_limits()
        .core
        .expect("REST reports its rate limit");
    println!("core: {} of {} left", core.remaining, core.limit);
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn a_matrix_run_reads_with_every_job_back_to_its_job_in_the_file() {
    // pingdotgg/t3code's Release run #4520 ran its whole graph: an eight-way
    // matrix, six calls of a reusable workflow, and a fan-in to one release.
    let github = client();
    let repo = RepoRef::new("pingdotgg", "t3code");
    let run = github.workflow_run(&repo, 43012008138).expect("run loads");
    assert_eq!(run.number, 4520);
    assert!(run.is_finished());
    let jobs = github.run_jobs(&repo, run.id).expect("jobs load");
    println!("jobs: {}", jobs.len());
    assert!(jobs.len() >= 30);
    let spec = github
        .workflow_spec(&repo, &run.path, &run.head_sha)
        .expect("workflow file loads");
    let names: Vec<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
    let assigned = loupe_github::assign_jobs(&spec, &names);
    for (name, job) in names.iter().zip(&assigned) {
        println!("{name} -> {:?}", job.map(|ix| &spec.jobs[ix].key));
    }
    assert!(
        assigned.iter().all(Option::is_some),
        "a job has no job in the file"
    );
    let matrix = spec
        .jobs
        .iter()
        .find(|job| job.key == "test_server")
        .expect("the matrix job");
    assert!(matrix.matrix);
    assert_eq!(matrix.needs, ["preflight"]);

    let finished = jobs
        .iter()
        .find(|job| job.name == "Preflight")
        .expect("a job that ran");
    let log = github.job_log(&repo, finished).expect("log loads");
    assert!(!log.steps.is_empty());
}

#[test]
#[ignore = "needs GITHUB_TOKEN and network"]
fn runs_of_a_branch_list_with_annotations_and_artifacts() {
    let github = client();
    let repo = RepoRef::new("IsraelAraujo70", "loupe");
    let runs = github
        .workflow_runs(&repo, Some("main"))
        .expect("runs load");
    assert!(!runs.is_empty());
    assert!(runs.iter().all(|run| run.branch.as_deref() == Some("main")));
    let run = runs
        .iter()
        .find(|run| run.workflow == "Release" && run.is_finished())
        .expect("a finished release run");
    let artifacts = github.run_artifacts(&repo, run.id).expect("artifacts load");
    println!("artifacts: {artifacts:?}");
    let suite = run.check_suite.as_deref().expect("a check suite");
    let annotations = github.run_annotations(suite).expect("annotations load");
    println!("annotations: {}", annotations.len());
}
