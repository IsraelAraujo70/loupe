//! Quiet release checks and an explicit install-and-restart action.
use crate::services;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    App, Context, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
};
use std::time::Duration;

#[derive(Default)]
enum Status {
    /// Not checked yet.
    #[default]
    Quiet,
    /// A check is running and nothing was found before it.
    Checking,
    /// The last check found no newer release.
    Current,
    /// The last check could not reach the releases.
    CheckFailed,
    Available(loupe_update::Release),
    Installing,
    /// The install failed.
    Failed,
}
#[derive(Default)]
struct Updater {
    token: Option<String>,
    status: Status,
}
impl Global for Updater {}

/// Installs updater state before token discovery and checks once an hour.
pub fn start(cx: &mut App) {
    cx.set_global(Updater::default());
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_secs(3600))
                .await;
            cx.update(check);
        }
    })
    .detach();
}

/// Reuses exactly the account credential selected by the sign-in flow.
pub fn set_token(token: Option<String>, cx: &mut App) {
    if cx.try_global::<Updater>().is_none() {
        return;
    }
    cx.set_global(Updater {
        token,
        status: Status::Quiet,
    });
    check(cx);
}

fn check(cx: &mut App) {
    let state = cx.global::<Updater>();
    if matches!(state.status, Status::Installing) {
        return;
    }
    let Some(token) = state.token.clone().filter(|v| !v.is_empty()) else {
        return;
    };
    // A release already found stays on screen while the next check runs.
    if !matches!(state.status, Status::Available(_)) {
        cx.global_mut::<Updater>().status = Status::Checking;
        cx.refresh_windows();
    }
    let task_token = token.clone();
    let work = services::background(cx, move || {
        let test = std::env::var("LOUPE_UPDATE_TEST_TAG").ok();
        loupe_update::check(&task_token, env!("CARGO_PKG_VERSION"), test.as_deref())
    });
    cx.spawn(async move |cx| {
        let result = work.await;
        cx.update(|cx| {
            let state = cx.global_mut::<Updater>();
            if state.token.as_ref() != Some(&token) || matches!(state.status, Status::Installing) {
                return;
            }
            match result {
                Ok(Some(release)) => state.status = Status::Available(release),
                Ok(None) if matches!(state.status, Status::Checking) => {
                    state.status = Status::Current;
                }
                Err(_) if matches!(state.status, Status::Checking) => {
                    state.status = Status::CheckFailed;
                }
                _ => {}
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Where updates stand, for Settings > About.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// No GitHub account to read the releases with.
    SignedOut,
    NotChecked,
    Checking,
    UpToDate,
    CheckFailed,
    /// A newer release, by its tag.
    Available(String),
    Installing,
    InstallFailed,
}

/// Where updates stand now.
pub fn state(cx: &App) -> UpdateState {
    let Some(updater) = cx.try_global::<Updater>() else {
        return UpdateState::SignedOut;
    };
    if updater.token.as_deref().is_none_or(str::is_empty) {
        return UpdateState::SignedOut;
    }
    match &updater.status {
        Status::Quiet => UpdateState::NotChecked,
        Status::Checking => UpdateState::Checking,
        Status::Current => UpdateState::UpToDate,
        Status::CheckFailed => UpdateState::CheckFailed,
        Status::Available(release) => UpdateState::Available(release.tag_name.clone()),
        Status::Installing => UpdateState::Installing,
        Status::Failed => UpdateState::InstallFailed,
    }
}

/// Checks for a release now, as the hourly check does.
pub fn check_now(cx: &mut App) {
    if cx.try_global::<Updater>().is_some() {
        check(cx);
    }
}

/// Installs the release found and restarts, or checks again when none was
/// found: what the update pill in the top bar does.
pub fn install(cx: &mut App) {
    if cx.try_global::<Updater>().is_some() {
        accept(cx);
    }
}

fn accept(cx: &mut App) {
    let state = cx.global::<Updater>();
    let Status::Available(release) = &state.status else {
        check(cx);
        return;
    };
    let release = release.clone();
    let Some(token) = state.token.clone() else {
        return;
    };
    cx.global_mut::<Updater>().status = Status::Installing;
    cx.refresh_windows();
    let work = services::background(cx, move || -> anyhow::Result<()> {
        let target = loupe_update::install_target(&std::env::current_exe()?)?;
        let stage = loupe_update::prepare(&token, &release, &target)?;
        loupe_update::install(&stage, &target)
    });
    cx.spawn(async move |cx| {
        let result = work.await;
        cx.update(|cx| {
            if result.is_ok() {
                cx.quit();
            } else {
                cx.global_mut::<Updater>().status = Status::Failed;
                cx.refresh_windows();
            }
        });
    })
    .detach();
}

/// Renders one nonmodal action beside the tabs; errors can retry discovery.
pub fn render<T: 'static>(cx: &mut Context<T>) -> impl IntoElement {
    let status = cx.try_global::<Updater>().map(|s| &s.status);
    // A short label on the pill; the tooltip says what the click does.
    let (label, hint) = match status {
        Some(Status::Available(release)) => (
            format!("Update to {}", release.tag_name),
            format!(
                "Loupe {} is available. Update and restart.",
                release.tag_name
            ),
        ),
        Some(Status::Installing) => (
            "Installing update…".to_owned(),
            "The update is installing.".to_owned(),
        ),
        Some(Status::Failed) => (
            "Update failed".to_owned(),
            "The update failed. Try again.".to_owned(),
        ),
        _ => return h_flex(),
    };
    // A 24 px neutral pill with a border: it says an update is there
    // without competing with the page's own actions. The dot carries the
    // state (accent for an update, red for a failure); the click target is
    // the whole pill, and the margin keeps it off the tabs.
    let dot = match status {
        Some(Status::Failed) => cx.theme().danger,
        Some(Status::Installing) => cx.theme().muted_foreground,
        _ => crate::theme::tokens::accent(),
    };
    let (border, foreground) = (cx.theme().border, cx.theme().foreground);
    let name = gpui_kit::SharedString::from(label.clone());
    let hint = gpui_kit::SharedString::from(hint);
    h_flex().flex_none().ml_3().child(
        h_flex()
            .id("install-update")
            .role(gpui_kit::Role::Button)
            .aria_label(name)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(hint.clone()).build(window, cx)
            })
            .on_click(|_, _, cx| accept(cx))
            .h_6()
            .px_2p5()
            .gap_1p5()
            .items_center()
            .rounded_full()
            .border_1()
            .border_color(border)
            .text_size(crate::theme::scale::meta())
            .text_color(foreground)
            .hover(|pill| pill.bg(crate::theme::tokens::glass_selected()))
            .child(
                gpui_kit::div()
                    .flex_none()
                    .size_1p5()
                    .rounded_full()
                    .bg(dot),
            )
            .child(label),
    )
}
