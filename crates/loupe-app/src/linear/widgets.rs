//! Linear-style building blocks shared by the Linear screens, after
//! docs/DESIGN.md: priority and state icons, label tags, avatars with
//! initials, issue rows (36 px, never wrapping) and board cards, the page
//! header, and the loading, empty, error, and not-connected states.

use chrono::{DateTime, Utc};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{PrRef, RepoRef};
use loupe_linear::views::DisplayProperty;
use loupe_linear::{
    IssueSummary, Label, Priority, PullRequestLink, PullRequestState, StateRef, StateType, User,
};

use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

/// Row height for every Linear list (docs/DESIGN.md, "Rows and cards").
pub const ROW_HEIGHT: f32 = 32.;
/// Height of a board column header, the same as a page header.
pub const HEADER_HEIGHT: f32 = 44.;
/// Secondary toolbar height (tabs, filters).
pub const TOOLBAR_HEIGHT: f32 = 40.;
/// Board column width.
pub const COLUMN_WIDTH: f32 = 280.;
const ICON: f32 = 16.;
const LABELS_SHOWN: usize = 2;
/// The widest a chip or a reference gets: a board card's width less its
/// padding and border, so it is cut only when it cannot fit a card at all.
const CARD_TEXT_WIDTH: f32 = COLUMN_WIDTH - 26.;

/// Parses `#rrggbb` (or `rrggbb`); `None` for anything else. Linear's state,
/// label, and project colors are data, so they are the one place a hex value
/// reaches a view.
pub fn hex_color(hex: &str) -> Option<Hsla> {
    crate::theme::palette::data_color(hex)
}

/// Linear's priority glyph: three bars filled by level, a warning-colored box
/// with "!" for urgent, three dashes for none.
pub fn priority_icon(priority: Priority, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let frame = || {
        div()
            .flex()
            .flex_none()
            .w(dp(ICON))
            .h(dp(ICON))
            .items_center()
            .justify_center()
    };
    let level = match priority {
        Priority::None => {
            return frame()
                .gap(px(1.))
                .children((0..3).map(|_| {
                    div()
                        .w(dp(3.))
                        .h(dp(1.5))
                        .rounded(dp(0.5))
                        .bg(muted.opacity(0.6))
                }))
                .into_any_element();
        }
        Priority::Urgent => {
            let cut = theme.background;
            return frame()
                .child(
                    v_flex()
                        .items_center()
                        .justify_center()
                        .gap(dp(1.5))
                        .size(dp(ICON - 1.))
                        .rounded(dp(3.))
                        .bg(theme.warning)
                        .child(div().w(dp(1.5)).h(dp(5.)).rounded_full().bg(cut))
                        .child(div().size(dp(1.8)).rounded_full().bg(cut)),
                )
                .into_any_element();
        }
        Priority::Low => 1,
        Priority::Medium => 2,
        Priority::High => 3,
    };
    const HEIGHTS: [f32; 3] = [4.5, 7.5, 11.];
    frame()
        .items_end()
        .gap(px(1.))
        .pb(dp(1.5))
        .children(HEIGHTS.iter().enumerate().map(|(index, height)| {
            let color = if index < level {
                muted
            } else {
                muted.opacity(0.3)
            };
            div().w(dp(3.)).h(dp(*height)).rounded(px(1.)).bg(color)
        }))
        .into_any_element()
}

/// A circle in the state's color: dashed for backlog and triage, empty for
/// todo, half filled for in progress, filled with a check for done, filled
/// with a cross for canceled and duplicate.
pub fn state_icon(state: &StateRef, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = hex_color(&state.color).unwrap_or(theme.muted_foreground);
    let cut = theme.background;
    let circle = || div().flex().flex_none().size(dp(ICON)).rounded_full();
    let ring = || {
        circle()
            .items_center()
            .justify_center()
            .border_2()
            .border_color(color)
    };
    let disc = |icon: IconName| {
        circle()
            .items_center()
            .justify_center()
            .bg(color)
            .child(Icon::new(icon).size(dp(9.)).text_color(cut))
    };
    match state.kind {
        StateType::Backlog | StateType::Triage => ring().border_dashed().into_any_element(),
        StateType::Unstarted => ring().into_any_element(),
        StateType::Started => ring()
            .child(
                div().flex().justify_end().size(dp(6.)).child(
                    div()
                        .w(dp(3.))
                        .h(dp(6.))
                        .rounded_tr(dp(3.))
                        .rounded_br(dp(3.))
                        .bg(color),
                ),
            )
            .into_any_element(),
        StateType::Completed => disc(IconName::Check).into_any_element(),
        StateType::Canceled | StateType::Duplicate => disc(IconName::Close).into_any_element(),
    }
}

/// Whether a label marks the issue as blocked. Teams name it "Blocked";
/// it is the one label that says the issue cannot move, so it is drawn as a
/// state and not as a category.
fn is_blocked(label: &Label) -> bool {
    label.name.trim().eq_ignore_ascii_case("blocked")
}

/// A small rounded tag with the label's color dot and name. A "Blocked"
/// label is a danger-tinted chip with an icon instead.
pub fn label_tag(label: &Label, cx: &App) -> AnyElement {
    if is_blocked(label) {
        return crate::ui::tinted_chip(tokens::closed())
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Icon::new(gpui_kit::assets::IconName::Ban)
                            .size(dp(12.))
                            .text_color(tokens::closed()),
                    )
                    .child(label.name.clone()),
            )
            .into_any_element();
    }
    let dot = hex_color(&label.color).unwrap_or(cx.theme().muted_foreground);
    crate::ui::chip()
        .small()
        .rounded_full()
        .child(
            h_flex()
                .gap_1()
                .max_w(dp(CARD_TEXT_WIDTH - 18.))
                .child(div().flex_none().size(dp(8.)).rounded_full().bg(dot))
                .child(div().min_w_0().truncate().child(label.name.clone())),
        )
        .into_any_element()
}

/// A user's avatar with initials as the fallback; an anonymous placeholder
/// when there is no user.
pub fn avatar(user: Option<&User>, size: Pixels) -> AnyElement {
    match user {
        Some(user) => {
            let name = if user.name.trim().is_empty() {
                user.display_name.clone()
            } else {
                user.name.clone()
            };
            ui::avatar_named(user.avatar_url.clone().unwrap_or_default(), name, size)
                .into_any_element()
        }
        None => ui::avatar("", size).into_any_element(),
    }
}

/// An issue identifier: 12 px, muted, never wrapping. It is a name, not
/// code, so it is set in the interface font, as in Linear.
pub fn identifier(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn meta(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// The color a pull request state carries on its icon and tag.
pub fn pull_request_color(state: PullRequestState, cx: &App) -> Hsla {
    match state {
        PullRequestState::Open => tokens::open(),
        PullRequestState::Draft => tokens::draft(),
        PullRequestState::Merged => tokens::merged(),
        PullRequestState::Closed => tokens::closed(),
        PullRequestState::Unknown => cx.theme().muted_foreground,
    }
}

pub fn pull_request_label(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "Open",
        PullRequestState::Draft => "Draft",
        PullRequestState::Merged => "Merged",
        PullRequestState::Closed => "Closed",
        PullRequestState::Unknown => "Linked",
    }
}

/// The glyph of a pull request state: each state has its own shape, so the
/// state does not rest on its color.
fn pull_request_icon(state: PullRequestState) -> gpui_kit::assets::IconName {
    use gpui_kit::assets::IconName;
    match state {
        PullRequestState::Open | PullRequestState::Unknown => IconName::GitPullRequest,
        PullRequestState::Draft => IconName::GitPullRequestDraft,
        PullRequestState::Merged => IconName::GitMerge,
        PullRequestState::Closed => IconName::GitPullRequestClosed,
    }
}

fn pull_request_route(link: &PullRequestLink) -> Route {
    Route::PullRequest(PrRef::new(
        RepoRef::new(link.owner.clone(), link.repo.clone()),
        link.number,
    ))
}

/// What the chip of these pull requests says: the number of the one, or how
/// many.
fn pull_requests_text(prs: &[PullRequestLink]) -> String {
    match prs {
        [only] => format!("#{}", only.number),
        several => format!("{} PRs", several.len()),
    }
}

/// An issue's pull requests as one chip, the height of a label: the icon in
/// the color of the most advanced state, then the number of the one, or how
/// many. A click opens the pull request (in a new tab for a Cmd-click or a
/// middle click); with several it opens a menu of them, each with its state.
/// The click stays on the chip, so the row or card under it does not open.
fn pull_requests_chip(issue: &IssueSummary, cx: &App) -> Option<AnyElement> {
    let prs = issue.pull_requests.as_slice();
    let rank = |state: PullRequestState| match state {
        PullRequestState::Merged => 0,
        PullRequestState::Open => 1,
        PullRequestState::Draft => 2,
        PullRequestState::Closed => 3,
        PullRequestState::Unknown => 4,
    };
    let lead = prs
        .iter()
        .map(|pr| pr.state)
        .min_by_key(|state| rank(*state))?;
    let id = SharedString::from(format!("pr-chip-{}", issue.id));
    let selector = id.to_string();
    let content = h_flex()
        .gap_1()
        .items_center()
        .child(
            Icon::new(gpui_kit::assets::IconName::GitPullRequest)
                .size(dp(12.))
                .text_color(pull_request_color(lead, cx)),
        )
        .child(pull_requests_text(prs));
    let chip = match prs {
        [only] => {
            let route = pull_request_route(only);
            let reference = format!("{}/{}#{}", only.owner, only.repo, only.number);
            let hint: SharedString =
                format!("{reference} · {}", pull_request_label(only.state)).into();
            ui::chip_button(id, format!("Open pull request {reference}"), cx)
                .debug_selector(move || selector)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(hint.clone()).build(window, cx)
                })
                .child(content)
                .on_open(move |event, window, cx| {
                    cx.stop_propagation();
                    crate::shell::open(route.clone(), event, window, cx);
                })
                .into_any_element()
        }
        several => {
            let links = several.to_vec();
            ui::chip_button(id, format!("{} pull requests", several.len()), cx)
                .debug_selector(move || selector)
                .child(content)
                // A click (which an accessibility client can send) opens the
                // menu through the press it simulates; neither it nor a
                // middle click reaches the row.
                .on_open(|_, _, cx| cx.stop_propagation())
                .dropdown_menu(move |menu, _, cx| {
                    links.iter().fold(menu.min_w(dp(240.)), |menu, link| {
                        let route = pull_request_route(link);
                        menu.item(
                            PopupMenuItem::new(format!(
                                "{}/{}#{} · {}",
                                link.owner,
                                link.repo,
                                link.number,
                                pull_request_label(link.state)
                            ))
                            .icon(
                                Icon::new(pull_request_icon(link.state))
                                    .text_color(pull_request_color(link.state, cx)),
                            )
                            .on_click(move |event, window, cx| {
                                crate::shell::open(route.clone(), event, window, cx)
                            }),
                        )
                    })
                })
                .into_any_element()
        }
    };
    Some(chip)
}

/// Up to [`LABELS_SHOWN`] tags, then a "+N" tag for the rest.
fn label_tags(labels: &[Label], cx: &App) -> Vec<AnyElement> {
    // "Blocked" leads, beside the priority and apart from the categories.
    let mut ordered: Vec<&Label> = labels.iter().collect();
    ordered.sort_by_key(|label| !is_blocked(label));
    let mut tags: Vec<AnyElement> = ordered
        .into_iter()
        .take(LABELS_SHOWN)
        .map(|label| label_tag(label, cx))
        .collect();
    if labels.len() > LABELS_SHOWN {
        tags.push(
            crate::ui::chip()
                .small()
                .rounded_full()
                .child(format!("+{}", labels.len() - LABELS_SHOWN))
                .into_any_element(),
        );
    }
    tags
}

fn project_meta(issue: &IssueSummary, cx: &App) -> Option<AnyElement> {
    let project = issue.project.as_ref()?;
    let swatch = project
        .color
        .as_deref()
        .and_then(hex_color)
        .unwrap_or(cx.theme().muted_foreground);
    // The same pill as a label, with the project's color as a square.
    Some(
        crate::ui::chip()
            .child(
                h_flex()
                    .gap_1()
                    .max_w(dp(CARD_TEXT_WIDTH - 18.))
                    .child(div().flex_none().size(dp(8.)).rounded(dp(2.)).bg(swatch))
                    .child(div().min_w_0().truncate().child(project.name.clone())),
            )
            .into_any_element(),
    )
}

fn due_meta(issue: &IssueSummary, now: DateTime<Utc>, cx: &App) -> Option<AnyElement> {
    let due = issue.due_date?;
    let overdue = !issue.state.kind.is_closed() && due < now.date_naive();
    let color = if overdue {
        cx.theme().danger
    } else {
        cx.theme().muted_foreground
    };
    Some(
        h_flex()
            .flex_none()
            .gap_1()
            .text_xs()
            .text_color(color)
            .child(Icon::new(IconName::Calendar).size(dp(12.)))
            .child(due.format("%b %-d").to_string())
            .into_any_element(),
    )
}

/// The trailing metadata a view shows for an issue, in a fixed order.
fn trailing(
    issue: &IssueSummary,
    now: DateTime<Utc>,
    props: &[DisplayProperty],
    cx: &App,
) -> Vec<AnyElement> {
    let shows = |property| props.contains(&property);
    let mut items = Vec::new();
    if shows(DisplayProperty::Labels) {
        items.extend(label_tags(&issue.labels, cx));
    }
    if shows(DisplayProperty::Project) {
        items.extend(project_meta(issue, cx));
    }
    if shows(DisplayProperty::Cycle)
        && let Some(cycle) = &issue.cycle
    {
        items.push(
            meta(
                cycle
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Cycle {}", cycle.number)),
                cx,
            )
            .into_any_element(),
        );
    }
    if shows(DisplayProperty::Estimate)
        && let Some(estimate) = issue.estimate
    {
        items.push(meta(format!("{estimate}"), cx).into_any_element());
    }
    if shows(DisplayProperty::DueDate) {
        items.extend(due_meta(issue, now, cx));
    }
    if shows(DisplayProperty::PullRequests) {
        items.extend(pull_requests_chip(issue, cx));
    }
    items
}

/// One 36 px list row: priority, identifier, state icon, title (truncated,
/// never wrapped), then right-aligned tags and metadata, the assignee, and
/// the age. `props` picks what shows, as the view's display settings say.
/// The caller adds `on_click`.
pub fn issue_row(
    issue: &IssueSummary,
    now: DateTime<Utc>,
    props: &[DisplayProperty],
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let shows = |property| props.contains(&property);
    let age = if shows(DisplayProperty::Created) {
        Some(crate::ui::relative_time(issue.created_at, now))
    } else if shows(DisplayProperty::Updated) {
        Some(crate::ui::relative_time(issue.updated_at, now))
    } else {
        None
    };
    h_flex()
        .id(SharedString::from(issue.id.clone()))
        .role(Role::Link)
        .aria_label(format!("{} {}", issue.identifier, issue.title))
        .w_full()
        .h(scale::row_dp(ROW_HEIGHT))
        .flex_none()
        .gap_2()
        .px(scale::content_inset())
        .text_size(scale::body())
        .overflow_hidden()
        .hover(|style| style.bg(theme.list_hover))
        .active(|style| style.bg(theme.list_active))
        .when(shows(DisplayProperty::Priority), |row| {
            row.child(priority_icon(issue.priority, cx))
        })
        .when(shows(DisplayProperty::Id), |row| {
            row.child(identifier(issue.identifier.clone(), cx).w(dp(76.)))
        })
        .when(shows(DisplayProperty::Status), |row| {
            row.child(state_icon(&issue.state, cx))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(issue.title.clone()),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_2()
                .children(trailing(issue, now, props, cx))
                .when(shows(DisplayProperty::Assignee), |row| {
                    row.child(avatar(issue.assignee.as_ref(), dp(20.)))
                })
                .children(age.map(|age| meta(age, cx).w(dp(28.)).text_right())),
        )
}

/// One board card: identifier and assignee on top, state icon and title,
/// then priority and the view's metadata. The caller adds `on_click`.
pub fn issue_card(
    issue: &IssueSummary,
    now: DateTime<Utc>,
    props: &[DisplayProperty],
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let shows = |property| props.contains(&property);
    // Always in this order, 4 px apart: identification (state, identifier,
    // assignee), the title (three lines at most), one line of pills in a
    // fixed order (priority, Blocked, linked pull request, labels, project)
    // with "+N" for the ones that do not fit, then a footer in the faint
    // color (the date, then cycle, estimate, due date). A block with nothing
    // in it is not drawn.
    let mut pills: Vec<Pill> = Vec::new();
    if shows(DisplayProperty::Priority) && issue.priority != Priority::None {
        pills.push(Pill {
            name: issue.priority.label().into(),
            width: ICON,
            element: priority_icon(issue.priority, cx),
        });
    }
    // Blocked comes right after the priority: when the line is short, the
    // two that say whether the issue can move are the ones that stay.
    let mut labels: Vec<&Label> = if shows(DisplayProperty::Labels) {
        issue.labels.iter().collect()
    } else {
        Vec::new()
    };
    labels.sort_by_key(|label| !is_blocked(label));
    let blocked = labels.iter().take_while(|label| is_blocked(label)).count();
    pills.extend(
        labels[..blocked]
            .iter()
            .map(|label| Pill::chip(label.name.clone(), label_tag(label, cx))),
    );
    if shows(DisplayProperty::PullRequests)
        && let Some(element) = pull_requests_chip(issue, cx)
    {
        pills.push(Pill::chip(
            pull_requests_text(&issue.pull_requests),
            element,
        ));
    }
    let mut categories: Vec<Pill> = labels[blocked..]
        .iter()
        .map(|label| Pill::chip(label.name.clone(), label_tag(label, cx)))
        .collect();
    if shows(DisplayProperty::Project)
        && let (Some(project), Some(element)) = (&issue.project, project_meta(issue, cx))
    {
        categories.push(Pill::chip(project.name.clone(), element));
    }
    // The priority, Blocked, and the pull request never go behind "+N":
    // the categories after them are what gives way.
    let kept = pills.len();
    pills.extend(categories);
    let chips = fit_pills(pills, kept, &issue.id);
    let mut references: Vec<AnyElement> = Vec::new();
    if shows(DisplayProperty::Cycle)
        && let Some(cycle) = &issue.cycle
    {
        let name = cycle
            .name
            .clone()
            .unwrap_or_else(|| format!("Cycle {}", cycle.number));
        references.push(meta(name, cx).into_any_element());
    }
    if shows(DisplayProperty::Estimate)
        && let Some(estimate) = issue.estimate
    {
        references.push(meta(format!("{estimate}"), cx).into_any_element());
    }
    if shows(DisplayProperty::DueDate) {
        references.extend(due_meta(issue, now, cx));
    }
    let date = if shows(DisplayProperty::Created) {
        Some(format!("Created {}", issue.created_at.format("%b %-d")))
    } else if shows(DisplayProperty::Updated) {
        Some(format!("Updated {}", issue.updated_at.format("%b %-d")))
    } else {
        None
    };
    let hover_border = theme.muted_foreground.opacity(0.45);

    v_flex()
        .id(SharedString::from(issue.id.clone()))
        .role(Role::Link)
        .aria_label(format!("{} {}", issue.identifier, issue.title))
        .w_full()
        .gap_1()
        .p_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(tokens::surface())
        .hover(move |style| style.border_color(hover_border))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .when(shows(DisplayProperty::Status), |row| {
                    row.child(state_icon(&issue.state, cx))
                })
                .when(shows(DisplayProperty::Id), |row| {
                    row.child(identifier(issue.identifier.clone(), cx))
                })
                .child(div().flex_1())
                .when(shows(DisplayProperty::Assignee), |row| {
                    row.child(avatar(issue.assignee.as_ref(), dp(16.)))
                }),
        )
        // `line_clamp` alone cuts the last line mid-word; the ellipsis says
        // the title goes on.
        .child(
            div()
                .w_full()
                // Three lines at most, 19 px each: a long title reads
                // whole more often, and the tighter gaps around it give the
                // height back.
                .line_clamp(3)
                .text_ellipsis()
                .text_size(scale::body())
                .line_height(rems(1.1875))
                .child(issue.title.clone()),
        )
        // One line, never wrapped: what does not fit is behind "+N".
        .when(!chips.is_empty(), |card| {
            card.child(h_flex().w_full().items_center().gap_1().children(chips))
        })
        .when(!references.is_empty() || date.is_some(), |card| {
            card.child(
                h_flex()
                    .w_full()
                    .overflow_hidden()
                    .gap_3()
                    .items_center()
                    .children(date.map(|date| meta(date, cx).text_color(tokens::text_faint())))
                    .children(references),
            )
        })
}

/// One pill of a board card: what it says, how wide it is expected to draw,
/// and the pill itself.
struct Pill {
    name: SharedString,
    width: f32,
    element: AnyElement,
}

impl Pill {
    /// A chip that shows `name` beside a dot or an icon. Its width is an
    /// estimate from the text's length: 30 px of padding, border, mark, and
    /// gap, and 7.2 px a character, the upper end for 12 px Inter (names in
    /// capitals and wide letters). Estimating high hides a chip behind "+N"
    /// a little early; estimating low cut the "+N" at the card's edge.
    fn chip(name: impl Into<SharedString>, element: AnyElement) -> Pill {
        let name: SharedString = name.into();
        let width = 30. + 7.2 * name.chars().count() as f32;
        Pill {
            name,
            width,
            element,
        }
    }
}

/// How many of `widths` fit a card's line, 4 px apart, leaving room for a
/// "+N" pill when some do not. The first `kept` always count: they are the
/// pills a card never hides.
fn pills_that_fit(widths: &[f32], kept: usize) -> usize {
    const GAP: f32 = 4.;
    const MORE: f32 = 36.;
    let total: f32 = widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32;
    if total <= CARD_TEXT_WIDTH {
        return widths.len();
    }
    let mut used = MORE;
    widths
        .iter()
        .take_while(|width| {
            used += **width + GAP;
            used <= CARD_TEXT_WIDTH
        })
        .count()
        .max(kept.min(widths.len()))
}

/// The pills that fit one line of a card, then "+N" for the rest with their
/// names in its tooltip. The pills sit in a part of the line that clips; the
/// "+N" follows it and is never cut.
fn fit_pills(pills: Vec<Pill>, kept: usize, id: &str) -> Vec<AnyElement> {
    let widths: Vec<f32> = pills.iter().map(|pill| pill.width).collect();
    let shown = pills_that_fit(&widths, kept);
    let hidden: Vec<SharedString> = pills[shown..]
        .iter()
        .map(|pill| pill.name.clone())
        .collect();
    let fitting: Vec<AnyElement> = pills
        .into_iter()
        .take(shown)
        .map(|pill| pill.element)
        .collect();
    if fitting.is_empty() && hidden.is_empty() {
        return Vec::new();
    }
    let mut line: Vec<AnyElement> = vec![
        h_flex()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap_1()
            .children(fitting)
            .into_any_element(),
    ];
    if !hidden.is_empty() {
        let names: SharedString = hidden.join(", ").into();
        line.push(
            div()
                .id(SharedString::from(format!("more-pills-{id}")))
                .flex_none()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(names.clone()).build(window, cx)
                })
                .child(crate::ui::chip().child(format!("+{}", hidden.len())))
                .into_any_element(),
        );
    }
    line
}

/// A pull request state as a small colored tag.
pub fn pull_request_tag(state: PullRequestState, cx: &App) -> AnyElement {
    let color = pull_request_color(state, cx);
    crate::ui::tinted_chip(color)
        .child(pull_request_label(state))
        .into_any_element()
}

/// Loading columns shaped like the board.
pub fn skeleton_board(cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .size_full()
        .items_start()
        .gap_3()
        .px_5()
        .py_3()
        .children((0..4).map(|column| {
            v_flex()
                .w(dp(COLUMN_WIDTH))
                .flex_none()
                .gap_2()
                .child(Skeleton::new().w(dp(96.)).h(dp(12.)))
                .children((0..(3 - column % 2)).map(|_| {
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .child(Skeleton::new().w(dp(56.)).h(dp(10.)))
                        .child(Skeleton::new().h(dp(12.)))
                        .child(Skeleton::new().w(dp(120.)).h(dp(10.)))
                }))
        }))
        .into_any_element()
}

/// A failed load as an inline error banner with Retry, inset like content.
pub fn load_error(
    message: SharedString,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .w_full()
        .px(scale::content_inset())
        .py_4()
        .child(ui::error_banner(message, on_retry))
        .into_any_element()
}

/// A Linear screen without a key: its page header and "Connect Linear" with
/// the one action that fixes it.
pub fn not_connected(
    icon: impl Into<gpui_kit::assets::IconName>,
    title: impl Into<SharedString>,
) -> AnyElement {
    let icon: gpui_kit::assets::IconName = icon.into();
    v_flex()
        .size_full()
        .child(ui::page_header(title).icon(icon))
        .child(
            ui::empty_state(
                IconName::CircleUser.into(),
                "Connect Linear",
                "Add a Linear personal API key in Settings to see your issues.",
            )
            .action(
                Button::new("linear-open-settings")
                    .small()
                    .primary()
                    .icon(IconName::Settings)
                    .label("Open Settings…")
                    .on_click(|_, window, cx| {
                        crate::shell::navigate(crate::route::Route::Settings, window, cx);
                    }),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{hex_color, issue_card, issue_row, pills_that_fit};
    use chrono::{DateTime, Utc};
    use gpui_kit::component::v_flex;
    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, Hsla, InputEvent as _, InteractiveElement as _,
        IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        WindowBounds, WindowOptions, px, rgb, size,
    };
    use loupe_github::{PrRef, RepoRef};
    use loupe_linear::views::DisplayProperty;
    use loupe_linear::{
        IssueSummary, Priority, PullRequestLink, PullRequestState, StateRef, StateType, TeamRef,
    };

    use crate::route::Route;
    use crate::services::{Accounts, Connection, Services};
    use crate::shell::{OpenOnClick as _, Workspace};
    use crate::store::Settings;

    fn issue(identifier: &str, pull_requests: Vec<PullRequestLink>) -> IssueSummary {
        let at = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("a valid time");
        IssueSummary {
            id: format!("id-{identifier}"),
            identifier: identifier.into(),
            title: format!("Issue {identifier}"),
            priority: Priority::None,
            sort_order: 0.,
            state: StateRef {
                id: "state".into(),
                name: "Todo".into(),
                kind: StateType::Unstarted,
                color: "#888888".into(),
            },
            team: TeamRef {
                id: "team".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: None,
            labels: Vec::new(),
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: format!("https://linear.app/acme/issue/{identifier}"),
            branch_name: String::new(),
            created_at: at,
            updated_at: at,
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests,
            pull_requests_complete: true,
        }
    }

    fn link(number: u64, state: PullRequestState) -> PullRequestLink {
        PullRequestLink {
            owner: "acme".into(),
            repo: "web".into(),
            number,
            state,
        }
    }

    fn pull_request(number: u64) -> Route {
        Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), number))
    }

    fn issue_route(identifier: &str) -> Route {
        Route::LinearIssue {
            identifier: identifier.into(),
        }
    }

    /// List rows and a board card that open their issue, as the screens draw
    /// them, in a window with a workspace to navigate.
    struct Issues {
        rows: Vec<IssueSummary>,
        card: IssueSummary,
        _workspace: Entity<Workspace>,
    }

    impl Render for Issues {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let now = Utc::now();
            let props = [DisplayProperty::Id, DisplayProperty::PullRequests];
            let route = |issue: &IssueSummary| issue_route(&issue.identifier);
            v_flex()
                .size_full()
                .children(self.rows.iter().map(|issue| {
                    let selector = format!("row-{}", issue.identifier);
                    issue_row(issue, now, &props, cx)
                        .debug_selector(move || selector)
                        .opens(route(issue))
                }))
                .child(
                    issue_card(&self.card, now, &props, cx)
                        .debug_selector(|| "card".into())
                        .opens(route(&self.card)),
                )
        }
    }

    /// The rows: ENG-1 has one pull request and ENG-2 has two. The card:
    /// ENG-3, with one.
    fn open_issues(cx: &mut TestAppContext) -> VisualTestContext {
        let rows = vec![
            issue("ENG-1", vec![link(7, PullRequestState::Open)]),
            issue(
                "ENG-2",
                vec![
                    link(8, PullRequestState::Merged),
                    link(9, PullRequestState::Draft),
                ],
            ),
        ];
        let card = issue("ENG-3", vec![link(10, PullRequestState::Open)]);
        let (handle, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            cx.set_global(Services::default());
            cx.set_global(Accounts {
                github: Connection::Connected(loupe_github::Viewer {
                    login: "octocat".into(),
                    name: None,
                    avatar_url: String::new(),
                }),
                ..Accounts::default()
            });
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(900.), px(600.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let workspace = cx.new(|cx| Workspace::new(Route::Inbox, window, cx));
                    cx.new(|_| Issues {
                        rows,
                        card,
                        _workspace: workspace,
                    })
                },
            )
            .expect("open the window")
        });
        VisualTestContext::from_window(handle, cx)
    }

    /// Clicks the middle of the element with `selector` the way the platform
    /// does, with a frame between each step so hit testing sees the pointer.
    fn click(
        cx: &mut VisualTestContext,
        selector: &'static str,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let position = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
            .center();
        let events = [
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }
            .to_platform_input(),
            MouseDownEvent {
                position,
                button,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            MouseUpEvent {
                position,
                button,
                modifiers,
                click_count: 1,
            }
            .to_platform_input(),
        ];
        for event in events {
            cx.update(|window, cx| {
                window.dispatch_event(event, cx);
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        cx.run_until_parked();
    }

    /// The page of each tab, and which tab is active.
    fn tabs(cx: &mut VisualTestContext) -> (Vec<Route>, usize) {
        cx.update(|_, cx| Settings::get(cx).single_pane_tabs())
    }

    #[gpui_kit::test]
    fn the_pull_request_chip_opens_the_pull_request_and_the_row_the_issue(cx: &mut TestAppContext) {
        let cx = &mut open_issues(cx);
        let none = Modifiers::none();

        click(cx, "pr-chip-id-ENG-1", MouseButton::Left, none);
        assert_eq!(
            tabs(cx),
            (vec![pull_request(7)], 0),
            "a click on the chip opens the pull request, not the issue"
        );

        click(cx, "row-ENG-1", MouseButton::Left, none);
        assert_eq!(
            tabs(cx),
            (vec![issue_route("ENG-1")], 0),
            "the rest of the row still opens the issue"
        );

        click(
            cx,
            "pr-chip-id-ENG-1",
            MouseButton::Left,
            Modifiers::command(),
        );
        click(cx, "pr-chip-id-ENG-1", MouseButton::Middle, none);
        assert_eq!(
            tabs(cx),
            (
                vec![issue_route("ENG-1"), pull_request(7), pull_request(7)],
                0
            ),
            "a Cmd-click and a middle click open the pull request in a new tab, and only it"
        );
    }

    #[gpui_kit::test]
    fn the_chip_of_several_pull_requests_opens_a_menu_of_them(cx: &mut TestAppContext) {
        let cx = &mut open_issues(cx);
        let start = tabs(cx);

        click(cx, "pr-chip-id-ENG-2", MouseButton::Left, Modifiers::none());
        assert_eq!(tabs(cx), start, "the chip opens its menu, not the issue");

        // The menu has the focus: Down goes to its first row, Enter picks it.
        cx.simulate_keystrokes("down enter");
        assert_eq!(
            tabs(cx),
            (vec![pull_request(8)], 0),
            "picking a row of the menu opens that pull request"
        );
    }

    #[gpui_kit::test]
    fn the_chip_on_a_board_card_opens_the_pull_request(cx: &mut TestAppContext) {
        let cx = &mut open_issues(cx);
        click(cx, "pr-chip-id-ENG-3", MouseButton::Left, Modifiers::none());
        assert_eq!(
            tabs(cx),
            (vec![pull_request(10)], 0),
            "the chip opens the pull request"
        );

        click(cx, "card", MouseButton::Left, Modifiers::none());
        assert_eq!(
            tabs(cx),
            (vec![issue_route("ENG-3")], 0),
            "the rest of the card still opens the issue"
        );
    }

    #[test]
    fn a_card_line_keeps_the_pills_that_fit_and_room_for_the_count() {
        // Everything fits: nothing is held back.
        assert_eq!(pills_that_fit(&[16., 60., 80.], 0), 3);
        assert_eq!(pills_that_fit(&[], 0), 0);
        // 16 + 60 + 90 + 150 with gaps is over the line: the last pill goes
        // behind "+1", which needs its own room.
        assert_eq!(pills_that_fit(&[16., 60., 90., 150.], 0), 3);
        // A pill wider than the line alone leaves only the count.
        assert_eq!(pills_that_fit(&[300., 40.], 0), 0);
    }

    #[test]
    fn a_card_line_never_hides_the_pills_it_must_keep() {
        // Priority, Blocked, and a pull request, then two categories that
        // do not fit: the categories give way, the first three stay.
        assert_eq!(pills_that_fit(&[16., 73., 60., 170., 170.], 3), 3);
        // Even when the line is short for them, the kept ones are shown.
        assert_eq!(pills_that_fit(&[16., 120., 120., 60.], 3), 3);
        // Keeping more than there are is keeping all of them.
        assert_eq!(pills_that_fit(&[16., 60.], 3), 2);
    }

    fn color(value: u32) -> Hsla {
        rgb(value).into()
    }

    #[test]
    fn parses_hex_with_and_without_hash() {
        assert_eq!(hex_color("#f2c94c"), Some(color(0xf2c94c)));
        assert_eq!(hex_color("f2c94c"), Some(color(0xf2c94c)));
        assert_eq!(hex_color("#F2C94C"), Some(color(0xf2c94c)));
        assert_eq!(hex_color("#000000"), Some(color(0x000000)));
    }

    #[test]
    fn rejects_anything_but_six_hex_digits() {
        for bad in [
            "", "#", "#fff", "#f2c94", "#f2c94c0", "#gggggg", "+2c94c", "##f2c94c", " f2c94c",
            "red", "#f2c94é",
        ] {
            assert_eq!(hex_color(bad), None, "{bad:?}");
        }
    }
}
