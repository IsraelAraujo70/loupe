//! Settings > Appearance: the color scheme, the themes (create, add,
//! duplicate, edit, export, delete), the interface (contrast, glass opacity,
//! density, zoom), and the typography, with a live preview of each font.
//! Every change goes through [`theme::apply`], so the whole app repaints at
//! once.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::view::{Palette as Colors, SettingsView, message};
use crate::github::highlight::highlight_lines;
use crate::settings::catalog::{Entry, Item};
use crate::store::{self, Settings};
use crate::theme::palette::{Color, Contrast, Palette, TOKENS, ThemeFile};
use crate::theme::scale::{self, dp};
use crate::theme::themes::{self, DEFAULT_THEME, ThemeEntry, Themes};
use crate::theme::{self, Appearance, CODE_FONT, ColorScheme, Density, GLASS, UI_FONT, tokens};
use crate::ui::{self, ControlSize as _, MenuTriggerExt as _};

/// What the Appearance section keeps between frames: its sliders, the
/// theme being edited, the theme being added, and the last outcome.
pub(super) struct AppearanceState {
    contrast: Entity<SliderState>,
    glass: Entity<SliderState>,
    editor: Option<ThemeEditor>,
    import: Option<Import>,
    /// The user theme whose Delete waits for a confirmation.
    confirm_delete: Option<String>,
    notice: Option<Notice>,
    _subscriptions: Vec<Subscription>,
}

/// The outcome of the last theme action, shown under the themes.
enum Notice {
    Done(SharedString),
    Failed(SharedString),
}

/// The editor of one user theme: its name and a field per token of the
/// palette shown, which saves and repaints on every valid color.
struct ThemeEditor {
    id: String,
    name: Entity<InputState>,
    /// One field per [`TOKENS`] entry, for the variant shown.
    fields: Vec<Entity<InputState>>,
    dark: bool,
    _subscriptions: Vec<Subscription>,
}

/// "Add theme": a theme file's JSON pasted or read from a file.
struct Import {
    json: Entity<TextareaState>,
    error: Option<SharedString>,
}

impl AppearanceState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let appearance = Settings::get(cx).appearance.clone();
        let contrast = cx.new(|_| {
            SliderState::new()
                .min(f32::from(Contrast::MIN))
                .max(f32::from(Contrast::MAX))
                .step(5.)
                .default_value(f32::from(appearance.contrast))
        });
        let glass = cx.new(|_| {
            SliderState::new()
                .min(f32::from(Appearance::GLASS_MIN))
                .max(f32::from(Appearance::GLASS_MAX))
                .step(5.)
                .default_value(f32::from(appearance.glass_opacity))
        });
        let subscriptions = vec![
            cx.subscribe_in(&contrast, window, |_, _, event: &SliderEvent, _, cx| {
                slider_changed(event, cx, |appearance, value| appearance.contrast = value);
            }),
            cx.subscribe_in(&glass, window, |_, _, event: &SliderEvent, _, cx| {
                slider_changed(event, cx, |appearance, value| {
                    appearance.glass_opacity = value
                });
            }),
        ];
        AppearanceState {
            contrast,
            glass,
            editor: None,
            import: None,
            confirm_delete: None,
            notice: None,
            _subscriptions: subscriptions,
        }
    }

    /// Puts the sliders where the settings are, after Restore defaults.
    pub(super) fn sync(&self, window: &mut Window, cx: &mut App) {
        let appearance = Settings::get(cx).appearance.clone();
        self.contrast.update(cx, |slider, cx| {
            slider.set_value(f32::from(appearance.contrast), window, cx)
        });
        self.glass.update(cx, |slider, cx| {
            slider.set_value(f32::from(appearance.glass_opacity), window, cx)
        });
    }
}

/// A slider moved: the app repaints while it moves and the value is saved
/// when it is let go.
fn slider_changed(event: &SliderEvent, cx: &mut App, set: fn(&mut Appearance, u16)) {
    let (value, save) = match event {
        SliderEvent::Change(value) => (value.start(), false),
        SliderEvent::Release(value) => (value.start(), true),
    };
    let value = value.round() as u16;
    if save {
        store::update_settings(cx, |settings| set(&mut settings.appearance, value));
    } else {
        set(&mut cx.global_mut::<Settings>().appearance, value);
    }
    theme::apply(cx);
}

/// Changes the appearance, saves it, and repaints every window.
fn set_appearance(cx: &mut App, change: impl FnOnce(&mut Appearance)) {
    store::update_settings(cx, |settings| change(&mut settings.appearance));
    theme::apply(cx);
}

/// Reads the theme files again and repaints.
fn reload_themes(cx: &mut App) {
    cx.set_global(Themes::load(&themes::themes_dir()));
    theme::apply(cx);
}

/// Saves `file` as a new user theme named after it and shows it. Returns
/// its id.
fn add_theme(mut file: ThemeFile, cx: &mut App) -> Result<String, String> {
    let list = Themes::get(cx);
    file.name = file.name.trim().to_owned();
    // Two themes with one name could not be told apart on their cards.
    if list
        .entries
        .iter()
        .any(|entry| entry.file.name == file.name)
    {
        file.name = list.copy_name(&file.name);
    }
    let id = list.free_id(&file.name, &themes::themes_dir());
    themes::save(&themes::themes_dir(), &id, &file).map_err(|error| error.to_string())?;
    cx.set_global(Themes::load(&themes::themes_dir()));
    let chosen = id.clone();
    set_appearance(cx, move |appearance| appearance.theme = chosen);
    Ok(id)
}

/// A copy of the theme `id` under a free name, shown at once.
fn duplicate_theme(id: &str, cx: &mut App) -> Result<String, String> {
    let list = Themes::get(cx);
    let mut file = list.resolve(id).file.clone();
    file.name = list.copy_name(&file.name);
    add_theme(file, cx)
}

impl SettingsView {
    /// The control or block of one Appearance setting.
    pub(super) fn appearance_row(
        &mut self,
        entry: &Entry,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let appearance = Settings::get(cx).appearance.clone();
        let row = |control: AnyElement| {
            super::view::setting_row(entry.title, entry.description, control, colors)
                .into_any_element()
        };
        match entry.item {
            Item::ColorScheme => self.color_schemes(entry, &appearance, colors, cx),
            Item::Themes => self.themes_block(entry, &appearance, colors, cx),
            Item::Contrast => row(slider_control(
                &self.appearance.contrast,
                "Contrast",
                appearance.contrast,
                false,
                colors,
            )),
            Item::GlassOpacity => super::view::setting_row(
                entry.title,
                if GLASS {
                    entry.description
                } else {
                    "Only on macOS, where the window is frosted glass."
                },
                slider_control(
                    &self.appearance.glass,
                    "Glass opacity",
                    appearance.glass_opacity,
                    !GLASS,
                    colors,
                ),
                colors,
            )
            .into_any_element(),
            Item::Density => row(ui::segmented(
                "density",
                "Density",
                Density::ALL.map(|density| (None, density.label())),
                Density::ALL
                    .iter()
                    .position(|density| *density == appearance.density)
                    .unwrap_or_default(),
                |index, _, cx| {
                    if let Some(density) = Density::ALL.get(index).copied() {
                        set_appearance(cx, |appearance| appearance.density = density);
                    }
                },
            )
            .into_any_element()),
            Item::InterfaceFont => font_block(entry, &appearance, false, colors, cx),
            Item::MonoFont => font_block(entry, &appearance, true, colors, cx),
            _ => div().into_any_element(),
        }
    }

    /// Three thumbnails, System, Light, and Dark, each drawn in the
    /// current theme's palettes; the chosen one has the accent outline.
    fn color_schemes(
        &self,
        entry: &Entry,
        appearance: &Appearance,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let file = theme::current_theme(cx);
        let cards = ColorScheme::ALL
            .into_iter()
            .enumerate()
            .map(|(index, scheme)| {
                let selected = appearance.color_scheme == scheme;
                let thumbnail = match scheme {
                    ColorScheme::System => h_flex()
                        .size_full()
                        .child(
                            div()
                                .w(relative(0.5))
                                .h_full()
                                .overflow_hidden()
                                .child(scheme_thumbnail(&file.light, false)),
                        )
                        .child(
                            div()
                                .w(relative(0.5))
                                .h_full()
                                .overflow_hidden()
                                .child(scheme_thumbnail(&file.dark, true)),
                        )
                        .into_any_element(),
                    ColorScheme::Light => scheme_thumbnail(&file.light, false).into_any_element(),
                    ColorScheme::Dark => scheme_thumbnail(&file.dark, false).into_any_element(),
                };
                v_flex()
                    .id(("color-scheme", index))
                    .debug_selector(move || {
                        format!("color-scheme-{}", scheme.label().to_lowercase())
                    })
                    .role(Role::RadioButton)
                    .aria_label(format!("{} color scheme", scheme.label()))
                    .aria_toggled(if selected {
                        accesskit::Toggled::True
                    } else {
                        accesskit::Toggled::False
                    })
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(if selected {
                        tokens::accent()
                    } else {
                        colors.border
                    })
                    .bg(colors.card)
                    .on_click(move |_, _, cx| {
                        set_appearance(cx, move |appearance| appearance.color_scheme = scheme)
                    })
                    .child(
                        div()
                            .h(dp(88.))
                            .w_full()
                            .rounded(dp(6.))
                            .overflow_hidden()
                            .child(thumbnail),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(scale::body())
                            .text_color(if selected {
                                colors.foreground
                            } else {
                                colors.muted
                            })
                            .child(scheme.label()),
                    )
            });
        v_flex()
            .w_full()
            .gap_3()
            .child(ui::section_label(entry.title, cx))
            .child(h_flex().w_full().gap_3().children(cards))
            .into_any_element()
    }

    /// The themes: a card per theme with its light and dark previews and
    /// its actions, Create theme and Add theme in the heading, the theme
    /// being added or edited under the cards.
    fn themes_block(
        &mut self,
        entry: &Entry,
        appearance: &Appearance,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let list = Themes::get(cx);
        let chosen = list.resolve(&appearance.theme).id.clone();
        let dark = theme::is_dark();
        let heading = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(ui::section_label(entry.title, cx).flex_1())
            .child(
                div().debug_selector(|| "create-theme".to_owned()).child(
                    Button::new("create-theme")
                        .outline()
                        .control()
                        .icon(IconName::Paintbrush)
                        .label("Create theme")
                        .tooltip("Start a theme from the one shown and edit it")
                        .on_click(cx.listener(|this, _, window, cx| this.create_theme(window, cx))),
                ),
            )
            .child(
                Button::new("add-theme")
                    .outline()
                    .control()
                    .icon(IconName::Plus)
                    .label("Add theme")
                    .tooltip("Add a theme from its JSON file")
                    .on_click(cx.listener(|this, _, window, cx| this.open_import(window, cx))),
            );
        let cards: Vec<AnyElement> = list
            .entries
            .iter()
            .enumerate()
            .map(|(index, theme)| {
                self.theme_card(index, theme, theme.id == chosen, dark, colors, cx)
            })
            .collect();
        let notice = self.appearance.notice.as_ref().map(|notice| match notice {
            Notice::Done(text) => message(text.clone(), colors.muted),
            Notice::Failed(text) => message(text.clone(), colors.danger),
        });
        v_flex()
            .w_full()
            .gap_3()
            .child(heading)
            .when_some(self.import_card(colors, cx), |block, card| {
                block.child(card)
            })
            .child(div().w_full().grid().grid_cols(3).gap_3().children(cards))
            .children(notice)
            .when_some(self.editor_card(&list, colors, cx), |block, card| {
                block.child(card)
            })
            .into_any_element()
    }

    fn theme_card(
        &self,
        index: usize,
        entry: &ThemeEntry,
        selected: bool,
        dark: bool,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id.clone();
        let name: SharedString = entry.file.name.clone().into();
        let confirming = self.appearance.confirm_delete.as_deref() == Some(entry.id.as_str());
        let orb = |palette: &Palette, is_dark: bool| {
            let active = selected && is_dark == dark;
            v_flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .size(dp(44.))
                        .rounded_full()
                        .p(dp(3.))
                        .border_2()
                        .border_color(if active {
                            tokens::accent()
                        } else {
                            transparent_black()
                        })
                        .child(
                            div()
                                .size_full()
                                .rounded_full()
                                .border_1()
                                .border_color(palette.border.0)
                                .bg(linear_gradient(
                                    135.,
                                    linear_color_stop(palette.accent.0, 0.),
                                    linear_color_stop(palette.panel.0, 0.85),
                                )),
                        ),
                )
                .child(
                    svg()
                        .path(if is_dark {
                            IconName::Moon.path()
                        } else {
                            IconName::Sun.path()
                        })
                        .size_3()
                        .text_color(colors.faint),
                )
        };
        let actions = if confirming {
            h_flex()
                .gap_1()
                .items_center()
                .child(message("Delete?".into(), colors.danger))
                .child(
                    Button::new(("theme-delete-cancel", index))
                        .ghost()
                        .xsmall()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.appearance.confirm_delete = None;
                            cx.notify();
                        })),
                )
                .child({
                    let id = id.clone();
                    Button::new(("theme-delete-confirm", index))
                        .danger()
                        .outline()
                        .xsmall()
                        .label("Delete")
                        .accessibility_label(format!("Delete {name}"))
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_theme(&id, cx)))
                })
                .into_any_element()
        } else {
            h_flex()
                .gap_0p5()
                .items_center()
                .child({
                    let id = id.clone();
                    ui::icon_button(
                        ("theme-duplicate", index),
                        IconName::Copy,
                        format!("Duplicate {name}"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.appearance.notice = Some(match duplicate_theme(&id, cx) {
                            Ok(_) => {
                                Notice::Done("Duplicated. Edit the copy with its pencil.".into())
                            }
                            Err(error) => {
                                Notice::Failed(format!("Could not save the copy: {error}").into())
                            }
                        });
                        cx.notify();
                    }))
                })
                .when(!entry.built_in, |row| {
                    let edit = id.clone();
                    let this = cx.entity().downgrade();
                    let more_id = id.clone();
                    row.child(
                        ui::icon_button(
                            ("theme-edit", index),
                            IconName::Pencil,
                            format!("Edit {name}"),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.open_editor(&edit, window, cx),
                        )),
                    )
                    .child(
                        ui::icon_button(
                            ("theme-more", index),
                            IconName::Ellipsis,
                            format!("More actions for {name}"),
                        )
                        .menu_trigger()
                        .dropdown_menu_with_anchor(
                            Anchor::TopRight,
                            move |menu, _, _| {
                                let (export, delete) = (more_id.clone(), more_id.clone());
                                let (this_export, this_delete) = (this.clone(), this.clone());
                                menu.item(
                                    PopupMenuItem::new("Export…")
                                        .icon(IconName::Upload)
                                        .on_click(move |_, _, cx| {
                                            this_export
                                                .update(cx, |this, cx| {
                                                    this.export_theme(&export, cx)
                                                })
                                                .ok();
                                        }),
                                )
                                .item(PopupMenuItem::separator())
                                .item(
                                    PopupMenuItem::element(|_, cx| {
                                        // A name of its own: the menu row
                                        // around a custom element has none.
                                        h_flex()
                                            .id("theme-delete-item")
                                            .aria_label("Delete")
                                            .gap_2()
                                            .items_center()
                                            .text_color(cx.theme().danger)
                                            .child(Icon::new(IconName::Trash).small())
                                            .child("Delete")
                                    })
                                    .on_click(
                                        move |_, _, cx| {
                                            this_delete
                                                .update(cx, |this, cx| {
                                                    this.appearance.confirm_delete =
                                                        Some(delete.clone());
                                                    cx.notify();
                                                })
                                                .ok();
                                        },
                                    ),
                                )
                            },
                        ),
                    )
                })
                .into_any_element()
        };
        let pick = id.clone();
        v_flex()
            .id(("theme-card", index))
            .debug_selector({
                let id = id.clone();
                move || format!("theme-card-{id}")
            })
            .role(Role::RadioButton)
            .aria_label(format!("Theme: {name}"))
            .aria_toggled(if selected {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            })
            .min_w_0()
            .p_3()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(if selected {
                tokens::accent()
            } else {
                colors.border
            })
            .bg(colors.card)
            .hover(|style| style.border_color(tokens::accent().opacity(0.6)))
            .on_click(move |_, _, cx| {
                let pick = pick.clone();
                set_appearance(cx, move |appearance| appearance.theme = pick);
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .gap_4()
                    .child(orb(&entry.file.light, false))
                    .child(orb(&entry.file.dark, true)),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(dp(24.))
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(scale::body())
                            .text_color(colors.foreground)
                            .child(name.clone()),
                    )
                    .child(
                        // A click on an action is not a click on the card,
                        // which would show the theme just deleted or copied.
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("theme-actions-{id}")
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(actions),
                    ),
            )
            .into_any_element()
    }

    /// "Create theme": a copy of the theme shown, opened in the editor.
    fn create_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = Themes::get(cx);
        let mut file = list
            .resolve(&Settings::get(cx).appearance.theme)
            .file
            .clone();
        file.name = {
            let mut name = "Custom theme".to_owned();
            let mut n = 2;
            while list.entries.iter().any(|entry| entry.file.name == name) {
                name = format!("Custom theme {n}");
                n += 1;
            }
            name
        };
        match add_theme(file, cx) {
            Ok(id) => {
                self.appearance.notice = None;
                self.open_editor(&id, window, cx);
            }
            Err(error) => {
                self.appearance.notice = Some(Notice::Failed(
                    format!("Could not save the theme: {error}").into(),
                ));
            }
        }
        cx.notify();
    }

    fn delete_theme(&mut self, id: &str, cx: &mut Context<Self>) {
        self.appearance.confirm_delete = None;
        if self
            .appearance
            .editor
            .as_ref()
            .is_some_and(|editor| editor.id == id)
        {
            self.appearance.editor = None;
        }
        let name = Themes::get(cx)
            .find(id)
            .map(|entry| entry.file.name.clone())
            .unwrap_or_default();
        match themes::remove(&themes::themes_dir(), id) {
            Ok(()) => {
                if Settings::get(cx).appearance.theme == id {
                    store::update_settings(cx, |settings| {
                        settings.appearance.theme = DEFAULT_THEME.to_owned()
                    });
                }
                reload_themes(cx);
                self.appearance.notice = Some(Notice::Done(format!("Deleted {name}.").into()));
            }
            Err(error) => {
                self.appearance.notice = Some(Notice::Failed(
                    format!("Could not delete {name}: {error}").into(),
                ));
            }
        }
        cx.notify();
    }

    /// "Export": saves the theme's JSON where the user picks, or copies it
    /// to the clipboard where there is no file picker.
    fn export_theme(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(entry) = Themes::get(cx).find(id).cloned() else {
            return;
        };
        let json = entry.file.to_json();
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let picked = cx.prompt_for_new_path(&home, Some(&format!("{id}.json")));
        cx.spawn(async move |this, cx| {
            let notice = match picked.await {
                Ok(Ok(Some(path))) => match std::fs::write(&path, &json) {
                    Ok(()) => Notice::Done(format!("Exported to {}.", path.display()).into()),
                    Err(error) => Notice::Failed(
                        format!("Could not write {}: {error}", path.display()).into(),
                    ),
                },
                Ok(Ok(None)) => return,
                // No file picker (a Linux desktop without the portal).
                Ok(Err(_)) | Err(_) => {
                    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(json.clone())));
                    Notice::Done(
                        format!("Copied the JSON of {} to the clipboard.", entry.file.name).into(),
                    )
                }
            };
            this.update(cx, |this, cx| {
                this.appearance.notice = Some(notice);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance.import.is_some() {
            return;
        }
        let json = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 10)
                .placeholder("Paste a theme's JSON: a name, a light palette, and a dark palette")
        });
        self.appearance.import = Some(Import { json, error: None });
        self.appearance.notice = None;
        cx.notify();
    }

    /// Adds the pasted theme, or says what is wrong with it.
    fn submit_import(&mut self, cx: &mut Context<Self>) {
        let Some(import) = self.appearance.import.as_mut() else {
            return;
        };
        let text = import.json.read(cx).value().to_string();
        let fallback = Themes::get(cx).resolve(DEFAULT_THEME).file.clone();
        match ThemeFile::parse_user(&text, &fallback) {
            Ok(file) => {
                let name = file.name.clone();
                match add_theme(file, cx) {
                    Ok(_) => {
                        self.appearance.import = None;
                        self.appearance.notice =
                            Some(Notice::Done(format!("Added {name}.").into()));
                    }
                    Err(error) => {
                        if let Some(import) = self.appearance.import.as_mut() {
                            import.error = Some(format!("Could not save it: {error}").into());
                        }
                    }
                }
            }
            Err(error) => import.error = Some(format!("Not a theme: {error}").into()),
        }
        cx.notify();
    }

    /// "Choose file…": reads a theme file into the field.
    fn choose_import_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add theme".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = picked.await;
            this.update_in(cx, |this, window, cx| {
                let Some(import) = this.appearance.import.as_mut() else {
                    return;
                };
                match result {
                    Ok(Ok(Some(paths))) => {
                        let Some(path) = paths.first() else { return };
                        match std::fs::read_to_string(path) {
                            Ok(text) => {
                                import.error = None;
                                import
                                    .json
                                    .update(cx, |field, cx| field.set_value(text, window, cx));
                            }
                            Err(error) => {
                                import.error = Some(
                                    format!("Could not read {}: {error}", path.display()).into(),
                                )
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(_)) | Err(_) => {
                        import.error = Some(
                            "There is no file picker here. Paste the theme's JSON instead.".into(),
                        )
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn import_card(&mut self, colors: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let import = self.appearance.import.as_ref()?;
        Some(
            ui::card(cx)
                .gap_3()
                .child(ui::group_title("Add theme"))
                .child(Textarea::new(&import.json).aria_label("Theme JSON"))
                .when_some(import.error.clone(), |card, error| {
                    card.child(message(error, colors.danger))
                })
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("import-file")
                                .outline()
                                .control()
                                .icon(IconName::FolderOpen)
                                .label("Choose file…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_import_file(window, cx)
                                })),
                        )
                        .child(
                            Button::new("import-cancel")
                                .ghost()
                                .control()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.appearance.import = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("import-add")
                                .primary()
                                .control()
                                .label("Add theme")
                                .on_click(cx.listener(|this, _, _, cx| this.submit_import(cx))),
                        ),
                ),
        )
    }

    /// Opens the editor on the user theme `id`, for the palette on screen.
    fn open_editor(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = Themes::get(cx).find(id).cloned() else {
            return;
        };
        self.appearance.editor = Some(ThemeEditor::new(entry, theme::is_dark(), window, cx));
        cx.notify();
    }

    fn editor_card(&self, list: &Themes, colors: Colors, cx: &mut Context<Self>) -> Option<Div> {
        let editor = self.appearance.editor.as_ref()?;
        let entry = list.find(&editor.id)?;
        let palette = entry.file.variant(editor.dark);
        let mut body = v_flex().w_full().gap_3();
        let mut group = "";
        let mut rows: Vec<AnyElement> = Vec::new();
        let flush = |body: Div, group: &'static str, rows: &mut Vec<AnyElement>, cx: &App| {
            if rows.is_empty() {
                return body;
            }
            body.child(
                v_flex().gap_1().child(ui::section_label(group, cx)).child(
                    div()
                        .w_full()
                        .grid()
                        .grid_cols(2)
                        .gap_x_4()
                        .gap_y_1()
                        .children(rows.drain(..)),
                ),
            )
        };
        for (index, token) in TOKENS.iter().enumerate() {
            if token.group != group {
                body = flush(body, group, &mut rows, cx);
                group = token.group;
            }
            let color = (token.get)(palette);
            let Some(field) = editor.fields.get(index) else {
                continue;
            };
            rows.push(
                h_flex()
                    .h(dp(32.))
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size(dp(16.))
                            .rounded(dp(4.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(color.0),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(scale::body())
                            .text_color(colors.foreground)
                            .child(token.key.replace('_', " ")),
                    )
                    .child(
                        div().flex_none().w(dp(112.)).child(
                            Input::new(field)
                                .small()
                                .aria_label(token.key.replace('_', " ")),
                        ),
                    )
                    .into_any_element(),
            );
        }
        body = flush(body, group, &mut rows, cx);
        let variant = usize::from(editor.dark);
        Some(
            ui::card(cx)
                .gap_4()
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .child(ui::group_title("Edit theme").flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&editor.name).small().aria_label("Theme name")),
                        )
                        .child(ui::segmented(
                            "editor-variant",
                            "Palette",
                            [(Some(IconName::Sun), "Light"), (Some(IconName::Moon), "Dark")],
                            variant,
                            |index, _, cx| {
                                let scheme = if index == 1 {
                                    ColorScheme::Dark
                                } else {
                                    ColorScheme::Light
                                };
                                set_appearance(cx, |appearance| appearance.color_scheme = scheme);
                            },
                        ))
                        .child(
                            Button::new("editor-done")
                                .primary()
                                .control()
                                .label("Done")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.appearance.editor = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(message(
                    "Colors are #rrggbb or #rrggbbaa. Each one is saved and shown as you type. Syntax colors live in the theme's JSON."
                        .into(),
                    colors.muted,
                ))
                .child(body),
        )
    }

    /// Keeps the editor on the palette on screen, so switching Light and
    /// Dark (in the editor or in Color scheme) edits what is shown.
    pub(super) fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = theme::is_dark();
        let Some(editor) = self.appearance.editor.as_ref() else {
            return;
        };
        // Another theme on screen (a card picked, Restore defaults): edits
        // would land on a theme that is not shown, so the editor closes.
        if Themes::get(cx)
            .resolve(&Settings::get(cx).appearance.theme)
            .id
            != editor.id
        {
            self.appearance.editor = None;
            return;
        }
        if editor.dark == dark {
            return;
        }
        let id = editor.id.clone();
        match Themes::get(cx).find(&id).cloned() {
            Some(entry) => self.appearance.editor = Some(ThemeEditor::new(entry, dark, window, cx)),
            None => self.appearance.editor = None,
        }
    }
}

impl ThemeEditor {
    fn new(
        entry: ThemeEntry,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> Self {
        let palette = entry.file.variant(dark).clone();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(entry.file.name.clone()));
        let fields: Vec<Entity<InputState>> = TOKENS
            .iter()
            .map(|token| {
                let value = (token.get)(&palette).hex();
                cx.new(|cx| InputState::new(window, cx).default_value(value))
            })
            .collect();
        let mut subscriptions = vec![cx.subscribe_in(
            &name,
            window,
            |this: &mut SettingsView, field, event: &InputEvent, _, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let name = field.read(cx).value().trim().to_owned();
                let editing = this
                    .appearance
                    .editor
                    .as_ref()
                    .map(|editor| editor.id.clone());
                // Two cards with one name could not be told apart.
                let taken = Themes::get(cx)
                    .entries
                    .iter()
                    .any(|entry| entry.file.name == name && Some(&entry.id) != editing.as_ref());
                if !name.is_empty() && !taken {
                    this.edit_theme(cx, |file| file.name = name);
                }
            },
        )];
        for (index, field) in fields.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                move |this: &mut SettingsView, field, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    let Some(color) = Color::parse(&field.read(cx).value()) else {
                        return;
                    };
                    let Some(token) = TOKENS.get(index) else {
                        return;
                    };
                    let dark = this
                        .appearance
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.dark);
                    this.edit_theme(cx, |file| (token.set)(file.variant_mut(dark), color));
                },
            ));
        }
        ThemeEditor {
            id: entry.id,
            name,
            fields,
            dark,
            _subscriptions: subscriptions,
        }
    }
}

impl SettingsView {
    /// Changes the theme being edited, saves it, and repaints.
    fn edit_theme(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ThemeFile)) {
        let Some(id) = self
            .appearance
            .editor
            .as_ref()
            .map(|editor| editor.id.clone())
        else {
            return;
        };
        let mut list = Themes::get(cx);
        let Some(entry) = list.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        change(&mut entry.file);
        if let Err(error) = themes::save(&themes::themes_dir(), &id, &entry.file) {
            self.appearance.notice = Some(Notice::Failed(
                format!("Could not save the theme: {error}").into(),
            ));
        }
        cx.set_global(list);
        theme::apply(cx);
        cx.notify();
    }
}

/// A slider with its value beside it.
fn slider_control(
    state: &Entity<SliderState>,
    name: &'static str,
    value: u16,
    disabled: bool,
    colors: Colors,
) -> AnyElement {
    h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .w(dp(40.))
                .text_right()
                .text_size(scale::meta())
                .text_color(colors.muted)
                .child(format!("{value}%")),
        )
        .child(
            div().w(dp(160.)).child(
                Slider::new(state)
                    .horizontal()
                    .disabled(disabled)
                    .accessibility_label(name),
            ),
        )
        .into_any_element()
}

/// A miniature of the app in `palette`: the sidebar, the panel with a title
/// and lines, a card with state dots, and the composer with the accent.
fn scheme_thumbnail(palette: &Palette, flush_right: bool) -> Div {
    let bar = |width: f32, height: f32, color: Hsla| {
        div().w(dp(width)).h(dp(height)).rounded_full().bg(color)
    };
    let dot = |color: Hsla| div().size(dp(4.)).rounded_full().bg(color);
    h_flex()
        .size_full()
        .bg(palette.window.0)
        .when(flush_right, |row| row.justify_end())
        .child(
            v_flex()
                .flex_none()
                .w(dp(44.))
                .h_full()
                .p(dp(6.))
                .gap(dp(5.))
                .child(
                    bar(30., 6., palette.selected.0)
                        .border_1()
                        .border_color(palette.border.0),
                )
                .child(bar(26., 4., palette.text_faint.0.opacity(0.5)))
                .child(bar(22., 4., palette.text_faint.0.opacity(0.5)))
                .child(bar(28., 4., palette.text_faint.0.opacity(0.5))),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(dp(100.))
                .h_full()
                .my(dp(5.))
                .mr(dp(5.))
                .p(dp(8.))
                .gap(dp(6.))
                .rounded(dp(4.))
                .border_1()
                .border_color(palette.border.0)
                .bg(palette.panel.0)
                .child(
                    h_flex()
                        .gap(dp(6.))
                        .items_start()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap(dp(5.))
                                .child(bar(48., 6., palette.text_muted.0.opacity(0.7)))
                                .child(bar(64., 4., palette.text_faint.0.opacity(0.45)))
                                .child(bar(40., 4., palette.text_faint.0.opacity(0.45))),
                        )
                        .child(
                            v_flex()
                                .flex_none()
                                .p(dp(4.))
                                .gap(dp(4.))
                                .rounded(dp(3.))
                                .border_1()
                                .border_color(palette.border.0)
                                .bg(palette.surface.0)
                                .child(h_flex().gap(dp(3.)).child(dot(palette.open.0)).child(bar(
                                    14.,
                                    3.,
                                    palette.text_faint.0.opacity(0.5),
                                )))
                                .child(
                                    h_flex().gap(dp(3.)).child(dot(palette.merged.0)).child(bar(
                                        14.,
                                        3.,
                                        palette.text_faint.0.opacity(0.5),
                                    )),
                                )
                                .child(
                                    h_flex()
                                        .gap(dp(3.))
                                        .child(dot(palette.pending.0))
                                        .child(bar(14., 3., palette.text_faint.0.opacity(0.5))),
                                ),
                        ),
                )
                .child(div().flex_1())
                .child(
                    h_flex()
                        .h(dp(12.))
                        .px(dp(4.))
                        .items_center()
                        .justify_between()
                        .rounded(dp(3.))
                        .border_1()
                        .border_color(palette.border.0)
                        .bg(palette.surface.0)
                        .child(bar(36., 3., palette.text_faint.0.opacity(0.5)))
                        .child(dot(palette.accent.0)),
                ),
        )
}

/// A typeface row: the font and size menus, and a preview in them.
fn font_block(
    entry: &Entry,
    appearance: &Appearance,
    mono: bool,
    colors: Colors,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let (family, size, sizes, default_family) = if mono {
        (
            appearance.mono_font_family().to_owned(),
            appearance.mono_font_size,
            &Appearance::MONO_FONT_SIZES[..],
            CODE_FONT,
        )
    } else {
        (
            appearance.ui_font_family().to_owned(),
            appearance.ui_font_size,
            &Appearance::UI_FONT_SIZES[..],
            UI_FONT,
        )
    };
    let kind = if mono { "mono" } else { "ui" };
    let current = family.clone();
    let font_menu = Button::new(SharedString::from(format!("{kind}-font")))
        .outline()
        .control()
        .w(dp(184.))
        .label(family.clone())
        .dropdown_caret(true)
        .accessibility_label(format!("{}: {family}", entry.title))
        .menu_trigger()
        .dropdown_menu(move |menu, _, cx| {
            let mut names: Vec<String> = cx.text_system().all_font_names();
            names.sort_by_key(|name| name.to_lowercase());
            names.dedup();
            names.retain(|name| name != default_family && !name.starts_with('.'));
            let mut menu = menu.min_w(dp(220.)).max_h(dp(360.)).scrollable(true);
            for (index, name) in std::iter::once(default_family.to_owned())
                .chain(names)
                .enumerate()
            {
                let label = if index == 0 {
                    format!("{name} (built in)")
                } else {
                    name.clone()
                };
                let checked = name == current;
                menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                    move |_, _, cx| {
                        let chosen = (name != default_family).then(|| name.clone());
                        set_appearance(cx, move |appearance| {
                            if mono {
                                appearance.mono_font = chosen;
                            } else {
                                appearance.ui_font = chosen;
                            }
                        });
                    },
                ));
            }
            menu
        });
    let size_menu = Button::new(SharedString::from(format!("{kind}-font-size")))
        .outline()
        .control()
        .w(dp(80.))
        .label(format!("{size} px"))
        .dropdown_caret(true)
        .accessibility_label(format!("{} size: {size} px", entry.title))
        .menu_trigger()
        .dropdown_menu({
            let sizes = sizes.to_vec();
            move |menu, _, _| {
                sizes.iter().fold(menu.min_w(dp(96.)), |menu, step| {
                    let step = *step;
                    menu.item(
                        PopupMenuItem::new(format!("{step} px"))
                            .checked(step == size)
                            .on_click(move |_, _, cx| {
                                set_appearance(cx, move |appearance| {
                                    if mono {
                                        appearance.mono_font_size = step;
                                    } else {
                                        appearance.ui_font_size = step;
                                    }
                                })
                            }),
                    )
                })
            }
        });
    let preview = if mono {
        diff_preview(colors, cx).into_any_element()
    } else {
        text_preview(colors, cx).into_any_element()
    };
    v_flex()
        .w_full()
        .pb_3()
        .child(super::view::setting_row(
            entry.title,
            entry.description,
            h_flex().gap_2().child(font_menu).child(size_menu),
            colors,
        ))
        .child(preview)
        .into_any_element()
}

fn preview_box(colors: Colors) -> Div {
    v_flex()
        .w_full()
        .rounded(dp(6.))
        .border_1()
        .border_color(colors.border)
        .bg(tokens::panel())
        .overflow_hidden()
}

/// Interface text at the chosen size: a row title, its meta, and a link.
fn text_preview(colors: Colors, cx: &App) -> Div {
    let font = cx.theme().font_family.clone();
    preview_box(colors)
        .px_3()
        .py_2()
        .gap_1()
        .font_family(font)
        .child(
            div()
                .text_size(scale::body())
                .text_color(colors.foreground)
                .child("Review the pull request, then move the issue to Done before the release."),
        )
        .child(
            h_flex()
                .gap_2()
                .text_size(scale::meta())
                .text_color(colors.muted)
                .child("ENG-290")
                .child("·")
                .child(div().text_color(tokens::link()).child("hooli-api#374"))
                .child("·")
                .child("2 hours ago"),
        )
}

/// A diff in the chosen code font and size, with the theme's syntax and
/// diff colors.
fn diff_preview(colors: Colors, cx: &App) -> Div {
    const LINES: [(&str, Option<bool>); 4] = [
        ("pub fn format_user(user: &User) -> String {", None),
        ("    user.name.to_uppercase()", Some(false)),
        (
            "    format!(\"{} <{}>\", user.name, user.email) // 0O 1lI",
            Some(true),
        ),
        ("}", None),
    ];
    let theme = cx.theme();
    let font = theme.mono_font_family.clone();
    let texts: Vec<&str> = LINES.iter().map(|(text, _)| *text).collect();
    let styles = highlight_lines("rust", &texts, &theme.highlight_theme);
    let mut number = 0;
    let rows = LINES.iter().zip(styles).map(|((text, change), runs)| {
        let (bg, mark, mark_color) = match change {
            Some(true) => (tokens::diff_added_bg(), "+", tokens::diff_added_text()),
            Some(false) => (tokens::diff_removed_bg(), "-", tokens::diff_removed_text()),
            None => (transparent_black(), " ", colors.faint),
        };
        // An added line takes the number of the line it replaces.
        if !matches!(change, Some(true)) {
            number += 1;
        }
        h_flex()
            .w_full()
            .px_3()
            .gap_3()
            .bg(bg)
            .child(
                div()
                    .w(dp(16.))
                    .text_right()
                    .text_color(colors.faint)
                    .child(number.to_string()),
            )
            .child(div().w(dp(8.)).text_color(mark_color).child(mark))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.foreground)
                    .child(StyledText::new(text.to_string()).with_highlights(runs)),
            )
    });
    preview_box(colors)
        .font_family(font)
        .text_size(scale::code())
        .child(
            h_flex()
                .px_3()
                .py_1()
                .gap_2()
                .border_b_1()
                .border_color(tokens::border_subtle())
                .child(
                    div()
                        .flex_1()
                        .text_color(colors.foreground)
                        .child("src/format_user.rs"),
                )
                .child(div().text_color(tokens::diff_removed_text()).child("-1"))
                .child(div().text_color(tokens::diff_added_text()).child("+1")),
        )
        .child(v_flex().py_1().children(rows))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };

    use crate::services::{Accounts, Services};
    use crate::settings::view::SettingsView;
    use crate::store::Settings;
    use crate::theme::themes::Themes;
    use crate::theme::{self, ColorScheme, tokens};

    fn open(cx: &mut TestAppContext) -> (Entity<SettingsView>, VisualTestContext) {
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            cx.set_global(Themes::load(&std::env::temp_dir().join("loupe-no-themes")));
            theme::init(cx);
            crate::shell::menus::bind_keys(cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1180.), px(2400.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| SettingsView::new(window, cx)),
            )
            .expect("open the window")
        });
        let mut cx = VisualTestContext::from_window(handle, cx);
        draw(&mut cx);
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} on the page"));
        cx.simulate_click(bounds.center(), Default::default());
        draw(cx);
    }

    #[gpui_kit::test]
    fn picking_light_repaints_the_app_in_the_light_palette(cx: &mut TestAppContext) {
        let (_view, mut cx) = open(cx);
        let (dark_text, dark_danger) = cx.update(|_, cx| {
            let theme = gpui_kit::component::ActiveTheme::theme(cx);
            (theme.foreground, theme.button_danger)
        });
        assert_eq!(dark_danger, Themes::default_palette(true).closed_fill.0);
        assert!(theme::is_dark());

        click(&mut cx, "color-scheme-light");
        let (scheme, foreground, panel) = cx.update(|_, cx| {
            (
                Settings::get(cx).appearance.color_scheme,
                gpui_kit::component::ActiveTheme::theme(cx).foreground,
                tokens::panel(),
            )
        });
        assert_eq!(scheme, ColorScheme::Light);
        assert!(!theme::is_dark());
        assert_ne!(
            foreground, dark_text,
            "stock components read the new palette"
        );
        let light = Themes::default_palette(false);
        assert_eq!(panel, light.panel.0, "tokens read the new palette");
        assert_eq!(foreground, light.text.0);
        // Slots stock components derive from their own theme file follow too:
        // outline buttons and the switch.
        let (button_text, switch) = cx.update(|_, cx| {
            let theme = gpui_kit::component::ActiveTheme::theme(cx);
            (theme.button_foreground, theme.switch)
        });
        assert_eq!(button_text, light.text.0);
        assert_eq!(switch, light.secondary_active.0);
    }

    #[gpui_kit::test]
    fn picking_a_theme_repaints_with_its_palette(cx: &mut TestAppContext) {
        let (_view, mut cx) = open(cx);
        click(&mut cx, "theme-card-tokyo-night");
        let (chosen, accent, primary) = cx.update(|_, cx| {
            (
                Settings::get(cx).appearance.theme.clone(),
                tokens::accent(),
                gpui_kit::component::ActiveTheme::theme(cx).primary,
            )
        });
        assert_eq!(chosen, "tokyo-night");
        let tokyo = Themes::load(&std::env::temp_dir().join("loupe-no-themes"))
            .find("tokyo-night")
            .expect("built in")
            .file
            .dark
            .clone();
        assert_eq!(accent, tokyo.accent.0);
        assert_eq!(primary, tokyo.accent_fill.0);
    }

    #[gpui_kit::test]
    fn picking_another_theme_closes_the_editor(cx: &mut TestAppContext) {
        let (view, mut cx) = open(cx);
        click(&mut cx, "create-theme");
        let open_editor = cx.update(|_, cx| view.read(cx).appearance.editor.is_some());
        assert!(open_editor);
        click(&mut cx, "theme-card-grove");
        let open_editor = cx.update(|_, cx| view.read(cx).appearance.editor.is_some());
        assert!(
            !open_editor,
            "edits would land on a theme that is not shown"
        );
    }

    #[gpui_kit::test]
    fn a_card_action_does_not_also_pick_the_card(cx: &mut TestAppContext) {
        let (_view, mut cx) = open(cx);
        // Duplicate is the first action of Grove's card.
        let actions = cx
            .debug_bounds("theme-actions-grove")
            .expect("Grove's actions");
        let duplicate = gpui_kit::point(actions.left() + px(12.), actions.center().y);
        cx.simulate_click(duplicate, Default::default());
        draw(&mut cx);
        let chosen = cx.update(|_, cx| Settings::get(cx).appearance.theme.clone());
        assert_eq!(chosen, "grove-copy", "the copy is shown, not Grove itself");
        let copy = crate::theme::themes::themes_dir().join("grove-copy.json");
        assert!(copy.exists());
    }

    #[gpui_kit::test]
    fn create_theme_saves_a_copy_and_editing_it_repaints(cx: &mut TestAppContext) {
        let (view, mut cx) = open(cx);
        click(&mut cx, "create-theme");
        let id = cx.update(|_, cx| Settings::get(cx).appearance.theme.clone());
        assert_eq!(id, "custom-theme");
        let saved = crate::theme::themes::themes_dir().join(format!("{id}.json"));
        assert!(saved.exists(), "the copy is a file in the themes folder");

        // The editor's field for `accent` saves and repaints as it is typed.
        let index = crate::theme::palette::TOKENS
            .iter()
            .position(|token| token.key == "accent")
            .unwrap();
        cx.update(|window, cx| {
            let field = view.read(cx).appearance.editor.as_ref().unwrap().fields[index].clone();
            field.update(cx, |field, cx| field.set_value("", window, cx));
            gpui_kit::Focusable::focus_handle(field.read(cx), cx).focus(window, cx);
        });
        cx.simulate_input("#ff00aa");
        draw(&mut cx);
        let accent = cx.update(|_, _| tokens::accent());
        assert_eq!(
            accent,
            crate::theme::palette::Color::parse("#ff00aa").unwrap().0
        );
        let text = std::fs::read_to_string(&saved).unwrap();
        assert!(
            text.contains("\"accent\": \"#ff00aa\""),
            "saved to the file"
        );
    }
}
