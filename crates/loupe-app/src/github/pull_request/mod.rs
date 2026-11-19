//! `Route::PullRequest`: header, tabs, review bar, actions, side column.

pub(crate) mod checks;
mod commits;
mod conversation;
mod files;
mod tab;
mod thread;
mod view;

use gpui_kit::{AnyView, App, AppContext as _, Window};
use loupe_github::{PrRef, PullRequestDetail};

use view::PullRequestView;

pub fn view(pr: PrRef, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| PullRequestView::new(pr, window, cx)).into()
}

/// The pull request the page `view` shows, once loaded (from the cache or
/// GitHub). `None` when `view` is not a pull request page or has nothing yet.
pub fn loaded_detail<'a>(view: &AnyView, cx: &'a App) -> Option<&'a PullRequestDetail> {
    let view = view.clone().downcast::<PullRequestView>().ok()?;
    view.read(cx).loaded_detail()
}
