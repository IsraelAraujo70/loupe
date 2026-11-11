//! The Overview tab's reading column: the project's icon, name, summary,
//! and a band with its status, lead, dates, and progress; then the
//! description, the milestones with their progress, the latest updates, and
//! the resources.

use chrono::{NaiveDate, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::{
    ProjectDetail, ProjectHealth, ProjectMilestone, ProjectPost, ProjectResource,
    ProjectResourceKind,
};

use super::{model, progress, project_icon};
use crate::linear::widgets;
use crate::shell::svg_icon;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;
use crate::ui;

/// How many updates the overview shows; the rest are in Linear.
const UPDATES_SHOWN: usize = 3;

fn day(date: NaiveDate) -> String {
    date.format("%b %-d, %Y").to_string()
}

/// The project's start and target dates as one span: "Nov 6, 2026 → Nov 20,
/// 2026", with "No date" for a missing end.
pub(super) fn date_span(project: &ProjectDetail) -> Option<String> {
    match (project.start_date, project.target_date) {
        (None, None) => None,
        (start, target) => Some(format!(
            "{} → {}",
            start.map_or_else(|| "No date".to_owned(), day),
            target.map_or_else(|| "No date".to_owned(), day)
        )),
    }
}

fn health_color(health: ProjectHealth) -> Hsla {
    match health {
        ProjectHealth::OnTrack => tokens::open(),
        ProjectHealth::AtRisk => tokens::pending(),
        ProjectHealth::OffTrack => tokens::closed(),
    }
}

/// A section of the column: its label (with a count when it has rows) over
/// its body.
fn section(label: &'static str, count: Option<usize>, body: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(ui::section_label(label, cx))
                .children(
                    count
                        .filter(|count| *count > 0)
                        .map(|count| ui::count_badge(count, cx)),
                ),
        )
        .child(body)
}

/// The band under the name: status, lead, dates, and progress.
fn band(project: &ProjectDetail, cx: &App) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    let status_color = widgets::hex_color(&project.status.color).unwrap_or(muted);
    let lead = project.lead.as_ref();
    let fact = |content: Div| content.flex_none().gap_1p5().items_center();
    h_flex()
        .id("project-band")
        .min_h_8()
        .flex_wrap()
        .gap_x_4()
        .gap_y_1()
        .items_center()
        .text_size(scale::meta())
        .child(
            ui::tinted_chip(status_color)
                .gap_1p5()
                .child(div().size_2().rounded_full().bg(status_color))
                .child(project.status.name.clone()),
        )
        .child(
            fact(h_flex()).child(widgets::avatar(lead, dp(16.))).child(
                div()
                    .when(lead.is_none(), |text| text.text_color(muted))
                    .child(lead.map_or_else(|| "No lead".to_owned(), |lead| lead.name.clone())),
            ),
        )
        .child(
            fact(h_flex())
                .child(svg_icon(IconName::Calendar, dp(14.), tokens::text_faint()))
                .child(
                    div()
                        .when(date_span(project).is_none(), |text| text.text_color(muted))
                        .child(date_span(project).unwrap_or_else(|| "No dates".into())),
                ),
        )
        .child(progress(project.progress, 96., status_color, cx))
}

fn milestone_row(ix: usize, milestone: &ProjectMilestone, cx: &App) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    let percent = model::percent(milestone.progress);
    let done = milestone.progress >= 1.;
    h_flex()
        .id(("milestone", ix))
        .role(Role::ListItem)
        .aria_label(format!("{}, {percent}%", milestone.name))
        .h(dp(widgets::ROW_HEIGHT))
        .gap_2()
        .items_center()
        .text_size(scale::body())
        .child(svg_icon(
            if done {
                IconName::CircleCheck
            } else {
                IconName::Milestone
            },
            dp(16.),
            if done { tokens::open() } else { muted },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(milestone.name.clone()),
        )
        .child(
            div()
                .flex_none()
                .w(dp(96.))
                .text_right()
                .text_size(scale::meta())
                .text_color(muted)
                .child(milestone.target_date.map(day).unwrap_or_default()),
        )
        .child(
            div()
                .flex_none()
                .w(dp(112.))
                .flex()
                .justify_end()
                .child(progress(milestone.progress, 64., tokens::accent(), cx)),
        )
}

fn update_card(ix: usize, post: &ProjectPost, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    let color = health_color(post.health);
    let author = post.author.as_ref();
    ui::card(cx)
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_size(scale::meta())
                .child(
                    ui::tinted_chip(color)
                        .gap_1p5()
                        .child(div().size_2().rounded_full().bg(color))
                        .child(post.health.label()),
                )
                .child(widgets::avatar(author, dp(16.)))
                .child(
                    div().child(
                        author.map_or_else(|| "Someone".to_owned(), |user| user.name.clone()),
                    ),
                )
                .child(
                    div()
                        .text_color(muted)
                        .child(ui::relative_time(post.created_at, Utc::now())),
                ),
        )
        .child(ui::markdown(("project-update", ix), post.body.clone()))
}

fn resource_row(ix: usize, resource: &ProjectResource, cx: &App) -> Stateful<Div> {
    let hover = cx.theme().list_hover;
    let url = resource.url.clone();
    h_flex()
        .id(("resource", ix))
        .role(Role::Link)
        .aria_label(resource.title.clone())
        .h(dp(widgets::ROW_HEIGHT))
        .px_2()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius)
        .text_size(scale::body())
        .cursor_pointer()
        .hover(move |row| row.bg(hover))
        .on_click(move |_, _, cx| cx.open_url(&url))
        .child(svg_icon(
            match resource.kind {
                ProjectResourceKind::Document => IconName::FileText,
                ProjectResourceKind::Link => IconName::Link,
            },
            dp(16.),
            tokens::text_faint(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(resource.title.clone()),
        )
}

/// The Overview tab's column, scrolling as a whole.
pub(super) fn render(project: &ProjectDetail, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let heading = h_flex()
        .gap_3()
        .items_center()
        .child(project_icon(project, 24., cx))
        .child(
            div()
                .min_w_0()
                .text_size(rems(1.375))
                .line_height(rems(1.75))
                .font_weight(FontWeight::SEMIBOLD)
                .child(project.name.clone()),
        );
    let summary = (!project.summary.trim().is_empty()).then(|| {
        div()
            .text_size(scale::body())
            .text_color(muted)
            .child(project.summary.clone())
    });
    let description = match &project.content {
        Some(content) => div()
            .w_full()
            .child(ui::markdown("project-description", content.clone()))
            .into_any_element(),
        None => ui::empty_line(IconName::TextQuote, "No description", cx).into_any_element(),
    };
    let milestones = if project.milestones.is_empty() {
        ui::empty_line(IconName::Milestone, "None", cx).into_any_element()
    } else {
        v_flex()
            .children(
                project
                    .milestones
                    .iter()
                    .enumerate()
                    .map(|(ix, milestone)| milestone_row(ix, milestone, cx)),
            )
            .into_any_element()
    };
    let updates = if project.updates.is_empty() {
        ui::empty_line(IconName::MessageSquare, "No updates yet", cx).into_any_element()
    } else {
        v_flex()
            .gap_3()
            .children(
                project
                    .updates
                    .iter()
                    .take(UPDATES_SHOWN)
                    .enumerate()
                    .map(|(ix, post)| update_card(ix, post, cx)),
            )
            .into_any_element()
    };
    let resources = if project.resources.is_empty() {
        ui::empty_line(IconName::Link, "None", cx).into_any_element()
    } else {
        // The rows' hover fill reaches 8 px past the column's edge.
        v_flex()
            .mx(dp(-8.))
            .children(
                project
                    .resources
                    .iter()
                    .enumerate()
                    .map(|(ix, resource)| resource_row(ix, resource, cx)),
            )
            .into_any_element()
    };
    v_flex()
        .id("project-overview")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .child(
            v_flex()
                .w_full()
                .max_w(dp(ui::READING_WIDTH + 32.))
                .px(scale::content_inset())
                .py_5()
                .gap_6()
                .child(
                    v_flex()
                        .gap_2()
                        .child(heading)
                        .children(summary)
                        .child(band(project, cx)),
                )
                .child(description)
                .child(section(
                    "Milestones",
                    Some(project.milestones.len()),
                    milestones,
                    cx,
                ))
                .child(section(
                    "Latest updates",
                    Some(project.updates.len()),
                    updates,
                    cx,
                ))
                .child(section(
                    "Resources",
                    Some(project.resources.len()),
                    resources,
                    cx,
                )),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use loupe_linear::{ProjectDetail, ProjectStatus, ProjectStatusKind};

    use super::date_span;

    fn project(start: Option<NaiveDate>, target: Option<NaiveDate>) -> ProjectDetail {
        ProjectDetail {
            id: "p1".into(),
            name: "Integrations".into(),
            icon: None,
            color: "#4cb782".into(),
            summary: String::new(),
            content: None,
            status: ProjectStatus {
                id: "s".into(),
                name: "Started".into(),
                kind: ProjectStatusKind::Started,
                color: "#f2c94c".into(),
            },
            progress: 0.,
            lead: None,
            start_date: start,
            target_date: target,
            url: String::new(),
            lead_team_id: String::new(),
            teams: Vec::new(),
            members: Vec::new(),
            labels: Vec::new(),
            milestones: Vec::new(),
            updates: Vec::new(),
            resources: Vec::new(),
        }
    }

    #[test]
    fn the_dates_read_as_one_span() {
        let day = |d| NaiveDate::from_ymd_opt(2026, 11, d);
        assert_eq!(
            date_span(&project(day(6), day(20))).as_deref(),
            Some("Nov 6, 2026 → Nov 20, 2026")
        );
        assert_eq!(
            date_span(&project(None, day(20))).as_deref(),
            Some("No date → Nov 20, 2026")
        );
        assert_eq!(date_span(&project(None, None)), None);
    }
}
