//! The projects list, after Linear's: one row per project with its color,
//! name, status, lead, progress, and target date. A row opens the project's
//! page (`Route::LinearProject`).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_linear::{Linear, ProjectSummary};

use crate::linear::data::{self, Loadable};
use crate::linear::widgets;
use crate::route::Route;
use crate::services::background;
use crate::shell::OpenOnClick as _;
use crate::theme::scale;
use crate::theme::scale::dp;
use crate::ui;

pub fn view(team_id: Option<String>, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut view = ProjectsView {
            team_id,
            client: None,
            projects: Loadable::Loading,
            task: None,
        };
        view.sync_client(cx);
        view
    })
    .into()
}

struct ProjectsView {
    /// `None`: every project the key can see.
    team_id: Option<String>,
    client: Option<Arc<Linear>>,
    projects: Loadable<Vec<ProjectSummary>>,
    task: Option<Task<()>>,
}

impl ProjectsView {
    fn sync_client(&mut self, cx: &mut Context<Self>) {
        let client = data::client(cx);
        if data::same_client(&self.client, &client) {
            return;
        }
        self.client = client;
        self.task = None;
        self.load(cx);
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.projects = Loadable::Loading;
        let team_id = self.team_id.clone();
        let work = background(cx, move || client.projects(team_id.as_deref()));
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |view, cx| {
                view.projects = match result {
                    Ok(projects) => Loadable::Ready(projects),
                    Err(error) => Loadable::Failed(data::error_text(&error)),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn row(&self, project: &ProjectSummary, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let color = widgets::hex_color(&project.color).unwrap_or(muted);
        let status_color = widgets::hex_color(&project.status.color).unwrap_or(muted);
        let route = Route::LinearProject {
            id: project.id.clone(),
            name: project.name.clone(),
        };
        let percent = (project.progress * 100.).round() as u32;
        h_flex()
            .id(SharedString::from(format!("project-{}", project.id)))
            .role(Role::Link)
            .aria_label(format!(
                "{}, {}, {percent}%",
                project.name, project.status.name
            ))
            .h(scale::row_height())
            .px(scale::content_inset())
            .gap_3()
            .items_center()
            .hover(move |row| row.bg(hover))
            .opens(route)
            .child(div().flex_none().size_2p5().rounded_sm().bg(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::body())
                    .font_weight(FontWeight::MEDIUM)
                    .child(project.name.clone()),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_32()
                    .gap_1p5()
                    .items_center()
                    .text_size(scale::meta())
                    .child(div().flex_none().size_2().rounded_full().bg(status_color))
                    .child(div().truncate().child(project.status.name.clone())),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_40()
                    .gap_1p5()
                    .items_center()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(widgets::avatar(project.lead.as_ref(), dp(18.)))
                    .child(
                        div().truncate().child(
                            project
                                .lead
                                .as_ref()
                                .map_or_else(|| "No lead".to_owned(), |lead| lead.name.clone()),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_24()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div().w_12().h_1().rounded_full().bg(theme.muted).child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(status_color)
                                .w(relative(project.progress)),
                        ),
                    )
                    .child(
                        div()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .child(format!("{percent}%")),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w_20()
                    .text_right()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .child(
                        project
                            .target_date
                            .map_or_else(String::new, |date| date.format("%b %-d").to_string()),
                    ),
            )
    }
}

impl Render for ProjectsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_client(cx);
        if self.client.is_none() {
            return widgets::not_connected(IconName::Box, "Projects");
        }
        let header = match &self.projects {
            Loadable::Ready(projects) => ui::page_header("Projects")
                .icon(IconName::Box)
                .count(projects.len()),
            _ => ui::page_header("Projects").icon(IconName::Box),
        };
        let body = match &self.projects {
            Loadable::Loading => ui::skeleton_rows(8).into_any_element(),
            Loadable::Failed(message) => {
                widgets::load_error(message.clone(), cx.listener(|this, _, _, cx| this.load(cx)))
            }
            Loadable::Ready(projects) if projects.is_empty() => ui::empty_state(
                IconName::Box,
                "No projects",
                "Projects you can see in Linear show up here.",
            )
            .into_any_element(),
            Loadable::Ready(projects) => v_flex()
                .py_2()
                .children(projects.iter().map(|project| self.row(project, cx)))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("projects")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}
