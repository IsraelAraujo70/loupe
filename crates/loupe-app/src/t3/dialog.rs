//! The "Review with T3 Code" dialog: a tab per review prompt of Settings,
//! the model the picked one runs on, the review request, editable, and Send.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_t3::{Error, PullRequestKey, ReviewLaunch, T3};

use super::review::{self, OPERATE_HINT, Plan, ReviewPrompt};
use crate::links::PrLinkContext;
use crate::services;
use crate::store::Settings;
use crate::theme::scale::{self, dp};
use crate::ui;

/// What runs once T3 started the thread, after the dialog closed.
pub(super) type OnLaunched = Box<dyn Fn(&mut Window, &mut App)>;

/// Opens the dialog on the prompt Settings chose for this pull request: the
/// risk prompt when it is risky, else the default one.
pub(super) fn open(
    context: PrLinkContext,
    plan: Plan,
    t3: Arc<T3>,
    on_launched: OnLaunched,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| ReviewForm::new(context, plan, t3, on_launched, window, cx));
    let text_focus = form.read(cx).text.clone();
    let focus_text = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus_text.replace(false) {
            text_focus.focus_handle(cx).focus(window, cx);
        }
        dialog
            .title("Review with T3 Code")
            .w(dp(640.))
            .overlay_closable(false)
            .keyboard(false)
            .close_button(false)
            .child(form.clone())
    });
}

enum Submit {
    Idle,
    Sending,
    Failed(SharedString),
}

struct ReviewForm {
    context: PrLinkContext,
    plan: Plan,
    t3: Arc<T3>,
    on_launched: OnLaunched,
    /// The review prompts of Settings > T3 Code when the dialog opened.
    prompts: Vec<ReviewPrompt>,
    /// The index in `prompts` of the open tab.
    selected: usize,
    /// The template of Settings > T3 Code when the dialog opened.
    template: String,
    text: Entity<TextareaState>,
    /// Kept for the life of the dialog: sending again after a lost answer
    /// does not start a second thread.
    request_id: String,
    submit: Submit,
}

impl ReviewForm {
    fn new(
        context: PrLinkContext,
        plan: Plan,
        t3: Arc<T3>,
        on_launched: OnLaunched,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let review = &Settings::get(cx).t3_review;
        let template = review.prompt().to_owned();
        let prompts = review.prompts();
        let selected = review.starting_index(!plan.risk.is_empty());
        let initial = review::request(&context, &prompts[selected].text, &plan.risk, &template);
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(12, 20);
            state.set_value(initial, window, cx);
            state
        });
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        ReviewForm {
            request_id: format!(
                "loupe-review:{}:{}:{nonce}",
                context.pr.repo, context.pr.number
            ),
            context,
            plan,
            t3,
            on_launched,
            prompts,
            selected,
            template,
            text,
            submit: Submit::Idle,
        }
    }

    fn sending(&self) -> bool {
        matches!(self.submit, Submit::Sending)
    }

    /// The model the open tab runs on, or why the server cannot run it,
    /// which keeps Send off.
    fn model(&self) -> Result<loupe_t3::ModelChoice, String> {
        self.plan.model(&self.prompts[self.selected])
    }

    fn request_for(&self, index: usize) -> String {
        review::request(
            &self.context,
            &self.prompts[index].text,
            &self.plan.risk,
            &self.template,
        )
    }

    /// Opens another prompt's tab. The request follows unless the user
    /// edited it.
    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.selected || index >= self.prompts.len() || self.sending() {
            return;
        }
        if self.text.read(cx).value() == self.request_for(self.selected) {
            let next = self.request_for(index);
            self.text
                .update(cx, |state, cx| state.set_value(next, window, cx));
        }
        self.selected = index;
        self.submit = Submit::Idle;
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending() {
            return;
        }
        let model = match self.model() {
            Ok(model) => model,
            Err(why) => {
                self.submit = Submit::Failed(why.into());
                cx.notify();
                return;
            }
        };
        let message = self.text.read(cx).value().trim().to_owned();
        if message.is_empty() {
            self.submit = Submit::Failed("Write the review request first.".into());
            cx.notify();
            return;
        }
        let context = &self.context;
        let launch = ReviewLaunch {
            request_id: self.request_id.clone(),
            project_id: self.plan.project_id.clone(),
            title: review::thread_title(context),
            message,
            model,
            base_ref: context.head_ref.clone(),
            branch: self.plan.branch.clone(),
            pull_request: PullRequestKey::github(context.pr.repo.to_string(), context.pr.number),
            pull_request_url: context.url.clone(),
        };
        self.submit = Submit::Sending;
        cx.notify();
        let t3 = self.t3.clone();
        let task = services::background(cx, move || t3.launch_review(&launch));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |form, window, cx| match result {
                Ok(launched) => {
                    window.close_dialog(cx);
                    let notification = match launched.link_error {
                        None => Notification::success(
                            "The review thread is running in T3 and linked to this pull request.",
                        )
                        .title("Review started"),
                        Some(error) => Notification::warning(format!(
                            "The review thread is running, but T3 did not link it to this pull \
                             request: {error}"
                        ))
                        .title("Review started"),
                    };
                    window.push_notification(notification, cx);
                    (form.on_launched)(window, cx);
                }
                Err(error) => {
                    form.submit = Submit::Failed(failure_text(&error).into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

fn failure_text(error: &Error) -> String {
    match error {
        Error::MissingScope(_) => OPERATE_HINT.to_owned(),
        Error::Unauthorized => review::REJECTED_HINT.to_owned(),
        other => format!("T3 did not start the review: {other}"),
    }
}

impl Render for ReviewForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sending = self.sending();
        let muted = cx.theme().muted_foreground;
        let error = match &self.submit {
            Submit::Failed(message) => Some(message.clone()),
            Submit::Idle | Submit::Sending => None,
        };
        let model = self.model();
        let tabs = h_flex()
            .id("t3-review-prompts")
            .role(Role::TabList)
            .aria_label("Review prompt")
            .flex_wrap()
            .gap_1()
            .children(self.prompts.iter().enumerate().map(|(index, prompt)| {
                let unavailable = self.plan.model(prompt).err();
                ui::view_tab(
                    ("t3-review-prompt", index),
                    prompt.name.clone(),
                    index == self.selected,
                    cx,
                )
                .when_some(unavailable, |tab, why| {
                    tab.icon(IconName::TriangleAlert).tooltip(why)
                })
                .disabled(sending && index != self.selected)
                .on_click(cx.listener(move |form, _, window, cx| form.pick(index, window, cx)))
            }));
        let runs_on = match &model {
            Ok(choice) => div()
                .text_size(scale::meta())
                .text_color(muted)
                .child(format!(
                    "Runs on {}.",
                    review::model_summary(&self.plan.providers, choice)
                )),
            Err(why) => h_flex()
                .gap_1p5()
                .items_start()
                .text_size(scale::meta())
                .text_color(cx.theme().danger)
                .child(
                    gpui_kit::component::Icon::new(IconName::TriangleAlert)
                        .xsmall()
                        .flex_none()
                        .mt_0p5(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(format!("Unavailable: {why}")),
                ),
        };
        let place = format!(
            "A new thread in {} on a worktree of {} ({}).",
            self.plan.project_title, self.context.head_ref, self.plan.branch
        );
        let risk = (!self.plan.risk.is_empty()).then(|| {
            format!(
                "Risky pull request: {}.",
                self.plan
                    .risk
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
                    + &match self.plan.risk.len().saturating_sub(3) {
                        0 => String::new(),
                        more => format!(" and {more} more"),
                    }
            )
        });
        v_flex()
            .gap_3()
            .child(v_flex().gap_2().child(tabs).child(runs_on))
            .child(
                v_flex()
                    .gap_1()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(place)
                    .children(risk),
            )
            .child(Textarea::new(&self.text).disabled(sending))
            .children(
                error.map(|message| div().text_sm().text_color(cx.theme().danger).child(message)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1())
                    .child(
                        Button::new("t3-review-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .disabled(sending)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("t3-review-send")
                            .primary()
                            .small()
                            .label(if sending { "Sending\u{2026}" } else { "Send" })
                            .loading(sending)
                            .disabled(sending || model.is_err())
                            .on_click(cx.listener(|form, _, window, cx| form.send(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };
    use loupe_github::{PrRef, RepoRef};
    use loupe_t3::{Effort, Provider, ProviderModel, T3};

    use super::{ReviewForm, Submit};
    use crate::links::PrLinkContext;
    use crate::store::Settings;
    use crate::t3::ReviewSettings;
    use crate::t3::review::Plan;

    fn context() -> PrLinkContext {
        PrLinkContext {
            pr: PrRef::new(RepoRef::new("acme", "app"), 7),
            url: "https://github.com/acme/app/pull/7".into(),
            title: "Fix the thing".into(),
            body: String::new(),
            head_ref: "fix/thing".into(),
            head_oid: "abc123".into(),
            head_fork: None,
            author: Some("hubot".into()),
            viewer_login: "octocat".into(),
            labels: Vec::new(),
        }
    }

    /// A server whose one provider offers both default models.
    fn plan(risk: &[&str]) -> Plan {
        let model = |slug: &str| ProviderModel {
            slug: slug.into(),
            name: slug.into(),
            efforts: ["low", "medium", "high"]
                .map(|id| Effort {
                    id: id.into(),
                    label: id.into(),
                    is_default: id == "medium",
                })
                .to_vec(),
        };
        Plan {
            project_id: "p".into(),
            project_title: "app".into(),
            branch: "review/pr-7".into(),
            providers: vec![Provider {
                instance_id: "pool".into(),
                name: "Pool".into(),
                unavailable: None,
                models: vec![model("claude-opus-5-5"), model("claude-fable-5-2")],
            }],
            risk: risk.iter().map(|reason| (*reason).to_owned()).collect(),
        }
    }

    /// The dialog's form for `plan` under `review`. Nothing reaches a
    /// server: the T3 address is never called unless Send goes through.
    fn open(
        plan: Plan,
        review: ReviewSettings,
        cx: &mut TestAppContext,
    ) -> (Entity<ReviewForm>, VisualTestContext) {
        let (handle, form) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings {
                t3_review: review,
                ..Settings::default()
            });
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(640.), px(600.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let t3 = Arc::new(T3::new("http://127.0.0.1:9", "unused"));
                    cx.new(|cx| {
                        ReviewForm::new(context(), plan, t3, Box::new(|_, _| {}), window, cx)
                    })
                },
            )
            .expect("open the window")
        });
        (form, VisualTestContext::from_window(handle, cx))
    }

    #[gpui_kit::test]
    fn a_risky_pull_request_opens_on_the_risk_prompt(cx: &mut TestAppContext) {
        let (form, mut cx) = open(plan(&["src/auth.ts"]), ReviewSettings::default(), cx);
        let (name, text) = cx.update(|_, cx| {
            let form = form.read(cx);
            (
                form.prompts[form.selected].name.clone(),
                form.text.read(cx).value().to_string(),
            )
        });
        assert_eq!(name, "General + security");
        assert!(
            text.contains("Este PR é de risco (`src/auth.ts`)"),
            "{text}"
        );
    }

    #[gpui_kit::test]
    fn an_ordinary_pull_request_opens_on_the_default_prompt(cx: &mut TestAppContext) {
        let mut review = ReviewSettings::default();
        let quick = review.add_prompt();
        review.set_default_prompt(&quick);
        let (form, mut cx) = open(plan(&[]), review, cx);
        let name = cx.update(|_, cx| {
            let form = form.read(cx);
            form.prompts[form.selected].name.clone()
        });
        assert_eq!(name, "New prompt");
    }

    #[gpui_kit::test]
    fn a_prompt_the_server_cannot_run_is_not_sent(cx: &mut TestAppContext) {
        let mut review = ReviewSettings::default();
        let gone = review.add_prompt();
        review.update_prompt(&gone, |prompt| prompt.provider = Some("gone".into()));
        review.set_default_prompt(&gone);
        let (form, mut cx) = open(plan(&[]), review, cx);
        cx.update(|window, cx| {
            form.update(cx, |form, cx| form.send(window, cx));
        });
        cx.update(|_, cx| {
            let form = form.read(cx);
            assert!(form.model().is_err(), "Send is off for this tab");
            match &form.submit {
                Submit::Failed(why) => {
                    assert_eq!(why.as_ref(), "The T3 server has no provider gone.");
                }
                Submit::Idle | Submit::Sending => panic!("the review was sent"),
            }
        });
    }
}
