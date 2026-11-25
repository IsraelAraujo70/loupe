//! Local guided review coordinator: per-commit disk cache, Claude Code
//! availability probe, active generation runs with live progress and
//! cancellation, and cross-commit step change detection.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::StreamExt as _;
use gpui_kit::{App, Global, Task};
use loupe_github::{
    ChangedFile, FileStatus, PrRef, PullRequestDetail, StackGraph, StackNode, TimelineItem,
};
use loupe_guide::{
    CancelToken, ChapterRequest, ClaudeAvailability, GeneratedStack, GuideLanguage,
    GuidePromptInput, GuideRunError, ProgressEvent, PromptComment, PromptCommit, PromptFile,
    SCHEMA_VERSION, STACK_SCHEMA_VERSION, StackMember, StepChange, StoredGuide, StoredStackGuide,
    changed_steps_since, check_availability, generate_for_pr, generate_stack, validate,
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
    /// Stack guide runs by the stack's root pull request.
    stack_runs: HashMap<PrRef, StackRun>,
    /// Failed stack guide runs by the stack's root pull request.
    stack_errors: HashMap<PrRef, String>,
    /// "Summarize this PR only" (or back to the stack) picked from Summarize's
    /// menu for one pull request in this session.
    scope_override: HashMap<PrRef, GuideScope>,
    /// Languages picked from Summarize's menu for one pull request in this
    /// session, instead of the one Settings resolves to.
    language_override: HashMap<PrRef, GuideLanguage>,
    /// Monotonic counter bumped whenever availability, runs, errors, or stored
    /// guides change so views only recompute `view_state` when needed.
    revision: u64,
}

impl Global for GuideService {}

/// A stack guide run and the pull requests it writes chapters for.
struct StackRun {
    members: Vec<PrRef>,
    run: ActiveRun,
}

/// Which guide Summarize writes and the Guide tab shows for a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GuideScope {
    /// One guide across every pull request of the stack it belongs to.
    Stack,
    /// A guide for this pull request alone.
    PullRequest,
}

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

/// Whether a guide generation, its own or its stack's, is running for `pr`.
pub fn is_generating(cx: &App, pr: &PrRef) -> bool {
    service(cx).is_some_and(|s| {
        s.runs.contains_key(pr) || s.stack_runs.values().any(|run| run.members.contains(pr))
    })
}

/// Whether ANY guide generation is currently running (used by the sequential
/// saved-view auto-summarizer so only one PR generates at a time).
pub fn any_generating(cx: &App) -> bool {
    service(cx).is_some_and(|s| !s.runs.is_empty() || !s.stack_runs.is_empty())
}

/// The scope Summarize and the Guide tab use for `pr`: its stack when it is
/// in one, unless "Summarize this PR only" was picked for it this session.
pub fn wanted_scope(cx: &App, pr: &PrRef, stack: Option<&StackGraph>) -> GuideScope {
    if stack.is_none_or(|graph| graph.len() < 2 || !graph.contains(pr.number)) {
        return GuideScope::PullRequest;
    }
    service(cx)
        .and_then(|s| s.scope_override.get(pr).copied())
        .unwrap_or(GuideScope::Stack)
}

/// Summarizes `pr` in `scope` for the rest of this session.
pub fn set_scope_for(pr: &PrRef, scope: GuideScope, cx: &mut App) {
    let svc = service_mut(cx);
    if scope == GuideScope::Stack {
        svc.scope_override.remove(pr);
    } else {
        svc.scope_override.insert(pr.clone(), scope);
    }
    svc.bump_revision();
}

/// Whether the guide `detail`'s scope asks for is up to date.
pub fn has_fresh_scoped_guide(
    cx: &App,
    detail: &PullRequestDetail,
    stack: Option<&StackGraph>,
) -> bool {
    match (wanted_scope(cx, &detail.pr, stack), stack) {
        (GuideScope::Stack, Some(graph)) => {
            let language = wanted_language(cx, detail);
            matches!(
                load_stack_guide(cx, &detail.pr, graph, &language),
                Some(Ok(stored)) if stored.changes(&stack_members(graph, &detail.pr, &detail.head_oid)).is_empty()
            )
        }
        _ => has_fresh_guide(cx, detail),
    }
}

/// Writes the guide `detail`'s scope asks for: its stack's or its own.
pub fn start_scoped_generation(
    detail: Arc<PullRequestDetail>,
    stack: Option<&StackGraph>,
    cx: &mut App,
) {
    match (wanted_scope(cx, &detail.pr, stack), stack) {
        (GuideScope::Stack, Some(graph)) => start_stack_generation(detail, graph, cx),
        _ => start_generation(detail, cx),
    }
}

/// The stack's pull requests in the order its guide's chapters follow: the
/// order the stack merges (base first; in a tree, children before the root).
pub fn chapter_order(graph: &StackGraph) -> Vec<&StackNode> {
    graph
        .merge_order()
        .into_iter()
        .map(|item| &item.node)
        .collect()
}

/// The stack's pull requests and their head commits now, taking `head_oid`
/// for `pr`, the one on screen (fresher than the stack lookup).
fn stack_members<'a>(graph: &'a StackGraph, pr: &PrRef, head_oid: &'a str) -> Vec<StackMember<'a>> {
    chapter_order(graph)
        .into_iter()
        .map(|node| StackMember {
            pr_number: node.pr.number,
            head_oid: if node.pr == *pr {
                head_oid
            } else {
                node.head_oid.as_str()
            },
        })
        .collect()
}

/// The language the Guide tab shows and generates for `detail`: the one picked
/// from Summarize's menu for this pull request in this session, else the one
/// Settings resolves to for its title and description.
pub fn wanted_language(cx: &App, detail: &PullRequestDetail) -> GuideLanguage {
    if let Some(language) = service(cx).and_then(|s| s.language_override.get(&detail.pr)) {
        return language.clone();
    }
    configured_language(cx, detail)
}

fn configured_language(cx: &App, detail: &PullRequestDetail) -> GuideLanguage {
    Settings::get(cx)
        .guide
        .language
        .resolve(&detail.title, &detail.body)
}

/// Languages Summarize's menu offers for `detail`: the one Settings resolves
/// to first, then English, Brazilian Portuguese, and the pull request's own.
pub fn language_choices(cx: &App, detail: &PullRequestDetail) -> Vec<GuideLanguage> {
    let mut choices = vec![configured_language(cx, detail)];
    for language in [
        GuideLanguage::english(),
        GuideLanguage::portuguese_brazil(),
        loupe_guide::detect_language(&detail.title, &detail.body),
    ] {
        if !choices.contains(&language) {
            choices.push(language);
        }
    }
    choices
}

/// Shows and generates `detail`'s guide in `language` for the rest of this
/// session, without changing Settings.
pub fn set_language_for(detail: &PullRequestDetail, language: GuideLanguage, cx: &mut App) {
    let configured = configured_language(cx, detail);
    let svc = service_mut(cx);
    if language == configured {
        svc.language_override.remove(&detail.pr);
    } else {
        svc.language_override.insert(detail.pr.clone(), language);
    }
    svc.bump_revision();
}

/// Returns whether a valid cached guide in the wanted language exists for
/// `detail`'s head commit.
pub fn has_fresh_guide(cx: &App, detail: &PullRequestDetail) -> bool {
    let language = wanted_language(cx, detail);
    matches!(
        load_cached_commit(cx, &detail.pr, &detail.head_oid, &language),
        Some(Ok(_))
    )
}

/// Returns whether `detail` has a cached guide in the wanted language for an
/// older commit than its head.
pub fn has_stale_guide(cx: &App, detail: &PullRequestDetail) -> bool {
    let language = wanted_language(cx, detail);
    load_latest_guide(cx, &detail.pr, &language)
        .is_some_and(|stored| stored.is_stale(&detail.head_oid))
}

/// When the most recent cached guide for `pr` in `language` was generated, if any.
pub fn latest_generated_at(
    cx: &App,
    pr: &PrRef,
    language: &GuideLanguage,
) -> Option<DateTime<Utc>> {
    load_latest_guide(cx, pr, language).map(|stored| stored.generated_at)
}

/// Returns the cached guide for `pr` at `head_oid` in `language`, validated at
/// the boundary.
fn load_cached_commit(
    cx: &App,
    pr: &PrRef,
    head_oid: &str,
    language: &GuideLanguage,
) -> Option<Result<StoredGuide, String>> {
    let key = commit_cache_key(cx, pr, head_oid, language);
    let stored = crate::cache::get::<StoredGuide>(&key)?;
    if stored.language != *language {
        return None;
    }
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
    let commit_key = commit_cache_key(cx, pr, &stored.head_oid, &stored.language);
    crate::cache::put(&commit_key, stored);

    let index_key = index_cache_key(cx, pr, &stored.language);
    let mut index = crate::cache::get::<PrGuideIndex>(&index_key).unwrap_or_default();
    if index.latest_head.as_deref() != Some(stored.head_oid.as_str()) {
        index.previous_head = index.latest_head.take();
        index.latest_head = Some(stored.head_oid.clone());
    }
    crate::cache::put(&index_key, &index);
    service_mut(cx).bump_revision();
}

/// Computes the current [`GuideViewState`] for `detail` in the wanted language
/// and scope. A pull request in a stack with no stack guide yet keeps showing
/// a guide written for it alone.
pub fn view_state(
    cx: &App,
    detail: &PullRequestDetail,
    stack: Option<&StackGraph>,
) -> GuideViewState {
    let language = wanted_language(cx, detail);
    let alone = || view_state_in(cx, &detail.pr, &detail.head_oid, &language);
    match (wanted_scope(cx, &detail.pr, stack), stack) {
        (GuideScope::Stack, Some(graph)) => {
            let state = stack_view_state(cx, &detail.pr, &detail.head_oid, graph, &language);
            if matches!(
                state,
                GuideViewState::Empty | GuideViewState::Unavailable { .. }
            ) && let ready @ GuideViewState::Ready { .. } = alone()
            {
                return ready;
            }
            state
        }
        _ => alone(),
    }
}

/// [`GuideViewState`] for the stack guide `detail`'s pull request belongs to.
fn stack_view_state(
    cx: &App,
    pr: &PrRef,
    head_oid: &str,
    graph: &StackGraph,
    language: &GuideLanguage,
) -> GuideViewState {
    let svc = service(cx);
    if let Some(run) = svc.and_then(|s| s.stack_runs.values().find(|run| run.members.contains(pr)))
    {
        return GuideViewState::Generating {
            status: run.run.status.clone(),
            started_at: run.run.started_at,
        };
    }
    let root = PrRef::new(pr.repo.clone(), graph.root);
    if let Some(message) = svc.and_then(|s| s.stack_errors.get(&root)) {
        return GuideViewState::Invalid {
            message: message.clone(),
        };
    }
    match load_stack_guide(cx, pr, graph, language) {
        Some(Ok(stored)) => {
            let changes = stored.changes(&stack_members(graph, pr, head_oid));
            GuideViewState::StackReady {
                stored: Box::new(stored),
                changes,
                nodes: chapter_order(graph).into_iter().cloned().collect(),
            }
        }
        Some(Err(message)) => GuideViewState::Invalid { message },
        None => unavailable_or_empty(cx),
    }
}

fn unavailable_or_empty(cx: &App) -> GuideViewState {
    match service(cx).and_then(|s| s.availability.as_ref()) {
        Some(ClaudeAvailability::NotInstalled { tried }) => GuideViewState::Unavailable {
            reason: format!("Could not find the Claude Code executable ({tried})."),
        },
        Some(ClaudeAvailability::NotLoggedIn { detail, .. }) => GuideViewState::Unavailable {
            reason: detail.clone(),
        },
        _ => GuideViewState::Empty,
    }
}

/// [`GuideViewState`] for `(pr, current_head_oid)` in `language`. A guide in
/// another language never shows: the tab offers Summarize instead.
fn view_state_in(
    cx: &App,
    pr: &PrRef,
    current_head_oid: &str,
    language: &GuideLanguage,
) -> GuideViewState {
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

    if let Some(result) = load_cached_commit(cx, pr, current_head_oid, language) {
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
    if let Some(latest) = load_latest_guide(cx, pr, language) {
        return GuideViewState::Ready {
            stored: Box::new(latest),
            changed_steps: HashMap::new(),
        };
    }

    unavailable_or_empty(cx)
}

fn load_latest_guide(cx: &App, pr: &PrRef, language: &GuideLanguage) -> Option<StoredGuide> {
    let index = crate::cache::get::<PrGuideIndex>(&index_cache_key(cx, pr, language))?;
    let head = index.latest_head?;
    load_cached_commit(cx, pr, &head, language)?.ok()
}

fn diff_against_previous(
    cx: &App,
    pr: &PrRef,
    current: &StoredGuide,
) -> HashMap<String, StepChange> {
    let Some(index) =
        crate::cache::get::<PrGuideIndex>(&index_cache_key(cx, pr, &current.language))
    else {
        return HashMap::new();
    };
    let Some(prev_head) = index.previous_head.as_deref() else {
        return HashMap::new();
    };
    if prev_head == current.head_oid {
        return HashMap::new();
    }
    match load_cached_commit(cx, pr, prev_head, &current.language) {
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
    let language = wanted_language(cx, &detail);
    let github_client = Services::get(cx).github.clone();
    let github_token = service(cx).and_then(|s| s.github_token.clone());
    let previous_step_ids = load_latest_guide(cx, &pr, &language)
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
    let worker_language = language.clone();

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
        let input = build_prompt_input(&worker_detail, &files, previous_step_ids, worker_language);
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
                        language,
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

/// Loads the stack guide `pr` has a chapter in (or, for a pull request that
/// joined the stack later, the one of `graph`'s root), validated at the boundary.
fn load_stack_guide(
    cx: &App,
    pr: &PrRef,
    graph: &StackGraph,
    language: &GuideLanguage,
) -> Option<Result<StoredStackGuide, String>> {
    let root = crate::cache::get::<u64>(&stack_member_key(cx, pr, language))
        .into_iter()
        .chain([graph.root]);
    let stored = root
        .map(|root| PrRef::new(pr.repo.clone(), root))
        .find_map(|root| {
            crate::cache::get::<StoredStackGuide>(&stack_cache_key(cx, &root, language))
        })?;
    if stored.language != *language {
        return None;
    }
    if stored.schema_version != STACK_SCHEMA_VERSION {
        return Some(Err(format!(
            "Unsupported cached stack guide schema version {} (expected {STACK_SCHEMA_VERSION}).",
            stored.schema_version
        )));
    }
    Some(
        stored
            .validate()
            .map(|()| stored)
            .map_err(|err| err.to_string()),
    )
}

/// Persists a stack guide under its root and points each chapter's pull
/// request at it, so the Guide tab of every pull request in the stack finds it.
pub fn store_stack_guide(cx: &mut App, root: &PrRef, stored: &StoredStackGuide) {
    crate::cache::put(&stack_cache_key(cx, root, &stored.language), stored);
    for chapter in &stored.chapters {
        let member = PrRef::new(root.repo.clone(), chapter.pr_number);
        crate::cache::put(
            &stack_member_key(cx, &member, &stored.language),
            &root.number,
        );
    }
    service_mut(cx).bump_revision();
}

/// Starts writing the stack guide for `graph`, which `detail` belongs to:
/// one chapter per pull request in merge order, then the overview. When the
/// cached guide is outdated, chapters still written for their pull request's
/// head are kept and only the changed ones are written again.
pub fn start_stack_generation(detail: Arc<PullRequestDetail>, graph: &StackGraph, cx: &mut App) {
    let root = PrRef::new(detail.pr.repo.clone(), graph.root);
    if service(cx).is_some_and(|s| s.stack_runs.contains_key(&root)) {
        return;
    }
    let config = Settings::get(cx).guide.claude_config();
    let language = wanted_language(cx, &detail);
    let github_client = Services::get(cx).github.clone();
    let github_token = service(cx).and_then(|s| s.github_token.clone());
    let previous = match load_stack_guide(cx, &detail.pr, graph, &language) {
        Some(Ok(stored)) => Some(stored),
        _ => None,
    };
    let outdated = previous.as_ref().is_some_and(|stored| {
        !stored
            .changes(&stack_members(graph, &detail.pr, &detail.head_oid))
            .is_empty()
    });
    let members: Vec<PrRef> = chapter_order(graph)
        .into_iter()
        .map(|node| node.pr.clone())
        .collect();
    let base_ref = graph.base_ref.clone();
    let effort_label = config.effort.as_cli_arg().to_owned();

    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
    let worker_members = members.clone();
    let worker_language = language.clone();
    let worker_base = base_ref.clone();
    let worker = cx.background_executor().spawn(async move {
        let Some(gh) = github_client else {
            return StackOutcome::LoadFailed("Sign in to GitHub to summarize a stack.".into());
        };
        let mut requests = Vec::with_capacity(worker_members.len());
        for pr in &worker_members {
            if worker_cancel.is_cancelled() {
                return StackOutcome::Run(Err(GuideRunError::Cancelled));
            }
            let _ = tx.unbounded_send(format!("Loading #{}…", pr.number));
            let member = if *pr == detail.pr {
                detail.clone()
            } else {
                match gh.pull_request(pr) {
                    Ok(member) => Arc::new(member),
                    Err(err) => {
                        return StackOutcome::LoadFailed(format!(
                            "Could not load #{}: {err}",
                            pr.number
                        ));
                    }
                }
            };
            let files = gh.pull_request_files(pr).unwrap_or_default();
            let reuse = previous
                .as_ref()
                .filter(|_| outdated)
                .and_then(|stored| stored.chapter(pr.number))
                .map(|(_, chapter)| chapter.clone());
            let previous_step_ids = previous
                .as_ref()
                .and_then(|stored| stored.chapter(pr.number))
                .map(|(_, chapter)| chapter.guide.steps.iter().map(|s| s.id.clone()).collect())
                .unwrap_or_default();
            requests.push(ChapterRequest {
                input: build_prompt_input(
                    &member,
                    &files,
                    previous_step_ids,
                    worker_language.clone(),
                ),
                changed_files: files.iter().map(|f| f.path.clone()).collect(),
                reuse,
            });
        }
        StackOutcome::Run(generate_stack(
            requests,
            &worker_base,
            &worker_language,
            github_token.as_deref(),
            &config,
            &worker_cancel,
            |event| {
                let _ = tx.unbounded_send(event.message());
            },
        ))
    });

    let task_root = root.clone();
    let task = cx.spawn(async move |cx| {
        while let Some(message) = rx.next().await {
            let root = task_root.clone();
            cx.update(|cx| {
                let svc = service_mut(cx);
                if let Some(run) = svc.stack_runs.get_mut(&root) {
                    run.run.status = message;
                    svc.bump_revision();
                    cx.refresh_windows();
                }
            });
        }
        let outcome = worker.await;
        cx.update(|cx| {
            let svc = service_mut(cx);
            if svc.stack_runs.remove(&task_root).is_none() {
                cx.refresh_windows();
                return;
            }
            svc.stack_errors.remove(&task_root);
            match outcome {
                StackOutcome::Run(Ok(generated)) => {
                    let GeneratedStack {
                        overview,
                        chapters,
                        model,
                    } = generated;
                    let stored = StoredStackGuide {
                        schema_version: STACK_SCHEMA_VERSION,
                        repo: task_root.repo.to_string(),
                        base_ref,
                        model,
                        effort: effort_label,
                        generated_at: Utc::now(),
                        language,
                        overview,
                        chapters,
                    };
                    store_stack_guide(cx, &task_root, &stored);
                }
                StackOutcome::Run(Err(GuideRunError::Cancelled)) => svc.bump_revision(),
                StackOutcome::Run(Err(GuideRunError::NotInstalled { tried })) => {
                    svc.availability = Some(ClaudeAvailability::NotInstalled { tried });
                    svc.bump_revision();
                }
                StackOutcome::Run(Err(GuideRunError::NotLoggedIn(detail))) => {
                    svc.availability = Some(ClaudeAvailability::NotLoggedIn {
                        path: std::path::PathBuf::from("claude"),
                        detail,
                    });
                    svc.bump_revision();
                }
                StackOutcome::Run(Err(err)) => {
                    svc.stack_errors.insert(task_root.clone(), err.to_string());
                    svc.bump_revision();
                }
                StackOutcome::LoadFailed(message) => {
                    svc.stack_errors.insert(task_root.clone(), message);
                    svc.bump_revision();
                }
            }
            cx.refresh_windows();
        });
    });

    let svc = service_mut(cx);
    svc.stack_errors.remove(&root);
    svc.stack_runs.insert(
        root,
        StackRun {
            members,
            run: ActiveRun {
                status: "Loading the stack's pull requests…".into(),
                started_at: Utc::now(),
                cancel,
                _task: task,
            },
        },
    );
    svc.bump_revision();
    cx.refresh_windows();
}

/// How a stack guide run ended.
enum StackOutcome {
    Run(Result<GeneratedStack, GuideRunError>),
    /// A pull request of the stack could not be loaded from GitHub.
    LoadFailed(String),
}

/// Cancels an active generation run for `pr`, terminating Claude Code and
/// deleting the temporary checkout.
pub fn cancel_generation(pr: &PrRef, cx: &mut App) {
    let svc = service_mut(cx);
    if let Some(run) = svc.runs.remove(pr) {
        run.cancel.cancel();
    }
    svc.errors.remove(pr);
    let stacks: Vec<PrRef> = svc
        .stack_runs
        .iter()
        .filter(|(_, run)| run.members.contains(pr))
        .map(|(root, _)| root.clone())
        .collect();
    for root in stacks {
        if let Some(run) = svc.stack_runs.remove(&root) {
            run.run.cancel.cancel();
        }
        svc.stack_errors.remove(&root);
    }
    svc.bump_revision();
    cx.refresh_windows();
}

/// Builds a [`GuidePromptInput`] from a loaded pull request detail and its changed files.
pub fn build_prompt_input(
    detail: &PullRequestDetail,
    files: &[ChangedFile],
    previous_step_ids: Vec<String>,
    language: GuideLanguage,
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
        language,
        stack: None,
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

fn commit_cache_key(cx: &App, pr: &PrRef, head_oid: &str, language: &GuideLanguage) -> String {
    let resource = format!(
        "guide-v1/{}/{head_oid}{}",
        pr.url(),
        language_segment(language)
    );
    crate::cache::github_key(cx, &resource).unwrap_or_else(|| format!("local/{resource}"))
}

fn stack_cache_key(cx: &App, root: &PrRef, language: &GuideLanguage) -> String {
    let resource = format!(
        "guide-stack-v1/{}{}",
        root.url(),
        language_segment(language)
    );
    crate::cache::github_key(cx, &resource).unwrap_or_else(|| format!("local/{resource}"))
}

/// Points a pull request at the root its stack guide is stored under.
fn stack_member_key(cx: &App, pr: &PrRef, language: &GuideLanguage) -> String {
    let resource = format!(
        "guide-stack-member-v1/{}{}",
        pr.url(),
        language_segment(language)
    );
    crate::cache::github_key(cx, &resource).unwrap_or_else(|| format!("local/{resource}"))
}

fn index_cache_key(cx: &App, pr: &PrRef, language: &GuideLanguage) -> String {
    let resource = format!("guide-index-v1/{}{}", pr.url(), language_segment(language));
    crate::cache::github_key(cx, &resource).unwrap_or_else(|| format!("local/{resource}"))
}

/// The cache keys' language part. English has none: guides saved before
/// Loupe asked for a language were English and keep loading from their keys.
fn language_segment(language: &GuideLanguage) -> String {
    if language.is_english() {
        String::new()
    } else {
        format!("/lang/{}", language.name().to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use gpui_kit::TestAppContext;
    use loupe_github::{PrRef, RepoRef};
    use loupe_guide::{
        GuideLanguage, GuideStep, GuideSummary, ReviewGuide, SCHEMA_VERSION, StepChange, StepKind,
        StoredGuide,
    };

    use super::{
        chapter_order, load_cached_commit, stack_view_state, store_guide, store_stack_guide,
        view_state_in,
    };
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
            language: GuideLanguage::english(),
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
        let english = GuideLanguage::english();
        let has_fresh_guide = |cx: &gpui_kit::App, pr: &PrRef, head: &str| {
            matches!(load_cached_commit(cx, pr, head, &english), Some(Ok(_)))
        };
        let view_state = |cx: &gpui_kit::App, pr: &PrRef, head: &str| {
            view_state_in(cx, pr, head, &GuideLanguage::english())
        };
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

    #[gpui_kit::test]
    fn guides_are_cached_per_language_and_never_shown_in_another_one(cx: &mut TestAppContext) {
        let pr = PrRef::new(RepoRef::new("acme", "language-test"), 701);
        let portuguese = GuideLanguage::portuguese_brazil();
        let english = GuideLanguage::english();
        cx.update(|cx| {
            cx.set_global(Accounts::default());
            store_guide(cx, &pr, &sample_stored(&pr, "commit_ccc3333", "In English"));

            // Asking for Portuguese never serves the English guide, fresh or stale.
            assert!(load_cached_commit(cx, &pr, "commit_ccc3333", &portuguese).is_none());
            assert_eq!(
                view_state_in(cx, &pr, "commit_ccc3333", &portuguese),
                GuideViewState::Empty
            );
            assert_eq!(
                view_state_in(cx, &pr, "commit_ddd4444", &portuguese),
                GuideViewState::Empty
            );

            let mut localized = sample_stored(&pr, "commit_ccc3333", "Em português");
            localized.language = portuguese.clone();
            store_guide(cx, &pr, &localized);

            for (language, explanation) in [(&portuguese, "Em português"), (&english, "In English")]
            {
                match view_state_in(cx, &pr, "commit_ccc3333", language) {
                    GuideViewState::Ready {
                        stored,
                        changed_steps,
                    } => {
                        assert_eq!(stored.language, *language);
                        assert_eq!(stored.guide.steps[0].explanation, explanation);
                        assert!(changed_steps.is_empty());
                    }
                    other => panic!("expected the {} guide, got {other:?}", language.name()),
                }
            }
        });
    }

    fn stack_node(number: u64, base: &str, head: &str, head_oid: &str) -> loupe_github::StackNode {
        loupe_github::StackNode {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new("acme", "stack-test"), number),
            title: format!("PR {number}"),
            state: loupe_github::PrState::Open,
            is_draft: false,
            base_ref: base.into(),
            head_ref: head.into(),
            head_oid: head_oid.into(),
            cross_repository: false,
            mergeable: loupe_github::Mergeable::Mergeable,
            merge_state: loupe_github::MergeStateStatus::Clean,
            checks: Some(loupe_github::ChecksState::Passing),
            review_decision: None,
            base_requires_review: false,
            base_requires_checks: false,
            viewer_can_update: true,
            stack: None,
        }
    }

    fn graph(root: u64, nodes: Vec<loupe_github::StackNode>) -> loupe_github::StackGraph {
        loupe_github::StackGraph::build(root, nodes, Some("main".into()), Vec::new())
            .expect("a stack")
    }

    fn numbers(graph: &loupe_github::StackGraph) -> Vec<u64> {
        chapter_order(graph)
            .iter()
            .map(|node| node.pr.number)
            .collect()
    }

    /// #144: chapters follow the order the stack merges, from the base up; in
    /// a tree, the children before the root.
    #[test]
    fn chapters_follow_the_stack_merge_order_for_chains_and_trees() {
        // Listed out of order on purpose: #30 -> #20 -> #10 -> main.
        let chain = graph(
            10,
            vec![
                stack_node(30, "feat/b", "feat/c", "c1"),
                stack_node(10, "main", "feat/a", "a1"),
                stack_node(20, "feat/a", "feat/b", "b1"),
            ],
        );
        assert_eq!(numbers(&chain), vec![10, 20, 30]);

        // Root #1 into main; #2 and #3 into the root; #4 into #2.
        let tree = graph(
            1,
            vec![
                stack_node(1, "main", "root", "r1"),
                stack_node(2, "root", "left", "l1"),
                stack_node(3, "root", "right", "g1"),
                stack_node(4, "left", "leaf", "f1"),
            ],
        );
        assert_eq!(numbers(&tree), vec![4, 2, 3, 1]);

        for stack in [&chain, &tree] {
            let merges: Vec<u64> = stack
                .merge_plan()
                .steps
                .iter()
                .map(|step| step.node.pr.number)
                .collect();
            assert_eq!(numbers(stack), merges, "chapters read in merge order");
        }
    }

    fn stack_guide(chapters: &[(u64, &str)]) -> loupe_guide::StoredStackGuide {
        loupe_guide::StoredStackGuide {
            schema_version: loupe_guide::STACK_SCHEMA_VERSION,
            repo: "acme/stack-test".into(),
            base_ref: "main".into(),
            model: "claude-sonnet-5-5".into(),
            effort: "medium".into(),
            generated_at: Utc::now(),
            language: GuideLanguage::english(),
            overview: loupe_guide::StackOverview {
                headline: "Ships it in three steps".into(),
                why: "Each builds on the last.".into(),
            },
            chapters: chapters
                .iter()
                .map(|(number, head)| {
                    let pr = PrRef::new(RepoRef::new("acme", "stack-test"), *number);
                    loupe_guide::StackChapter {
                        pr_number: *number,
                        title: format!("PR {number}"),
                        base_ref: "main".into(),
                        head_oid: (*head).into(),
                        base_oid: "base".into(),
                        changed_files: Vec::new(),
                        guide: sample_stored(&pr, head, "Chapter").guide,
                    }
                })
                .collect(),
        }
    }

    /// #144: every pull request of the stack finds the stack guide, and a new
    /// commit in one of them marks only its chapter as changed.
    #[gpui_kit::test]
    fn every_pull_request_finds_the_stack_guide_and_a_push_marks_only_its_chapter(
        cx: &mut TestAppContext,
    ) {
        let english = GuideLanguage::english();
        let pr = |number| PrRef::new(RepoRef::new("acme", "stack-test"), number);
        let nodes = |b_head: &str| {
            vec![
                stack_node(10, "main", "feat/a", "a1"),
                stack_node(20, "feat/a", "feat/b", b_head),
                stack_node(30, "feat/b", "feat/c", "c1"),
            ]
        };
        cx.update(|cx| {
            cx.set_global(Accounts::default());
            store_stack_guide(
                cx,
                &pr(10),
                &stack_guide(&[(10, "a1"), (20, "b1"), (30, "c1")]),
            );

            let stack = graph(10, nodes("b1"));
            for (number, head) in [(10, "a1"), (20, "b1"), (30, "c1")] {
                match stack_view_state(cx, &pr(number), head, &stack, &english) {
                    GuideViewState::StackReady {
                        stored,
                        changes,
                        nodes,
                    } => {
                        assert_eq!(stored.chapters.len(), 3);
                        assert!(changes.is_empty(), "#{number} sees an up-to-date guide");
                        assert_eq!(
                            nodes.iter().map(|n| n.pr.number).collect::<Vec<_>>(),
                            vec![10, 20, 30]
                        );
                    }
                    other => panic!("#{number} expected the stack guide, got {other:?}"),
                }
            }

            // #20 gets a new commit; seen from #30 (stack lookup) and from #20 itself.
            let pushed = graph(10, nodes("b2"));
            for (number, head) in [(30, "c1"), (20, "b2")] {
                match stack_view_state(cx, &pr(number), head, &pushed, &english) {
                    GuideViewState::StackReady { changes, .. } => assert_eq!(
                        changes.into_iter().collect::<Vec<_>>(),
                        vec![(20, loupe_guide::ChapterChange::NewCommits)]
                    ),
                    other => panic!("expected an outdated stack guide, got {other:?}"),
                }
            }

            // Another language never shows it.
            assert_eq!(
                stack_view_state(
                    cx,
                    &pr(20),
                    "b1",
                    &stack,
                    &GuideLanguage::portuguese_brazil()
                ),
                GuideViewState::Empty
            );
        });
    }
}
