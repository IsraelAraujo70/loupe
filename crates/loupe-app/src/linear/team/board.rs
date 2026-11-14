//! The body of the team view: loading, error, and empty states, the board
//! (280 px columns, an empty one as narrow as its heading, that scroll
//! sideways, with cards that drag between and within columns, and columns
//! that drag by their heading into another order), the grouped list, and the
//! "Load more" row.

use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::IssueSummary;
use loupe_linear::views::{DisplayProperty, Grouping, Layout, Ordering};

use super::issues::{self, DropTarget, Group, GroupMark};
use super::{More, TeamView};
use crate::linear::data::Loadable;
use crate::linear::widgets;
use crate::route::Route;
use crate::shell::OpenOnClick as _;
use crate::shell::item_menu::{Item, with_item_menu};
use crate::theme::scale::dp;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

/// How close to a side of the board, in px, a dragged card starts to scroll
/// it.
const EDGE_ZONE: f32 = 72.;

/// How far the board scrolls in one 16 ms tick while a card is dragged with
/// the pointer at `x`, for a board from `left` to `right`: nothing away from
/// the sides, then 4 px a tick at the start of a side's zone up to 24 px at
/// the side and past it. Positive is toward the columns on the right.
fn edge_scroll_speed(left: f32, right: f32, x: f32) -> f32 {
    let speed = |depth: f32| 4. + 20. * (depth / EDGE_ZONE).clamp(0., 1.);
    if x > right - EDGE_ZONE {
        speed(x - (right - EDGE_ZONE))
    } else if x < left + EDGE_ZONE {
        -speed(left + EDGE_ZONE - x)
    } else {
        0.
    }
}

impl TeamView {
    /// A dragged card or column near a side of the board scrolls the board
    /// that way, so a column that is out of sight comes under the pointer. It
    /// keeps scrolling while the pointer rests there, and stops when the
    /// pointer leaves the side, the board ends, or the drag ends.
    fn drag_near_edge<T: 'static>(
        &mut self,
        event: &DragMoveEvent<T>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (bounds, pointer) = (event.bounds, event.event.position);
        let inside = pointer.y >= bounds.top() && pointer.y <= bounds.bottom();
        self.edge_scroll = if inside {
            edge_scroll_speed(
                f32::from(bounds.left()),
                f32::from(bounds.right()),
                f32::from(pointer.x),
            )
        } else {
            0.
        };
        if self.edge_scroll == 0. || self.edge_scrolling {
            return;
        }
        self.edge_scrolling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let scrolled = this
                    .update(cx, |view, cx| {
                        if view.edge_scroll == 0. || !cx.has_active_drag() {
                            return false;
                        }
                        let mut offset = view.board_scroll.offset();
                        let end = -view.board_scroll.max_offset().x;
                        let next = (offset.x - px(view.edge_scroll)).clamp(end, px(0.));
                        if next == offset.x {
                            return false;
                        }
                        offset.x = next;
                        view.board_scroll.set_offset(offset);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !scrolled {
                    break;
                }
            }
            this.update(cx, |view, _| view.edge_scrolling = false).ok();
        })
        .detach();
    }

    /// The loaded issues this view keeps, in its order.
    pub(super) fn visible_issues<'a>(&self, listed: &'a [IssueSummary]) -> Vec<&'a IssueSummary> {
        let now = Utc::now();
        let context = match &self.team {
            Loadable::Ready(team) => issues::match_context(team, self.me.clone(), now),
            _ => loupe_linear::filter::MatchContext {
                me: self.me.clone(),
                ..Default::default()
            },
        };
        if self.local_filters.is_empty() && !self.loaded_by_linear {
            return issues::visible(listed, &self.page.draft, &context, now);
        }
        let mut config = self.page.draft.clone();
        // A custom view's whole set, as Linear evaluated its filter: the
        // filter is not applied a second time here, where it could only
        // disagree with Linear.
        if self.loaded_by_linear {
            config.filters.clear();
        }
        // Narrowed here: the local clauses must hold too.
        config.filters.extend(self.local_filters.iter().cloned());
        issues::visible(listed, &config, &context, now)
    }

    pub(super) fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let board = self.page.draft.layout == Layout::Board;
        let (team, listed) = match (&self.team, &self.issues) {
            (Loadable::Failed(message), _) | (_, Loadable::Failed(message)) => {
                return widgets::load_error(
                    message.clone(),
                    cx.listener(|this, _, _, cx| this.reload_all(cx)),
                );
            }
            (Loadable::Ready(team), Loadable::Ready(listed)) => (team, listed),
            _ if board => return widgets::skeleton_board(cx),
            _ => return div().py_2().child(ui::skeleton_rows(8)).into_any_element(),
        };
        let shown = self.with_drops(listed, team);
        let visible = self.visible_issues(&shown);
        if visible.is_empty() && self.next_cursor.is_none() {
            let filtered = !self.filters().is_empty();
            let state = ui::empty_state(
                IconName::Inbox.into(),
                if filtered {
                    "No issues match these filters"
                } else {
                    "No issues"
                },
                if filtered {
                    "Change or clear the filters to see more."
                } else {
                    "Issues of this team show up here."
                },
            );
            return state
                .when(filtered, |state| {
                    state.action(
                        Button::new("empty-clear")
                            .outline()
                            .small()
                            .label("Clear filters")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_filters(Vec::clear, cx);
                                this.sync_search_box(window, cx);
                            })),
                    )
                })
                .into_any_element();
        }

        let now = Utc::now();
        let mut groups = issues::group_issues(&visible, self.page.draft.group_by, team);
        if board {
            issues::order_columns(&mut groups, &self.page.draft.column_order);
        }
        if !self.page.draft.show_empty_columns || !board {
            groups.retain(|group| !group.issues.is_empty());
        }
        groups.retain(|group| !self.page.draft.hidden_groups.contains(&group.key));
        if board {
            self.render_board(groups, now, cx)
        } else {
            self.render_list(groups, now, cx)
        }
    }

    fn render_board(
        &self,
        groups: Vec<Group<'_>>,
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drag = Drag {
            view: cx.weak_entity(),
            props: self.page.draft.properties.clone(),
            dragging: self.dragging.clone().filter(|_| cx.has_active_drag()),
            manual: self.page.draft.order_by == Ordering::Manual,
            creates: self.project_id.is_none(),
            column: self
                .dragging_column
                .as_ref()
                .filter(|_| cx.has_active_drag())
                .and_then(|key| groups.iter().position(|group| group.key == *key)),
        };
        let keys: Vec<String> = groups.iter().map(|group| group.key.clone()).collect();
        let columns: Vec<AnyElement> = groups
            .into_iter()
            .enumerate()
            .map(|(ix, group)| {
                let neighbors = Neighbors {
                    left: ix.checked_sub(1).map(|left| keys[left].clone()),
                    right: keys.get(ix + 1).cloned(),
                };
                render_column(ix, group, neighbors, now, &drag, cx)
            })
            .collect();
        // Columns past a side: that side fades until the board is scrolled
        // to its end.
        let (offset, max) = (
            self.board_scroll.offset().x,
            self.board_scroll.max_offset().x,
        );
        let (more_before, more_after) = (offset < px(-1.), max + offset > px(1.));
        let shade = crate::theme::tokens::panel();
        let fade = |toward_start: bool| {
            let (from, to) = if toward_start {
                (shade, shade.opacity(0.))
            } else {
                (shade.opacity(0.), shade)
            };
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(dp(40.))
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ))
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        // A row whose one child keeps its content's width:
                        // what a scroller can scroll to is the extent of
                        // its direct children, and a block child would be
                        // as wide as the board and no wider.
                        div()
                            .id("team-board")
                            .flex()
                            .flex_row()
                            .size_full()
                            .overflow_x_scroll()
                            // A sideways gesture scrolls the board and a
                            // vertical one the column under the pointer,
                            // never both.
                            .restrict_scroll_to_axis()
                            .track_scroll(&self.board_scroll)
                            .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
                            .on_drag_move(cx.listener(Self::drag_near_edge::<DraggedIssue>))
                            .on_drag_move(cx.listener(Self::drag_near_edge::<DraggedColumn>))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .h_full()
                                    .items_start()
                                    .gap_3()
                                    .px_4()
                                    .pt_2()
                                    .children(columns),
                            ),
                    )
                    .when(more_before, |board| board.child(fade(true).left_0()))
                    .when(more_after, |board| board.child(fade(false).right_0()))
                    // In sight while the board scrolls, as the theme has it.
                    .child(Scrollbar::horizontal(&self.board_scroll)),
            )
            .children(self.render_load_more(cx).map(|row| {
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(row)
            }))
            .into_any_element()
    }

    fn render_list(
        &self,
        groups: Vec<Group<'_>>,
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let props = self.page.draft.properties.clone();
        let flat = self.page.draft.group_by == Grouping::None;
        let sections: Vec<AnyElement> = groups
            .into_iter()
            .map(|group| {
                let rows: Vec<AnyElement> = group
                    .issues
                    .iter()
                    .map(|issue| {
                        with_item_menu(
                            widgets::issue_row(issue, now, &props, cx).opens(issue_route(issue)),
                            Item::issue(issue),
                        )
                        .into_any_element()
                    })
                    .collect();
                v_flex()
                    .when(!flat, |section| {
                        section.child(
                            group_header(&group, cx)
                                .h(dp(widgets::ROW_HEIGHT))
                                .px_5()
                                .bg(cx.theme().muted.opacity(0.35))
                                .border_b_1()
                                .border_color(cx.theme().border),
                        )
                    })
                    .children(rows)
                    .into_any_element()
            })
            .collect();
        div()
            .id("team-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(v_flex().children(sections))
            .children(self.render_load_more(cx))
            .into_any_element()
    }

    /// The row at the end of the issues: the next-page button, or what went
    /// wrong with the last one. `None` on the last page.
    fn render_load_more(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.next_cursor.as_ref()?;
        let loading = matches!(self.more, More::Loading);
        let failed = match &self.more {
            More::Failed(message) => Some(message.clone()),
            More::Idle | More::Loading => None,
        };
        Some(
            h_flex()
                .h(dp(widgets::HEADER_HEIGHT))
                .gap_2()
                .items_center()
                .justify_center()
                .children(
                    failed.clone().map(|message| {
                        div().text_sm().text_color(cx.theme().danger).child(message)
                    }),
                )
                .child(
                    Button::new("team-load-more")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronDown)
                        .label(if failed.is_some() {
                            "Retry"
                        } else {
                            "Load more"
                        })
                        .loading(loading)
                        .disabled(loading)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.fetch_issues(super::Fetch::More, cx)),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// The least width of a column with no card, in px: its heading may be
/// short ("Done 0").
const EMPTY_COLUMN_MIN_WIDTH: f32 = 148.;

/// What every column needs to draw and accept dragged cards.
struct Drag {
    view: WeakEntity<TeamView>,
    props: Vec<DisplayProperty>,
    /// The card being dragged and the column it left.
    dragging: Option<(String, String)>,
    manual: bool,
    /// A column's "+" creates an issue in it. Off on a project's issues: the
    /// new issue dialog does not set a project.
    creates: bool,
    /// Where on the board the column being dragged by its heading is.
    column: Option<usize>,
}

/// The columns on either side of one, which its menu moves it past.
struct Neighbors {
    left: Option<String>,
    right: Option<String>,
}

/// A column while it is dragged by its heading: its key, and the floating
/// copy of its heading under the pointer.
#[derive(Clone)]
pub(super) struct DraggedColumn {
    key: String,
    title: SharedString,
    count: usize,
}

impl Render for DraggedColumn {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .w(px(widgets::COLUMN_WIDTH))
            .h(dp(widgets::TOOLBAR_HEIGHT))
            .px_3()
            .gap_2()
            .items_center()
            .rounded(theme.radius_lg)
            .bg(theme.popover)
            .border_1()
            .border_color(theme.ring)
            .shadow_lg()
            .text_color(theme.foreground)
            .child(ui::group_title(self.title.clone()).truncate())
            .child(ui::count_badge(self.count, cx))
    }
}

/// A card while it is dragged: what the drop carries, and the floating copy
/// under the pointer.
#[derive(Clone)]
pub(super) struct DraggedIssue {
    pub issue: IssueSummary,
    /// The key of the column the card left.
    pub from_group: String,
    props: Vec<DisplayProperty>,
}

impl Render for DraggedIssue {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The drag layer is drawn outside the window's tree, so it inherits
        // no text color.
        div()
            .w(px(widgets::COLUMN_WIDTH))
            .text_color(cx.theme().foreground)
            .child(
                widgets::issue_card(&self.issue, Utc::now(), &self.props, cx)
                    .border_color(cx.theme().ring)
                    .shadow_lg()
                    .opacity(0.92),
            )
    }
}

fn render_column(
    ix: usize,
    group: Group<'_>,
    neighbors: Neighbors,
    now: DateTime<Utc>,
    drag: &Drag,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let header = column_header(ix, &group, neighbors, &drag.view, drag.creates, cx);
    // A column dragged onto this one takes its place: the line on the side
    // it comes in from, between this column and the next one over, is where
    // it lands.
    let lands_on_right = drag.column.map(|from| from < ix);
    let lifted_column = drag.column == Some(ix);
    let group_name = SharedString::from(format!("team-board-column-{ix}"));
    let landing_line = lands_on_right.filter(|_| !lifted_column).map(|on_right| {
        let ring = theme.ring;
        // Half the 12 px gap out from the column, so the line sits between
        // the two columns.
        let line = div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(2.))
            .rounded_full()
            .group_drag_over::<DraggedColumn>(group_name.clone(), move |style| style.bg(ring))
            // GPUI applies a group's drag-over style only to an element with
            // a hitbox of its own, which a drag-over style of its own gives.
            .drag_over::<DraggedColumn>(move |style, _, _, _| style.bg(ring));
        if on_right {
            line.right(px(-7.))
        } else {
            line.left(px(-7.))
        }
    });
    let column_drop = {
        let (view, target) = (drag.view.clone(), group.key.clone());
        move |dragged: &DraggedColumn, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| this.move_column(&dragged.key, &target, cx))
                .ok();
        }
    };
    // Where a card lands only matters in manual order or inside its own
    // column; elsewhere the column's order places it, so only the column
    // lights up.
    let positional = drag
        .dragging
        .as_ref()
        .is_some_and(|(_, from)| drag.manual || *from == group.key);
    let line = theme.ring;
    // A drop target whose top border lights up as the line where the card
    // lands. The border is always there, so lighting it moves nothing.
    let drop_zone = |zone: Div| {
        zone.mt(px(3.))
            .pt(px(3.))
            .border_t_2()
            .border_color(transparent_black())
            .when(positional, |zone| {
                zone.drag_over::<DraggedIssue>(move |style, _, _, _| style.border_color(line))
            })
    };
    let on_drop = |before: Option<String>| {
        let target = DropTarget {
            group_key: group.key.clone(),
            before,
        };
        let view = drag.view.clone();
        move |dragged: &DraggedIssue, _: &mut Window, cx: &mut App| {
            let target = target.clone();
            view.update(cx, |this, cx| this.drop_issue(dragged, target, cx))
                .ok();
        }
    };
    let cards: Vec<AnyElement> = group
        .issues
        .iter()
        .map(|issue| {
            let lifted = drag
                .dragging
                .as_ref()
                .is_some_and(|(id, from)| *id == issue.id && *from == group.key);
            let dragged = DraggedIssue {
                issue: (*issue).clone(),
                from_group: group.key.clone(),
                props: drag.props.clone(),
            };
            let view = drag.view.clone();
            let card = widgets::issue_card(issue, now, &drag.props, cx)
                .when(lifted, |card| card.opacity(0.4))
                .opens(issue_route(issue))
                .on_drag(dragged, move |dragged, _, _, cx| {
                    let lifted = (dragged.issue.id.clone(), dragged.from_group.clone());
                    view.update(cx, |this, cx| {
                        this.dragging = Some(lifted);
                        this.dragging_column = None;
                        cx.notify();
                    })
                    .ok();
                    cx.new(|_| dragged.clone())
                });
            drop_zone(v_flex())
                .on_drop(on_drop(Some(issue.id.clone())))
                .child(with_item_menu(card, Item::issue(issue)))
                .into_any_element()
        })
        .collect();
    let hover_bg = theme.muted.opacity(0.5);
    let radius = theme.radius_lg;
    // A column with no card is as narrow as its heading, as in Linear: it
    // stays a place to drop a card and to add an issue, and gives its
    // width to the columns that have cards.
    let empty = group.issues.is_empty();
    v_flex()
        .group(group_name)
        .relative()
        .when(!empty, |column| column.w(dp(widgets::COLUMN_WIDTH)))
        .when(empty, |column| column.min_w(dp(EMPTY_COLUMN_MIN_WIDTH)))
        .flex_none()
        .h_full()
        .rounded(radius)
        .when(lifted_column, |column| column.opacity(0.4))
        .drag_over::<DraggedIssue>(move |style, _, _, _| style.bg(hover_bg))
        .on_drop(on_drop(None))
        .on_drop(column_drop)
        .children(landing_line)
        .child(header)
        .child(
            div()
                .id(("team-board-column", ix))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                // A sideways gesture over a column is the board's.
                .restrict_scroll_to_axis()
                .pb_3()
                .child(
                    v_flex().min_h_full().children(cards).child(
                        // Fills the rest of the column, so a drop anywhere
                        // below the cards lands at the end and says so.
                        drop_zone(div())
                            .flex_1()
                            .min_h(px(48.))
                            .on_drop(on_drop(None)),
                    ),
                ),
        )
        .into_any_element()
}

/// A column's heading, which drags the column into another place, with
/// Linear's "…" (move the column, hide it) and, when grouped by status, "+"
/// (a new issue in that state).
fn column_header(
    ix: usize,
    group: &Group<'_>,
    neighbors: Neighbors,
    view: &WeakEntity<TeamView>,
    creates: bool,
    cx: &App,
) -> impl IntoElement {
    let title = group.title.clone();
    let key = group.key.clone();
    let hide_view = view.clone();
    let move_item = move |label: &'static str,
                          icon: gpui_kit::assets::IconName,
                          target: Option<String>,
                          key: String,
                          view: WeakEntity<TeamView>| {
        PopupMenuItem::new(label)
            .icon(icon)
            .disabled(target.is_none())
            .on_click(move |_, _, cx| {
                if let Some(target) = &target {
                    view.update(cx, |this, cx| this.move_column(&key, target, cx))
                        .ok();
                }
            })
    };
    let Neighbors { left, right } = neighbors;
    let menu = Button::new(("column-menu", ix))
        .ghost()
        .small()
        .icon(IconName::Ellipsis)
        .tooltip("Column actions")
        .accessibility_label(format!("{title} column actions"))
        .menu_trigger()
        .dropdown_menu(move |menu, _, _| {
            let (key, view) = (key.clone(), hide_view.clone());
            menu.item(move_item(
                "Move left",
                gpui_kit::assets::IconName::ArrowLeft,
                left.clone(),
                key.clone(),
                view.clone(),
            ))
            .item(move_item(
                "Move right",
                gpui_kit::assets::IconName::ArrowRight,
                right.clone(),
                key.clone(),
                view.clone(),
            ))
            .item(PopupMenuItem::separator())
            .item(
                PopupMenuItem::new("Hide column")
                    .icon(IconName::EyeOff)
                    .on_click(move |_, _, cx| {
                        let key = key.clone();
                        view.update(cx, |this, cx| {
                            this.edit_draft(|draft| draft.hidden_groups.push(key), cx)
                        })
                        .ok();
                    }),
            )
        });
    let add = match &group.mark {
        GroupMark::State(state) if creates => {
            let (state_id, view) = (state.id.clone(), view.clone());
            Some(
                Button::new(("column-add", ix))
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip("New issue")
                    .accessibility_label(format!("New issue in {title}"))
                    .on_click(move |_, window, cx| {
                        let state_id = state_id.clone();
                        view.update(cx, |this, cx| {
                            this.open_new_issue_in(Some(state_id), window, cx)
                        })
                        .ok();
                    }),
            )
        }
        _ => None,
    };
    let dragged = DraggedColumn {
        key: group.key.clone(),
        title: group.title.clone().into(),
        count: group.issues.len(),
    };
    let drag_view = view.clone();
    // On the cards' own edges: the mark over their leading border, the
    // buttons over their trailing one.
    group_header(group, cx)
        .id(("column-heading", ix))
        .h(dp(widgets::TOOLBAR_HEIGHT))
        .on_drag(dragged, move |dragged, _, _, cx| {
            let key = dragged.key.clone();
            drag_view
                .update(cx, |this, cx| {
                    this.dragging_column = Some(key);
                    this.dragging = None;
                    cx.notify();
                })
                .ok();
            cx.new(|_| dragged.clone())
        })
        .child(div().flex_1().min_w_2())
        .child(menu)
        .children(add)
}

fn group_header(group: &Group<'_>, cx: &App) -> Div {
    let theme = cx.theme();
    let mark: Option<AnyElement> = match &group.mark {
        GroupMark::State(state) => Some(widgets::state_icon(state, cx)),
        GroupMark::Priority(priority) => Some(widgets::priority_icon(*priority, cx)),
        GroupMark::Person(user) => Some(widgets::avatar(user.as_ref(), dp(16.))),
        GroupMark::Swatch(color) => Some(
            div()
                .flex_none()
                .size(dp(8.))
                .rounded_full()
                .bg(color
                    .as_deref()
                    .and_then(widgets::hex_color)
                    .unwrap_or(theme.muted_foreground))
                .into_any_element(),
        ),
        GroupMark::Plain => None,
    };
    h_flex()
        .flex_none()
        .gap_2()
        .items_center()
        .children(mark)
        .child(ui::group_title(group.title.clone()).truncate())
        .child(ui::count_badge(group.issues.len(), cx))
}

fn issue_route(issue: &IssueSummary) -> Route {
    Route::LinearIssue {
        identifier: issue.identifier.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{EDGE_ZONE, edge_scroll_speed};

    #[test]
    fn a_dragged_card_scrolls_the_board_only_near_its_sides() {
        let (left, right) = (300., 1400.);
        // Away from the sides nothing moves.
        assert_eq!(edge_scroll_speed(left, right, 850.), 0.);
        assert_eq!(edge_scroll_speed(left, right, left + EDGE_ZONE), 0.);
        assert_eq!(edge_scroll_speed(left, right, right - EDGE_ZONE), 0.);
        // Toward the right side it speeds up, to its most at the side and
        // past it.
        let slow = edge_scroll_speed(left, right, right - EDGE_ZONE + 1.);
        let fast = edge_scroll_speed(left, right, right - 1.);
        assert!(slow > 0. && slow < fast);
        assert_eq!(edge_scroll_speed(left, right, right), 24.);
        assert_eq!(edge_scroll_speed(left, right, right + 500.), 24.);
        // The left side is the same, the other way.
        assert_eq!(edge_scroll_speed(left, right, left), -24.);
        assert_eq!(
            edge_scroll_speed(left, right, left + 10.),
            -edge_scroll_speed(left, right, right - 10.)
        );
    }
}
