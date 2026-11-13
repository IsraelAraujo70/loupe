//! Settings > T3 Code's review settings: the template of the request a
//! review thread starts with, and the rules that mark a pull request risky.
//! Each field saves as it changes.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::settings::catalog::Entry;
use crate::settings::view::{Palette, SettingsView, message, setting_row};
use crate::store::{self, Settings};
use crate::t3::{DEFAULT_PROMPT, RiskList, VARIABLES, format_list, unknown_variables};
use crate::theme::{scale, tokens};
use crate::ui::{self, ControlSize as _};

/// The review settings' text fields.
pub(super) struct ReviewFields {
    prompt: Entity<TextareaState>,
    lists: [(RiskList, Entity<TextareaState>); 3],
}

impl ReviewFields {
    /// The fields, filled from the settings, and the subscriptions that save
    /// each one as it changes.
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> (ReviewFields, Vec<Subscription>) {
        let review = Settings::get(cx).t3_review.clone();
        let field = |text: String, rows: (usize, usize), window: &mut Window, cx: &mut App| {
            cx.new(|cx| {
                let mut state = TextareaState::new(window, cx).auto_grow(rows.0, rows.1);
                state.set_value(text, window, cx);
                state
            })
        };
        let prompt = field(review.prompt().to_owned(), (10, 24), window, cx);
        let lists = [RiskList::Labels, RiskList::Words, RiskList::Exceptions].map(|list| {
            (
                list,
                field(format_list(&review.list(list)), (1, 8), window, cx),
            )
        });
        let mut subscriptions = vec![cx.subscribe(&prompt, |_, prompt, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = prompt.read(cx).value().to_string();
                store::update_settings(cx, |settings| settings.t3_review.set_prompt(&text));
            }
        })];
        for (list, state) in &lists {
            let list = *list;
            subscriptions.push(
                cx.subscribe(state, move |_, state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = state.read(cx).value().to_string();
                        store::update_settings(cx, |settings| {
                            settings.t3_review.set_list(list, &text)
                        });
                    }
                }),
            );
        }
        (ReviewFields { prompt, lists }, subscriptions)
    }

    /// Puts the saved values back into the fields, after Restore defaults.
    pub(super) fn sync(&self, window: &mut Window, cx: &mut App) {
        let review = Settings::get(cx).t3_review.clone();
        let prompt = review.prompt().to_owned();
        self.prompt
            .update(cx, |state, cx| state.set_value(prompt, window, cx));
        for (list, state) in &self.lists {
            let text = format_list(&review.list(*list));
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    fn list(&self, list: RiskList) -> &Entity<TextareaState> {
        &self
            .lists
            .iter()
            .find(|(each, _)| *each == list)
            .expect("a field per list")
            .1
    }
}

impl SettingsView {
    /// The review request card: the template, the variables it can use,
    /// the ones it uses that Loupe does not know, and Restore default.
    pub(super) fn review_prompt_card(
        &self,
        entry: &Entry,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let review = &Settings::get(cx).t3_review;
        let edited = review.prompt.is_some();
        let unknown = unknown_variables(review.prompt());
        let no_security = !review.prompt().contains("{security}");
        let mono = cx.theme().mono_font_family.clone();
        let variables = h_flex()
            .flex_wrap()
            .gap_x_3()
            .gap_y_1()
            .children(VARIABLES.iter().map(|(name, meaning)| {
                h_flex()
                    .gap_1()
                    .text_size(scale::meta())
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_color(colors.foreground)
                            .child(format!("{{{name}}}")),
                    )
                    .child(div().text_color(colors.muted).child(*meaning))
            }));
        v_flex()
            .w_full()
            .p_4()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(colors.card)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(ui::group_title(entry.title).flex_1())
                    .child(
                        Button::new("t3-review-prompt-restore")
                            .ghost()
                            .control()
                            .icon(IconName::RotateCcw)
                            .label("Restore default")
                            .accessibility_label("Restore the default review request")
                            .disabled(!edited)
                            .on_click(cx.listener(|this, _, window, cx| {
                                store::update_settings(cx, |settings| {
                                    settings.t3_review.set_prompt(DEFAULT_PROMPT)
                                });
                                this.review.sync(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(message(entry.description.into(), colors.muted))
            .child(
                Textarea::new(&self.review.prompt)
                    .aria_label(entry.title)
                    .bg(tokens::background()),
            )
            .child(variables)
            .children((!unknown.is_empty()).then(|| {
                let names = unknown
                    .iter()
                    .map(|name| format!("{{{name}}}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                message(
                    format!("Not a variable, so sent as typed: {names}.").into(),
                    colors.danger,
                )
            }))
            .children(no_security.then(|| {
                message(
                    "Without {security}, the security reviewer changes only the model.".into(),
                    colors.muted,
                )
            }))
    }

    /// The switch that turns the risk check on and off.
    pub(super) fn risk_check_row(&self, entry: &Entry, colors: Palette, cx: &App) -> AnyElement {
        setting_row(
            entry.title,
            entry.description,
            Switch::new("t3-risk-check")
                .checked(Settings::get(cx).t3_review.risk_check)
                .accessibility_label(entry.title)
                .on_click(|checked, _, cx| {
                    let checked = *checked;
                    store::update_settings(cx, |settings| settings.t3_review.risk_check = checked);
                }),
            colors,
        )
        .into_any_element()
    }

    /// One risk list: its name and description over its words, which wait
    /// while the risk check is off.
    pub(super) fn risk_list_row(
        &self,
        entry: &Entry,
        list: RiskList,
        colors: Palette,
        cx: &App,
    ) -> AnyElement {
        let on = Settings::get(cx).t3_review.risk_check;
        v_flex()
            .w_full()
            .py_3()
            .gap_2()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(scale::body())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.foreground)
                            .child(entry.title),
                    )
                    .child(message(entry.description.into(), colors.muted)),
            )
            .child(
                Textarea::new(self.review.list(list))
                    .aria_label(entry.title)
                    .disabled(!on)
                    .bg(tokens::background()),
            )
            .into_any_element()
    }
}
