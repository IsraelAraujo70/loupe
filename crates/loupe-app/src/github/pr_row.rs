//! The pull request list row shared by the inbox and repository lists.

use std::rc::Rc;

use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::queue::{QueueGroup, SizeClass};
use loupe_github::reviews::{self, Sort, SortDir, SortKey};
use loupe_github::{ChecksState, PrRef, PrState, PrSummary, ReviewDecision, ReviewStanding};

use super::live::Change;
use super::widgets;
use crate::route::Route;
use crate::shell::item_menu::{Item, with_item_menu};
use crate::shell::{OpenOnClick as _, open};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui::{self, ListRowFocus as _};

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
/// author lane (avatar and login) comes before them and exists only where
/// authors differ.
const CHANGES_LANE: f32 = 90.;
const COMMENTS_LANE: f32 = 72.;
const CHECKS_LANE: f32 = 48.;
const AUTHOR_LANE: f32 = 140.;
const AGE_LANE: f32 = 40.;
/// The checkbox lane at the leading edge of a list whose rows can be picked.
const CHECKBOX_LANE: f32 = 16.;

/// What picking a row asks for: its checkbox toggles it, a Shift+click
/// extends the selection to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectGesture {
    Toggle,
    Range,
}

/// What picking a row does.
pub type OnSelect = Rc<dyn Fn(&PrRef, SelectGesture, &mut Window, &mut App)>;

/// A row that can be picked: whether it is, whether the list has anything
/// picked (then every box shows, not only the hovered row's), and what a
/// pick does.
pub struct RowSelect {
    pub checked: bool,
    pub showing: bool,
    pub on_select: OnSelect,
}

/// The 16 px checkbox lane of a row or a section heading inside the hover
/// group `group`. The box is transparent until the group is hovered, unless
/// `showing` or `checked`; it stays clickable and in the accessibility tree.
pub fn select_box(
    id: SharedString,
    group: &'static str,
    checked: bool,
    showing: bool,
    label: impl Into<SharedString>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let selector = id.clone();
    h_flex()
        .debug_selector(move || selector.to_string())
        .flex_none()
        .w(dp(CHECKBOX_LANE))
        .justify_center()
        .when(!showing && !checked, |lane| {
            lane.opacity(0.)
                .group_hover(group, |style| style.opacity(1.))
        })
        .child(
            Checkbox::new(id)
                .checked(checked)
                .accessibility_label(label)
                .on_click(move |_, window, cx| {
                    // The box sits inside a row that opens its pull request
                    // on a click; picking must not open it too.
                    cx.stop_propagation();
                    on_click(window, cx)
                }),
        )
}

/// A trailing lane `width` wide, its content at the trailing edge.
fn lane(width: f32) -> Div {
    h_flex().flex_none().w(dp(width)).justify_end()
}

/// The hover group of a row, which shows its checkbox.
pub const ROW_GROUP: &str = "pr-row";

/// What a click on a column heading does: sort by that column.
pub type OnSort = Rc<dyn Fn(SortKey, &mut Window, &mut App)>;

/// The 24 px heading over a list of [`pr_row`]s: a 12 px label over each
/// lane `lanes` shows, aligned to the rows' own leading edge (after the
/// 16 px checkbox lane when `checkbox_lane` is true). With `on_sort`, every
/// lane but the title's sorts the list when clicked, and the column `sort`
/// names shows its direction with an arrow. The age lane sorts by the date
/// its rows show: created when `sort` is by creation, else updated.
pub fn column_header(
    checkbox_lane: bool,
    lanes: Lanes,
    sort: Option<Sort>,
    on_sort: Option<OnSort>,
    cx: &App,
) -> Div {
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
    ui::list_column_header(cx)
        .when(checkbox_lane, |row| {
            row.child(div().flex_none().w(dp(CHECKBOX_LANE)))
        })
        .child(div().flex_1().min_w_0().child("Pull request"))
        .when(lanes.author, |row| {
            row.child(
                sort_heading(SortKey::Author, "Author", sort, on_sort.clone(), cx)
                    .flex_none()
                    .w(dp(AUTHOR_LANE)),
            )
        })
        .when(lanes.repo, |row| {
            row.child(
                sort_heading(SortKey::Repository, "Repository", sort, on_sort.clone(), cx)
                    .flex_none()
                    .w(dp(REPO_LANE)),
            )
        })
        .child(heading(SortKey::Changes, "Size", CHANGES_LANE, false))
        .when(lanes.comments, |row| {
            row.child(heading(SortKey::Comments, "Comments", COMMENTS_LANE, false))
        })
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
        .children(arrow.map(|arrow| Icon::new(arrow).with_size(scale::icon_xs())))
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

/// The lanes a list's rows show besides those every list has: the
/// repository, the author, the comments, and which date the age counts
/// from. A list drops the repository when every row shares one, and the
/// comments when no row has any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lanes {
    pub repo: bool,
    pub author: bool,
    pub comments: bool,
    pub age_from: AgeFrom,
    /// The queue group the rows sit under, whose heading already says
    /// their state: a row in it shows no state chip.
    pub group: Option<QueueGroup>,
}

impl Lanes {
    /// The lanes `rows` need: the repository only when they come from more
    /// than one, the comments only when one of them has any.
    pub fn for_rows(rows: &[&PrSummary], author: bool, age_from: AgeFrom) -> Self {
        let repo = rows
            .first()
            .is_some_and(|first| rows.iter().any(|row| row.pr.repo != first.pr.repo));
        Lanes {
            repo,
            author,
            comments: rows.iter().any(|row| row.comment_count > 0),
            age_from,
            group: None,
        }
    }
}

/// The author lane: the avatar, then the login, cut at the lane's end.
fn author_lane(avatar_url: impl Into<SharedString>, login: SharedString, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .w(dp(AUTHOR_LANE))
        .gap_2()
        .items_center()
        .child(ui::avatar_named(
            avatar_url,
            login.clone(),
            scale::avatar_md(),
        ))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(scale::meta())
                .text_color(cx.theme().muted_foreground)
                .child(login),
        )
}

/// A row inside an open stack: its place from the stack's base, and whether
/// it is the last row shown, where the line linking the stack ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackPlace {
    pub position: usize,
    pub total: usize,
    pub last: bool,
}

/// What a lane shows when the row has no value for it: a faint dash, so an
/// empty cell reads as "none" and not as missing.
fn none(cx: &App) -> Div {
    div()
        .text_size(scale::meta())
        .text_color(cx.theme().muted_foreground.opacity(0.55))
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
        ReviewDecision::ReviewRequired => {
            ui::tinted_chip(tokens::pending()).child("Review required")
        }
    }
}

/// What an open pull request asks of the viewer, as a label and its icon:
/// a draft, a review asked of them, commits since their review, or their
/// review on the head. Their own pull requests, those they have no part
/// in, and closed ones have none.
pub fn viewer_state(summary: &PrSummary) -> Option<(&'static str, IconName)> {
    if summary.state != PrState::Open {
        return None;
    }
    if summary.is_draft {
        return Some(("Draft", IconName::GitPullRequestDraft));
    }
    match summary.viewer_review? {
        ReviewStanding::Requested => Some(("Review requested", IconName::Eye)),
        ReviewStanding::ChangesSinceReview => {
            Some(("Changes since your review", IconName::GitCommitHorizontal))
        }
        ReviewStanding::Reviewed => Some(("Reviewed", IconName::Check)),
        ReviewStanding::Authored => None,
    }
}

/// [`viewer_state`] as a chip with its icon, in the color of what it asks:
/// pending for a review asked, the accent for commits not seen yet (as the
/// unread dot), the draft color for a draft, and neutral once reviewed.
fn viewer_chip(summary: &PrSummary) -> Option<Tag> {
    let (label, icon) = viewer_state(summary)?;
    let chip = match summary.viewer_review {
        _ if summary.is_draft => ui::tinted_chip(tokens::draft()),
        Some(ReviewStanding::Requested) => ui::tinted_chip(tokens::pending()),
        Some(ReviewStanding::ChangesSinceReview) => ui::tinted_chip(tokens::accent()),
        _ => ui::chip(),
    };
    Some(
        chip.gap_1()
            .child(Icon::new(icon).with_size(scale::icon_xs()))
            .child(label),
    )
}

/// `n` with a comma between thousands, as the tooltips write counts.
fn grouped(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// What the size lane says to a tooltip and a screen reader: the class, the
/// lines it covers, and the lines added and removed.
pub fn size_description(additions: u32, deletions: u32) -> String {
    let class = SizeClass::of(additions, deletions);
    format!(
        "Size {}, {} · +{} −{}",
        class.label(),
        class.range(),
        grouped(additions),
        grouped(deletions)
    )
}

/// The size class of a stack's rows added up, as its row's name says it.
fn size_description_short(rows: &[&PrSummary]) -> String {
    let (additions, deletions) = rows.iter().fold((0u32, 0u32), |sum, row| {
        (
            sum.0.saturating_add(row.additions),
            sum.1.saturating_add(row.deletions),
        )
    });
    format!("size {}", SizeClass::of(additions, deletions).label())
}

/// The size lane: a meter of four bars, one per class, filled up to the
/// pull request's, then the class. The lines added and removed are in its
/// tooltip.
fn size_lane(id: SharedString, additions: u32, deletions: u32, cx: &App) -> Div {
    let theme = cx.theme();
    let class = SizeClass::of(additions, deletions);
    let tip = size_description(additions, deletions);
    let selector = id.clone();
    lane(CHANGES_LANE).child(
        h_flex()
            .id(id)
            .debug_selector(move || selector.to_string())
            .gap_1p5()
            .items_center()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(
                h_flex()
                    .h_2p5()
                    .gap_0p5()
                    .items_end()
                    .children(SizeClass::ALL.iter().map(|step| {
                        div()
                            .w(px(2.))
                            .rounded_t(px(1.))
                            .map(|bar| match step {
                                SizeClass::S => bar.h_1(),
                                SizeClass::M => bar.h_1p5(),
                                SizeClass::L => bar.h_2(),
                                SizeClass::Xl => bar.h_2p5(),
                            })
                            .bg(if *step <= class {
                                theme.muted_foreground
                            } else {
                                theme.border
                            })
                    })),
            )
            .child(
                div()
                    .w_5()
                    .text_size(scale::meta())
                    .text_color(if class == SizeClass::Xl {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(class.label()),
            ),
    )
}

/// One 32 px row: state icon, `#number`, the title with its review decision
/// right after it, then fixed lanes on the right for the author and the
/// repository (as `lanes` asks), diff size, comments, checks, and age.
/// Fixed lanes keep every column on one spine down the list. `focused`
/// marks the row under the keyboard cursor while the list has the focus. Clicking it opens
/// `Route::PullRequest`. With `select`, a checkbox lane leads the row and a
/// Shift+click picks instead of opening. With `place`, the row sits inside
/// an open stack: indented past the stack's chevron, on the line linking the
/// stack, with its place after the title. With `marked`, a dot in the
/// leading edge says the pull request came or changed in the last refresh.
pub fn pr_row(
    summary: &PrSummary,
    lanes: Lanes,
    focused: bool,
    select: Option<RowSelect>,
    place: Option<StackPlace>,
    marked: Option<Change>,
    cx: &App,
) -> impl IntoElement + use<> {
    let Lanes {
        repo: show_repo,
        author: show_author,
        comments: show_comments,
        age_from,
        group: _,
    } = lanes;
    let theme = cx.theme();
    let muted = theme.muted_foreground;
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
    // agents can find a pull request by number, title, repository, or author.
    let by = if show_author {
        format!(", by {login}")
    } else {
        String::new()
    };
    let state = viewer_state(summary)
        .map(|(label, _)| format!(", {}", label.to_lowercase()))
        .unwrap_or_default();
    let size = format!(
        ", size {}",
        SizeClass::of(summary.additions, summary.deletions).label()
    );
    let name = match marked {
        Some(Change::Entered) => format!("{reference} {}{by}{state}{size}, new", summary.title),
        Some(Change::Updated) => {
            format!("{reference} {}{by}{state}{size}, updated", summary.title)
        }
        None => format!("{reference} {}{by}{state}{size}", summary.title),
    };
    let has_state = viewer_state(summary).is_some();
    // Under a heading that already says the state, the chip would only
    // take the title's room.
    let viewer_chip = viewer_chip(summary).filter(|_| lanes.group != Some(QueueGroup::of(summary)));
    // With a state in front, "Review required" says nothing more.
    let decision = summary
        .review_decision
        .filter(|decision| !has_state || *decision != ReviewDecision::ReviewRequired);
    let item = Item::pull_request(summary);
    let pr = summary.pr.clone();
    let row = ui::list_row(SharedString::from(format!("pr-row-{id}")), cx)
        .debug_selector({
            let id = id.clone();
            move || format!("pr-row-{id}")
        })
        .role(Role::Link)
        .aria_label(name)
        .relative()
        .group(ROW_GROUP)
        .keyboard_focus(focused, cx)
        // Came or changed in the last refresh: the unread dot in the
        // leading edge.
        .when(marked.is_some(), |row| {
            row.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("pr-mark-{id}")
                            })
                            .size_1p5()
                            .rounded_full()
                            .bg(tokens::accent()),
                    ),
            )
        })
        // A hairline between rows, so a wide row reads across its columns.
        // It starts and ends on the content's edge, under the icons, while
        // the hover fill reaches 8 px past it.
        .child(ui::list_row_divider(cx))
        .map(|row| match &select {
            Some(select) => {
                let (on_select, pr) = (select.on_select.clone(), pr.clone());
                row.on_open(move |event, window, cx| {
                    if !event.is_middle_click() && event.modifiers().shift {
                        on_select(&pr, SelectGesture::Range, window, cx);
                    } else {
                        open(route.clone(), event, window, cx);
                    }
                })
            }
            None => row.opens(route),
        })
        .when_some(select, |row, select| {
            let RowSelect {
                checked,
                showing,
                on_select,
            } = select;
            let label = format!("Select {}", summary.pr);
            row.child(select_box(
                SharedString::from(format!("pr-select-{id}")),
                ROW_GROUP,
                checked,
                showing,
                label,
                move |window, cx| {
                    let gesture = if window.modifiers().shift {
                        SelectGesture::Range
                    } else {
                        SelectGesture::Toggle
                    };
                    on_select(&pr, gesture, window, cx)
                },
            ))
        })
        .when_some(place, |row, place| row.child(stack_guide(place.last, cx)))
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
        // request has one: no row carries an empty lane for it.
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .child(summary.title.clone()),
                )
                .children(place.map(|place| {
                    div()
                        .flex_none()
                        .ml_2()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(format!("{}/{}", place.position, place.total))
                }))
                // The chips only take the room the title leaves. One that
                // does not fit whole wraps onto a second line that is cut
                // off, so it is shown whole or not at all; the empty first
                // item keeps the first line from holding a chip that does
                // not fit.
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h(scale::chip_height())
                        .flex_wrap()
                        .overflow_hidden()
                        .child(div().h_full())
                        .children(viewer_chip.map(|chip| chip.ml_2()))
                        .children(decision.map(|decision| review_chip(decision).ml_2())),
                ),
        )
        // The author, before the columns every section shares, and only
        // where authors differ: on the viewer's own pull requests its width
        // goes to the title.
        .when(show_author, |row| {
            row.child(author_lane(avatar_url, login.into(), cx))
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
        .child(size_lane(
            SharedString::from(format!("pr-size-{id}")),
            summary.additions,
            summary.deletions,
            cx,
        ))
        .when(show_comments, |row| {
            row.child(
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
        })
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
                            .with_size(scale::icon_md())
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

/// The 16 px lane under an open stack's chevron that holds the line linking
/// its rows; on the last row the line stops at the row's middle.
fn stack_guide(last: bool, cx: &App) -> Div {
    let line = cx.theme().border;
    div().flex_none().w(dp(16.)).h_full().relative().child(
        div()
            .absolute()
            .top_0()
            .left(dp(7.5))
            .w(px(1.))
            .bg(line)
            .map(|guide| {
                if last {
                    guide.h_1_2()
                } else {
                    guide.bottom_0()
                }
            }),
    )
}

/// A stack as one row of the list.
pub struct StackRow<'a> {
    /// Stable across renders, so hover and the accessibility tree hold.
    pub id: SharedString,
    pub name: &'a str,
    /// The stack's rows the list shows, base first.
    pub rows: &'a [&'a PrSummary],
    /// The stack's open pull requests, shown or not.
    pub total: usize,
    pub expanded: bool,
}

/// What a stack's row says after its name: how many pull requests it holds
/// (or how many of them the filters leave), then how many fail their checks,
/// or if none does, how many still run them.
pub fn stack_summary(rows: &[&PrSummary], total: usize) -> String {
    let shown = rows.len();
    let mut text = if shown < total {
        format!("{shown} of {total} shown")
    } else {
        format!("{shown} PRs")
    };
    if let Some(checks) = stack_checks(rows) {
        text.push_str(" · ");
        text.push_str(&checks);
    }
    text
}

/// How many of a stack's rows fail their checks, or if none does, how many
/// still run them.
fn stack_checks(rows: &[&PrSummary]) -> Option<String> {
    let count = |state: ChecksState| {
        rows.iter()
            .filter(|summary| summary.checks == Some(state))
            .count()
    };
    let (failing, running) = (count(ChecksState::Failing), count(ChecksState::Pending));
    match (failing, running) {
        (0, 0) => None,
        (0, 1) => Some("1 check running".to_owned()),
        (0, running) => Some(format!("{running} checks running")),
        (1, _) => Some("1 check failing".to_owned()),
        (failing, _) => Some(format!("{failing} checks failing")),
    }
}

/// The worst of the rows' checks: failing over running over passing.
fn worst_checks(rows: &[&PrSummary]) -> Option<ChecksState> {
    let has = |state: ChecksState| rows.iter().any(|summary| summary.checks == Some(state));
    [
        ChecksState::Failing,
        ChecksState::Pending,
        ChecksState::Passing,
    ]
    .into_iter()
    .find(|&state| has(state))
}

/// A stack's 32 px row on the rows' grid: `lead` (the box that picks the
/// stack's rows) or an empty lane, a chevron over the state icons (down
/// open, right folded), a stack mark in the number lane, its name and
/// [`stack_summary`], then the lanes added up over its shown rows: the
/// author when they share one, the repository, the changes and comments
/// summed, the worst checks, and the latest age. The caller makes a click
/// open or fold it.
pub fn stack_row(
    stack: StackRow<'_>,
    lanes: Lanes,
    focused: bool,
    lead: Option<Div>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let rows = stack.rows;
    let summary = stack_summary(rows, stack.total);
    let name = format!(
        "Stack {}, {summary}, {}",
        stack.name,
        size_description_short(rows)
    );
    // The chip names the line as a stack and how many it holds, in merge
    // order under it; the name keeps the base's title.
    let count = if rows.len() < stack.total {
        format!("Stack · {} of {}", rows.len(), stack.total)
    } else {
        format!("Stack · {}", stack.total)
    };
    let (additions, deletions, comments) = rows.iter().fold((0, 0, 0), |sum, row| {
        (
            sum.0 + row.additions,
            sum.1 + row.deletions,
            sum.2 + row.comment_count,
        )
    });
    let latest = rows
        .iter()
        .map(|row| match lanes.age_from {
            AgeFrom::Updated => row.updated_at,
            AgeFrom::Created => row.created_at,
        })
        .max();
    let author = rows.first().and_then(|first| {
        let login = first.author.as_ref().map(|author| &author.login);
        rows.iter()
            .all(|row| row.author.as_ref().map(|author| &author.login) == login)
            .then(|| first.author.clone())
            .flatten()
    });
    let repo = rows.first().map(|row| row.pr.repo.clone());
    let checks = worst_checks(rows);
    let id = stack.id.clone();
    ui::list_row(stack.id.clone(), cx)
        .debug_selector(move || id.to_string())
        .role(Role::Button)
        .aria_label(name)
        .aria_expanded(stack.expanded)
        .relative()
        .group(ROW_GROUP)
        .keyboard_focus(focused, cx)
        .child(
            div()
                .absolute()
                .left_2()
                .right_2()
                .bottom_0()
                .h(px(1.))
                .bg(tokens::border_subtle()),
        )
        .children(lead)
        .child(
            Icon::new(if stack.expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .with_size(dp(16.))
            .text_color(muted),
        )
        .child(
            h_flex()
                .flex_none()
                .w_10()
                .justify_end()
                .text_color(muted)
                .child(Icon::new(IconName::Layers).with_size(dp(16.))),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(ui::chip().child(count))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(scale::body())
                        .child(stack.name.to_owned()),
                )
                .children(stack_checks(rows).map(|checks| {
                    div()
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(checks)
                })),
        )
        .when(lanes.author, |row| {
            row.child(match author {
                Some(author) => author_lane(author.avatar_url, author.login.into(), cx),
                None => h_flex().flex_none().w(dp(AUTHOR_LANE)).child(none(cx)),
            })
        })
        .when(lanes.repo, |row| {
            row.child(
                div()
                    .flex_none()
                    .w(dp(REPO_LANE))
                    .truncate()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .children(repo.map(|repo| repo.name)),
            )
        })
        .child(size_lane(
            SharedString::from(format!("{}-size", stack.id)),
            additions,
            deletions,
            cx,
        ))
        .when(lanes.comments, |row| {
            row.child(
                lane(COMMENTS_LANE)
                    .gap_1()
                    .text_size(scale::meta())
                    .text_color(muted)
                    .when(comments > 0, |lane| {
                        lane.child(Icon::new(IconName::MessageSquare).xsmall())
                            .child(comments.to_string())
                    })
                    .when(comments == 0, |lane| lane.child(none(cx))),
            )
        })
        .child(
            lane(CHECKS_LANE)
                .justify_center()
                .when(checks.is_none(), |lane| lane.child(none(cx)))
                .when_some(checks, |lane, checks| {
                    lane.child(
                        Icon::new(widgets::checks_icon_name(checks))
                            .with_size(scale::icon_md())
                            .text_color(widgets::checks_color(checks)),
                    )
                }),
        )
        .child(
            lane(AGE_LANE)
                .text_size(scale::meta())
                .text_color(muted)
                .children(latest.map(|at| ui::relative_time(at, Utc::now()))),
        )
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use chrono::Utc;
    use gpui_kit::{
        App, AppContext as _, Context, InteractiveElement as _, IntoElement, Modifiers,
        ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use loupe_github::{PrRef, PrState, PrSummary, RepoRef};

    use super::{AgeFrom, Lanes, RowSelect, SelectGesture, pr_row};

    pub(in crate::github) fn summary(repo: &str, number: u64, comments: u32) -> PrSummary {
        let at = Utc::now();
        PrSummary {
            id: format!("PR_{number}"),
            pr: PrRef::new(RepoRef::new("a", repo), number),
            title: String::new(),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            author: None,
            created_at: at,
            updated_at: at,
            comment_count: comments,
            additions: 0,
            deletions: 0,
            checks: None,
            review_decision: None,
            head_ref: String::new(),
            base_ref: String::new(),
            cross_repository: false,
            stack: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: loupe_github::Mergeable::Unknown,
            review_requests: Vec::new(),
            reviewed_by: Vec::new(),
            viewer_review: None,
        }
    }

    /// A lane that would say the same on every row is dropped: the
    /// repository when all rows share one, the comments when none has any.
    #[test]
    fn lanes_drop_what_every_row_shares() {
        let lanes = |rows: &[PrSummary]| {
            let rows: Vec<&PrSummary> = rows.iter().collect();
            let lanes = Lanes::for_rows(&rows, true, AgeFrom::Updated);
            (lanes.repo, lanes.comments)
        };
        assert_eq!(
            lanes(&[summary("x", 1, 0), summary("x", 2, 0)]),
            (false, false)
        );
        assert_eq!(
            lanes(&[summary("x", 1, 0), summary("y", 2, 0)]),
            (true, false)
        );
        assert_eq!(
            lanes(&[summary("x", 1, 0), summary("x", 2, 3)]),
            (false, true)
        );
        assert_eq!(lanes(&[]), (false, false));
    }
    use crate::store::Settings;

    type Picks = Rc<RefCell<Vec<SelectGesture>>>;

    /// One pickable row inside a parent that counts the clicks reaching it.
    struct Harness {
        picks: Picks,
        parent_clicks: Rc<RefCell<usize>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let at = Utc::now();
            let summary = PrSummary {
                id: "PR_1".into(),
                pr: PrRef::new(RepoRef::new("a", "repo"), 1),
                title: "A title".into(),
                url: String::new(),
                state: PrState::Open,
                is_draft: false,
                author: None,
                created_at: at,
                updated_at: at,
                comment_count: 0,
                additions: 0,
                deletions: 0,
                checks: None,
                review_decision: None,
                head_ref: String::new(),
                base_ref: String::new(),
                cross_repository: false,
                stack: None,
                labels: Vec::new(),
                assignees: Vec::new(),
                reviewers: Vec::new(),
                mergeable: loupe_github::Mergeable::Unknown,
                review_requests: Vec::new(),
                reviewed_by: Vec::new(),
                viewer_review: None,
            };
            let picks = self.picks.clone();
            let parent_clicks = self.parent_clicks.clone();
            div()
                .id("parent")
                .w(px(1000.))
                .on_click(move |_, _, _| *parent_clicks.borrow_mut() += 1)
                .child(pr_row(
                    &summary,
                    Lanes {
                        repo: true,
                        author: true,
                        comments: true,
                        age_from: AgeFrom::Updated,
                        group: None,
                    },
                    false,
                    Some(RowSelect {
                        checked: false,
                        showing: true,
                        on_select: Rc::new(move |_, gesture, _, _| {
                            picks.borrow_mut().push(gesture)
                        }),
                    }),
                    None,
                    None,
                    cx,
                ))
        }
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str, modifiers: Modifiers) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let at = cx.debug_bounds(selector).expect(selector);
        cx.simulate_click(at.center(), modifiers);
    }

    /// The checkbox picks without the click reaching the row, which would
    /// open the pull request; a Shift+click on the row picks a range.
    #[gpui_kit::test]
    fn the_checkbox_picks_without_opening_the_row(cx: &mut TestAppContext) {
        let picks: Picks = Rc::default();
        let parent_clicks = Rc::new(RefCell::new(0));
        let (handle, _) = cx.update(|cx: &mut App| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            let (picks, parent_clicks) = (picks.clone(), parent_clicks.clone());
            gpui_kit::open_window(Default::default(), cx, move |_, cx| {
                cx.new(|_| Harness {
                    picks,
                    parent_clicks,
                })
            })
            .expect("open a window")
        });
        let cx = &mut VisualTestContext::from_window(handle, cx);

        click(cx, "pr-select-PR_1", Modifiers::none());
        assert_eq!(*picks.borrow(), [SelectGesture::Toggle]);
        assert_eq!(*parent_clicks.borrow(), 0, "the click stopped at the box");
        click(cx, "pr-select-PR_1", Modifiers::shift());
        assert_eq!(picks.borrow().last(), Some(&SelectGesture::Range));
        assert_eq!(*parent_clicks.borrow(), 0);

        // A click on the row itself goes on to it.
        click(cx, "pr-row-PR_1", Modifiers::shift());
        assert_eq!(picks.borrow().len(), 3);
        assert_eq!(picks.borrow().last(), Some(&SelectGesture::Range));
        assert_eq!(*parent_clicks.borrow(), 1);
    }
}
