use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::GitHub;

use crate::services::{self, Accounts, Connection};
use crate::settings::auth::{self, DeviceCode, PollOutcome};
use crate::settings::session;
use crate::shell::{TITLE_BAR_HEIGHT, svg_icon};
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui::{self, WindowDragArea as _};

/// The full-window sign-in screen the shell shows while GitHub is not
/// connected: OAuth device flow first, a pasted token as the fallback.
pub fn sign_in_view(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| SignIn::new(window, cx)).into()
}

enum DeviceFlow {
    Idle,
    Requesting,
    Waiting {
        code: DeviceCode,
        copied: bool,
    },
    /// GitHub accepted the code; the token is being checked and saved.
    Finishing,
    Expired,
    Denied,
    Failed(String),
}

enum PasteToken {
    Closed,
    Open,
    Validating,
    Failed(String),
}

struct SignIn {
    flow: DeviceFlow,
    paste: PasteToken,
    token_input: Entity<InputState>,
    /// The device-flow request and polling loop; dropping it stops polling.
    flow_task: Option<Task<()>>,
    paste_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl SignIn {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let token_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("ghp_… or github_pat_…")
        });
        let subscription = cx.subscribe_in(
            &token_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_token(window, cx);
                }
            },
        );
        Self {
            flow: DeviceFlow::Idle,
            paste: PasteToken::Closed,
            token_input,
            flow_task: None,
            paste_task: None,
            _subscription: subscription,
        }
    }

    fn set_flow(&mut self, flow: DeviceFlow, cx: &mut Context<Self>) {
        self.flow = flow;
        cx.notify();
    }

    fn start_device_flow(&mut self, cx: &mut Context<Self>) {
        self.set_flow(DeviceFlow::Requesting, cx);
        let request = services::background(cx, auth::request_device_code);
        self.flow_task = Some(cx.spawn(async move |this, cx| {
            let code = match request.await {
                Ok(code) => code,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.set_flow(DeviceFlow::Failed(error.to_string()), cx)
                    })
                    .ok();
                    return;
                }
            };
            let deadline = Instant::now() + code.expires_in;
            let device_code = code.device_code.clone();
            let mut interval = code.interval;
            if this
                .update(cx, |this, cx| {
                    this.set_flow(
                        DeviceFlow::Waiting {
                            code,
                            copied: false,
                        },
                        cx,
                    )
                })
                .is_err()
            {
                return;
            }
            loop {
                cx.background_executor().timer(interval).await;
                if Instant::now() >= deadline {
                    this.update(cx, |this, cx| this.set_flow(DeviceFlow::Expired, cx))
                        .ok();
                    return;
                }
                let poll = {
                    let device_code = device_code.clone();
                    cx.background_executor()
                        .spawn(async move { auth::poll_device_token(&device_code) })
                };
                let next = match poll.await {
                    Ok(PollOutcome::Pending) => continue,
                    Ok(PollOutcome::SlowDown { interval: slower }) => {
                        interval = slower;
                        continue;
                    }
                    Ok(PollOutcome::Granted { token }) => {
                        this.update(cx, |this, cx| this.set_flow(DeviceFlow::Finishing, cx))
                            .ok();
                        match validate(token, cx).await {
                            Ok(()) => return,
                            Err(message) => DeviceFlow::Failed(message),
                        }
                    }
                    Ok(PollOutcome::Expired) => DeviceFlow::Expired,
                    Ok(PollOutcome::Denied) => DeviceFlow::Denied,
                    Err(error) => DeviceFlow::Failed(error.to_string()),
                };
                this.update(cx, |this, cx| this.set_flow(next, cx)).ok();
                return;
            }
        }));
    }

    fn cancel_device_flow(&mut self, cx: &mut Context<Self>) {
        self.flow_task = None;
        self.set_flow(DeviceFlow::Idle, cx);
    }

    fn copy_code(&mut self, cx: &mut Context<Self>) {
        if let DeviceFlow::Waiting { code, copied } = &mut self.flow {
            cx.write_to_clipboard(ClipboardItem::new_string(code.user_code.clone()));
            *copied = true;
            cx.notify();
        }
    }

    fn submit_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.paste, PasteToken::Validating) {
            return;
        }
        let token = self.token_input.read(cx).value().trim().to_owned();
        if token.is_empty() {
            self.paste = PasteToken::Failed("Paste a token first.".into());
            cx.notify();
            return;
        }
        self.paste = PasteToken::Validating;
        cx.notify();
        let input = self.token_input.clone();
        self.paste_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = validate(token, cx).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.paste = PasteToken::Open;
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(message) => this.paste = PasteToken::Failed(message),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

/// Checks the token with `GitHub::viewer()` and signs in with it. The error
/// is safe to show: it never contains the token.
async fn validate(token: String, cx: &mut AsyncApp) -> Result<(), String> {
    let check = {
        let token = token.clone();
        cx.background_executor()
            .spawn(async move { GitHub::new(token).viewer() })
    };
    let viewer = match check.await {
        Ok(viewer) => viewer,
        Err(loupe_github::Error::Unauthorized) => {
            return Err("GitHub rejected this token.".into());
        }
        Err(error) => return Err(error.to_string()),
    };
    cx.update(|cx| session::sign_in_github(token, viewer, cx))
        .map_err(|error| format!("Could not save the token: {error:#}"))
}

impl Render for SignIn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, surface, danger) = (
            theme.muted_foreground,
            theme.border,
            theme.secondary,
            theme.danger,
        );
        let mono = theme.mono_font_family.clone();

        let device = match &self.flow {
            DeviceFlow::Idle => v_flex().gap_2().child(
                Button::new("device-start")
                    .primary()
                    .w_full()
                    .label("Sign in with GitHub")
                    .on_click(cx.listener(|this, _, _, cx| this.start_device_flow(cx))),
            ),
            DeviceFlow::Requesting => v_flex().gap_2().child(
                Button::new("device-start")
                    .primary()
                    .w_full()
                    .loading(true)
                    .label("Requesting a code…"),
            ),
            DeviceFlow::Waiting { code, copied } => {
                let uri = code.verification_uri.clone();
                let host = uri
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .to_owned();
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_size(scale::body())
                            .text_color(muted)
                            .child(format!("Enter this code at {host}:")),
                    )
                    .child(
                        div()
                            .id("device-code")
                            .py_3()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(surface)
                            .text_center()
                            .text_2xl()
                            .font_family(mono.clone())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(code.user_code.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("device-copy")
                                    .outline()
                                    .flex_1()
                                    .icon(if *copied {
                                        IconName::Check
                                    } else {
                                        IconName::Copy
                                    })
                                    .label(if *copied { "Copied" } else { "Copy code" })
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_code(cx))),
                            )
                            .child(
                                Button::new("device-open")
                                    .primary()
                                    .flex_1()
                                    .icon(IconName::ExternalLink)
                                    .label(format!("Open {host}"))
                                    .on_click(move |_, _, cx| cx.open_url(&uri)),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child("Waiting for authorization…")
                            .child(
                                Button::new("device-cancel")
                                    .ghost()
                                    .compact()
                                    .label("Cancel")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_device_flow(cx)),
                                    ),
                            ),
                    )
            }
            DeviceFlow::Finishing => v_flex().gap_2().child(
                Button::new("device-start")
                    .primary()
                    .w_full()
                    .loading(true)
                    .label("Signing in…"),
            ),
            DeviceFlow::Expired | DeviceFlow::Denied | DeviceFlow::Failed(_) => {
                let message: SharedString = match &self.flow {
                    DeviceFlow::Expired => "The code expired.".into(),
                    DeviceFlow::Denied => "Authorization was cancelled on GitHub.".into(),
                    DeviceFlow::Failed(message) => message.clone().into(),
                    _ => SharedString::default(),
                };
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(scale::body())
                            .text_color(danger)
                            .child(message),
                    )
                    .child(
                        Button::new("device-retry")
                            .primary()
                            .w_full()
                            .label("Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.start_device_flow(cx))),
                    )
            }
        };

        let unreachable = match &Accounts::get(cx).github {
            Connection::Failed(message) => Some(message.clone()),
            _ => None,
        };

        let paste = match &self.paste {
            PasteToken::Closed => h_flex()
                .child(
                    Button::new("paste-open")
                        .ghost()
                        .compact()
                        .label("Paste a token instead")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.paste = PasteToken::Open;
                            cx.notify();
                        })),
                )
                .into_any_element(),
            state => {
                let validating = matches!(state, PasteToken::Validating);
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child("Personal access token with repo, read:org and notifications"),
                    )
                    .child(Input::new(&self.token_input).disabled(validating))
                    .when_some(
                        match state {
                            PasteToken::Failed(message) => Some(message.clone()),
                            _ => None,
                        },
                        |column, message| {
                            column.child(
                                div()
                                    .text_size(scale::meta())
                                    .text_color(danger)
                                    .child(message),
                            )
                        },
                    )
                    .child(
                        Button::new("paste-continue")
                            .outline()
                            .w_full()
                            .loading(validating)
                            .disabled(validating)
                            .label("Continue")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_token(window, cx)),
                            ),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .child(
                div()
                    .id("sign-in-drag")
                    .h(TITLE_BAR_HEIGHT)
                    .w_full()
                    .flex_none()
                    .window_drag_area(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .pb(TITLE_BAR_HEIGHT)
                    .child(
                        v_flex()
                            .w(dp(360.))
                            .gap_5()
                            .child(
                                v_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(svg_icon(IconName::Search, dp(32.), theme.foreground))
                                    .child(
                                        div()
                                            .text_size(scale::heading())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Sign in to Loupe"),
                                    )
                                    .child(div().text_size(scale::body()).text_color(muted).text_center().child(
                                        "Review GitHub pull requests next to your Linear issues.",
                                    )),
                            )
                            .when_some(unreachable, |column, message| {
                                column.child(ui::error_banner(message, |_, _, cx| {
                                    session::discover_github(cx)
                                }))
                            })
                            .child(device)
                            .child(div().h(px(1.)).bg(border))
                            .child(paste),
                    ),
            )
    }
}
