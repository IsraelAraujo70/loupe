//! The side-column panel listing the T3 threads linked to a pull request.
//! Clicking a thread opens it in the T3 Code desktop app. On someone else's
//! pull request its heading holds "Review with T3 Code".

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_t3::{Error, LinkedThread, PullRequestKey, ThreadStatus, thread_deep_link};

use super::dialog;
use super::review::{self, Readiness};
use crate::links::PrLinkContext;
use crate::route::Route;
use crate::services::{self, Services};
use crate::shell;
use crate::theme::scale;
use crate::ui::{self, relative_time};

const AUTH_HINT: &str =
    "T3 rejected the token. Issue one with the orchestration:read scope in Settings.";

/// Why a load failed, as far as the panel's remedy differs.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The token is rejected or lacks `orchestration:read`; Settings fixes it.
    Credential,
    Other(String),
}

/// The threads linked to the PR and the server they live on, which the
/// desktop deep link names.
#[derive(Debug, PartialEq, Eq)]
struct Linked {
    environment_id: String,
    threads: Vec<LinkedThread>,
}

#[derive(Debug, PartialEq, Eq)]
enum PanelState {
    NotConnected,
    Loading,
    Loaded(Linked),
    Failed(Failure),
}

impl PanelState {
    fn from_result(result: Result<Linked, Error>) -> Self {
        match result {
            Ok(linked) => PanelState::Loaded(linked),
            Err(Error::Unauthorized | Error::MissingScope(_)) => {
                PanelState::Failed(Failure::Credential)
            }
            Err(error) => PanelState::Failed(Failure::Other(error.to_string())),
        }
    }
}

/// How a thread's status label is colored.
#[derive(Debug, PartialEq, Eq)]
enum Tone {
    Live,
    Failed,
    Quiet,
}

impl Tone {
    fn of(status: &ThreadStatus, settled: bool) -> Self {
        if settled {
            Tone::Quiet
        } else if *status == ThreadStatus::Failed {
            Tone::Failed
        } else if status.is_live() {
            Tone::Live
        } else {
            Tone::Quiet
        }
    }
}

fn status_text(status: &ThreadStatus, settled: bool) -> &str {
    if settled { "settled" } else { status.label() }
}

pub struct LinkedThreads {
    context: PrLinkContext,
    key: PullRequestKey,
    state: PanelState,
    load: Option<Task<()>>,
    /// The review button's state; `None` hides it (the user's own pull
    /// request).
    review: Option<Readiness>,
    review_load: Option<Task<()>>,
    _services: Subscription,
}

impl LinkedThreads {
    pub fn view(context: PrLinkContext, _window: &mut Window, cx: &mut App) -> AnyView {
        let key = PullRequestKey::github(context.pr.repo.to_string(), context.pr.number);
        cx.new(|cx| {
            let mut panel = LinkedThreads {
                context,
                key,
                state: PanelState::Loading,
                load: None,
                review: None,
                review_load: None,
                _services: cx
                    .observe_global::<Services>(|panel: &mut LinkedThreads, cx| panel.reload(cx)),
            };
            panel.reload(cx);
            panel
        })
        .into()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_review(cx);
        let Some(t3) = Services::get(cx).t3.clone() else {
            self.state = PanelState::NotConnected;
            self.load = None;
            cx.notify();
            return;
        };
        self.state = PanelState::Loading;
        let key = self.key.clone();
        let work = services::background(cx, move || {
            let environment = t3.environment()?;
            let threads = t3.linked_threads(&key)?;
            Ok(Linked {
                environment_id: environment.id,
                threads,
            })
        });
        self.load = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |panel, cx| {
                panel.state = PanelState::from_result(result);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Works out whether the review button is shown, and whether it works:
    /// the token may start agents and a T3 project tracks the repository.
    /// The server's providers say which review prompts can run, and the
    /// changed files whether the pull request is risky.
    fn reload_review(&mut self, cx: &mut Context<Self>) {
        self.review_load = None;
        if self.context.author.is_none() || self.context.is_own() {
            self.review = None;
            return;
        }
        if let Some(readiness) = review::precheck(&self.context) {
            self.review = Some(readiness);
            return;
        }
        let services = Services::get(cx);
        let Some(t3) = services.t3.clone() else {
            self.review = Some(Readiness::Unavailable(review::CONNECT_HINT.to_owned()));
            return;
        };
        let github = services.github.clone();
        let context = self.context.clone();
        let settings = crate::store::Settings::get(cx).t3_review.clone();
        self.review = Some(Readiness::Checking);
        let work = services::background(cx, move || {
            let check = || -> Result<Readiness, Error> {
                let session = t3.session()?;
                if !session.can_operate {
                    return Ok(Readiness::Unavailable(review::OPERATE_HINT.to_owned()));
                }
                let projects = t3.projects()?;
                let facts = t3.review_facts()?;
                let providers = t3.providers()?;
                // Without the file list the labels alone decide the risk.
                let files: Vec<String> = github
                    .and_then(|github| github.pull_request_files(&context.pr).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|file| file.path)
                    .collect();
                Ok(review::assess(
                    &context, session, &projects, &facts, providers, &files, &settings,
                ))
            };
            check().unwrap_or_else(|error| {
                Readiness::Unavailable(match error {
                    Error::Unauthorized | Error::MissingScope(_) => {
                        review::REJECTED_HINT.to_owned()
                    }
                    other => format!("Could not check T3: {other}"),
                })
            })
        });
        self.review_load = Some(cx.spawn(async move |this, cx| {
            let readiness = work.await;
            this.update(cx, |panel, cx| {
                panel.review = Some(readiness);
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Readiness::Ready(plan)), Some(t3)) =
            (self.review.clone(), Services::get(cx).t3.clone())
        else {
            return;
        };
        let panel = cx.weak_entity();
        let on_launched: dialog::OnLaunched = Box::new(move |_, cx| {
            panel.update(cx, |panel, cx| panel.reload(cx)).ok();
        });
        dialog::open(self.context.clone(), plan, t3, on_launched, window, cx);
    }

    fn review_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let readiness = self.review.as_ref()?;
        let button = Button::new("t3-review")
            .outline()
            .xsmall()
            .icon(IconName::Bot)
            .label("Review with T3 Code");
        let button = match readiness {
            Readiness::Checking => button.loading(true).disabled(true),
            Readiness::Unavailable(reason) => button.disabled(true).tooltip(reason.clone()),
            Readiness::Ready(_) => {
                button.on_click(cx.listener(|panel, _, window, cx| panel.open_review(window, cx)))
            }
        };
        Some(
            div()
                .flex_none()
                .debug_selector(|| "t3-review".to_owned())
                .child(button)
                .into_any_element(),
        )
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.state == PanelState::Loading;
        let count = match &self.state {
            PanelState::Loaded(linked) if !linked.threads.is_empty() => Some(linked.threads.len()),
            _ => None,
        };
        let refresh = (self.state != PanelState::NotConnected).then(|| {
            ui::icon_button("t3-refresh", IconName::RefreshCw, "Refresh T3 threads")
                .xsmall()
                .loading(loading)
                .disabled(loading)
                .on_click(cx.listener(|panel, _, _, cx| panel.reload(cx)))
        });
        h_flex()
            .h_6()
            .gap_2()
            .items_center()
            .child(ui::section_label("T3 threads", cx))
            .children(count.map(|count| {
                div()
                    .text_size(scale::label())
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string())
            }))
            .child(div().flex_1())
            .children(self.review_button(cx))
            .children(refresh)
            .into_any_element()
    }

    fn settings_button(label: &'static str) -> Button {
        Button::new("t3-settings")
            .outline()
            .xsmall()
            .icon(IconName::Settings)
            .label(label)
            .on_click(|_, window, cx| shell::navigate(Route::Settings, window, cx))
    }

    fn rows(linked: &Linked, cx: &App) -> Vec<Stateful<Div>> {
        let theme = cx.theme();
        let (hover, muted, info, danger, radius) = (
            theme.list_hover,
            theme.muted_foreground,
            theme.info,
            theme.danger,
            theme.radius,
        );
        let now = Utc::now();
        linked
            .threads
            .iter()
            .enumerate()
            .map(|(index, thread)| {
                let tone = match Tone::of(&thread.status, thread.settled) {
                    Tone::Live => info,
                    Tone::Failed => danger,
                    Tone::Quiet => muted,
                };
                let link = thread_deep_link(&linked.environment_id, &thread.id);
                let status = status_text(&thread.status, thread.settled).to_owned();
                h_flex()
                    .id(("t3-thread", index))
                    .role(Role::Link)
                    .aria_label(format!("Open in T3: {}", thread.title))
                    .group("t3-thread")
                    .w_full()
                    .min_w_0()
                    .px_1p5()
                    .py_1()
                    .gap_2()
                    .items_start()
                    .rounded(radius)
                    .hover(move |row| row.bg(hover))
                    .tooltip(|window, cx| Tooltip::new("Open in T3").build(window, cx))
                    .on_click(move |_, _, cx| cx.open_url(&link))
                    .child(
                        h_flex()
                            .flex_none()
                            .h_5()
                            .items_center()
                            .child(div().size_2().rounded_full().bg(tone)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .h_5()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(scale::body())
                                            .child(thread.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(scale::meta())
                                            .text_color(muted)
                                            .child(relative_time(thread.updated_at, now)),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .min_w_0()
                                    .items_center()
                                    .text_size(scale::meta())
                                    .text_color(muted)
                                    .child(div().flex_none().text_color(tone).child(status))
                                    .child("·")
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(thread.project.clone()),
                                    )
                                    .child(
                                        Icon::new(IconName::SquareArrowOutUpRight)
                                            .xsmall()
                                            .text_color(muted),
                                    ),
                            ),
                    )
            })
            .collect()
    }
}

impl Render for LinkedThreads {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.header(cx);
        let body = match &self.state {
            PanelState::NotConnected => v_flex()
                .items_start()
                .gap_2()
                .child(ui::empty_line(IconName::Unplug, "T3 is not connected", cx))
                .child(Self::settings_button("Connect in Settings…"))
                .into_any_element(),
            PanelState::Loading => v_flex()
                .gap_2()
                .children((0..2).map(|_| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_1p5()
                        .child(Skeleton::new().size_2().rounded_full())
                        .child(Skeleton::new().h_3().flex_1())
                }))
                .into_any_element(),
            PanelState::Loaded(linked) if linked.threads.is_empty() => {
                ui::empty_line(IconName::Bot, "No T3 threads linked to this PR", cx)
                    .into_any_element()
            }
            PanelState::Loaded(linked) => v_flex()
                .w_full()
                .children(Self::rows(linked, cx))
                .into_any_element(),
            PanelState::Failed(failure) => {
                let (text, credential) = match failure {
                    Failure::Credential => (AUTH_HINT, true),
                    Failure::Other(message) => (message.as_str(), false),
                };
                v_flex()
                    .items_start()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .text_size(scale::meta())
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .xsmall()
                                    .text_color(cx.theme().danger),
                            )
                            .child(div().flex_1().min_w_0().child(text.to_owned())),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(credential, |row| {
                                row.child(Self::settings_button("Open Settings…"))
                            })
                            .child(
                                Button::new("t3-retry")
                                    .outline()
                                    .xsmall()
                                    .icon(IconName::RefreshCw)
                                    .label("Retry")
                                    .on_click(cx.listener(|panel, _, _, cx| panel.reload(cx))),
                            ),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .id("t3-linked-threads")
            .w_full()
            .gap_2()
            .child(header)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, Linked, PanelState, Tone, status_text};
    use chrono::DateTime;
    use loupe_t3::{Error, LinkedThread, ThreadStatus};

    fn thread() -> LinkedThread {
        LinkedThread {
            id: "t1".into(),
            title: "Fix the thing".into(),
            project: "loupe".into(),
            status: ThreadStatus::Running,
            settled: false,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn credential_errors_point_to_settings() {
        assert_eq!(
            PanelState::from_result(Err(Error::Unauthorized)),
            PanelState::Failed(Failure::Credential)
        );
        assert_eq!(
            PanelState::from_result(Err(Error::MissingScope("orchestration:read".into()))),
            PanelState::Failed(Failure::Credential)
        );
    }

    #[test]
    fn other_errors_keep_their_message() {
        assert_eq!(
            PanelState::from_result(Err(Error::Transport("timed out".into()))),
            PanelState::Failed(Failure::Other("network error: timed out".into()))
        );
    }

    #[test]
    fn success_keeps_the_threads() {
        let linked = || Linked {
            environment_id: "env".into(),
            threads: vec![thread()],
        };
        assert_eq!(
            PanelState::from_result(Ok(linked())),
            PanelState::Loaded(linked())
        );
    }

    #[test]
    fn tone_follows_status_and_settlement() {
        assert_eq!(Tone::of(&ThreadStatus::Running, false), Tone::Live);
        assert_eq!(Tone::of(&ThreadStatus::Waiting, false), Tone::Live);
        assert_eq!(Tone::of(&ThreadStatus::Failed, false), Tone::Failed);
        assert_eq!(Tone::of(&ThreadStatus::Completed, false), Tone::Quiet);
        assert_eq!(Tone::of(&ThreadStatus::Idle, false), Tone::Quiet);
        assert_eq!(Tone::of(&ThreadStatus::Running, true), Tone::Quiet);
        assert_eq!(Tone::of(&ThreadStatus::Failed, true), Tone::Quiet);
    }

    #[test]
    fn settled_replaces_the_status_label() {
        assert_eq!(status_text(&ThreadStatus::Completed, true), "settled");
        assert_eq!(status_text(&ThreadStatus::RolledBack, false), "rolled back");
    }

    mod ui {
        use gpui_kit::{
            Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, px, size,
        };
        use loupe_github::{PrRef, RepoRef};

        use super::super::LinkedThreads;
        use crate::links::PrLinkContext;
        use crate::services::Services;
        use crate::store::Settings;

        fn context(author: &str) -> PrLinkContext {
            PrLinkContext {
                pr: PrRef::new(RepoRef::new("acme", "app"), 7),
                url: "https://github.com/acme/app/pull/7".into(),
                title: "Fix the thing".into(),
                body: String::new(),
                head_ref: "fix/thing".into(),
                head_oid: "abc123".into(),
                head_fork: None,
                author: Some(author.into()),
                viewer_login: "octocat".into(),
                labels: Vec::new(),
            }
        }

        /// The panel for a pull request by `author`, seen by `octocat`, with
        /// T3 not connected, so nothing reaches a server.
        fn open(author: &str, cx: &mut TestAppContext) -> VisualTestContext {
            let context = context(author);
            let (handle, _) = cx.update(|cx| {
                gpui_kit::init(cx);
                crate::theme::init(cx);
                cx.set_global(Settings::default());
                cx.set_global(Services::default());
                gpui_kit::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            Default::default(),
                            size(px(296.), px(400.)),
                        ))),
                        ..Default::default()
                    },
                    cx,
                    |window, cx| {
                        LinkedThreads::view(context, window, cx)
                            .downcast::<LinkedThreads>()
                            .expect("the panel")
                    },
                )
                .expect("open the window")
            });
            let mut cx = VisualTestContext::from_window(handle, cx);
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            cx
        }

        #[gpui_kit::test]
        fn the_review_button_is_on_someone_elses_pull_request(cx: &mut TestAppContext) {
            let mut cx = open("hubot", cx);
            assert!(cx.debug_bounds("t3-review").is_some());
        }

        #[gpui_kit::test]
        fn the_review_button_is_not_on_your_own_pull_request(cx: &mut TestAppContext) {
            let mut cx = open("OctoCat", cx);
            assert!(cx.debug_bounds("t3-review").is_none());
        }
    }
}
