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
use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
use crate::theme::scale;
use crate::ui::{self, ListRowFocus as _};

pub fn view(team_id: Option<String>, _window: &mut Window, cx: &mut App) -> AnyView {
    build(team_id, cx).into()
}

fn build(team_id: Option<String>, cx: &mut App) -> Entity<ProjectsView> {
    cx.new(|cx| {
        let mut view = ProjectsView {
            team_id,
            client: None,
            projects: Loadable::Loading,
            task: None,
            scroll: ScrollHandle::new(),
            rows: ScrollHandle::new(),
            focus: cx.focus_handle(),
            cursor: None,
        };
        view.sync_client(cx);
        view
    })
}

struct ProjectsView {
    /// `None`: every project the key can see.
    team_id: Option<String>,
    client: Option<Arc<Linear>>,
    projects: Loadable<Vec<ProjectSummary>>,
    task: Option<Task<()>>,
    scroll: ScrollHandle,
    /// The list's children (the column heading, then a row per project), to
    /// scroll the keyboard cursor's row into view.
    rows: ScrollHandle,
    focus: FocusHandle,
    /// The id of the project under the keyboard cursor (J and K, or the
    /// arrows).
    cursor: Option<String>,
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

    /// The projects on screen, top to bottom.
    fn listed(&self) -> &[ProjectSummary] {
        match &self.projects {
            Loadable::Ready(projects) => projects,
            _ => &[],
        }
    }

    /// Moves the keyboard cursor one row and scrolls it into view, with the
    /// column heading when it is the first row.
    fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .listed()
            .iter()
            .map(|project| project.id.clone())
            .collect();
        let Some(next) = ui::step_cursor(&ids, self.cursor.as_ref(), down) else {
            return;
        };
        let at = ids.iter().position(|id| *id == next).unwrap_or(0);
        let row = at + 1;
        ui::scroll_to_rows(
            &self.scroll,
            &self.rows,
            if at == 0 { 0 } else { row }..=row,
        );
        self.cursor = Some(next);
        cx.notify();
    }

    /// Enter or O: opens the project under the cursor in this tab. With no
    /// cursor the key goes on to whoever handles it next.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .listed()
            .iter()
            .find(|project| self.cursor.as_ref() == Some(&project.id));
        match project {
            Some(project) => crate::shell::navigate(project_route(project), window, cx),
            None => cx.propagate(),
        }
    }

    fn heading(&self, cx: &App) -> impl IntoElement {
        ui::list_column_header(cx)
            .child(div().flex_none().size_2p5())
            .child(div().flex_1().min_w_0().child("Project"))
            .child(div().flex_none().w_32().child("Status"))
            .child(div().flex_none().w_40().child("Lead"))
            .child(div().flex_none().w_24().child("Progress"))
            .child(div().flex_none().w_20().text_right().child("Target"))
    }

    /// One project's row. `focused` marks the row under the keyboard cursor
    /// while the list has the focus.
    fn row(&self, project: &ProjectSummary, focused: bool, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let color = widgets::hex_color(&project.color).unwrap_or(muted);
        let status_color = widgets::hex_color(&project.status.color).unwrap_or(muted);
        let route = project_route(project);
        let percent = (project.progress * 100.).round() as u32;
        ui::list_row(SharedString::from(format!("project-{}", project.id)), cx)
            .role(Role::Link)
            .aria_label(format!(
                "{}, {}, {percent}%",
                project.name, project.status.name
            ))
            .keyboard_focus(focused, cx)
            .opens(route)
            .child(div().flex_none().size_2p5().rounded_sm().bg(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
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
                    .child(widgets::avatar(project.lead.as_ref(), scale::avatar_sm()))
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

fn project_route(project: &ProjectSummary) -> Route {
    Route::LinearProject {
        id: project.id.clone(),
        name: project.name.clone(),
    }
}

impl Render for ProjectsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_client(cx);
        if self.client.is_none() {
            return widgets::not_connected(IconName::Box, "Projects");
        }
        if crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        let focused = self.focus.is_focused(window);
        let header = match &self.projects {
            Loadable::Ready(projects) => ui::page_header("Projects")
                .icon(IconName::Box)
                .count(projects.len()),
            _ => ui::page_header("Projects").icon(IconName::Box),
        };
        let body = match &self.projects {
            Loadable::Loading => ui::loading_rows(8).into_any_element(),
            Loadable::Failed(message) => {
                widgets::load_error(message.clone(), cx.listener(|this, _, _, cx| this.load(cx)))
            }
            Loadable::Ready(projects) if projects.is_empty() => ui::empty_state(
                IconName::Box,
                "No projects",
                "Projects you can see in Linear show up here.",
            )
            .into_any_element(),
            Loadable::Ready(projects) => ui::list_container()
                .id("projects-rows")
                .track_scroll(&self.rows)
                .child(self.heading(cx))
                .children(projects.iter().map(|project| {
                    let at_cursor = focused && self.cursor.as_ref() == Some(&project.id);
                    self.row(project, at_cursor, cx)
                }))
                .into_any_element(),
        };
        v_flex()
            .id("projects-page")
            .role(Role::Group)
            .aria_label("Projects")
            .size_full()
            .key_context("ProjectsView")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_cursor(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_cursor(false, cx)))
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.open_cursor(window, cx)),
            )
            .child(header)
            .child(
                div()
                    .id("projects")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use loupe_linear::{ProjectStatus, ProjectStatusKind, ProjectSummary};

    use super::build;
    use crate::linear::data::Loadable;

    fn project(id: &str) -> ProjectSummary {
        ProjectSummary {
            id: id.into(),
            name: format!("Project {id}"),
            color: "#888888".into(),
            status: ProjectStatus {
                id: "started".into(),
                name: "In progress".into(),
                kind: ProjectStatusKind::Started,
                color: "#888888".into(),
            },
            progress: 0.5,
            lead: None,
            target_date: None,
            url: String::new(),
            team_ids: Vec::new(),
        }
    }

    /// The keyboard cursor walks the projects from the first, stopping at
    /// either end.
    #[gpui_kit::test]
    fn the_cursor_walks_the_projects(cx: &mut TestAppContext) {
        let view = cx.update(|cx| {
            cx.set_global(crate::services::Services::default());
            build(None, cx)
        });
        view.update(cx, |view, cx| {
            view.projects = Loadable::Ready(vec![project("a"), project("b")]);
            cx.notify();
        });
        let step = |down: bool, cx: &mut TestAppContext| {
            view.update(cx, |view, cx| {
                view.move_cursor(down, cx);
                view.cursor.clone()
            })
        };
        assert_eq!(step(true, cx).as_deref(), Some("a"));
        assert_eq!(step(true, cx).as_deref(), Some("b"));
        assert_eq!(
            step(true, cx).as_deref(),
            Some("b"),
            "the cursor stops at the end"
        );
        assert_eq!(step(false, cx).as_deref(), Some("a"));
    }
}
