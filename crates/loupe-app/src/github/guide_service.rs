//! Local guided review coordinator: per-commit disk cache, Claude Code
//! availability probe, active generation runs with live progress and
//! cancellation, and cross-commit step change detection.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::StreamExt as _;
use gpui_kit::{App, Global, Task};
use loupe_github::{ChangedFile, FileStatus, PrRef, PullRequestDetail, TimelineItem};
use loupe_guide::{
    CancelToken, ClaudeAvailability, GuidePromptInput, GuideRunError, ProgressEvent, PromptComment,
    PromptCommit, PromptFile, SCHEMA_VERSION, StepChange, StoredGuide, changed_steps_since,
    check_availability, generate_for_pr, validate,
};
use serde::{Deserialize, Serialize};

use crate::github::pull_request::guide::GuideViewState;
use crate::services::Services;
use crate::store::Settings;

/// Pointer saved per PR so Loupe remembers the most recently generated commit
/// (to show a stale guide banner when new commits land) and the previous commit
/// (to highlight `New` / `Updated` steps after regeneration).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PrGuideIndex {
    latest_head: Option<String>,
    previous_head: Option<String>,
}

struct ActiveRun {
    status: String,
    started_at: DateTime<Utc>,
    cancel: CancelToken,
    _task: Task<()>,
}

#[derive(Default)]
struct GuideService {
    /// GitHub token kept in memory solely to authenticate `git fetch` on private
    /// repositories; never serialized, logged, or exposed to Claude Code.
    github_token: Option<String>,
    availability: Option<ClaudeAvailability>,
    probing: bool,
    _probe_task: Option<Task<()>>,
    runs: HashMap<PrRef, ActiveRun>,
    errors: HashMap<PrRef, String>,
    /// Monotonic counter bumped whenever availability, runs, errors, or stored
    /// guides change so views only recompute `view_state` when needed.
    revision: u64,
}

impl Global for GuideService {}

impl GuideService {
    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Monotonic revision of guide service state for cheap render-time change detection.
pub fn revision(cx: &App) -> u64 {
    service(cx).map_or(0, |s| s.revision)
}

fn service_mut(cx: &mut App) -> &mut GuideService {
    if !cx.has_global::<GuideService>() {
        cx.set_global(GuideService::default());
    }
    cx.global_mut::<GuideService>()
}

fn service(cx: &App) -> Option<&GuideService> {
    cx.try_global::<GuideService>()
}

/// Updates the in-memory GitHub token used for authenticated temporary checkouts.
pub fn set_github_token(token: Option<String>, cx: &mut App) {
    service_mut(cx).github_token = token.filter(|t| !t.trim().is_empty());
}

/// Triggers an asynchronous Claude Code availability check if one has not run yet.
pub fn probe_availability_if_needed(cx: &mut App) {
    let should_probe = service(cx).is_none_or(|s| s.availability.is_none() && !s.probing);
    if should_probe {
        recheck_availability(cx);
    }
}

/// Forces a fresh background check of the configured Claude Code executable and login.
pub fn recheck_availability(cx: &mut App) {
    let custom_exe = Settings::get(cx).guide.executable.clone();
    let svc = service_mut(cx);
    svc.probing = true;
    let work = crate::services::background(cx, move || {
        let path = custom_exe
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(std::path::Path::new);
        check_availability(path)
    });
    let task = cx.spawn(async move |cx| {
        let availability = work.await;
        cx.update(|cx| {
            let svc = service_mut(cx);
            svc.probing = false;
            svc.availability = Some(availability);
            svc.bump_revision();
            cx.refresh_windows();
        });
    });
    service_mut(cx)._probe_task = Some(task);
}

/// The last Claude Code check, and whether a new one is running.
pub fn availability(cx: &App) -> (Option<ClaudeAvailability>, bool) {
    service(cx).map_or((None, false), |s| (s.availability.clone(), s.probing))
}

/// Whether a guide generation is currently running for `pr`.
pub fn is_generating(cx: &App, pr: &PrRef) -> bool {
    service(cx).is_some_and(|s| s.runs.contains_key(pr))
}

/// Whether ANY guide generation is currently running (used by the sequential
/// saved-view auto-summarizer so only one PR generates at a time).
pub fn any_generating(cx: &App) -> bool {
    service(cx).is_some_and(|s| !s.runs.is_empty())
}

/// Returns whether a valid cached guide already exists for `(pr, head_oid)`.
pub fn has_fresh_guide(cx: &App, pr: &PrRef, head_oid: &str) -> bool {
    matches!(load_cached_commit(cx, pr, head_oid), Some(Ok(_)))
}

/// Returns whether `pr` has a cached guide for an older commit than `head_oid`.
pub fn has_stale_guide(cx: &App, pr: &PrRef, head_oid: &str) -> bool {
    load_latest_guide(cx, pr).is_some_and(|stored| stored.is_stale(head_oid))
}

/// When the most recent cached guide for `pr` was generated, if any.
pub fn latest_generated_at(cx: &App, pr: &PrRef) -> Option<DateTime<Utc>> {
    load_latest_guide(cx, pr).map(|stored| stored.generated_at)
}

/// Returns the cached guide for `pr` at `head_oid`, validated at the boundary.
fn load_cached_commit(cx: &App, pr: &PrRef, head_oid: &str) -> Option<Result<StoredGuide, String>> {
    let key = commit_cache_key(cx, pr, head_oid);
    let stored = crate::cache::get::<StoredGuide>(&key)?;
    if stored.schema_version != SCHEMA_VERSION {
        return Some(Err(format!(
            "Unsupported cached guide schema version {} (expected {SCHEMA_VERSION}).",
            stored.schema_version
        )));
    }
    match validate(&stored.guide) {
        Ok(()) => Some(Ok(stored)),
        Err(err) => Some(Err(err.to_string())),
    }
}

/// Persists a validated guide for `(pr, stored.head_oid)` and updates the PR's
/// latest/previous commit pointers for stale detection and step diffing.
pub fn store_guide(cx: &mut App, pr: &PrRef, stored: &StoredGuide) {
    let commit_key = commit_cache_key(cx, pr, &stored.head_oid);
    crate::cache::put(&commit_key, stored);

    let index_key = index_cache_key(cx, pr);
    let mut index = crate::cache::get::<PrGuideIndex>(&index_key).unwrap_or_default();
    if index.latest_head.as_deref() != Some(stored.head_oid.as_str()) {
        index.previous_head = index.latest_head.take();
        index.latest_head = Some(stored.head_oid.clone());
    }
    crate::cache::put(&index_key, &index);
    service_mut(cx).bump_revision();
}

/// Computes the current [`GuideViewState`] for `(pr, current_head_oid)`.
pub fn view_state(cx: &App, pr: &PrRef, current_head_oid: &str) -> GuideViewState {
    if let Some(run) = service(cx).and_then(|s| s.runs.get(pr)) {
        return GuideViewState::Generating {
            status: run.status.clone(),
            started_at: run.started_at,
        };
    }

    if let Some(err) = service(cx).and_then(|s| s.errors.get(pr)) {
        return GuideViewState::Invalid {
            message: err.clone(),
        };
    }

    if let Some(result) = load_cached_commit(cx, pr, current_head_oid) {
        return match result {
            Ok(stored) => {
                let changed_steps = diff_against_previous(cx, pr, &stored);
                GuideViewState::Ready {
                    stored: Box::new(stored),
                    changed_steps,
                }
            }
            Err(message) => GuideViewState::Invalid { message },
        };
    }

    // No guide for `current_head_oid` yet: check if we have a guide for an earlier commit
    // so the Guide tab can display it with a stale warning banner.
    if let Some(latest) = load_latest_guide(cx, pr) {
        return GuideViewState::Ready {
            stored: Box::new(latest),
            changed_steps: HashMap::new(),
        };
    }

    if let Some(avail) = service(cx).and_then(|s| s.availability.as_ref()) {
        match avail {
            ClaudeAvailability::NotInstalled { tried } => {
                return GuideViewState::Unavailable {
                    reason: format!("Could not find the Claude Code executable ({tried})."),
                };
            }
            ClaudeAvailability::NotLoggedIn { detail, .. } => {
                return GuideViewState::Unavailable {
                    reason: detail.clone(),
                };
            }
            ClaudeAvailability::Ready { .. } => {}
        }
    }

    GuideViewState::Empty
}

fn load_latest_guide(cx: &App, pr: &PrRef) -> Option<StoredGuide> {
    let index = crate::cache::get::<PrGuideIndex>(&index_cache_key(cx, pr))?;
    let head = index.latest_head?;
    load_cached_commit(cx, pr, &head)?.ok()
}

fn diff_against_previous(
    cx: &App,
    pr: &PrRef,
    current: &StoredGuide,
) -> HashMap<String, StepChange> {
    let Some(index) = crate::cache::get::<PrGuideIndex>(&index_cache_key(cx, pr)) else {
        return HashMap::new();
    };
    let Some(prev_head) = index.previous_head.as_deref() else {
        return HashMap::new();
    };
    if prev_head == current.head_oid {
        return HashMap::new();
    }
    match load_cached_commit(cx, pr, prev_head) {
        Some(Ok(prev)) => changed_steps_since(&prev.guide, &current.guide),
        _ => HashMap::new(),
    }
}

/// Starts local Claude Code generation for `detail`. Does nothing if a run is
/// already active for the same pull request.
pub fn start_generation(detail: Arc<PullRequestDetail>, cx: &mut App) {
    let pr = detail.pr.clone();
    if is_generating(cx, &pr) {
        return;
    }

    let config = Settings::get(cx).guide.claude_config();
    let github_client = Services::get(cx).github.clone();
    let github_token = service(cx).and_then(|s| s.github_token.clone());
    let previous_step_ids = load_latest_guide(cx, &pr)
        .map(|prev| prev.guide.steps.into_iter().map(|s| s.id).collect())
        .unwrap_or_default();
    let cached_files_key = crate::cache::github_key(
        cx,
        &format!(
            "files/{}/{}/{}/{}",
            pr.url(),
            detail.head_oid,
            detail.base_ref,
            detail.base_oid
        ),
    );
    let cached_files = cached_files_key
        .as_deref()
        .and_then(crate::cache::get::<Vec<ChangedFile>>);

    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<ProgressEvent>();

    let head_oid = detail.head_oid.clone();
    let base_oid = detail.base_oid.clone();
    let repo = pr.repo.to_string();
    let pr_number = pr.number;
    let effort_label = config.effort.as_cli_arg().to_owned();

    let worker_pr = pr.clone();
    let worker_detail = detail.clone();
    let worker = cx.background_executor().spawn(async move {
        let files = match cached_files {
            Some(files) => files,
            None => match github_client {
                Some(gh) => gh.pull_request_files(&worker_pr).unwrap_or_default(),
                None => Vec::new(),
            },
        };
        let changed_files: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let input = build_prompt_input(&worker_detail, &files, previous_step_ids);
        let outcome = generate_for_pr(
            &input,
            github_token.as_deref(),
            &config,
            &worker_cancel,
            |event| {
                let _ = tx.unbounded_send(event);
            },
        );
        drop(tx);
        (changed_files, outcome)
    });

    let pr_for_task = pr.clone();
    let task = cx.spawn(async move |cx| {
        while let Some(event) = rx.next().await {
            let message = event.message();
            let pr_key = pr_for_task.clone();
            cx.update(|cx| {
                let svc = service_mut(cx);
                if let Some(run) = svc.runs.get_mut(&pr_key) {
                    run.status = message;
                    svc.bump_revision();
                    cx.refresh_windows();
                }
            });
        }
        let (changed_files, outcome) = worker.await;
        cx.update(|cx| {
            let svc = service_mut(cx);
            let removed = svc.runs.remove(&pr_for_task);
            // If cancelled and already removed, ignore late completion.
            if removed.is_none() {
                cx.refresh_windows();
                return;
            }
            match outcome {
                Ok(generated) => {
                    svc.errors.remove(&pr_for_task);
                    let stored = StoredGuide {
                        schema_version: SCHEMA_VERSION,
                        repo,
                        pr_number,
                        head_oid,
                        base_oid,
                        model: generated.model,
                        effort: effort_label,
                        generated_at: Utc::now(),
                        changed_files,
                        guide: generated.guide,
                    };
                    store_guide(cx, &pr_for_task, &stored);
                }
                Err(GuideRunError::Cancelled) => {
                    svc.errors.remove(&pr_for_task);
                    svc.bump_revision();
                }
                Err(GuideRunError::NotInstalled { tried }) => {
                    svc.errors.remove(&pr_for_task);
                    svc.availability = Some(ClaudeAvailability::NotInstalled { tried });
                    svc.bump_revision();
                }
                Err(GuideRunError::NotLoggedIn(detail)) => {
                    svc.errors.remove(&pr_for_task);
                    svc.availability = Some(ClaudeAvailability::NotLoggedIn {
                        path: std::path::PathBuf::from("claude"),
                        detail,
                    });
                    svc.bump_revision();
                }
                Err(err) => {
                    svc.errors.insert(pr_for_task.clone(), err.to_string());
                    svc.bump_revision();
                }
            }
            cx.refresh_windows();
        });
    });

    let svc = service_mut(cx);
    svc.errors.remove(&pr);
    svc.runs.insert(
        pr,
        ActiveRun {
            status: ProgressEvent::PreparingCheckout.message(),
            started_at: Utc::now(),
            cancel,
            _task: task,
        },
    );
    svc.bump_revision();
    cx.refresh_windows();
}

/// Cancels an active generation run for `pr`, terminating Claude Code and
/// deleting the temporary checkout.
pub fn cancel_generation(pr: &PrRef, cx: &mut App) {
    let svc = service_mut(cx);
    if let Some(run) = svc.runs.remove(pr) {
        run.cancel.cancel();
    }
    svc.errors.remove(pr);
    svc.bump_revision();
    cx.refresh_windows();
}

/// Builds a [`GuidePromptInput`] from a loaded pull request detail and its changed files.
pub fn build_prompt_input(
    detail: &PullRequestDetail,
    files: &[ChangedFile],
    previous_step_ids: Vec<String>,
) -> GuidePromptInput {
    let author = detail
        .author
        .as_ref()
        .map_or_else(|| "ghost".to_owned(), |a| a.login.clone());
    let commits = detail
        .commits
        .iter()
        .map(|c| PromptCommit {
            oid: c.oid.clone(),
            headline: c.headline.clone(),
            body: c.body.clone(),
        })
        .collect();
    let mut comments = Vec::new();
    for item in &detail.timeline {
        match item {
            TimelineItem::Comment(comment)
                if !comment.hidden && !comment.body.trim().is_empty() =>
            {
                comments.push(PromptComment {
                    author: comment
                        .author
                        .as_ref()
                        .map_or_else(|| "ghost".to_owned(), |a| a.login.clone()),
                    body: comment.body.clone(),
                    path: None,
                    line: None,
                });
            }
            TimelineItem::Review(review) if !review.hidden && !review.body.trim().is_empty() => {
                comments.push(PromptComment {
                    author: review
                        .author
                        .as_ref()
                        .map_or_else(|| "ghost".to_owned(), |a| a.login.clone()),
                    body: review.body.clone(),
                    path: None,
                    line: None,
                });
            }
            _ => {}
        }
    }
    for thread in &detail.threads {
        for comment in &thread.comments {
            if comment.is_pending || comment.body.trim().is_empty() {
                continue;
            }
            comments.push(PromptComment {
                author: comment
                    .author
                    .as_ref()
                    .map_or_else(|| "ghost".to_owned(), |a| a.login.clone()),
                body: comment.body.clone(),
                path: Some(thread.path.clone()),
                line: thread.line.or(thread.original_line),
            });
        }
    }
    let files = files
        .iter()
        .map(|file| PromptFile {
            path: file.path.clone(),
            status: file_status_label(file.status).to_owned(),
            additions: file.additions,
            deletions: file.deletions,
            patch: file.patch.clone(),
        })
        .collect();

    GuidePromptInput {
        repo: detail.pr.repo.to_string(),
        pr_number: detail.pr.number,
        title: detail.title.clone(),
        body: detail.body.clone(),
        author,
        base_ref: detail.base_ref.clone(),
        base_oid: detail.base_oid.clone(),
        head_ref: detail.head_ref.clone(),
        head_oid: detail.head_oid.clone(),
        commits,
        comments,
        files,
        previous_step_ids,
    }
}

fn file_status_label(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "added",
        FileStatus::Removed => "removed",
        FileStatus::Renamed => "renamed",
        FileStatus::Copied => "copied",
        FileStatus::Modified | FileStatus::Changed => "modified",
        FileStatus::Unchanged => "unchanged",
    }
}

fn commit_cache_key(cx: &App, pr: &PrRef, head_oid: &str) -> String {
    let resource = format!("guide-v1/{}/{head_oid}", pr.url());
    crate::cache::github_key(cx, &resource).unwrap_or_else(|| format!("local/{resource}"))
}

fn index_cache_key(cx: &App, pr: &PrRef) -> String {
    let resource = format!("guide-index-v1/{}", pr.url());
    crate::cache::github_key(cx, &resource).unwrap_or_else(|| format!("local/{resource}"))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use gpui_kit::TestAppContext;
    use loupe_github::{PrRef, RepoRef};
    use loupe_guide::{
        GuideStep, GuideSummary, ReviewGuide, SCHEMA_VERSION, StepChange, StepKind, StoredGuide,
    };

    use super::{has_fresh_guide, store_guide, view_state};
    use crate::github::pull_request::guide::GuideViewState;
    use crate::services::Accounts;

    fn sample_stored(pr: &PrRef, head: &str, explanation: &str) -> StoredGuide {
        StoredGuide {
            schema_version: SCHEMA_VERSION,
            repo: pr.repo.to_string(),
            pr_number: pr.number,
            head_oid: head.into(),
            base_oid: "base000".into(),
            model: "claude-sonnet-5-5".into(),
            effort: "medium".into(),
            generated_at: Utc::now(),
            changed_files: Vec::new(),
            guide: ReviewGuide {
                schema_version: SCHEMA_VERSION,
                summary: GuideSummary {
                    headline: "Refactor guide cache".into(),
                    why: "Keep guides keyed by commit.".into(),
                    flow: None,
                    callers: Vec::new(),
                    contracts: Vec::new(),
                },
                decisions: Vec::new(),
                steps: vec![GuideStep {
                    id: "cache-layer".into(),
                    kind: StepKind::Subsystem,
                    title: "Cache by commit SHA".into(),
                    explanation: explanation.into(),
                    snippets: Vec::new(),
                }],
                removed_tests: Vec::new(),
            },
        }
    }

    #[gpui_kit::test]
    fn caches_by_commit_marks_stale_on_new_commit_and_diffs_steps_on_regeneration(
        cx: &mut TestAppContext,
    ) {
        let pr = PrRef::new(RepoRef::new("acme", "cache-test"), 700);
        cx.update(|cx| {
            cx.set_global(Accounts::default());
            let first = sample_stored(&pr, "commit_aaa1111", "Original explanation");
            store_guide(cx, &pr, &first);

            assert!(has_fresh_guide(cx, &pr, "commit_aaa1111"));
            assert!(!has_fresh_guide(cx, &pr, "commit_bbb2222"));

            // On commit_bbb2222, view_state serves the earlier guide so the UI marks it stale.
            match view_state(cx, &pr, "commit_bbb2222") {
                GuideViewState::Ready { stored, .. } => {
                    assert!(stored.is_stale("commit_bbb2222"));
                    assert_eq!(stored.head_oid, "commit_aaa1111");
                }
                other => panic!("expected Ready stale guide, got {other:?}"),
            }

            // Once regenerated for commit_bbb2222, changed steps are highlighted.
            let mut second = sample_stored(&pr, "commit_bbb2222", "Updated explanation");
            second.guide.steps.push(GuideStep {
                id: "new-step".into(),
                kind: StepKind::Tests,
                title: "Added test coverage".into(),
                explanation: "Covers commit invalidation.".into(),
                snippets: Vec::new(),
            });
            store_guide(cx, &pr, &second);

            match view_state(cx, &pr, "commit_bbb2222") {
                GuideViewState::Ready {
                    stored,
                    changed_steps,
                } => {
                    assert!(!stored.is_stale("commit_bbb2222"));
                    assert_eq!(changed_steps.get("cache-layer"), Some(&StepChange::Updated));
                    assert_eq!(changed_steps.get("new-step"), Some(&StepChange::New));
                }
                other => panic!("expected Ready fresh guide, got {other:?}"),
            }
        });
    }
}
