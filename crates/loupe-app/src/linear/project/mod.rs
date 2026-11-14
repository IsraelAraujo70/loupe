//! A Linear project's page, after Linear's: a header with the project's
//! name, its two tabs, and its progress; Overview (the summary, the
//! description, milestones with their progress, the latest updates, and the
//! resources, beside the properties, which edit the project in Linear) and
//! Issues (the project's issues with the team screen's filters, layouts, and
//! groupings).

mod model;
mod overview;
mod properties;

use std::sync::Arc;

use chrono::NaiveDate;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::calendar::{CalendarEvent, CalendarState, Date};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use loupe_linear::{Linear, ProjectChange, ProjectDetail, ProjectOptions};

use self::model::Field;
use super::data::{self, Loadable};
use super::team::{self, TeamView};
use super::widgets;
use crate::route::Route;
use crate::shell::svg_icon;
use crate::theme::scale::{self, dp};
use crate::ui;

pub fn view(id: String, name: String, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ProjectView::new(id, name, window, cx)).into()
}

/// The two halves of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Overview,
    Issues,
}

/// Which date of the project a calendar edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateField {
    Start,
    Target,
}

/// The picker on screen and the row its keyboard cursor is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenPicker {
    field: Field,
    cursor: usize,
}

struct ProjectView {
    id: String,
    name: SharedString,
    /// The client the loads below belong to; a different client restarts them.
    client: Option<Arc<Linear>>,
    project: Loadable<ProjectDetail>,
    options: Loadable<Arc<ProjectOptions>>,
    tab: Tab,
    /// The Issues tab, built once the project names its lead team.
    issues: Option<Entity<TeamView>>,
    picker: Option<OpenPicker>,
    picker_focus: FocusHandle,
    /// Bumped by every edit sent: only the answer to the newest one replaces
    /// what the page shows, so an older answer never undoes a newer edit.
    edit_seq: u64,
    /// Edits sent and not answered yet.
    saving: usize,
    edit_error: Option<SharedString>,
    /// Bumped by every load of the project, so an older answer is dropped.
    generation: u64,
    start_calendar: Entity<CalendarState>,
    target_calendar: Entity<CalendarState>,
    /// The date whose calendar is open.
    date_open: Option<DateField>,
    /// The dates the calendars were last set to, to set them again only when
    /// the project's change.
    shown_dates: (Option<NaiveDate>, Option<NaiveDate>),
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ProjectView {
    fn new(id: String, name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let start_calendar = cx.new(|cx| CalendarState::new(window, cx));
        let target_calendar = cx.new(|cx| CalendarState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&start_calendar, |view, _, event: &CalendarEvent, cx| {
                let CalendarEvent::Selected(date) = event;
                view.set_date(DateField::Start, date.start(), cx)
            }),
            cx.subscribe(&target_calendar, |view, _, event: &CalendarEvent, cx| {
                let CalendarEvent::Selected(date) = event;
                view.set_date(DateField::Target, date.start(), cx)
            }),
        ];
        let mut view = ProjectView {
            id,
            name: name.into(),
            client: None,
            project: Loadable::Loading,
            options: Loadable::Loading,
            tab: Tab::Issues,
            issues: None,
            picker: None,
            picker_focus: cx.focus_handle(),
            edit_seq: 0,
            saving: 0,
            edit_error: None,
            generation: 0,
            start_calendar,
            target_calendar,
            date_open: None,
            shown_dates: (None, None),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        view.sync_client(cx);
        view
    }

    /// Starts loading when the signed-in client is new to this view.
    fn sync_client(&mut self, cx: &mut Context<Self>) {
        let client = data::client(cx);
        if data::same_client(&self.client, &client) {
            return;
        }
        self.client = client;
        self.issues = None;
        if self.client.is_some() {
            self.load(cx);
            self.load_options(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let id = self.id.clone();
        let task = crate::services::background(cx, move || client.project(&id));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                view.project = match result {
                    Ok(project) => {
                        view.name = project.name.clone().into();
                        Loadable::Ready(project)
                    }
                    Err(error) => Loadable::Failed(data::error_text(&error)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn load_options(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.options = Loadable::Loading;
        let task = crate::services::background(cx, move || client.project_options());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.options = match result {
                    Ok(options) => Loadable::Ready(Arc::new(options)),
                    Err(error) => Loadable::Failed(data::error_text(&error)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the project, its options, and its issues again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
        self.load_options(cx);
        if let Some(issues) = &self.issues {
            issues.update(cx, |issues, cx| issues.refresh(cx));
        }
    }

    /// Shows `change` at once and sends it to Linear. Linear's answer to the
    /// newest edit is what stays; a refusal says why and reads the project
    /// again.
    fn edit(&mut self, noun: &'static str, change: ProjectChange, cx: &mut Context<Self>) {
        let (Some(client), Loadable::Ready(project), Loadable::Ready(options)) =
            (self.client.clone(), &mut self.project, &self.options)
        else {
            return;
        };
        model::apply(project, &change, options);
        self.edit_seq += 1;
        self.saving += 1;
        self.edit_error = None;
        let seq = self.edit_seq;
        let id = project.id.clone();
        let task = crate::services::background(cx, move || client.update_project(&id, &change));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.saving -= 1;
                match result {
                    Ok(project) if view.edit_seq == seq => {
                        view.project = Loadable::Ready(project);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        view.edit_error = Some(
                            format!("Couldn’t change {noun}: {}", data::error_text(&error)).into(),
                        );
                        view.load(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A day picked in a date's calendar, or `None` for Clear. The
    /// calendar closes.
    fn set_date(&mut self, field: DateField, picked: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.date_open = None;
        cx.notify();
        let Loadable::Ready(project) = &self.project else {
            return;
        };
        let (current, change, noun) = match field {
            DateField::Start => (
                project.start_date,
                ProjectChange {
                    start_date: Some(picked),
                    ..ProjectChange::default()
                },
                "start date",
            ),
            DateField::Target => (
                project.target_date,
                ProjectChange {
                    target_date: Some(picked),
                    ..ProjectChange::default()
                },
                "target date",
            ),
        };
        if picked == current {
            return;
        }
        // The calendar already shows the day it sent.
        match field {
            DateField::Start => self.shown_dates.0 = picked,
            DateField::Target => self.shown_dates.1 = picked,
        }
        self.edit(noun, change, cx);
    }

    /// Puts the project's dates in the calendars when they differ from what
    /// the calendars were last set to (a load, an answer from Linear).
    fn sync_dates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Loadable::Ready(project) = &self.project else {
            return;
        };
        let dates = (project.start_date, project.target_date);
        if dates.0 != self.shown_dates.0 {
            self.start_calendar.update(cx, |calendar, cx| {
                calendar.set_date(Date::Single(dates.0), window, cx)
            });
        }
        if dates.1 != self.shown_dates.1 {
            self.target_calendar.update(cx, |calendar, cx| {
                calendar.set_date(Date::Single(dates.1), window, cx)
            });
        }
        self.shown_dates = dates;
    }

    /// Builds the Issues tab once the project names its lead team.
    fn ensure_issues(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issues.is_some() {
            return;
        }
        let Loadable::Ready(project) = &self.project else {
            return;
        };
        let team_name = project
            .teams
            .iter()
            .find(|team| team.id == project.lead_team_id)
            .map(|team| team.name.clone())
            .unwrap_or_default();
        if project.lead_team_id.is_empty() {
            return;
        }
        self.issues = Some(team::project_issues(
            project.id.clone(),
            project.lead_team_id.clone(),
            team_name,
            window,
            cx,
        ));
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.picker = None;
        self.date_open = None;
        cx.notify();
    }
}

/// The project's icon: Linear's icon when Loupe has one like it, else a box,
/// in the project's color.
fn project_icon(project: &ProjectDetail, size: f32, cx: &App) -> Svg {
    let icon = match project.icon.as_deref() {
        Some("Shield") => IconName::Shield,
        Some("Rocket") => IconName::Rocket,
        Some("Target") => IconName::Target,
        Some("Flag") => IconName::Flag,
        Some("Diamond") => IconName::Diamond,
        Some("Gem") => IconName::Gem,
        Some("Briefcase") => IconName::Briefcase,
        Some("Globe") => IconName::Globe,
        Some("Database") => IconName::Database,
        Some("Server") => IconName::Server,
        Some("Cloud") => IconName::Cloud,
        Some("Bug") => IconName::Bug,
        Some("Heart") => IconName::Heart,
        Some("Lightbulb") => IconName::Lightbulb,
        Some("Megaphone") => IconName::Megaphone,
        Some("Key") => IconName::Key,
        Some("Users") => IconName::Users,
        Some("Bolt") => IconName::Zap,
        Some("Folder") => IconName::Folder,
        Some("Book") => IconName::BookOpen,
        _ => IconName::Box,
    };
    let color = widgets::hex_color(&project.color).unwrap_or(cx.theme().muted_foreground);
    svg_icon(icon, dp(size), color)
}

/// A progress bar of `width` and the percent after it.
fn progress(progress: f32, width: f32, color: Hsla, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .gap_1p5()
        .items_center()
        .child(
            div()
                .w(dp(width))
                .h_1()
                .rounded_full()
                .bg(theme.muted)
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(color)
                        .w(relative(progress.clamp(0., 1.))),
                ),
        )
        .child(
            div()
                .text_size(scale::meta())
                .text_color(theme.muted_foreground)
                .child(format!("{}%", model::percent(progress))),
        )
}

impl ProjectView {
    /// The header: "Projects" as a crumb back to the list, the project's
    /// icon and name, its two tabs, then its progress, Refresh, and Open in
    /// Linear.
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let crumb_route = Route::LinearProjects { team_id: None };
        let crumb = h_flex()
            .gap_1()
            .items_center()
            .child(
                Button::new("project-crumb")
                    .ghost()
                    .xsmall()
                    .label("Projects")
                    .accessibility_label("All projects")
                    .on_click(move |event, window, cx| {
                        crate::shell::open(crumb_route.clone(), event, window, cx)
                    }),
            )
            .child(div().text_color(cx.theme().muted_foreground).child("›"));
        let crumb = match &self.project {
            Loadable::Ready(project) => crumb.child(project_icon(project, 16., cx)),
            _ => crumb.child(svg_icon(
                IconName::Box,
                dp(16.),
                cx.theme().muted_foreground,
            )),
        };
        let tabs = h_flex()
            .id("project-tabs")
            .role(Role::TabList)
            .gap_1()
            .ml_2()
            .child(
                ui::view_tab(
                    "project-tab-overview",
                    "Overview",
                    self.tab == Tab::Overview,
                    cx,
                )
                .on_click(cx.listener(|view, _, _, cx| view.set_tab(Tab::Overview, cx))),
            )
            .child(
                ui::view_tab("project-tab-issues", "Issues", self.tab == Tab::Issues, cx)
                    .on_click(cx.listener(|view, _, _, cx| view.set_tab(Tab::Issues, cx))),
            );
        let refresh = Button::new("project-refresh")
            .ghost()
            .small()
            .icon(IconName::RefreshCw)
            .tooltip("Refresh")
            .accessibility_label("Refresh")
            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx)));
        let mut header = ui::page_header(self.name.clone())
            .prefix(crumb)
            .after_title(tabs);
        if let Loadable::Ready(project) = &self.project {
            let color =
                widgets::hex_color(&project.status.color).unwrap_or(cx.theme().muted_foreground);
            let percent = model::percent(project.progress);
            let url = project.url.clone();
            header = header
                .action(
                    div()
                        .id("project-progress")
                        .role(Role::ProgressIndicator)
                        .aria_label(format!("Progress, {percent}%"))
                        .mr_2()
                        .child(progress(project.progress, 64., color, cx)),
                )
                .action(refresh)
                .action(
                    Button::new("project-open-in-linear")
                        .ghost()
                        .small()
                        .icon(IconName::ExternalLink)
                        .label("Open in Linear")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
        } else {
            header = header.action(refresh);
        }
        header.into_any_element()
    }
}

impl Render for ProjectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_client(cx);
        if self.client.is_none() {
            return widgets::not_connected(IconName::Box, self.name.clone());
        }
        self.sync_dates(window, cx);
        self.ensure_issues(window, cx);
        if self.tab == Tab::Overview && crate::shell::page_may_take_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        let body = match (&self.project, self.tab) {
            (Loadable::Loading, _) => div().py_2().child(ui::skeleton_rows(8)).into_any_element(),
            (Loadable::Failed(message), _) => {
                widgets::load_error(message.clone(), cx.listener(|view, _, _, cx| view.load(cx)))
            }
            (Loadable::Ready(_), Tab::Issues) => match &self.issues {
                Some(issues) => div()
                    .flex_1()
                    .min_h_0()
                    .child(issues.clone())
                    .into_any_element(),
                None => ui::empty_state(
                    IconName::Box,
                    "No team",
                    "This project is not in a team, so it has no issues.",
                )
                .into_any_element(),
            },
            (Loadable::Ready(project), Tab::Overview) => {
                let project = project.clone();
                let side = self.render_properties(&project, cx);
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(overview::render(&project, cx))
                    .child(
                        v_flex()
                            .id("project-side")
                            .w(dp(360.))
                            .flex_none()
                            .h_full()
                            .overflow_y_scroll()
                            .border_l_1()
                            .border_color(crate::theme::tokens::border_subtle())
                            .child(side),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .key_context("ProjectView")
            .track_focus(&self.focus)
            .child(self.render_header(cx))
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::Route;

    #[test]
    fn a_project_tab_is_named_after_the_project() {
        let route = Route::LinearProject {
            id: "p1".into(),
            name: "Vendor Integrations".into(),
        };
        assert_eq!(route.label(), "Vendor Integrations");
    }
}
