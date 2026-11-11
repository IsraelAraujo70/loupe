//! The views list, after Linear's Views page: Linear's views with their
//! team, who can see them, and their owner, in fixed lanes under one
//! heading. A row opens its team on that view. The views still saved only in
//! Loupe (see `views_store`) are listed apart, each with the two things to
//! do with it: save it to Linear, or delete it.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::TeamSummary;
use loupe_linear::views::View;

use crate::linear::data::{self, Loadable};
use crate::linear::views_store::{self, Kind};
use crate::linear::{remote_views, widgets};
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::store::{self, Favorite, Settings};
use crate::theme::scale::{self, dp};
use crate::theme::tokens;
use crate::ui;

/// The fixed lanes after a view's name, in px: its team, who sees it, and
/// its owner. They keep their width and their x on every row.
const TEAM_LANE: f32 = 140.;
const VISIBILITY_LANE: f32 = 72.;
const OWNER_LANE: f32 = 160.;

pub fn view(team_id: Option<String>, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut view = ViewsListView {
            team_id,
            client: None,
            teams: Loadable::Loading,
            moving: None,
            notice: views_store::take_warning(cx),
            _teams_task: None,
        };
        view.load_teams(cx);
        view
    })
    .into()
}

struct ViewsListView {
    /// `None`: every team's views.
    team_id: Option<String>,
    client: Option<Arc<loupe_linear::Linear>>,
    teams: Loadable<Arc<Vec<TeamSummary>>>,
    /// The view only in Loupe that is on its way to Linear.
    moving: Option<String>,
    /// Why a view could not be saved to Linear or deleted.
    notice: Option<SharedString>,
    _teams_task: Option<Task<()>>,
}

impl ViewsListView {
    fn load_teams(&mut self, cx: &mut Context<Self>) {
        self.client = data::client(cx);
        self.teams = Loadable::Loading;
        let task = data::teams(cx);
        self._teams_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.teams = match result {
                    Ok(teams) => Loadable::Ready(teams),
                    Err(message) => Loadable::Failed(message),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// The team a view opens on: its own, the list's, or the first team.
    fn team_for(&self, view: &View) -> Option<TeamSummary> {
        let Loadable::Ready(teams) = &self.teams else {
            return None;
        };
        let id = view.team_id.as_ref().or(self.team_id.as_ref());
        match id {
            Some(id) => teams.iter().find(|team| &team.id == id).cloned(),
            None => teams.first().cloned(),
        }
    }

    /// Saves a view that is only in Loupe to Linear, as a view of `team`.
    fn move_to_linear(&mut self, view: &View, team: &TeamSummary, cx: &mut Context<Self>) {
        let task = views_store::create_in_linear(
            &view.name,
            &view.config,
            Some(team.id.clone()),
            Some(view.id.clone()),
            cx,
        );
        self.moving = Some(view.id.clone());
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |list, cx| {
                list.moving = None;
                list.notice = match result {
                    Ok(created) => created.warning,
                    Err(message) => Some(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn delete_legacy(&mut self, id: &str, cx: &mut Context<Self>) {
        let removed = views_store::update_legacy(cx, |views| views.delete(id));
        store::update_settings(cx, |settings| {
            let favorite = Favorite::View { id: id.to_owned() };
            if settings.is_favorite(&favorite) {
                settings.toggle_favorite(favorite);
            }
        });
        self.notice = removed.err();
        cx.notify();
    }

    /// One 32 px row: the star, the view's icon and name, then `trailing`.
    /// The whole row opens the view on its team.
    fn row(&self, view: &View, trailing: Vec<AnyElement>, cx: &mut Context<Self>) -> Stateful<Div> {
        let metadata = remote_views::metadata(&view.id, cx);
        let favorites = super::remote_favorites(cx);
        let favorite_color = favorites.iter().find_map(|favorite| {
            (super::favorite_view_id(favorite).as_deref() == Some(view.id.as_str()))
                .then(|| favorite.color.clone())
                .flatten()
        });
        let in_linear = views_store::kind(&view.id) != Kind::Legacy;
        let favorite = Favorite::View {
            id: view.id.clone(),
        };
        let starred = if in_linear {
            favorites
                .iter()
                .any(|known| super::favorite_view_id(known).as_deref() == Some(view.id.as_str()))
        } else {
            Settings::get(cx).is_favorite(&favorite)
        };
        let route = self.team_for(view).map(|team| Route::LinearTeam {
            team_id: team.id,
            name: team.name,
            view_id: Some(view.id.clone()),
        });
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let color = super::view_color(
            metadata
                .as_ref()
                .and_then(|view| view.color.as_deref())
                .or(favorite_color.as_deref()),
        )
        .unwrap_or(muted);
        let icon = super::view_icon(metadata.as_ref().and_then(|view| view.icon.as_deref()));
        let name: SharedString = view.name.clone().into();
        let tooltip = name.clone();
        h_flex()
            .id(SharedString::from(format!("view-{}", view.id)))
            .role(Role::Link)
            .aria_label(format!("View {}", view.name))
            .h(scale::row_height())
            .flex_none()
            .px_2()
            .gap_2()
            .items_center()
            .rounded(theme.radius)
            .hover(move |row| row.bg(hover))
            .when_some(route, |row, route| row.opens(route))
            .child(
                ui::icon_button(
                    SharedString::from(format!("star-view-{}", view.id)),
                    if starred {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    },
                    if in_linear {
                        format!("{} favorite in Linear", view.name)
                    } else if starred {
                        format!("Remove {} from favorites", view.name)
                    } else {
                        format!("Add {} to favorites", view.name)
                    },
                )
                .xsmall()
                .disabled(in_linear)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let favorite = favorite.clone();
                    store::update_settings(cx, move |settings| settings.toggle_favorite(favorite));
                }),
            )
            .child(
                svg()
                    .path(icon.path())
                    .flex_none()
                    .size_4()
                    .text_color(color),
            )
            .child(
                div()
                    .id(SharedString::from(format!("view-name-{}", view.id)))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::body())
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                            .build(window, cx)
                    })
                    .child(name),
            )
            .children(trailing)
    }

    /// The lanes of a view saved in Linear: team, visibility, owner.
    fn linear_lanes(&self, view: &View, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let metadata = remote_views::metadata(&view.id, cx);
        let team = match (&view.team_id, self.team_for(view)) {
            (Some(_), Some(team)) => team.name,
            (None, _) => "Workspace".to_owned(),
            (Some(_), None) => String::new(),
        };
        let (visibility, owner) = match &metadata {
            Some(view) => (
                if view.shared { "Shared" } else { "Private" },
                view.owner_name.clone(),
            ),
            // A team's built-in view: every member has it.
            None => ("Shared", String::new()),
        };
        vec![
            lane(team, TEAM_LANE, cx).into_any_element(),
            lane(visibility, VISIBILITY_LANE, cx).into_any_element(),
            lane(owner, OWNER_LANE, cx).into_any_element(),
        ]
    }

    /// What to do with a view that is only in Loupe.
    fn legacy_actions(&self, view: &View, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let team = self.team_for(view);
        let busy = self.moving.is_some();
        let moving = self.moving.as_deref() == Some(view.id.as_str());
        let save = {
            let (view, team) = (view.clone(), team.clone());
            Button::new(SharedString::from(format!("legacy-save-{}", view.id)))
                .outline()
                .small()
                .label("Save to Linear")
                .accessibility_label(format!("Save {} to Linear", view.name))
                .tooltip(match &team {
                    Some(team) => format!("Save it in Linear as a private view of {}", team.name),
                    None => "Save it in Linear".to_owned(),
                })
                .loading(moving)
                .disabled(busy || team.is_none())
                .on_click(cx.listener(move |list, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(team) = &team {
                        list.move_to_linear(&view, team, cx);
                    }
                }))
        };
        let delete = {
            let (id, name) = (view.id.clone(), view.name.clone());
            let list = cx.weak_entity();
            Button::new(SharedString::from(format!("legacy-delete-{}", view.id)))
                .ghost()
                .small()
                .label("Delete")
                .accessibility_label(format!("Delete {}", view.name))
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    confirm_delete(list.clone(), id.clone(), name.clone(), window, cx);
                })
        };
        vec![save.into_any_element(), delete.into_any_element()]
    }
}

/// One fixed lane of a row: 12 px, muted, cut with an ellipsis.
fn lane(text: impl Into<SharedString>, width: f32, cx: &App) -> Div {
    div()
        .flex_none()
        .w(dp(width))
        .truncate()
        .text_size(scale::meta())
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// The 24 px heading over the rows, one label per lane, on the rows' grid:
/// the name starts after the star and the icon.
fn column_heading(cx: &App) -> Div {
    h_flex()
        .h_6()
        .flex_none()
        .px_2()
        .gap_2()
        .items_center()
        .child(div().flex_none().w(dp(20. + 8. + 16.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(scale::meta())
                .text_color(cx.theme().muted_foreground)
                .child("Name"),
        )
        .child(lane("Team", TEAM_LANE, cx))
        .child(lane("Visibility", VISIBILITY_LANE, cx))
        .child(lane("Owner", OWNER_LANE, cx))
}

/// "Delete" on a view that is only in Loupe, behind a confirmation.
fn confirm_delete(
    list: WeakEntity<ViewsListView>,
    id: String,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    let title: SharedString = format!("Delete “{name}”?").into();
    window.open_dialog(cx, move |dialog, _, cx| {
        let (list, id) = (list.clone(), id.clone());
        dialog.title(title.clone()).w(dp(400.)).child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "The view is saved only in Loupe and is removed from it. Linear is not changed.",
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("legacy-delete-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("legacy-delete-confirm")
                                .danger()
                                .small()
                                .label("Delete")
                                .on_click(move |_, window, cx| {
                                    list.update(cx, |list, cx| list.delete_legacy(&id, cx))
                                        .ok();
                                    window.close_dialog(cx);
                                }),
                        ),
                ),
        )
    });
}

impl Render for ViewsListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !data::same_client(&self.client, &data::client(cx)) {
            self.load_teams(cx);
        }
        if self.client.is_none() {
            return widgets::not_connected(IconName::Layers, "Views");
        }
        let on_this_list = |view: &View| match (&self.team_id, &view.team_id) {
            (Some(team), Some(own)) => own == team,
            _ => true,
        };
        let (legacy, linear): (Vec<View>, Vec<View>) = views_store::snapshot(cx)
            .views
            .into_iter()
            .filter(on_this_list)
            .partition(|view| views_store::kind(&view.id) == Kind::Legacy);
        let remote = cx.global::<remote_views::RemoteViews>();
        let (loading, load_error) = (remote.loading, remote.error.clone());
        let header = ui::page_header("Views")
            .icon(IconName::Layers)
            .count(linear.len())
            .action(
                ui::icon_button(
                    "refresh-linear-views",
                    IconName::RefreshCw,
                    "Refresh Linear views",
                )
                .disabled(loading)
                .on_click(|_, _, cx| remote_views::retry(cx)),
            );

        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(message) = self.notice.clone() {
            rows.push(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(cx.theme().danger)
                            .child(message),
                    )
                    .child(
                        ui::icon_button("views-notice-dismiss", IconName::Close, "Dismiss")
                            .on_click(cx.listener(|list, _, _, cx| {
                                list.notice = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Loadable::Failed(message) = &self.teams {
            rows.push(widgets::load_error(
                message.clone(),
                cx.listener(|this, _, _, cx| this.load_teams(cx)),
            ));
        }
        if let Some(message) = load_error {
            rows.push(widgets::load_error(message, |_, _, cx| {
                remote_views::retry(cx)
            }));
        }
        if loading {
            rows.push(ui::skeleton_rows(3).into_any_element());
        } else if linear.is_empty() {
            rows.push(
                ui::empty_line(
                    IconName::Layers,
                    "No views in Linear yet. Save a filter on a team's screen to make one.",
                    cx,
                )
                .px_2()
                .py_2()
                .into_any_element(),
            );
        } else {
            rows.push(column_heading(cx).into_any_element());
            for view in &linear {
                let lanes = self.linear_lanes(view, cx);
                rows.push(self.row(view, lanes, cx).into_any_element());
            }
        }
        if !legacy.is_empty() {
            rows.push(
                v_flex()
                    .id("legacy-views-heading")
                    .role(Role::Heading)
                    .aria_label("Only in Loupe")
                    .px_2()
                    .pt_6()
                    .pb_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(ui::group_title("Only in Loupe"))
                            .child(ui::count_badge(legacy.len(), cx)),
                    )
                    .child(
                        div()
                            .text_size(scale::meta())
                            .text_color(tokens::text_faint())
                            .child(
                                "Loupe keeps its views in Linear now. Save these to Linear, or delete the ones you do not use.",
                            ),
                    )
                    .into_any_element(),
            );
            for view in &legacy {
                let actions = self.legacy_actions(view, cx);
                rows.push(self.row(view, actions, cx).into_any_element());
            }
        }
        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("views")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    // 8 px here and 8 px inside each row put every label on
                    // the 16 px content edge while the hover fill reaches
                    // past it.
                    .child(v_flex().w_full().px_2().py_2().children(rows)),
            )
            .into_any_element()
    }
}
