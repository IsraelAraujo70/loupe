//! `Route::PullRequest`: header, tabs, review bar, actions, side column.

pub(crate) mod checks;
mod commits;
mod conversation;
mod files;
pub(crate) mod guide;
mod tab;
mod thread;
mod view;

use gpui_kit::{AnyView, App, AppContext as _, Global, Window};
use loupe_github::{PrRef, PullRequestDetail};

use view::PullRequestView;

/// One-shot intent set when the user picks "Merge stack" from a stack row's
/// context menu in Reviews: the root PR view consumes it once the stack loads
/// and opens the Merge stack confirmation panel right away.
#[derive(Default)]
struct PendingStackMergeConfirm {
    pr: Option<PrRef>,
}

impl Global for PendingStackMergeConfirm {}

pub(in crate::github) fn request_stack_merge_confirm(pr: PrRef, cx: &mut App) {
    cx.default_global::<PendingStackMergeConfirm>().pr = Some(pr);
}

pub(super) fn take_stack_merge_confirm(pr: &PrRef, cx: &mut App) -> bool {
    let Some(pending) = cx.try_global::<PendingStackMergeConfirm>() else {
        return false;
    };
    if pending.pr.as_ref() != Some(pr) {
        return false;
    }
    cx.default_global::<PendingStackMergeConfirm>().pr = None;
    true
}

pub fn view(pr: PrRef, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| PullRequestView::new(pr, window, cx)).into()
}

pub(in crate::github) fn refresh(view: &AnyView, cx: &mut App) -> bool {
    crate::shell::refresh_as::<PullRequestView>(view, cx)
}

/// The pull request the page `view` shows, once loaded (from the cache or
/// GitHub). `None` when `view` is not a pull request page or has nothing yet.
pub fn loaded_detail<'a>(view: &AnyView, cx: &'a App) -> Option<&'a PullRequestDetail> {
    let view = view.clone().downcast::<PullRequestView>().ok()?;
    view.read(cx).loaded_detail()
}
