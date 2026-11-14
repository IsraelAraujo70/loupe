use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::settings::appearance::AppearanceState;
use crate::settings::catalog::{self, Entry, Item, Section};
use crate::settings::shortcuts::SHORTCUTS;
use crate::shell::actions::FocusSettingsSearch;
use crate::store::{self, CommentBoxPlace, CommentOrder, DiffLayout, FileTreePosition, Settings};
use crate::theme::scale::dp;
use crate::theme::{self, Zoom, scale, tokens};
use crate::ui::{self, ControlSize as _, MenuTriggerExt as _};
use crate::updater::{self, UpdateState};

/// The `Route::Settings` screen: one page of sections (General, Appearance,
/// the accounts, Keyboard shortcuts, About) beside a nav that follows the
/// section being read, with a search that narrows the page to the settings
/// it matches.
pub fn settings_view(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| SettingsView::new(window, cx)).into()
}

/// The width of the nav beside the page.
const NAV_WIDTH: f32 = 220.;
/// The width of the page's column of cards.
const PAGE_WIDTH: f32 = 640.;

/// The Linear key form's progress. The connected state lives in `Accounts`.
pub(super) enum LinearForm {
    Idle,
    Validating,
    Failed(String),
}

/// The outcome of the last T3 save or clear, shown under the card.
pub(super) enum Notice {
    None,
    Info(SharedString),
    Error(SharedString),
}

pub struct SettingsView {
    focus: FocusHandle,
    search: Entity<InputState>,
    scroll: ScrollHandle,
    pub(super) linear_key: Entity<InputState>,
    pub(super) linear_form: LinearForm,
    pub(super) linear_task: Option<Task<()>>,
    /// The add form of "Workspace by repository": the org or repository,
    /// the workspace picked in its menu, and why the last add failed.
    pub(super) mapping_target: Entity<InputState>,
    pub(super) mapping_workspace: Option<String>,
    pub(super) mapping_error: Option<SharedString>,
    pub(super) t3_url: Entity<InputState>,
    pub(super) t3_token: Entity<InputState>,
    pub(super) t3_notice: Notice,
    pub(super) github_error: Option<SharedString>,
    pub(super) appearance: AppearanceState,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = AppearanceState::new(window, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let linear_key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("lin_api_…")
        });
        let mapping_target =
            cx.new(|cx| InputState::new(window, cx).placeholder("Org or owner/repo"));
        let saved_url = Settings::get(cx).t3_server_url.clone().unwrap_or_default();
        let t3_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://t3.example.com:3774")
                .default_value(saved_url)
        });
        let t3_token = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(super::accounts::token_placeholder())
        });
        let subscriptions = vec![
            cx.observe_global::<crate::services::Accounts>(|_, cx| cx.notify()),
            cx.observe_global::<crate::services::Services>(|_, cx| cx.notify()),
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    // The page now holds other sections: start it at the top.
                    this.scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &mapping_target,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_mapping(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &linear_key,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.connect_linear(window, cx);
                    }
                },
            ),
        ];
        Self {
            focus: cx.focus_handle(),
            search,
            scroll: ScrollHandle::new(),
            linear_key,
            linear_form: LinearForm::Idle,
            linear_task: None,
            mapping_target,
            mapping_workspace: None,
            mapping_error: None,
            t3_url,
            t3_token,
            t3_notice: Notice::None,
            github_error: None,
            appearance,
            _subscriptions: subscriptions,
        }
    }

    /// The section being read among `shown`, from where the page has
    /// scrolled to in the last frame.
    fn reading(&self, shown: &[Section]) -> Option<Section> {
        let tops: Vec<f32> = (0..shown.len())
            .map_while(|ix| self.scroll.bounds_for_item(ix))
            .map(|bounds| f32::from(bounds.top()))
            .collect();
        if tops.len() != shown.len() {
            return shown.first().copied();
        }
        let offset = self.scroll.offset().y;
        let page_top = f32::from(self.scroll.bounds().top() - offset);
        let max = f32::from(self.scroll.max_offset().y);
        let at_end = max > 0. && f32::from(-offset) >= max - 1.;
        shown
            .get(catalog::reading_index(&tops, page_top, at_end))
            .copied()
    }

    /// Scrolls the page so the section at `index` sits where the first one
    /// sits at the top of the page, under the same 24 px of padding, or as
    /// far as the page goes.
    fn go_to(&mut self, index: usize, cx: &mut Context<Self>) {
        let (Some(first), Some(target)) = (
            self.scroll.bounds_for_item(0),
            self.scroll.bounds_for_item(index),
        ) else {
            return;
        };
        let distance = (target.top() - first.top()).min(self.scroll.max_offset().y);
        self.scroll.set_offset(point(px(0.), -distance.max(px(0.))));
        cx.notify();
    }

    fn restore_defaults(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        store::update_settings(cx, |settings| section.restore_defaults(settings));
        if section == Section::Appearance {
            theme::apply_zoom(Settings::get(cx).zoom, cx);
            theme::apply(cx);
            self.appearance.sync(window, cx);
        }
        cx.notify();
    }
}

/// Colors the settings page reads, copied out of the theme once per render.
#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub muted: Hsla,
    pub faint: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    pub card: Hsla,
    pub danger: Hsla,
}

pub(super) fn message(text: SharedString, color: Hsla) -> Div {
    div().text_size(scale::meta()).text_color(color).child(text)
}

/// The card around a section's settings: the kit's card surface with 16 px
/// of side padding, its rows separated by hairlines.
pub(super) fn rows_card(rows: Vec<AnyElement>, colors: Palette) -> Div {
    v_flex()
        .w_full()
        .px_4()
        .rounded_lg()
        .border_1()
        .border_color(colors.border)
        .bg(colors.card)
        .children(rows.into_iter().enumerate().map(|(index, row)| {
            div()
                .w_full()
                .when(index > 0, |row| {
                    row.border_t_1().border_color(tokens::border_subtle())
                })
                .child(row)
        }))
}

/// One setting: its name over a one-line description at the leading edge,
/// its control at the trailing edge.
pub(super) fn setting_row(
    title: &'static str,
    description: impl Into<SharedString>,
    control: impl IntoElement,
    colors: Palette,
) -> Div {
    let description: SharedString = description.into();
    h_flex()
        .w_full()
        .min_h(dp(64.))
        .py_3()
        .gap_4()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_size(scale::body())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.foreground)
                        .child(title),
                )
                .when(!description.is_empty(), |text| {
                    text.child(message(description, colors.muted))
                }),
        )
        .child(div().flex_none().child(control))
}

fn segmented_setting<T: Copy + PartialEq + 'static>(
    id: &'static str,
    name: &'static str,
    options: [(Option<IconName>, &'static str, T); 2],
    current: T,
    change: fn(&mut Settings, T),
) -> impl IntoElement {
    let values = options.map(|(_, _, value)| value);
    ui::segmented(
        id,
        name,
        options.map(|(icon, label, _)| (icon, label)),
        values
            .iter()
            .position(|value| *value == current)
            .unwrap_or_default(),
        move |index, _, cx| {
            if let Some(value) = values.get(index).copied() {
                store::update_settings(cx, |settings| change(settings, value));
            }
        },
    )
}

impl SettingsView {
    /// The control of one setting, or the whole block for the settings
    /// that are more than a row (accounts, repositories, shortcuts).
    fn entry_row(&mut self, entry: &Entry, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let settings = Settings::get(cx).clone();
        let row = |control: AnyElement| {
            setting_row(entry.title, entry.description, control, colors).into_any_element()
        };
        match entry.item {
            Item::SidebarIcons => row(Switch::new("sidebar-icons")
                .checked(settings.sidebar_collapsed)
                .accessibility_label(entry.title)
                .on_click(|checked, _, cx| {
                    let checked = *checked;
                    store::update_settings(cx, |settings| settings.sidebar_collapsed = checked);
                })
                .into_any_element()),
            Item::InboxFilter => {
                let filter = settings.notification_filter.join(" ");
                let empty = filter.is_empty();
                row(h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .max_w(dp(200.))
                            .truncate()
                            .text_size(scale::meta())
                            .text_color(colors.muted)
                            .child(if empty { "None".to_owned() } else { filter }),
                    )
                    .child(
                        Button::new("inbox-filter-clear")
                            .outline()
                            .control()
                            .label("Clear")
                            .accessibility_label("Clear the Inbox filter")
                            .disabled(empty)
                            .on_click(|_, _, cx| {
                                store::update_settings(cx, |settings| {
                                    settings.notification_filter.clear()
                                });
                            }),
                    )
                    .into_any_element())
            }
            Item::ColorScheme
            | Item::Themes
            | Item::Contrast
            | Item::GlassOpacity
            | Item::Density
            | Item::InterfaceFont
            | Item::MonoFont => self.appearance_row(entry, colors, cx),
            Item::Zoom => {
                let current = settings.zoom;
                row(Button::new("zoom")
                    .outline()
                    .control()
                    .label(format!("{}%", current.percent()))
                    .dropdown_caret(true)
                    .accessibility_label(format!("Zoom: {}%", current.percent()))
                    .menu_trigger()
                    .dropdown_menu(move |menu, _, _| {
                        Zoom::STEPS.iter().fold(menu.min_w(dp(120.)), |menu, step| {
                            let zoom = Zoom::from(*step);
                            menu.item(
                                PopupMenuItem::new(format!("{step}%"))
                                    .checked(zoom == current)
                                    .on_click(move |_, _, cx| {
                                        store::update_settings(cx, |settings| settings.zoom = zoom);
                                        theme::apply_zoom(zoom, cx);
                                    }),
                            )
                        })
                    })
                    .into_any_element())
            }
            Item::SidebarWidth => {
                let width = settings.sidebar_width;
                row(h_flex()
                    .gap_3()
                    .items_center()
                    .child(message(
                        match width {
                            Some(width) => format!("{} px", width.round()).into(),
                            None => "Default".into(),
                        },
                        colors.muted,
                    ))
                    .child(
                        Button::new("sidebar-width-reset")
                            .outline()
                            .control()
                            .label("Reset")
                            .accessibility_label("Reset the sidebar width")
                            .disabled(width.is_none())
                            .on_click(|_, _, cx| {
                                store::update_settings(cx, |settings| {
                                    settings.sidebar_width = None
                                });
                            }),
                    )
                    .into_any_element())
            }
            Item::GitHubAccount => self.github_card(colors, cx).into_any_element(),
            Item::DiffLayout => row(segmented_setting(
                "diff-layout",
                "Diff layout",
                [
                    (Some(IconName::Rows2), "Unified", DiffLayout::Unified),
                    (Some(IconName::Columns2), "Split", DiffLayout::Split),
                ],
                settings.diff_layout,
                |settings, layout| settings.diff_layout = layout,
            )
            .into_any_element()),
            Item::FileTree => row(segmented_setting(
                "file-tree-position",
                "File tree position",
                [
                    (Some(IconName::PanelLeft), "Left", FileTreePosition::Left),
                    (Some(IconName::PanelRight), "Right", FileTreePosition::Right),
                ],
                settings.file_tree_position,
                |settings, position| settings.file_tree_position = position,
            )
            .into_any_element()),
            Item::LongLines => row(segmented_setting(
                "long-lines",
                "Long lines",
                [(None, "Scroll", false), (None, "Wrap", true)],
                settings.diff_wrap,
                |settings, wrap| settings.diff_wrap = wrap,
            )
            .into_any_element()),
            Item::CommentOrder => row(segmented_setting(
                "comment-order",
                "Conversation order",
                [
                    (None, "Oldest first", CommentOrder::OldestFirst),
                    (None, "Newest first", CommentOrder::NewestFirst),
                ],
                settings.comment_order,
                |settings, order| settings.comment_order = order,
            )
            .into_any_element()),
            Item::CommentBox => row(segmented_setting(
                "comment-box",
                "Comment box",
                [
                    (None, "Top", CommentBoxPlace::Top),
                    (None, "End", CommentBoxPlace::Bottom),
                ],
                settings.comment_box_place(),
                |settings, place| settings.comment_box = Some(place),
            )
            .into_any_element()),
            Item::Repositories => self
                .repositories(entry, &settings, colors)
                .into_any_element(),
            Item::LinearAccount => self.linear_card(colors, cx).into_any_element(),
            Item::WorkspaceMappings => self
                .workspace_mappings(entry, &settings, colors, cx)
                .into_any_element(),
            Item::T3Server => self.t3_card(colors, cx).into_any_element(),
            Item::Shortcut(index) => shortcut_row(index, colors, cx),
            Item::Version => row(message(
                format!("Loupe {}", env!("CARGO_PKG_VERSION")).into(),
                colors.muted,
            )
            .into_any_element()),
            Item::Updates => updates_row(entry, colors, cx),
        }
    }

    /// The watched repositories, each with Unwatch, under a row that names
    /// the list.
    fn repositories(&self, entry: &Entry, settings: &Settings, colors: Palette) -> Div {
        let repos = settings.watched_repos.clone();
        let count = repos.len();
        v_flex()
            .w_full()
            .child(setting_row(
                entry.title,
                entry.description,
                message(
                    match count {
                        0 => "None".into(),
                        1 => "1 repository".into(),
                        count => format!("{count} repositories").into(),
                    },
                    colors.muted,
                ),
                colors,
            ))
            .children(repos.into_iter().enumerate().map(|(index, repo)| {
                let name: SharedString = format!("{}/{}", repo.owner, repo.name).into();
                h_flex()
                    .w_full()
                    .h(scale::row_height())
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(tokens::border_subtle())
                    .child(
                        svg()
                            .path(IconName::FolderGit2.path())
                            .flex_none()
                            .size_4()
                            .text_color(colors.faint),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(scale::body())
                            .child(name.clone()),
                    )
                    .child(
                        ui::icon_button(
                            ("unwatch", index),
                            IconName::Close,
                            format!("Unwatch {name}"),
                        )
                        .on_click(move |_, _, cx| {
                            let repo = repo.clone();
                            store::update_settings(cx, move |settings| {
                                settings.watched_repos.retain(|watched| *watched != repo);
                            });
                        }),
                    )
            }))
    }

    /// One section of the page: its heading, then its settings, a card per
    /// run of settings in the same group, with the group's name over it.
    fn section(
        &mut self,
        section: Section,
        entries: &[Entry],
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut block = v_flex()
            .id(section.id())
            .debug_selector(|| section.id().to_owned())
            .w_full()
            .max_w(dp(PAGE_WIDTH))
            .gap_3()
            .child(
                div()
                    .id(SharedString::from(format!("{}-heading", section.id())))
                    .role(Role::Heading)
                    .aria_label(section.title())
                    .text_size(scale::title())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(section.title()),
            );
        let mut index = 0;
        while index < entries.len() {
            let group = entries[index].group;
            let run = entries[index..]
                .iter()
                .take_while(|entry| entry.group == group)
                .count();
            let run_entries = &entries[index..index + run];
            index += run;
            // A block (an account, the themes) is a card or a grid of its
            // own, with its own heading; the other settings share one card.
            if let [entry] = run_entries
                && entry.item.is_block()
            {
                block = block.child(self.entry_row(entry, colors, cx));
                continue;
            }
            if !group.is_empty() {
                block = block.child(ui::section_label(group, cx).pt_1());
            }
            let rows = run_entries
                .iter()
                .map(|entry| self.entry_row(entry, colors, cx))
                .collect();
            block = block.child(rows_card(rows, colors));
        }
        block
    }

    /// The nav beside the page: the search field, then each group's
    /// sections, the one being read marked as the sidebar marks the open
    /// page.
    fn nav(
        &self,
        shown: &[Section],
        reading: Option<Section>,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut list = v_flex().w_full().gap_px();
        let mut last_group = "";
        for (index, section) in shown.iter().copied().enumerate() {
            if section.group() != last_group {
                last_group = section.group();
                list = list.child(
                    div()
                        .h(scale::nav_row_height())
                        .mt_2()
                        .px_2()
                        .flex()
                        .items_center()
                        .text_size(scale::body())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.faint)
                        .child(last_group),
                );
            }
            let selected = reading == Some(section);
            list = list.child(
                h_flex()
                    .id(("settings-nav", index))
                    .debug_selector(move || format!("settings-nav-{}", section.id()))
                    .role(Role::Link)
                    .aria_label(section.title())
                    .h(scale::nav_row_height())
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(dp(6.))
                    .text_size(scale::body())
                    .when(selected, |row| {
                        row.bg(tokens::glass_selected())
                            .text_color(colors.foreground)
                    })
                    .when(!selected, |row| {
                        row.text_color(colors.muted).hover(|style| {
                            style
                                .bg(tokens::glass_hover())
                                .text_color(colors.foreground)
                        })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(index, cx)))
                    .child(
                        svg()
                            .path(section.icon().path())
                            .flex_none()
                            .size_4()
                            .text_color(if selected {
                                tokens::accent()
                            } else {
                                colors.faint
                            }),
                    )
                    .child(div().min_w_0().truncate().child(section.title())),
            );
        }
        let search_empty = self.search.read(cx).value().is_empty();
        v_flex()
            .id("settings-nav")
            .flex_none()
            .w(dp(NAV_WIDTH))
            .h_full()
            .px_2()
            .py_3()
            .gap_1()
            .border_r_1()
            .border_color(tokens::border_subtle())
            .overflow_y_scroll()
            .child(
                Input::new(&self.search)
                    .small()
                    .prefix(
                        svg()
                            .path(IconName::Search.path())
                            .size_3p5()
                            .text_color(colors.faint),
                    )
                    .when_some(
                        Keystroke::parse("/").ok().filter(|_| search_empty),
                        |input, key| input.suffix(Kbd::new(key).outline()),
                    )
                    .cleanable(true),
            )
            .child(list)
    }

    /// The header: the breadcrumb to the section being read and its
    /// Restore defaults.
    fn header(
        &self,
        reading: Option<Section>,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = reading.map_or("Search", Section::title);
        let header = ui::page_header(title).prefix(
            h_flex()
                .gap_2()
                .items_center()
                .text_size(scale::title())
                .child(
                    svg()
                        .path(IconName::Settings.path())
                        .flex_none()
                        .size_4()
                        .text_color(colors.muted),
                )
                .child(div().text_color(colors.muted).child("Settings"))
                .child(div().text_color(colors.faint).child("/")),
        );
        let Some(section) = reading else {
            return header;
        };
        let at_defaults = section.is_default(Settings::get(cx));
        header.action(
            Button::new("restore-defaults")
                .ghost()
                .control()
                .icon(IconName::RotateCcw)
                .label("Restore defaults")
                .tooltip(if section.has_defaults() {
                    format!(
                        "Put the {} preferences back to their defaults",
                        section.title()
                    )
                } else {
                    format!("{} has no preferences to restore", section.title())
                })
                .disabled(at_defaults)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.restore_defaults(section, window, cx)
                })),
        )
    }
}

fn shortcut_row(index: usize, colors: Palette, cx: &App) -> AnyElement {
    let Some(shortcut) = SHORTCUTS.get(index) else {
        return div().into_any_element();
    };
    h_flex()
        .w_full()
        .h(dp(40.))
        .gap_4()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(scale::body())
                .text_color(colors.foreground)
                .child(shortcut.label),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1()
                .children(shortcut.keys(cx).into_iter().map(|strokes| {
                    // A sequence (O then W) reads as one, apart from the
                    // other keys of the same action.
                    let mut sequence = h_flex().gap_1().items_center();
                    for (index, stroke) in strokes.into_iter().enumerate() {
                        if index > 0 {
                            sequence = sequence.child(message("then".into(), colors.muted));
                        }
                        sequence = sequence.child(Kbd::new(stroke));
                    }
                    sequence
                })),
        )
        .into_any_element()
}

fn updates_row(entry: &Entry, colors: Palette, cx: &App) -> AnyElement {
    let state = updater::state(cx);
    let status: SharedString = match &state {
        UpdateState::SignedOut => "Sign in to GitHub to check for updates.".into(),
        UpdateState::NotChecked => entry.description.into(),
        UpdateState::Checking => "Checking for updates…".into(),
        UpdateState::UpToDate => "Loupe is up to date.".into(),
        UpdateState::CheckFailed => "Could not reach the releases. Try again later.".into(),
        UpdateState::Available(tag) => format!("Loupe {tag} is available.").into(),
        UpdateState::Installing => "Installing the update…".into(),
        UpdateState::InstallFailed => "The update failed.".into(),
        UpdateState::RolledBack(tag) => {
            format!("Loupe {tag} quit before it finished starting, so this version was restored.")
                .into()
        }
    };
    let button = match &state {
        UpdateState::Available(_) => Button::new("update-install")
            .primary()
            .control()
            .label("Update and restart")
            .on_click(|_, _, cx| updater::install(cx)),
        UpdateState::InstallFailed => Button::new("update-install")
            .outline()
            .control()
            .label("Try again")
            .on_click(|_, _, cx| updater::install(cx)),
        _ => Button::new("update-check")
            .outline()
            .control()
            .label("Check for updates")
            .loading(state == UpdateState::Checking)
            .disabled(matches!(
                state,
                UpdateState::SignedOut | UpdateState::Checking | UpdateState::Installing
            ))
            .on_click(|_, _, cx| updater::check_now(cx)),
    };
    setting_row(entry.title, status, button, colors).into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        self.sync_editor(window, cx);
        let theme = cx.theme();
        let colors = Palette {
            muted: theme.muted_foreground,
            faint: tokens::text_faint(),
            foreground: theme.foreground,
            border: theme.border,
            card: theme.group_box,
            danger: theme.danger,
        };
        let query = self.search.read(cx).value().to_string();
        let found = catalog::search(&query);
        let shown: Vec<Section> = found.iter().map(|(section, _)| *section).collect();
        let reading = self.reading(&shown);

        let mut page = v_flex()
            .id("settings-scroll")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(scale::content_inset())
            .py(scale::section_gap())
            .gap(scale::section_gap());
        if found.is_empty() {
            page = page.child(ui::empty_line(
                IconName::Search,
                format!("No settings match \u{201c}{}\u{201d}", query.trim()),
                cx,
            ));
        }
        for (section, entries) in &found {
            page = page.child(self.section(*section, entries, colors, cx));
        }

        v_flex()
            .key_context("SettingsView")
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|this, _: &FocusSettingsSearch, window, cx| {
                this.search.read(cx).focus_handle(cx).focus(window, cx);
            }))
            .child(self.header(reading, colors, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.nav(&shown, reading, colors, cx))
                    .child(page),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };

    use super::SettingsView;
    use crate::services::{Accounts, Services};
    use crate::settings::catalog::Section;
    use crate::store::Settings;

    fn open(cx: &mut TestAppContext) -> (Entity<SettingsView>, VisualTestContext) {
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            crate::shell::menus::bind_keys(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts::default());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1180.), px(860.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| SettingsView::new(window, cx)),
            )
            .expect("open the window")
        });
        let mut cx = VisualTestContext::from_window(handle, cx);
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        (view, cx)
    }

    fn search(view: &Entity<SettingsView>, query: &str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let input = view.read(cx).search.clone();
            input.update(cx, |input, cx| input.set_value(query, window, cx));
            view.update(cx, |_, cx| cx.notify());
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn the_page_shows_every_section_until_a_search_narrows_it(cx: &mut TestAppContext) {
        let (view, mut cx) = open(cx);
        for id in [
            "settings-general",
            "settings-appearance",
            "settings-github",
            "settings-linear",
            "settings-t3",
            "settings-shortcuts",
            "settings-about",
        ] {
            assert!(cx.debug_bounds(id).is_some(), "{id} is on the page");
        }

        search(&view, "file tree", &mut cx);
        assert!(cx.debug_bounds("settings-github").is_some());
        assert!(cx.debug_bounds("settings-general").is_none());
        assert!(cx.debug_bounds("settings-nav-settings-github").is_some());
        assert!(
            cx.debug_bounds("settings-nav-settings-general").is_none(),
            "the nav lists only the sections with a match"
        );

        search(&view, "", &mut cx);
        assert!(cx.debug_bounds("settings-general").is_some());
    }

    #[gpui_kit::test]
    fn the_nav_scrolls_the_page_to_a_section(cx: &mut TestAppContext) {
        let (view, mut cx) = open(cx);
        let page_top = cx.debug_bounds("settings-general").expect("General").top();
        let linear = cx
            .debug_bounds("settings-nav-settings-linear")
            .expect("nav");
        cx.simulate_click(linear.center(), Default::default());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let top = cx.debug_bounds("settings-linear").expect("Linear").top();
        assert!(
            (top - page_top).abs() < px(2.),
            "Linear starts where the page starts ({top:?} vs {page_top:?})"
        );
        let reading = cx.update(|_, cx| view.read(cx).reading(&Section::ALL));
        assert_eq!(reading, Some(Section::Linear), "the breadcrumb follows");
    }
}
