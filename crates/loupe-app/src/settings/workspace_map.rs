//! Settings > Linear > "Workspace by repository": which Linear workspace a
//! GitHub org or repository's pull requests link to.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::services::{Accounts, Connection, Services};
use crate::settings::catalog::Entry;
use crate::settings::view::{Palette, SettingsView, message, setting_row};
use crate::store::{self, Settings, WorkspaceMapping};
use crate::theme::scale;
use crate::theme::tokens;
use crate::ui::{self, ControlSize as _, MenuTriggerExt as _};

impl SettingsView {
    /// The workspace the add form maps to: the one picked in its menu while
    /// it is connected, else the active one.
    fn mapping_workspace(&self, cx: &App) -> Option<(String, String)> {
        let services = Services::get(cx);
        let picked = self
            .mapping_workspace
            .as_deref()
            .and_then(|id| services.linear_workspace(id));
        let active = match &Accounts::get(cx).linear {
            Connection::Connected(viewer) => services.linear_workspace(&viewer.organization.id),
            _ => None,
        };
        picked.or(active).map(|workspace| {
            (
                workspace.id().to_owned(),
                workspace.viewer.organization.name.clone(),
            )
        })
    }

    pub(super) fn add_mapping(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.mapping_target.read(cx).value().to_string();
        let Some(github) = store::parse_github_target(&typed) else {
            self.mapping_error =
                Some("Type a GitHub org, such as acme, or a repository, such as acme/web.".into());
            cx.notify();
            return;
        };
        let Some((workspace_id, workspace_name)) = self.mapping_workspace(cx) else {
            self.mapping_error = Some("Connect a Linear workspace first.".into());
            cx.notify();
            return;
        };
        store::update_settings(cx, move |settings| {
            settings.map_workspace(WorkspaceMapping {
                github,
                workspace_id,
                workspace_name,
            })
        });
        self.mapping_error = None;
        self.mapping_target
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// The mappings, each with Remove, under the row that names them, then
    /// the form that adds one.
    pub(super) fn workspace_mappings(
        &self,
        entry: &Entry,
        settings: &Settings,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let services = Services::get(cx).clone();
        let mappings = settings.workspace_mappings.clone();
        let count = mappings.len();
        let rows = mappings.into_iter().enumerate().map(|(index, mapping)| {
            let connected = services.linear_workspace(&mapping.workspace_id);
            let workspace: SharedString = match connected {
                Some(workspace) => workspace.viewer.organization.name.clone().into(),
                None => format!("{} (not connected)", mapping.workspace_name).into(),
            };
            let github: SharedString = mapping.github.clone().into();
            let owner = mapping
                .github
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            h_flex()
                .id(("workspace-mapping", index))
                .role(Role::Row)
                .aria_label(format!("{github} links to {workspace}"))
                .w_full()
                .h(scale::row_height())
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(tokens::border_subtle())
                .child(div().flex_none().child(ui::avatar_named(
                    format!("https://github.com/{owner}.png?size=40"),
                    owner.clone(),
                    scale::dp(16.),
                )))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .child(github.clone()),
                )
                .child(
                    svg()
                        .path(IconName::ArrowRight.path())
                        .flex_none()
                        .size_3()
                        .text_color(colors.faint),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .text_color(if connected.is_some() {
                            colors.foreground
                        } else {
                            colors.muted
                        })
                        .child(workspace),
                )
                .child(
                    ui::icon_button(
                        ("remove-mapping", index),
                        IconName::Close,
                        format!("Remove the workspace of {github}"),
                    )
                    .on_click(move |_, _, cx| {
                        let github = mapping.github.clone();
                        store::update_settings(cx, move |settings| {
                            settings
                                .workspace_mappings
                                .retain(|known| known.github != github);
                        });
                    }),
                )
        });

        let form =
            match self.mapping_workspace(cx) {
                None => message(
                    "Connect a Linear workspace to map repositories to it.".into(),
                    colors.muted,
                )
                .py_3()
                .into_any_element(),
                Some((chosen, chosen_name)) => {
                    let workspaces: Vec<(String, String)> = services
                        .linear_workspaces
                        .iter()
                        .map(|workspace| {
                            (
                                workspace.id().to_owned(),
                                workspace.viewer.organization.name.clone(),
                            )
                        })
                        .collect();
                    let view = cx.entity().downgrade();
                    v_flex()
                        .w_full()
                        .py_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(tokens::border_subtle())
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(div().flex_1().min_w_0().child(
                                    Input::new(&self.mapping_target).bg(tokens::background()),
                                ))
                                .child(
                                    Button::new("mapping-workspace")
                                        .outline()
                                        .control()
                                        .label(chosen_name.clone())
                                        .dropdown_caret(true)
                                        .accessibility_label(format!("Workspace: {chosen_name}"))
                                        .menu_trigger()
                                        .dropdown_menu(move |menu, _, _| {
                                            workspaces.iter().fold(menu, |menu, (id, name)| {
                                                let view = view.clone();
                                                let id = id.clone();
                                                menu.item(
                                                    PopupMenuItem::new(name.clone())
                                                        .checked(id == chosen)
                                                        .on_click(move |_, _, cx| {
                                                            let id = id.clone();
                                                            view.update(cx, |this, cx| {
                                                                this.mapping_workspace = Some(id);
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                        }),
                                                )
                                            })
                                        }),
                                )
                                .child(
                                    Button::new("mapping-add")
                                        .outline()
                                        .control()
                                        .label("Add")
                                        .accessibility_label("Add the mapping")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.add_mapping(window, cx)
                                        })),
                                ),
                        )
                        .when_some(self.mapping_error.clone(), |form, error| {
                            form.child(message(error, colors.danger))
                        })
                        .into_any_element()
                }
            };

        v_flex()
            .w_full()
            .child(setting_row(
                entry.title,
                entry.description,
                message(
                    match count {
                        0 => "None".into(),
                        1 => "1 mapping".into(),
                        count => format!("{count} mappings").into(),
                    },
                    colors.muted,
                ),
                colors,
            ))
            .children(rows)
            .child(form)
    }
}
