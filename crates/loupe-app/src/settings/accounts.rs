//! The account cards of Settings (GitHub, Linear, T3 Code) and what their
//! actions do.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::services::{self, Accounts, Connection, Services};
use crate::settings::view::{LinearForm, Notice, Palette, SettingsView, message};
use crate::settings::{auth, session};
use crate::store::{self, Secret};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::{self, ControlSize as _};

/// Linear's page for creating personal API keys.
const LINEAR_API_KEYS_URL: &str = "https://linear.app/settings/account/security";

/// Width of an account's action (Sign out, Disconnect), so the cards end in
/// buttons of one size.
const ACCOUNT_ACTION_WIDTH: f32 = 104.;

pub(super) fn token_placeholder() -> &'static str {
    if store::load_secret(Secret::T3Token).is_some() {
        "Saved (type to replace)"
    } else {
        "Paste the token from T3 Code"
    }
}

fn card(colors: Palette) -> Div {
    v_flex()
        .w_full()
        .p_4()
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(colors.border)
        .bg(colors.card)
}

/// A card's title row: its name on the left, an optional status on the
/// right.
fn card_title(name: &'static str, status: Option<AnyElement>) -> Div {
    h_flex()
        .gap_2()
        .child(ui::group_title(name).flex_1())
        .children(status)
}

/// A visible field label above its control.
fn field(label: &'static str, control: impl IntoElement, colors: Palette) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_size(scale::meta())
                .text_color(colors.muted)
                .child(label),
        )
        .child(control)
}

/// A connected account as one 80 px card: the connection's state over the
/// account on one line (avatar, name, then the handle or the workspace in
/// the muted color), and `action` centered on the card's height. The
/// section's heading above names the service.
fn account_card(
    state: &'static str,
    avatar_url: Option<SharedString>,
    name: SharedString,
    detail: SharedString,
    action: impl IntoElement,
    colors: Palette,
) -> Div {
    card(colors).child(
        h_flex()
            .gap_3()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(card_title(state, None))
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(div().flex_none().child(ui::avatar_named(
                                avatar_url.unwrap_or_default(),
                                name.clone(),
                                dp(20.),
                            )))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(relative(0.6))
                                    .truncate()
                                    .text_size(scale::body())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(scale::meta())
                                    .text_color(colors.muted)
                                    .child(detail),
                            ),
                    ),
            )
            .child(div().flex_none().child(action)),
    )
}

impl SettingsView {
    fn sign_out_github(&mut self, cx: &mut Context<Self>) {
        self.github_error = session::sign_out_github(cx)
            .err()
            .map(|error| format!("Could not sign out: {error:#}").into());
        cx.notify();
    }

    pub(super) fn connect_linear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.linear_form, LinearForm::Validating) {
            return;
        }
        let key = self.linear_key.read(cx).value().trim().to_owned();
        if key.is_empty() {
            self.linear_form = LinearForm::Failed("Paste an API key first.".into());
            cx.notify();
            return;
        }
        self.linear_form = LinearForm::Validating;
        cx.notify();
        let epoch = session::linear_epoch(cx);
        let check = services::background(cx, {
            let key = key.clone();
            move || auth::linear_account(&key)
        });
        self.linear_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = check.await;
            this.update_in(cx, |this, window, cx| {
                this.linear_form = match result {
                    Ok(account) => match session::connect_linear(key, account, epoch, cx) {
                        Ok(()) => {
                            // The key is saved; the field is ready for the next workspace's.
                            this.linear_key
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            LinearForm::Idle
                        }
                        Err(error) => {
                            LinearForm::Failed(format!("Could not save the key: {error:#}"))
                        }
                    },
                    Err(loupe_linear::Error::Unauthorized) => {
                        LinearForm::Failed("Linear rejected this key.".into())
                    }
                    Err(error) => LinearForm::Failed(error.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn disconnect_linear(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        self.linear_form = match session::disconnect_linear(workspace_id, cx) {
            Ok(()) => LinearForm::Idle,
            Err(error) => LinearForm::Failed(format!("Could not disconnect: {error:#}")),
        };
        cx.notify();
    }

    fn forget_failed_linear_key(&mut self, index: usize, cx: &mut Context<Self>) {
        self.linear_form = match session::forget_failed_linear_key(index, cx) {
            Ok(()) => LinearForm::Idle,
            Err(error) => LinearForm::Failed(format!("Could not remove the key: {error:#}")),
        };
        cx.notify();
    }

    fn save_t3(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.t3_url.read(cx).value().trim().to_owned();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.t3_notice =
                Notice::Error("The server URL must start with http:// or https://".into());
            cx.notify();
            return;
        }
        let typed = self.t3_token.read(cx).value().trim().to_owned();
        let token = if typed.is_empty() {
            store::load_secret(Secret::T3Token)
        } else {
            Some(typed)
        };
        self.t3_notice = match session::set_t3(Some(url), token, cx) {
            Ok(()) if Services::get(cx).t3.is_some() => Notice::Info("Saved.".into()),
            Ok(()) => Notice::Info("Saved the URL. Add a token to connect.".into()),
            Err(error) => Notice::Error(format!("Could not save: {error:#}").into()),
        };
        self.reset_t3_token(window, cx);
        cx.notify();
    }

    fn clear_t3(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.t3_notice = match session::set_t3(None, None, cx) {
            Ok(()) => Notice::Info("Cleared.".into()),
            Err(error) => Notice::Error(format!("Could not clear: {error:#}").into()),
        };
        self.t3_url
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.reset_t3_token(window, cx);
        cx.notify();
    }

    /// Empties the token field and updates its placeholder to say whether a
    /// token is stored, without ever showing the stored value.
    fn reset_t3_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = token_placeholder();
        self.t3_token.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
    }

    pub(super) fn github_card(&self, colors: Palette, cx: &mut Context<Self>) -> Div {
        let titled = |title: &'static str, body: Div| {
            card(colors).child(card_title(title, None)).child(body)
        };
        let card = match &Accounts::get(cx).github {
            Connection::Connected(viewer) => account_card(
                "Signed in",
                Some(viewer.avatar_url.clone().into()),
                viewer
                    .name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| viewer.login.clone())
                    .into(),
                format!("@{}", viewer.login).into(),
                Button::new("github-sign-out")
                    .outline()
                    .control()
                    .w(dp(ACCOUNT_ACTION_WIDTH))
                    .label("Sign out")
                    .on_click(cx.listener(|this, _, _, cx| this.sign_out_github(cx))),
                colors,
            ),
            Connection::Checking => titled("Account", message("Checking…".into(), colors.muted)),
            Connection::Disconnected => {
                titled("Account", message("Not signed in".into(), colors.muted))
            }
            Connection::Failed(error) => {
                titled("Account", message(error.clone().into(), colors.danger))
            }
        };
        card.when_some(self.github_error.clone(), |card, error| {
            card.child(message(error, colors.danger))
        })
    }

    /// Every connected workspace as an account card (the active one says
    /// so), each stored key that stopped working with Remove, then the form
    /// that connects another workspace.
    pub(super) fn linear_card(&self, colors: Palette, cx: &mut Context<Self>) -> Div {
        let validating = matches!(self.linear_form, LinearForm::Validating);
        let form_error = match &self.linear_form {
            LinearForm::Failed(error) => Some(SharedString::from(error.clone())),
            _ => None,
        };
        let accounts = Accounts::get(cx).clone();
        let workspaces = Services::get(cx).linear_workspaces.clone();
        let active = match &accounts.linear {
            Connection::Connected(viewer) => Some(viewer.organization.id.clone()),
            _ => None,
        };
        let mut column = v_flex().w_full().gap_3();
        if matches!(accounts.linear, Connection::Checking) {
            column = column.child(
                card(colors)
                    .child(card_title("Account", None))
                    .child(message("Checking…".into(), colors.muted)),
            );
        }
        for (index, workspace) in workspaces.iter().enumerate() {
            let id = workspace.id().to_owned();
            let account = &workspace.viewer;
            let name: SharedString = account.organization.name.clone().into();
            column = column.child(account_card(
                if active.as_deref() == Some(id.as_str()) {
                    "Connected · Active"
                } else {
                    "Connected"
                },
                account.user.avatar_url.clone().map(SharedString::from),
                account.user.name.clone().into(),
                name.clone(),
                Button::new(("linear-disconnect", index))
                    .outline()
                    .control()
                    .w(dp(ACCOUNT_ACTION_WIDTH))
                    .label("Disconnect")
                    .accessibility_label(format!("Disconnect {name}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.disconnect_linear(&id, cx))),
                colors,
            ));
        }
        for (index, error) in accounts.linear_failed.iter().enumerate() {
            column = column.child(
                card(colors).child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .child(card_title("Not connected", None))
                                .child(message(
                                    format!("A saved key stopped working: {error}").into(),
                                    colors.danger,
                                )),
                        )
                        .child(
                            Button::new(("linear-forget-key", index))
                                .outline()
                                .control()
                                .w(dp(ACCOUNT_ACTION_WIDTH))
                                .label("Remove")
                                .accessibility_label("Remove the saved key")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.forget_failed_linear_key(index, cx)
                                })),
                        ),
                ),
            );
        }
        let form = card(colors)
            .child(card_title(
                if workspaces.is_empty() {
                    "Not connected"
                } else {
                    "Add a workspace"
                },
                None,
            ))
            .child(field(
                "Personal API key",
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.linear_key)
                                .bg(tokens::background())
                                .disabled(validating),
                        ),
                    )
                    .child(
                        Button::new("linear-connect")
                            .outline()
                            .loading(validating)
                            .disabled(validating)
                            .label("Connect")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.connect_linear(window, cx)),
                            ),
                    ),
                colors,
            ))
            .child(message(
                "Paste a Linear personal API key (it starts with lin_api_). A key reads one \
                 workspace: connect one per workspace, from any of your Linear accounts. \
                 Loupe keeps them in the macOS Keychain and also picks up LINEAR_API_KEY when \
                 no key is saved."
                    .into(),
                colors.muted,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(message(
                            "Create one in Linear under Settings > Account > Security & access \
                         > Personal API keys, with write access so you can edit issues and \
                         comment."
                                .into(),
                            colors.muted,
                        )),
                    )
                    .child(
                        Button::new("linear-open-keys")
                            .flex_none()
                            .ghost()
                            .control()
                            .icon(IconName::ExternalLink)
                            .label("Open in Linear")
                            .on_click(|_, _, cx| cx.open_url(LINEAR_API_KEYS_URL)),
                    ),
            )
            .when_some(form_error, |card, error| {
                card.child(message(error, colors.danger))
            });
        column.child(form)
    }

    pub(super) fn t3_card(&self, colors: Palette, cx: &mut Context<Self>) -> Div {
        // A state, so it is a chip in the state's color: amber until the
        // server is set up, green once it is.
        let status = match &Services::get(cx).t3 {
            Some(t3) => {
                ui::tinted_chip(tokens::open()).child(format!("Connected to {}", t3.base_url))
            }
            None => ui::tinted_chip(tokens::pending()).child("Not configured"),
        };
        card(colors)
            .child(card_title("Server", Some(status.into_any_element())))
            .child(field(
                "Server URL",
                Input::new(&self.t3_url).bg(tokens::background()),
                colors,
            ))
            .child(field(
                "Token",
                Input::new(&self.t3_token).bg(tokens::background()),
                colors,
            ))
            .child(message(crate::t3::OPERATE_HINT.into(), colors.muted))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("t3-save")
                            .primary()
                            .control()
                            .label("Save")
                            .on_click(cx.listener(|this, _, window, cx| this.save_t3(window, cx))),
                    )
                    .child(
                        Button::new("t3-clear")
                            .ghost()
                            .control()
                            .label("Clear")
                            .on_click(cx.listener(|this, _, window, cx| this.clear_t3(window, cx))),
                    )
                    .map(|row| match &self.t3_notice {
                        Notice::None => row,
                        Notice::Info(text) => row.child(message(text.clone(), colors.muted)),
                        Notice::Error(text) => row.child(message(text.clone(), colors.danger)),
                    }),
            )
    }
}
