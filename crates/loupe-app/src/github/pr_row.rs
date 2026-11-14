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
use loupe_github::reviews::{self, Sort, SortDir, SortKey};
use loupe_github::{PrRef, PrSummary, ReviewDecision};

use super::widgets;
use crate::route::Route;
use crate::shell::item_menu::{Item, with_item_menu};
use crate::shell::{OpenOnClick as _, open};
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
const ROW_GROUP: &str = "pr-row";

/// What a click on a column heading does: sort by that column.
pub type OnSort = Rc<dyn Fn(SortKey, &mut Window, &mut App)>;

/// The 24 px heading over a list of [`pr_row`]s: a 12 px label over each
/// lane, starting on the rows' own leading edge. With `on_sort`, every lane
/// but the title's sorts the list when clicked, and the column `sort` names
/// shows its direction with an arrow. The age lane sorts by the date its
/// rows show: created when `sort` is by creation, else updated.
pub fn column_header(
    checkbox_lane: bool,
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
        .when(checkbox_lane, |row| {
            row.child(div().flex_none().w(dp(CHECKBOX_LANE)))
        })
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
/// cursor. Clicking it opens `Route::PullRequest`. With `select`, a checkbox
/// lane leads the row and a Shift+click picks instead of opening.
pub fn pr_row(
    summary: &PrSummary,
    show_repo: bool,
    show_author: bool,
    selected: bool,
    select: Option<RowSelect>,
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
    let pr = summary.pr.clone();
    let row = h_flex()
        .id(SharedString::from(format!("pr-row-{id}")))
        .debug_selector({
            let id = id.clone();
            move || format!("pr-row-{id}")
        })
        .role(Role::Link)
        .aria_label(name)
        .w_full()
        .h(scale::row_height())
        .px_2()
        .gap_3()
        .items_center()
        .relative()
        .group(ROW_GROUP)
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use chrono::Utc;
    use gpui_kit::{
        App, AppContext as _, Context, InteractiveElement as _, IntoElement, Modifiers,
        ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use loupe_github::{PrRef, PrState, PrSummary, RepoRef};

    use super::{AgeFrom, RowSelect, SelectGesture, pr_row};
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
                labels: Vec::new(),
                assignees: Vec::new(),
                reviewers: Vec::new(),
                mergeable: loupe_github::Mergeable::Unknown,
                review_requests: Vec::new(),
                reviewed_by: Vec::new(),
            };
            let picks = self.picks.clone();
            let parent_clicks = self.parent_clicks.clone();
            div()
                .id("parent")
                .w(px(1000.))
                .on_click(move |_, _, _| *parent_clicks.borrow_mut() += 1)
                .child(pr_row(
                    &summary,
                    true,
                    true,
                    false,
                    Some(RowSelect {
                        checked: false,
                        showing: true,
                        on_select: Rc::new(move |_, gesture, _, _| {
                            picks.borrow_mut().push(gesture)
                        }),
                    }),
                    AgeFrom::Updated,
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
