//! Settings > T3 Code's review settings: the template of the request a
//! review thread starts with, the named review prompts with the model and
//! effort each runs on, and the rules that mark a pull request risky. Each
//! field saves as it changes.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::select::{
    SearchableVec, Select, SelectEvent, SelectGroup, SelectItem, SelectState,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_t3::{Provider, ProviderModel};

use crate::services::{self, Services};
use crate::settings::catalog::Entry;
use crate::settings::view::{Palette, SettingsView, message, setting_row};
use crate::store::{self, Settings};
use crate::t3::{
    DEFAULT_PROMPT, ReviewPrompt, RiskList, VARIABLES, format_list, model_choice, model_summary,
    unknown_variables,
};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::{self, ControlSize as _, MenuTriggerExt as _};

/// The review settings' fields, the server's models the prompts pick from,
/// and the prompt being edited.
pub(super) struct ReviewFields {
    prompt: Entity<TextareaState>,
    lists: [(RiskList, Entity<TextareaState>); 3],
    providers: Providers,
    providers_task: Option<Task<()>>,
    editor: Option<PromptEditor>,
    /// The prompt whose Delete waits for a confirmation.
    confirm_delete: Option<String>,
}

/// The provider instances of the connected T3 server.
enum Providers {
    NotConnected,
    Loading,
    Loaded(Vec<Provider>),
    Failed(String),
}

impl Providers {
    fn list(&self) -> &[Provider] {
        match self {
            Providers::Loaded(providers) => providers,
            _ => &[],
        }
    }
}

/// The fields of the prompt open for editing.
struct PromptEditor {
    id: String,
    name: Entity<InputState>,
    text: Entity<TextareaState>,
    model: Entity<ModelSelect>,
    _subscriptions: Vec<Subscription>,
}

/// A model as the selector names it: the provider instance that serves it
/// and its id.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelKey {
    instance: String,
    slug: String,
}

/// One row of the model selector.
#[derive(Clone)]
struct ModelItem {
    key: ModelKey,
    name: SharedString,
    provider: SharedString,
}

impl SelectItem for ModelItem {
    type Value = ModelKey;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &ModelKey {
        &self.key
    }

    /// The model's name, its id, or its provider's name.
    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        [
            self.name.as_ref(),
            self.key.slug.as_str(),
            self.provider.as_ref(),
        ]
        .iter()
        .any(|text| text.to_lowercase().contains(&query))
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .child(div().flex_1().min_w_0().truncate().child(self.name.clone()))
            .child(
                div()
                    .flex_none()
                    .text_size(scale::meta())
                    .text_color(cx.theme().muted_foreground)
                    .child(self.key.slug.clone()),
            )
    }
}

type ModelSelect = SelectState<SearchableVec<SelectGroup<ModelItem>>>;

/// The selector's rows: the models of each provider that can run threads,
/// under the provider's name, in the server's order.
fn model_items(providers: &[Provider]) -> SearchableVec<SelectGroup<ModelItem>> {
    SearchableVec::new(
        providers
            .iter()
            .filter(|provider| provider.unavailable.is_none() && !provider.models.is_empty())
            .map(|provider| {
                SelectGroup::new(provider.name.clone()).items(provider.models.iter().map(|model| {
                    ModelItem {
                        key: ModelKey {
                            instance: provider.instance_id.clone(),
                            slug: model.slug.clone(),
                        },
                        name: model.name.clone().into(),
                        provider: provider.name.clone().into(),
                    }
                }))
            })
            .collect::<Vec<_>>(),
    )
}

/// The provider and model a prompt names, as the server reports them: its
/// own provider's, else the first that can run threads, else any.
fn offered<'a>(
    providers: &'a [Provider],
    prompt: &ReviewPrompt,
) -> Option<(&'a Provider, &'a ProviderModel)> {
    let model = |provider: &'a Provider| {
        provider
            .models
            .iter()
            .find(|model| model.slug == prompt.model)
            .map(|model| (provider, model))
    };
    match &prompt.provider {
        Some(id) => providers
            .iter()
            .find(|provider| provider.instance_id == *id)
            .and_then(model),
        None => providers
            .iter()
            .filter(|provider| provider.unavailable.is_none())
            .find_map(model)
            .or_else(|| providers.iter().find_map(model)),
    }
}

/// A prompt's name as a tab and a row show it.
fn prompt_name(prompt: &ReviewPrompt) -> SharedString {
    if prompt.name.trim().is_empty() {
        "Untitled prompt".into()
    } else {
        prompt.name.clone().into()
    }
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
        subscriptions.push(
            cx.observe_global_in::<Services>(window, |this, window, cx| {
                this.load_providers(window, cx)
            }),
        );
        (
            ReviewFields {
                prompt,
                lists,
                providers: Providers::NotConnected,
                providers_task: None,
                editor: None,
                confirm_delete: None,
            },
            subscriptions,
        )
    }

    /// Puts the saved values back into the fields, after Restore defaults,
    /// and closes the prompt editor: its prompt may be gone.
    pub(super) fn sync(&mut self, window: &mut Window, cx: &mut App) {
        self.editor = None;
        self.confirm_delete = None;
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
        let no_prompt =
            !review.prompt().contains("{prompt}") && !review.prompt().contains("{security}");
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
                                // Only the template: the prompt editor stays open.
                                let prompt = Settings::get(cx).t3_review.prompt().to_owned();
                                this.review
                                    .prompt
                                    .update(cx, |state, cx| state.set_value(prompt, window, cx));
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
            .children(no_prompt.then(|| {
                message(
                    "Without {prompt}, a review prompt's text is not sent: picking one \
                     changes only the model."
                        .into(),
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

impl SettingsView {
    /// Reads the connected server's providers and models in the background,
    /// for the model selector and to say which prompts cannot run.
    pub(super) fn load_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t3) = Services::get(cx).t3.clone() else {
            self.review.providers = Providers::NotConnected;
            self.review.providers_task = None;
            self.refresh_model_items(window, cx);
            return;
        };
        self.review.providers = Providers::Loading;
        let work = services::background(cx, move || t3.providers());
        self.review.providers_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.review.providers = match result {
                    Ok(providers) => Providers::Loaded(providers),
                    Err(error) => Providers::Failed(error.to_string()),
                };
                this.refresh_model_items(window, cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Gives the open editor's selector the models now known.
    fn refresh_model_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.review.editor else {
            return;
        };
        let items = model_items(self.review.providers.list());
        let selected = Settings::get(cx)
            .t3_review
            .prompts()
            .into_iter()
            .find(|prompt| prompt.id == editor.id)
            .and_then(|prompt| self.model_key(&prompt));
        editor.model.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            if let Some(key) = &selected {
                select.set_selected_value(key, window, cx);
            }
        });
    }

    fn model_key(&self, prompt: &ReviewPrompt) -> Option<ModelKey> {
        offered(self.review.providers.list(), prompt).map(|(provider, model)| ModelKey {
            instance: provider.instance_id.clone(),
            slug: model.slug.clone(),
        })
    }

    /// Opens the editor of the prompt with `id`, or closes it when it is
    /// the one open.
    fn toggle_prompt_editor(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .review
            .editor
            .as_ref()
            .is_some_and(|editor| editor.id == id)
        {
            self.review.editor = None;
            cx.notify();
            return;
        }
        let Some(prompt) = Settings::get(cx)
            .t3_review
            .prompts()
            .into_iter()
            .find(|prompt| prompt.id == id)
        else {
            return;
        };
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Name")
                .default_value(prompt.name.clone())
        });
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .auto_grow(3, 16)
                .placeholder("Nothing more: the review request alone.");
            state.set_value(prompt.text.clone(), window, cx);
            state
        });
        let key = self.model_key(&prompt);
        let items = model_items(self.review.providers.list());
        let model = cx.new(|cx| {
            let mut select = SelectState::new(items, None, window, cx).searchable(true);
            if let Some(key) = &key {
                select.set_selected_value(key, window, cx);
            }
            select
        });
        let edit = |id: &str| {
            let id = id.to_owned();
            move |change: &dyn Fn(&mut ReviewPrompt), cx: &mut App| {
                store::update_settings(cx, |settings| {
                    settings.t3_review.update_prompt(&id, change)
                });
            }
        };
        let (save_name, save_text) = (edit(id), edit(id));
        let model_id = id.to_owned();
        let subscriptions = vec![
            cx.subscribe(&name, move |_, name, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = name.read(cx).value().to_string();
                    save_name(&|prompt| prompt.name = value.clone(), cx);
                }
            }),
            cx.subscribe(&text, move |_, text, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = text.read(cx).value().to_string();
                    save_text(&|prompt| prompt.text = value.clone(), cx);
                }
            }),
            cx.subscribe(
                &model,
                move |this, _, event: &SelectEvent<SearchableVec<SelectGroup<ModelItem>>>, cx| {
                    let SelectEvent::Confirm(Some(key)) = event else {
                        return;
                    };
                    let efforts: Vec<String> = this
                        .review
                        .providers
                        .list()
                        .iter()
                        .find(|provider| provider.instance_id == key.instance)
                        .and_then(|provider| {
                            provider.models.iter().find(|model| model.slug == key.slug)
                        })
                        .map(|model| {
                            model
                                .efforts
                                .iter()
                                .map(|effort| effort.id.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    let key = key.clone();
                    store::update_settings(cx, |settings| {
                        settings.t3_review.update_prompt(&model_id, |prompt| {
                            prompt.provider = Some(key.instance.clone());
                            prompt.model = key.slug.clone();
                            // An effort the new model lacks would make the
                            // prompt unavailable: fall back to its default.
                            if prompt
                                .effort
                                .as_ref()
                                .is_some_and(|effort| !efforts.contains(effort))
                            {
                                prompt.effort = None;
                            }
                        })
                    });
                },
            ),
        ];
        self.review.confirm_delete = None;
        self.review.editor = Some(PromptEditor {
            id: id.to_owned(),
            name,
            text,
            model,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn add_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut id = String::new();
        store::update_settings(cx, |settings| id = settings.t3_review.add_prompt());
        self.review.editor = None;
        self.toggle_prompt_editor(&id, window, cx);
    }

    fn delete_prompt(&mut self, id: &str, cx: &mut Context<Self>) {
        store::update_settings(cx, |settings| settings.t3_review.remove_prompt(id));
        if self
            .review
            .editor
            .as_ref()
            .is_some_and(|editor| editor.id == id)
        {
            self.review.editor = None;
        }
        self.review.confirm_delete = None;
        cx.notify();
    }

    /// The review prompts card: one row per prompt, in the dialog's order,
    /// with its model and its role, and the editor of the one open.
    pub(super) fn review_prompts_card(
        &self,
        entry: &Entry,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let review = Settings::get(cx).t3_review.clone();
        let prompts = review.prompts();
        let status = match &self.review.providers {
            Providers::NotConnected => Some((
                "Connect T3 Code above to pick from the models of its server.".to_owned(),
                colors.muted,
            )),
            Providers::Loading => Some((
                "Reading the models of the T3 server…".to_owned(),
                colors.muted,
            )),
            Providers::Failed(error) => Some((
                format!("Could not read the models of the T3 server: {error}"),
                colors.danger,
            )),
            Providers::Loaded(_) => None,
        };
        v_flex()
            .id("t3-review-prompts")
            .debug_selector(|| "t3-review-prompts".to_owned())
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
                        Button::new("t3-review-prompt-add")
                            .ghost()
                            .control()
                            .icon(IconName::Plus)
                            .label("Add prompt")
                            .accessibility_label("Add a review prompt")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_prompt(window, cx)),
                            ),
                    ),
            )
            .child(message(entry.description.into(), colors.muted))
            .children(status.map(|(text, color)| message(text.into(), color)))
            .child(
                v_flex()
                    .w_full()
                    .children(prompts.iter().enumerate().map(|(index, prompt)| {
                        self.prompt_row(index, prompt, &review, prompts.len(), colors, cx)
                    })),
            )
    }

    fn prompt_row(
        &self,
        index: usize,
        prompt: &ReviewPrompt,
        review: &crate::t3::ReviewSettings,
        count: usize,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = prompt_name(prompt);
        let providers = self.review.providers.list();
        let is_default = review.starting_index(false) == index;
        let is_risk = review.risk_check
            && review.starting_index(true) == index
            && review
                .prompts()
                .iter()
                .any(|each| each.id == review.risk_prompt_id());
        let editing = self
            .review
            .editor
            .as_ref()
            .filter(|editor| editor.id == prompt.id);
        let confirming = self.review.confirm_delete.as_deref() == Some(prompt.id.as_str());
        let model_line = match (&self.review.providers, model_choice(providers, prompt)) {
            (Providers::Loaded(_), Ok(choice)) => {
                message(model_summary(providers, &choice).into(), colors.muted)
            }
            (Providers::Loaded(_), Err(why)) => h_flex()
                .gap_1p5()
                .items_start()
                .text_size(scale::meta())
                .text_color(colors.danger)
                .child(
                    Icon::new(IconName::TriangleAlert)
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
            _ => message(
                [Some(prompt.model.clone()), prompt.effort.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
                    .into(),
                colors.muted,
            ),
        };
        let id = prompt.id.clone();
        let actions = if confirming {
            h_flex()
                .gap_1()
                .items_center()
                .child(message("Delete?".into(), colors.danger))
                .child(
                    Button::new(("t3-prompt-delete-cancel", index))
                        .ghost()
                        .xsmall()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.review.confirm_delete = None;
                            cx.notify();
                        })),
                )
                .child({
                    let id = id.clone();
                    Button::new(("t3-prompt-delete-confirm", index))
                        .danger()
                        .outline()
                        .xsmall()
                        .label("Delete")
                        .accessibility_label(format!("Delete {name}"))
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_prompt(&id, cx)))
                })
                .into_any_element()
        } else {
            let this = cx.entity().downgrade();
            let (up, down, edit, more) = (id.clone(), id.clone(), id.clone(), id.clone());
            h_flex()
                .gap_0p5()
                .items_center()
                .child(
                    ui::icon_button(
                        ("t3-prompt-up", index),
                        IconName::ChevronUp,
                        format!("Move {name} up"),
                    )
                    .disabled(index == 0)
                    .on_click(move |_, _, cx| {
                        store::update_settings(cx, |settings| {
                            settings.t3_review.move_prompt(&up, true)
                        });
                    }),
                )
                .child(
                    ui::icon_button(
                        ("t3-prompt-down", index),
                        IconName::ChevronDown,
                        format!("Move {name} down"),
                    )
                    .disabled(index + 1 == count)
                    .on_click(move |_, _, cx| {
                        store::update_settings(cx, |settings| {
                            settings.t3_review.move_prompt(&down, false)
                        });
                    }),
                )
                .child(
                    ui::icon_button(
                        ("t3-prompt-edit", index),
                        IconName::Pencil,
                        format!("Edit {name}"),
                    )
                    .selected(editing.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_prompt_editor(&edit, window, cx)
                    })),
                )
                .child(
                    ui::icon_button(
                        ("t3-prompt-more", index),
                        IconName::Ellipsis,
                        format!("More actions for {name}"),
                    )
                    .menu_trigger()
                    .dropdown_menu_with_anchor(
                        Anchor::TopRight,
                        move |menu, _, _| {
                            let (default, risk, delete) =
                                (more.clone(), more.clone(), more.clone());
                            let this_delete = this.clone();
                            menu.item(
                                PopupMenuItem::new("Start the dialog on it")
                                    .checked(is_default)
                                    .on_click(move |_, _, cx| {
                                        store::update_settings(cx, |settings| {
                                            settings.t3_review.set_default_prompt(&default)
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Use for risky pull requests")
                                    .checked(is_risk)
                                    .on_click(move |_, _, cx| {
                                        store::update_settings(cx, |settings| {
                                            settings.t3_review.risk_check = true;
                                            settings.t3_review.set_risk_prompt(&risk)
                                        });
                                    }),
                            )
                            .item(PopupMenuItem::separator())
                            .item(
                                PopupMenuItem::element(move |_, cx| {
                                    // A name of its own: the menu row around a
                                    // custom element has none.
                                    h_flex()
                                        .id("t3-prompt-delete-item")
                                        .aria_label("Delete")
                                        .gap_2()
                                        .items_center()
                                        .text_color(if count > 1 {
                                            cx.theme().danger
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .child(Icon::new(IconName::Trash).small())
                                        .child(if count > 1 {
                                            "Delete"
                                        } else {
                                            "Delete (the last prompt stays)"
                                        })
                                })
                                .disabled(count < 2)
                                .on_click(move |_, _, cx| {
                                    this_delete
                                        .update(cx, |this, cx| {
                                            this.review.confirm_delete = Some(delete.clone());
                                            cx.notify();
                                        })
                                        .ok();
                                }),
                            )
                        },
                    ),
                )
                .into_any_element()
        };
        let chip = |label: &'static str| ui::chip().child(label);
        v_flex()
            .id(("t3-prompt", index))
            .debug_selector({
                let id = id.clone();
                move || format!("t3-prompt-{id}")
            })
            .w_full()
            .py_3()
            .gap_3()
            .border_t_1()
            .border_color(tokens::border_subtle())
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(scale::body())
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(colors.foreground)
                                            .child(name.clone()),
                                    )
                                    .when(is_default, |row| row.child(chip("Default")))
                                    .when(is_risk, |row| row.child(chip("Risky pull requests"))),
                            )
                            .child(model_line),
                    )
                    .child(div().flex_none().child(actions)),
            )
            .children(editing.map(|editor| self.prompt_editor(editor, prompt, colors)))
            .into_any_element()
    }

    /// The open prompt's fields: its name, its model and effort, and its
    /// text.
    fn prompt_editor(
        &self,
        editor: &PromptEditor,
        prompt: &ReviewPrompt,
        colors: Palette,
    ) -> AnyElement {
        let providers = self.review.providers.list();
        let model = offered(providers, prompt).map(|(_, model)| model.clone());
        let efforts: Vec<(Option<String>, String)> = {
            let default = model
                .as_ref()
                .and_then(|model| model.efforts.iter().find(|effort| effort.is_default))
                .map_or_else(
                    || "Default".to_owned(),
                    |effort| format!("Default ({})", effort.label),
                );
            let mut efforts = vec![(None, default)];
            match &model {
                Some(model) => efforts.extend(
                    model
                        .efforts
                        .iter()
                        .map(|effort| (Some(effort.id.clone()), effort.label.clone())),
                ),
                None => efforts.extend(prompt.effort.clone().map(|id| (Some(id.clone()), id))),
            }
            efforts
        };
        let current = prompt.effort.clone();
        let effort_label = efforts.iter().find(|(id, _)| *id == current).map_or_else(
            || current.clone().unwrap_or_default(),
            |(_, label)| label.clone(),
        );
        let id = prompt.id.clone();
        let label = |text: &'static str| {
            div()
                .text_size(scale::meta())
                .text_color(colors.muted)
                .child(text)
        };
        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex().gap_1().child(label("Name")).child(
                    Input::new(&editor.name)
                        .aria_label("Prompt name")
                        .bg(tokens::background()),
                ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(label("Model"))
                            .child(
                                Select::new(&editor.model)
                                    .placeholder(prompt.model.clone())
                                    .search_placeholder("Search models")
                                    .accessibility_label("Model")
                                    .menu_max_h(dp(320.))
                                    .disabled(providers.is_empty()),
                            ),
                    )
                    .child(
                        v_flex().flex_none().gap_1().child(label("Effort")).child(
                            Button::new("t3-prompt-effort")
                                .outline()
                                .small()
                                .label(effort_label.clone())
                                .dropdown_caret(true)
                                .accessibility_label(format!("Effort: {effort_label}"))
                                .menu_trigger()
                                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                    efforts.iter().fold(
                                        menu.min_w(dp(160.)),
                                        |menu, (effort, label)| {
                                            let (effort, id) = (effort.clone(), id.clone());
                                            menu.item(
                                                PopupMenuItem::new(label.clone())
                                                    .checked(effort == current)
                                                    .on_click(move |_, _, cx| {
                                                        let effort = effort.clone();
                                                        store::update_settings(cx, |settings| {
                                                            settings
                                                                .t3_review
                                                                .update_prompt(&id, |prompt| {
                                                                    prompt.effort = effort
                                                                })
                                                        });
                                                    }),
                                            )
                                        },
                                    )
                                }),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Text"))
                    .child(
                        Textarea::new(&editor.text)
                            .aria_label("Prompt text")
                            .bg(tokens::background()),
                    )
                    .child(message(
                        "Sent where the review request has {prompt}. It can use the request's \
                         variables, such as {risk}."
                            .into(),
                        colors.muted,
                    )),
            )
            .into_any_element()
    }

    /// The prompt a risky pull request starts on, picked from the prompts.
    pub(super) fn risk_prompt_row(&self, entry: &Entry, colors: Palette, cx: &App) -> AnyElement {
        let review = Settings::get(cx).t3_review.clone();
        let prompts = review.prompts();
        let current = review.starting_index(true);
        let name = prompts.get(current).map(prompt_name).unwrap_or_default();
        setting_row(
            entry.title,
            entry.description,
            Button::new("t3-risk-prompt")
                .outline()
                .control()
                .label(name.clone())
                .dropdown_caret(true)
                .accessibility_label(format!("{}: {name}", entry.title))
                .disabled(!review.risk_check)
                .menu_trigger()
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                    prompts.iter().enumerate().fold(
                        menu.min_w(dp(200.)),
                        |menu, (index, prompt)| {
                            let id = prompt.id.clone();
                            menu.item(
                                PopupMenuItem::new(prompt_name(prompt))
                                    .checked(index == current)
                                    .on_click(move |_, _, cx| {
                                        store::update_settings(cx, |settings| {
                                            settings.t3_review.set_risk_prompt(&id)
                                        });
                                    }),
                            )
                        },
                    )
                }),
            colors,
        )
        .into_any_element()
    }
}
