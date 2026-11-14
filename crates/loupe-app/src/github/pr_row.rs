//! The pull request list row shared by the inbox and repository lists.

use std::rc::Rc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::reviews::{self, Sort, SortDir, SortKey};
use loupe_github::{PrSummary, ReviewDecision};

use super::widgets;
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::shell::item_menu::{Item, with_item_menu};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

/// `child` with a hover tooltip naming what it shows.
pub fn hint(
    id: impl Into<ElementId>,
    text: &'static str,
    child: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .tooltip(move |window, cx| Tooltip::new(text).build(window, cx))
        .child(child)
}

/// Width of the repository lane. It shows the name, which is what differs
/// down a list; the tooltip has the owner and the whole reference.
const REPO_LANE: f32 = 150.;

/// Widths of the trailing lanes. From the repository on they are the same
/// in every section, so those columns hold their place down the list; the
/// author lane comes before them and exists only where authors differ.
const CHANGES_LANE: f32 = 90.;
const COMMENTS_LANE: f32 = 72.;
const CHECKS_LANE: f32 = 48.;
const AUTHOR_LANE: f32 = 48.;
const AGE_LANE: f32 = 40.;

/// A trailing lane `width` wide, its content at the trailing edge.
fn lane(width: f32) -> Div {
    h_flex().flex_none().w(dp(width)).justify_end()
}

/// What a click on a column heading does: sort by that column.
pub type OnSort = Rc<dyn Fn(SortKey, &mut Window, &mut App)>;

/// The 24 px heading over a list of [`pr_row`]s: a 12 px label over each
/// lane, starting on the rows' own leading edge. With `on_sort`, every lane
/// but the title's sorts the list when clicked, and the column `sort` names
/// shows its direction with an arrow. The age lane sorts by the date its
/// rows show: created when `sort` is by creation, else updated.
pub fn column_header(
    show_repo: bool,
    show_author: bool,
    sort: Option<Sort>,
    on_sort: Option<OnSort>,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    let age_key = match sort {
        Some(Sort {
            key: SortKey::Created,
            ..
        }) => SortKey::Created,
        _ => SortKey::Updated,
    };
    let heading = |key: SortKey, label: &'static str, width: f32, center: bool| {
        sort_heading(key, label, sort, on_sort.clone(), cx)
            .flex_none()
            .w(dp(width))
            .map(|lane| {
                if center {
                    lane.justify_center()
                } else {
                    lane.justify_end()
                }
            })
    };
    h_flex()
        .w_full()
        .h_6()
        .px_2()
        .gap_3()
        .items_center()
        .text_size(scale::meta())
        .text_color(muted)
        .child(div().flex_1().min_w_0().child("Pull request"))
        .when(show_author, |row| {
            row.child(heading(SortKey::Author, "Author", AUTHOR_LANE, true))
        })
        .when(show_repo, |row| {
            row.child(
                sort_heading(SortKey::Repository, "Repository", sort, on_sort.clone(), cx)
                    .flex_none()
                    .w(dp(REPO_LANE)),
            )
        })
        .child(heading(SortKey::Changes, "Changes", CHANGES_LANE, false))
        .child(heading(SortKey::Comments, "Comments", COMMENTS_LANE, false))
        .child(heading(SortKey::Checks, "Checks", CHECKS_LANE, true))
        .child(heading(age_key, "Age", AGE_LANE, false))
}

/// One column's label in [`column_header`]: a button that sorts by `key`
/// when `on_sort` is set, with an arrow while `sort` is by `key`.
fn sort_heading(
    key: SortKey,
    label: &'static str,
    sort: Option<Sort>,
    on_sort: Option<OnSort>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let active = sort.filter(|sort| sort.key == key);
    let id = SharedString::from(format!("reviews-sort-{}", key.label().to_lowercase()));
    let arrow = active.map(|sort| match sort.dir {
        SortDir::Ascending => IconName::ArrowUp,
        SortDir::Descending => IconName::ArrowDown,
    });
    let cell = h_flex()
        .id(id.clone())
        .h_full()
        .gap_0p5()
        .items_center()
        .when(active.is_some(), |cell| cell.text_color(theme.foreground))
        .children(arrow.map(|arrow| Icon::new(arrow).with_size(dp(12.))))
        .child(label);
    let Some(on_sort) = on_sort else {
        return cell;
    };
    // The name says what a click does next, so a screen reader hears the
    // order the list is in and the one it will get.
    let state = match active {
        Some(sort) => format!("{label}, sorted {}", sort.describe().to_lowercase()),
        None => label.to_owned(),
    };
    let tip = match reviews::cycle_sort(sort, key) {
        Some(next) => format!(
            "Sort by {}: {}",
            key.label().to_lowercase(),
            next.describe().to_lowercase()
        ),
        None => "Back to the default order".to_owned(),
    };
    let foreground = theme.foreground;
    cell.debug_selector(move || id.to_string())
        .role(Role::Button)
        .aria_label(format!("{state}. {tip}"))
        .hover(move |cell| cell.text_color(foreground))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, window, cx| on_sort(key, window, cx))
}

/// Which date a row's age lane counts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgeFrom {
    Updated,
    Created,
}

impl AgeFrom {
    /// The date the rows show under `sort`: created while it sorts by it.
    pub fn for_sort(sort: Option<Sort>) -> Self {
        match sort {
            Some(Sort {
                key: SortKey::Created,
                ..
            }) => AgeFrom::Created,
            _ => AgeFrom::Updated,
        }
    }
}

/// What a lane shows when the row has no value for it: a faint dash, so an
/// empty cell reads as "none" and not as missing.
fn none(cx: &App) -> Div {
    div()
        .text_size(scale::meta())
        .text_color(cx.theme().muted_foreground.opacity(0.4))
        .child("–")
}

/// The review decision as a chip. A pull request whose repository asks for
/// no review has no decision and shows no chip.
fn review_chip(decision: ReviewDecision) -> Tag {
    match decision {
        ReviewDecision::Approved => ui::tinted_chip(tokens::open()).child("Approved"),
        ReviewDecision::ChangesRequested => {
            ui::tinted_chip(tokens::closed()).child("Changes requested")
        }
        ReviewDecision::ReviewRequired => ui::chip().child("Review required"),
    }
}

/// One 36 px row: state icon, `#number`, the title with its review decision
/// right after it, then fixed lanes on the right for the author (when
/// `show_author`), the repository (when `show_repo`), diff size, comments,
/// checks, and age (counted from `age_from`). Fixed lanes keep every column
/// on one spine down the list. `selected` marks the row under the keyboard
/// cursor. Clicking it opens `Route::PullRequest`.
pub fn pr_row(
    summary: &PrSummary,
    show_repo: bool,
    show_author: bool,
    selected: bool,
    age_from: AgeFrom,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let hover = theme.list_hover;
    let number = format!("#{}", summary.pr.number);
    let reference: SharedString = summary.pr.to_string().into();
    let repo = summary.pr.repo.name.clone();
    let route = Route::PullRequest(summary.pr.clone());
    let at = match age_from {
        AgeFrom::Updated => summary.updated_at,
        AgeFrom::Created => summary.created_at,
    };
    let age = ui::relative_time(at, Utc::now());
    let (login, avatar_url) = summary.author.as_ref().map_or_else(
        || ("ghost".to_owned(), String::new()),
        |author| (author.login.clone(), author.avatar_url.clone()),
    );
    let id = summary.id.clone();
    // The accessible name: what the row shows, so assistive technology and UI
    // agents can find a pull request by number, title, or repository.
    let name = format!("{reference} {}", summary.title);
    let item = Item::pull_request(summary);
    let row = h_flex()
        .id(SharedString::from(format!("pr-row-{id}")))
        .role(Role::Link)
        .aria_label(name)
        .w_full()
        .h(scale::row_height())
        .px_2()
        .gap_3()
        .items_center()
        .relative()
        .rounded(theme.radius)
        .hover(move |row| row.bg(hover))
        // The keyboard cursor: a fill and the accent on the leading edge.
        .when(selected, |row| {
            row.bg(theme.list_active).child(
                div()
                    .absolute()
                    .left_0()
                    .top_1p5()
                    .bottom_1p5()
                    .w(px(2.))
                    .rounded(px(1.))
                    .bg(tokens::accent()),
            )
        })
        // A hairline between rows, so a wide row reads across its columns.
        // It starts and ends on the content's edge, under the icons, while
        // the hover fill reaches 8 px past it.
        .child(
            div()
                .absolute()
                .left_2()
                .right_2()
                .bottom_0()
                .h(px(1.))
                .bg(tokens::border_subtle()),
        )
        .opens(route)
        .child(hint(
            SharedString::from(format!("pr-state-{id}")),
            widgets::state_label(summary.state, summary.is_draft),
            widgets::state_icon(summary.state, summary.is_draft),
        ))
        // The number ends 12 px before the title on every row, however
        // many digits it has.
        .child(
            h_flex()
                .flex_none()
                .w_10()
                .justify_end()
                .text_size(scale::meta())
                .text_color(muted)
                .child(number),
        )
        // The title, and right after it the review decision when the pull
        // request has one: the chip takes room from a long title only on
        // its own row, and no row carries an empty lane for it.
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .child(summary.title.clone()),
                )
                .children(summary.review_decision.map(review_chip)),
        )
        // The author, before the columns every section shares, and only
        // where authors differ: on the viewer's own pull requests its width
        // goes to the title.
        .when(show_author, |row| {
            row.child(lane(AUTHOR_LANE).justify_center().child(ui::avatar_named(
                avatar_url,
                login,
                dp(20.),
            )))
        })
        // The lane shows the repository's name; the tooltip keeps the full
        // reference so repositories with one name stay distinguishable.
        .when(show_repo, |row| {
            row.child(
                div()
                    .id(SharedString::from(format!("pr-repo-{id}")))
                    .flex_none()
                    .w(dp(REPO_LANE))
                    .truncate()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .tooltip(move |window, cx| Tooltip::new(reference.clone()).build(window, cx))
                    .child(repo),
            )
        })
        .child(lane(CHANGES_LANE).child(widgets::diff_stat(summary.additions, summary.deletions)))
        .child(
            lane(COMMENTS_LANE)
                .gap_1()
                .text_size(scale::meta())
                .text_color(muted)
                .when(summary.comment_count > 0, |lane| {
                    lane.child(Icon::new(IconName::MessageSquare).xsmall())
                        .child(summary.comment_count.to_string())
                })
                .when(summary.comment_count == 0, |lane| lane.child(none(cx))),
        )
        // The checks: one state icon, with its name in a tooltip.
        .child(
            lane(CHECKS_LANE)
                .justify_center()
                .when(summary.checks.is_none(), |lane| lane.child(none(cx)))
                .when_some(summary.checks, |lane, checks| {
                    lane.child(hint(
                        SharedString::from(format!("pr-checks-{id}")),
                        widgets::checks_label(checks),
                        Icon::new(widgets::checks_icon_name(checks))
                            .with_size(dp(16.))
                            .text_color(widgets::checks_color(checks)),
                    ))
                }),
        )
        .child(
            lane(AGE_LANE)
                .text_size(scale::meta())
                .text_color(muted)
                .child(age),
        );
    with_item_menu(row, item)
}
