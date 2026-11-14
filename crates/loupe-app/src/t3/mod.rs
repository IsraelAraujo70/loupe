//! T3 Code: the threads linked to a pull request, each opening in the T3 Code
//! desktop app, and "Review with T3 Code", which starts a review thread on
//! someone else's pull request. Loupe does not read transcripts. Owned by
//! the T3 front.

mod dialog;
mod panel;
mod review;

pub use review::{
    DEFAULT_PROMPT, OPERATE_HINT, ReviewPrompt, ReviewSettings, RiskList, VARIABLES, format_list,
    model_choice, model_summary, unknown_variables,
};

use gpui_kit::{AnyView, App, Task, Window};

use crate::links::PrLinkContext;

/// The T3 threads linked to a pull request, for the PR view's side column.
pub fn pr_linked_threads(context: PrLinkContext, window: &mut Window, cx: &mut App) -> AnyView {
    panel::LinkedThreads::view(context, window, cx)
}

/// The T3 Code deep link of the thread linked to `pr` that changed last,
/// or `None` when no thread is linked or T3 cannot be reached. `None` at
/// once when T3 is not connected.
pub fn latest_thread_link(pr: &loupe_github::PrRef, cx: &mut App) -> Option<Task<Option<String>>> {
    let t3 = crate::services::Services::get(cx).t3.clone()?;
    let key = loupe_t3::PullRequestKey::github(pr.repo.to_string(), pr.number);
    Some(crate::services::background(cx, move || {
        let environment = t3.environment().ok()?;
        let thread = t3
            .linked_threads(&key)
            .ok()?
            .into_iter()
            .max_by_key(|thread| thread.updated_at)?;
        Some(loupe_t3::thread_deep_link(&environment.id, &thread.id))
    }))
}
