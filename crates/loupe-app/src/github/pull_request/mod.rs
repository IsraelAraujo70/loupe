//! `Route::PullRequest`: header, tabs, review bar, actions, side column.

pub(crate) mod checks;
mod commits;
mod conversation;
mod files;
mod tab;
mod thread;
mod view;

use gpui_kit::{AnyView, App, AppContext as _, Window};
use loupe_github::PrRef;

use view::PullRequestView;

pub fn view(pr: PrRef, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| PullRequestView::new(pr, window, cx)).into()
}
