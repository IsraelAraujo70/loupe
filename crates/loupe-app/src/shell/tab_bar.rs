//! The top bar, after Linear's: room for the traffic lights, the sidebar
//! toggle, back and forward for the active tab, the tabs, and a new-tab
//! button. The whole bar is a window drag area. Every tab is an accessible
//! tab named after its page, with a close button.
//!
//! With one pane its tabs sit in the top bar; once the window is split, each
//! pane shows its own row of tabs over its page ([`pane_strip`]) and the top
//! bar keeps the navigation and +, which act on the active pane. A tab drags
//! along its row to a new place, to another pane's row, or onto a page
//! (`Workspace` draws the drop zones there); its right-click menu splits.
//!
//! A row with more tabs than fit scrolls sideways, with the trackpad or the
//! wheel, once its tabs are down to [`TAB_MIN_WIDTH`]; + stays after it, in
//! view. When a pane's open tab changes, its row scrolls just enough to show
//! it whole; a tab opened in the background leaves the row where it is.
//!
//! Grouped tabs follow their group's name, a colored pill, and carry a line
//! of its color. A click on the name collapses or expands the group; its
//! right-click menu renames, recolors, ungroups, or closes it. A tab joins a
//! group from its own right-click menu, or when it is dropped on the group's
//! name or on one of its tabs; dropped on a tab outside every group, it
//! leaves. The name drags the whole group along its row, never into another
//! group. A right click on + lists the closed groups to reopen.
//!
//! Pinned tabs ("Pin tab" in a tab's right-click menu) come first, narrower,
//! with their icon and a short name and no close button; the whole name is
//! their tooltip. They are never in a group: adding one to a group unpins it.

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use super::actions::{GoBack, GoForward, NewTab, ToggleSidebar};
use super::panes::{Pane, PaneId, Zone};
use super::tab_groups;
use super::tabs::{Page, TabGroup};
use crate::route::Route;
use crate::store::{ClosedTabGroup, TabGroupColor};
use crate::theme::scale::dp;
use crate::theme::tokens;
use crate::ui::{self, WindowDragArea as _};

/// Height of the top bar; the traffic lights are centered in it.
pub(crate) const TOP_BAR_HEIGHT: Pixels = px(40.);
/// Room the macOS traffic lights take at the bar's leading edge. Other
/// platforms draw their window buttons outside the content, so the bar
/// starts at the sidebar's own inset there.
const LEADING_INSET: Pixels = px(if cfg!(target_os = "macos") { 78. } else { 8. });
const TAB_MAX_WIDTH: f32 = 220.;
/// Tabs shrink to this width and no further, so a title keeps about ten
/// characters; past it the row scrolls.
const TAB_MIN_WIDTH: f32 = 128.;
/// A pinned tab is as wide as its icon and name, up to this.
const PINNED_TAB_MAX_WIDTH: f32 = 112.;
/// A pane's own row of tabs, once the window is split.
const STRIP_HEIGHT: f32 = 36.;

/// What a tab calls its page: the page's own title where it has one (a
/// Reviews page names its open saved view), otherwise its route's label.
pub(super) fn page_label(page: &Page<AnyView>, cx: &App) -> SharedString {
    match page.route {
        Route::Inbox => crate::github::inbox_title(&page.view, cx),
        _ => None,
    }
    .unwrap_or_else(|| page.route.label().into())
}

/// The icon a tab showing `route` has.
pub(super) fn route_icon(route: &Route) -> IconName {
    match route {
        Route::Inbox => IconName::GitPullRequest,
        Route::Notifications => IconName::Inbox,
        Route::Repo(_) => IconName::FolderGit2,
        Route::PullRequest(_) => IconName::GitPullRequest,
        Route::Actions { .. } | Route::WorkflowRun { .. } => IconName::Workflow,
        Route::LinearMyIssues => IconName::CircleUser,
        Route::LinearTeam { .. } => IconName::Users,
        Route::LinearProjects { .. } | Route::LinearProject { .. } => IconName::Box,
        Route::LinearViews { .. } => IconName::Layers,
        Route::LinearIssue { .. } => IconName::CircleDot,
        Route::Settings => IconName::Settings,
    }
}

/// The short name a pinned tab shows: a repository by its name alone, since
/// its owner is what a narrow tab would show otherwise; any other page by
/// `label`, its tab's title.
fn pinned_label(route: &Route, label: &SharedString) -> SharedString {
    match route {
        Route::Repo(repo) => repo.name.clone().into(),
        _ => label.clone(),
    }
}

/// The sideways scroll of a pane's row of tabs, keyed by the open tab's id.
pub(super) type TabRow = ui::ScrollRow<u64>;

/// A tab while it is dragged: where it comes from, and the floating copy of
/// it under the pointer.
#[derive(Clone)]
pub(super) struct DraggedTab {
    pub pane: PaneId,
    pub tab: u64,
    label: SharedString,
    icon: IconName,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // The drag layer is drawn outside the window's tree, so it inherits
        // no text color.
        h_flex()
            .h(px(28.))
            .max_w(dp(TAB_MAX_WIDTH))
            .px_3()
            .gap_1p5()
            .items_center()
            .rounded(theme.radius)
            .bg(theme.popover)
            .border_1()
            .border_color(theme.ring)
            .shadow_lg()
            .text_size(rems(13. / 16.))
            .text_color(theme.foreground)
            .child(
                svg()
                    .path(self.icon.path())
                    .flex_none()
                    .size_3p5()
                    .text_color(theme.foreground),
            )
            .child(div().min_w_0().truncate().child(self.label.clone()))
    }
}

/// A group while it is dragged by its name: where it comes from, and the
/// floating copy of its name under the pointer.
#[derive(Clone)]
pub(super) struct DraggedGroup {
    pub pane: PaneId,
    pub group: u64,
    name: SharedString,
    color: TabGroupColor,
}

impl Render for DraggedGroup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(22.))
            .max_w(dp(TAB_MAX_WIDTH))
            .px_2()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(tokens::tab_group(self.color))
            .shadow_lg()
            .text_color(tokens::on_accent())
            .text_size(rems(12. / 16.))
            .font_weight(FontWeight::MEDIUM)
            .child(div().min_w_0().truncate().child(self.name.clone()))
    }
}

/// The group being dragged by its name, and the tab of its row under the
/// pointer (`onto`), whose place it would take if dropped now.
#[derive(Clone, Copy)]
pub(super) struct GroupLift {
    pub pane: PaneId,
    pub group: u64,
    pub onto: Option<usize>,
}

/// What shows where a dragged group would land: a group's name or a tab
/// (by index), and the side of it the line is on.
#[derive(Clone, Copy, PartialEq)]
enum Landing {
    Label(u64, bool),
    Tab(usize, bool),
}

/// Where the line goes for `lift` over `tabs`: before the tab or the group
/// the dragged group would go before, or after the one it would follow.
fn landing(tabs: &super::tabs::Tabs<AnyView>, lift: GroupLift) -> Option<Landing> {
    let to = tabs.group_drop_at(lift.group, Some(lift.onto?))?;
    let first = tabs.iter().position(|tab| tab.group == Some(lift.group))?;
    if to < first {
        // `to` starts what the group goes before: a group's first tab, whose
        // name leads it, or a tab outside every group.
        return Some(match tabs.group_at(to) {
            Some(group) => Landing::Label(group, false),
            None => Landing::Tab(to, false),
        });
    }
    let last = to - 1;
    Some(match tabs.group_at(last) {
        Some(group) if tabs.group(group).is_some_and(|group| group.collapsed) => {
            Landing::Label(group, true)
        }
        _ => Landing::Tab(last, true),
    })
}

/// The 2 px accent line that shows where a dragged tab or group lands, on
/// the leading or (`trailing`) trailing side of its parent.
fn landing_line(trailing: bool, color: Hsla) -> Div {
    let line = div()
        .absolute()
        .top_1()
        .bottom_1()
        .w(px(2.))
        .rounded_full()
        .bg(color);
    if trailing {
        line.right(px(-3.))
    } else {
        line.left(px(-3.))
    }
}

/// What a dragged group is over in its row: the tab at `index`, or the
/// name of the group that tab starts (`label`). A name and its first tab
/// share the index, so `label` keeps them apart: the one the pointer left
/// must not forget the one it is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GroupTarget {
    pub pane: PaneId,
    pub index: usize,
    pub label: bool,
}

/// Lets the tab at `index` of `pane`, or with `label` the name that leads
/// it, take a dragged group: it follows the pointer while the group's own
/// row is dragged over, and moves the group on a drop.
fn takes_groups<E: InteractiveElement>(
    element: E,
    pane: PaneId,
    index: usize,
    label: bool,
    cx: &mut Context<Workspace>,
) -> E {
    let target = GroupTarget { pane, index, label };
    element
        .on_drag_move(
            cx.listener(move |this, event: &DragMoveEvent<DraggedGroup>, _, cx| {
                if event.drag(cx).pane != pane {
                    return;
                }
                let over = event.bounds.contains(&event.event.position);
                this.hover_group_drop(target, over, cx);
            }),
        )
        .on_drop(cx.listener(move |this, dragged: &DraggedGroup, _, cx| {
            this.drop_group(dragged, pane, Some(index), cx)
        }))
}

/// The name of group `group` of `pane`, before its first tab, at `first`.
/// `hidden` is how many tabs it hides while collapsed; `lifted` fades it
/// while it is dragged; `line` is the side a dragged group's landing line
/// is on, if it lands next to this one.
fn group_label(
    pane: PaneId,
    group: &TabGroup,
    first: usize,
    hidden: usize,
    lifted: bool,
    line: Option<bool>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = group.id;
    let fill = tokens::tab_group(group.color);
    let on_fill = tokens::on_accent();
    let ring = cx.theme().foreground;
    let accent = cx.theme().ring;
    let name: SharedString = group.name.clone().into();
    let collapsed = group.collapsed;
    let color = group.color;
    let workspace = cx.entity().downgrade();
    let state = if collapsed { "collapsed" } else { "expanded" };
    let dragged = DraggedGroup {
        pane,
        group: id,
        name: name.clone(),
        color,
    };
    let on_drag = {
        let workspace = workspace.clone();
        move |dragged: &DraggedGroup, _: Point<Pixels>, _: &mut Window, cx: &mut App| {
            let started = dragged.clone();
            workspace
                .update(cx, |this, cx| {
                    this.group_drag = Some(started);
                    cx.notify();
                })
                .ok();
            cx.new(|_| dragged.clone())
        }
    };
    let label =
        h_flex()
            .id(("tab-group", id))
            .debug_selector(move || format!("tab-group-{pane}-{id}"))
            .role(Role::Button)
            .aria_label(format!("{name} group, {state}"))
            .aria_expanded(!collapsed)
            .relative()
            .flex_none()
            .h(px(22.))
            .max_w(dp(TAB_MAX_WIDTH))
            .px_2()
            .gap_1()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(fill)
            .border_1()
            .border_color(transparent_black())
            .text_color(on_fill)
            .text_size(rems(12. / 16.))
            .font_weight(FontWeight::MEDIUM)
            .when(lifted, |label| label.opacity(0.5))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_tabs(pane, |tabs| tabs.toggle_group(id), cx)
            }))
            .on_drag(dragged, on_drag)
            .drag_over::<DraggedTab>(move |style, _, _, _| style.border_color(ring))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.drop_on_group(dragged, pane, id, cx)
            }));
    takes_groups(label, pane, first, true, cx)
        .child(div().min_w_0().truncate().child(name.clone()))
        .when(collapsed, |label| {
            label.child(div().flex_none().opacity(0.8).child(hidden.to_string()))
        })
        .children(line.map(|trailing| landing_line(trailing, accent)))
        .context_menu(move |menu, window, cx| {
            let rename = {
                let workspace = workspace.clone();
                let name = name.to_string();
                PopupMenuItem::new("Rename group…").on_click(move |_, window, cx| {
                    let workspace = workspace.clone();
                    tab_groups::ask_name(
                        "Rename group",
                        "Rename",
                        name.clone(),
                        move |name, _, cx| {
                            workspace
                                .update(cx, |this, cx| {
                                    this.edit_tabs(pane, |tabs| tabs.rename_group(id, name), cx)
                                })
                                .ok();
                        },
                        window,
                        cx,
                    );
                })
            };
            let ungroup = {
                let workspace = workspace.clone();
                PopupMenuItem::new("Ungroup").on_click(move |_, _, cx| {
                    workspace
                        .update(cx, |this, cx| {
                            this.edit_tabs(pane, |tabs| tabs.ungroup(id), cx)
                        })
                        .ok();
                })
            };
            let close = {
                let workspace = workspace.clone();
                PopupMenuItem::new("Close group").on_click(move |_, window, cx| {
                    workspace
                        .update(cx, |this, cx| this.close_group(pane, id, window, cx))
                        .ok();
                })
            };
            let workspace = workspace.clone();
            menu.item(rename)
                .submenu("Color", window, cx, move |submenu, _, _| {
                    TabGroupColor::ALL
                        .into_iter()
                        .fold(submenu, |submenu, choice| {
                            let workspace = workspace.clone();
                            submenu.item(
                                PopupMenuItem::new(choice.label())
                                    .checked(choice == color)
                                    .on_click(move |_, _, cx| {
                                        workspace
                                            .update(cx, |this, cx| {
                                                this.edit_tabs(
                                                    pane,
                                                    |tabs| tabs.set_group_color(id, choice),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }),
                            )
                        })
                })
                .separator()
                .item(ungroup)
                .item(close)
        })
        .into_any_element()
}

/// The tabs of `pane`, each group's name before its first tab, without the
/// tabs a collapsed group hides; and the open tab's place among them.
/// `emphasized` is false for the selected tab of a pane that is not the
/// active one, which then keeps its fill but not its color. `lifted` is the
/// pane and the tab being dragged, if any; `group_lift` the group being
/// dragged by its name.
fn tabs(
    pane: &Pane<AnyView>,
    emphasized: bool,
    lifted: Option<(PaneId, u64)>,
    group_lift: Option<GroupLift>,
    cx: &mut Context<Workspace>,
) -> (Vec<AnyElement>, usize) {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let foreground = theme.foreground;
    let radius = theme.radius;
    let ring = theme.ring;
    let pane_id = pane.id;
    let active = pane.tabs.active_index();
    let lifted_index = lifted
        .filter(|(from, _)| *from == pane_id)
        .and_then(|(_, tab)| pane.tabs.index_of(tab));
    // A tab dragged along its row lands on its own side of the pinned tabs,
    // so a tab on the other side shows no landing line.
    let lifted_pinned = lifted_index
        .and_then(|index| pane.tabs.iter().nth(index))
        .map(|tab| tab.pinned);
    let workspace = cx.entity().downgrade();
    let group_lift = group_lift.filter(|lift| lift.pane == pane_id);
    let lifted_group = group_lift.map(|lift| lift.group);
    let landing = group_lift.and_then(|lift| landing(&pane.tabs, lift));
    let groups: Vec<(u64, SharedString)> = pane
        .tabs
        .groups()
        .iter()
        .map(|group| (group.id, group.name.clone().into()))
        .collect();
    let mut items = Vec::new();
    let mut open_child = 0;
    let mut previous_group = None;
    for (index, tab) in pane.tabs.iter().enumerate() {
        let group = tab.group.and_then(|id| pane.tabs.group(id));
        if let Some(group) = group
            && previous_group != Some(group.id)
        {
            let hidden = pane.tabs.group_len(group.id);
            let line = match landing {
                Some(Landing::Label(id, trailing)) if id == group.id => Some(trailing),
                _ => None,
            };
            let lifted = lifted_group == Some(group.id);
            items.push(group_label(pane_id, group, index, hidden, lifted, line, cx));
        }
        previous_group = tab.group;
        if pane.tabs.is_hidden(index) {
            continue;
        }
        if index == active {
            open_child = items.len();
        }
        let line_color = group.map(|group| tokens::tab_group(group.color));
        let element = {
            let route = &tab.current().route;
            let label = page_label(tab.current(), cx);
            let icon = route_icon(route);
            let selected = index == active;
            let pinned = tab.pinned;
            let shown_label = if pinned {
                pinned_label(route, &label)
            } else {
                label.clone()
            };
            let id = tab.id;
            let group = SharedString::from(format!("tab-{id}"));
            // A tab dropped here takes this one's place: from the left in
            // its own row it lands to the right of it, otherwise to the left.
            let landing_line = lifted
                .filter(|(_, tab)| *tab != id)
                .filter(|_| lifted_pinned.is_none_or(|lifted| lifted == pinned))
                .map(|_| {
                    let line = div()
                        .absolute()
                        .top_1()
                        .bottom_1()
                        .w(px(2.))
                        .rounded_full()
                        .group_drag_over::<DraggedTab>(group.clone(), move |style| style.bg(ring))
                        // GPUI applies a group's drag-over style only to an
                        // element with a hitbox of its own, which a drag-over
                        // style of its own gives.
                        .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(ring));
                    if lifted_index.is_some_and(|from| from < index) {
                        line.right(px(-3.))
                    } else {
                        line.left(px(-3.))
                    }
                });
            let dragged = DraggedTab {
                pane: pane_id,
                tab: id,
                label: label.clone(),
                icon,
            };
            let on_drag = {
                let workspace = workspace.clone();
                move |dragged: &DraggedTab, _: Point<Pixels>, _: &mut Window, cx: &mut App| {
                    let started = dragged.clone();
                    workspace
                        .update(cx, |this, cx| {
                            this.tab_drag = Some(started);
                            cx.notify();
                        })
                        .ok();
                    cx.new(|_| dragged.clone())
                }
            };
            let element = h_flex()
                .id(("tab", id))
                .debug_selector(|| format!("tab-{id}"))
                .group(group.clone())
                .role(Role::Tab)
                .aria_label(if pinned {
                    format!("{label}, pinned").into()
                } else {
                    label.clone()
                })
                .aria_selected(selected)
                .relative()
                .h(px(28.))
                .when(pinned, |tab| {
                    let label = label.clone();
                    tab.flex_none()
                        .max_w(dp(PINNED_TAB_MAX_WIDTH))
                        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                })
                .when(!pinned, |tab| {
                    tab.min_w(dp(TAB_MIN_WIDTH))
                        .max_w(dp(TAB_MAX_WIDTH))
                        .flex_shrink(1.)
                })
                // 12 px on both sides of every tab, so the space between two
                // labels is the same whatever their length. The open tab
                // makes room for its close button; the others show theirs
                // over the label's end on hover. A pinned tab has none.
                .pl_3()
                .when(selected && !pinned, |tab| tab.pr_1())
                .when(!selected || pinned, |tab| tab.pr_3())
                .gap_1p5()
                .items_center()
                .rounded(radius)
                .text_size(rems(13. / 16.))
                .when(selected, |tab| {
                    tab.bg(tokens::glass_selected()).text_color(if emphasized {
                        foreground
                    } else {
                        muted
                    })
                })
                .when(!selected, |tab| {
                    tab.text_color(muted)
                        .hover(|style| style.bg(tokens::glass_hover()))
                })
                .when(
                    lifted_index == Some(index)
                        || (lifted_group.is_some() && tab.group == lifted_group),
                    |tab| tab.opacity(0.5),
                )
                // A press on a tab is the tab's, not the window's to move.
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(cx.listener(move |this, _, _, cx| this.select_tab_id(id, cx)))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.close_tab_id(id, window, cx)),
                )
                .on_drag(dragged, on_drag)
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                    this.drop_on_row(dragged, pane_id, Some(index), cx)
                }))
                .child(svg().path(icon.path()).flex_none().size_3p5().text_color(
                    if selected && emphasized {
                        foreground
                    } else {
                        muted
                    },
                ))
                .child({
                    let text = shown_label.clone();
                    div()
                        .debug_selector(move || format!("tab-{id}-label:{text}"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(shown_label)
                })
                .when(!pinned, |tab| {
                    tab.child(
                        div()
                            .flex_none()
                            .when(!selected, |slot| {
                                slot.absolute()
                                    .right_1()
                                    .top_1()
                                    .rounded(radius)
                                    .bg(tokens::surface())
                                    .invisible()
                                    .group_hover(group.clone(), |style| style.visible())
                            })
                            .child(
                                ui::icon_button(
                                    ("close-tab", id),
                                    IconName::Close,
                                    format!("Close {label}"),
                                )
                                .xsmall()
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.close_tab_id(id, window, cx);
                                    },
                                )),
                            ),
                    )
                })
                .children(landing_line)
                .children(match landing {
                    Some(Landing::Tab(at, trailing)) if at == index => {
                        Some(self::landing_line(trailing, ring))
                    }
                    _ => None,
                })
                .children(line_color.map(|color| {
                    div()
                        .absolute()
                        .bottom_0()
                        .left_2()
                        .right_2()
                        .h(px(2.))
                        .rounded_full()
                        .bg(color)
                }));
            let element = takes_groups(element, pane_id, index, false, cx);
            let workspace = workspace.clone();
            let in_group = tab.group;
            let other_groups: Vec<(u64, SharedString)> = groups
                .iter()
                .filter(|(group, _)| Some(*group) != in_group)
                .cloned()
                .collect();
            element
                .context_menu(move |menu, window, cx| {
                    let item = |label: &'static str, zone: Zone| {
                        let workspace = workspace.clone();
                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.split_tab(id, zone, window, cx))
                                .ok();
                        })
                    };
                    let close = {
                        let workspace = workspace.clone();
                        PopupMenuItem::new("Close tab").on_click(move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.close_tab_id(id, window, cx))
                                .ok();
                        })
                    };
                    let new_group = {
                        let workspace = workspace.clone();
                        PopupMenuItem::new("Add to new group…").on_click(move |_, window, cx| {
                            let workspace = workspace.clone();
                            tab_groups::ask_name(
                                "New group",
                                "Create",
                                String::new(),
                                move |name, _, cx| {
                                    workspace
                                        .update(cx, |this, cx| {
                                            this.edit_tab(
                                                id,
                                                |tabs, index| tabs.new_group(index, name).is_some(),
                                                cx,
                                            )
                                        })
                                        .ok();
                                },
                                window,
                                cx,
                            );
                        })
                    };
                    let pin = {
                        let workspace = workspace.clone();
                        let label = if pinned { "Unpin tab" } else { "Pin tab" };
                        PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            workspace
                                .update(cx, |this, cx| {
                                    this.edit_tab(
                                        id,
                                        |tabs, index| {
                                            if pinned {
                                                tabs.unpin(index)
                                            } else {
                                                tabs.pin(index)
                                            }
                                        },
                                        cx,
                                    )
                                })
                                .ok();
                        })
                    };
                    let menu = menu
                        .item(item("Split right", Zone::Right))
                        .item(item("Split left", Zone::Left))
                        .item(item("Split up", Zone::Up))
                        .item(item("Split down", Zone::Down))
                        .separator()
                        .item(pin)
                        .separator()
                        .item(new_group);
                    let menu = if other_groups.is_empty() {
                        menu
                    } else {
                        let workspace = workspace.clone();
                        let other_groups = other_groups.clone();
                        menu.submenu("Add to group", window, cx, move |submenu, _, _| {
                            other_groups.iter().fold(submenu, |submenu, (group, name)| {
                                let (workspace, group) = (workspace.clone(), *group);
                                submenu.item(PopupMenuItem::new(name.clone()).on_click(
                                    move |_, _, cx| {
                                        workspace
                                            .update(cx, |this, cx| {
                                                this.edit_tab(
                                                    id,
                                                    |tabs, index| tabs.add_to_group(index, group),
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                ))
                            })
                        })
                    };
                    let menu = if in_group.is_some() {
                        let workspace = workspace.clone();
                        menu.item(PopupMenuItem::new("Remove from group").on_click(
                            move |_, _, cx| {
                                workspace
                                    .update(cx, |this, cx| {
                                        this.edit_tab(
                                            id,
                                            |tabs, index| tabs.remove_from_group(index),
                                            cx,
                                        )
                                    })
                                    .ok();
                            },
                        ))
                    } else {
                        menu
                    };
                    menu.separator().item(close)
                })
                .into_any_element()
        };
        items.push(element);
    }
    (items, open_child)
}

/// The row that holds `pane`'s tabs: a tab list that scrolls sideways and
/// also takes a tab dropped after its last one.
fn tab_list(
    pane: &Pane<AnyView>,
    row: &TabRow,
    label: SharedString,
    emphasized: bool,
    lifted: Option<(PaneId, u64)>,
    group_lift: Option<GroupLift>,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let pane_id = pane.id;
    let (items, open_child) = tabs(pane, emphasized, lifted, group_lift, cx);
    // `open_child` counts the groups' names and leaves out hidden tabs.
    row.follow(pane.tabs.active().id, open_child);
    h_flex()
        .on_children_prepainted(row.watch_width(cx.entity_id()))
        .id(("tabs", pane_id))
        .debug_selector(move || format!("tabs-{pane_id}"))
        .role(Role::TabList)
        .aria_label(label)
        .min_w_0()
        .overflow_x_scroll()
        .track_scroll(row.handle())
        // The row clips what it scrolls; the landing line before its first
        // tab and after its last needs this much room inside it.
        .px_1()
        .gap_1()
        .items_center()
        .children(items)
        .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
            this.drop_on_row(dragged, pane_id, None, cx)
        }))
        .on_drop(cx.listener(move |this, dragged: &DraggedGroup, _, cx| {
            this.drop_group(dragged, pane_id, None, cx)
        }))
}

/// The new-tab button. A right click lists the closed groups, the latest
/// first, each with its color, to reopen in the active pane.
fn new_tab_button(closed: &[ClosedTabGroup], cx: &mut Context<Workspace>) -> AnyElement {
    let button = div()
        .id("new-tab-slot")
        .flex_none()
        .debug_selector(|| "new-tab".into())
        .child(
            ui::icon_button("new-tab", IconName::Plus, "New tab (⌘T)")
                .small()
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTab), cx)),
        );
    if closed.is_empty() {
        return button.into_any_element();
    }
    let closed = closed.to_vec();
    let workspace = cx.entity().downgrade();
    button
        .context_menu(move |menu, _, _| {
            closed
                .iter()
                .enumerate()
                .fold(menu, |menu, (index, group)| {
                    let label: SharedString = format!("Reopen group {}", group.name).into();
                    let fill = tokens::tab_group(group.color);
                    let shown = label.clone();
                    let workspace = workspace.clone();
                    let group = group.clone();
                    menu.item(
                        PopupMenuItem::element(move |_, _| {
                            // The stock row names only text items; this one draws
                            // the group's color, so its content carries the name.
                            h_flex()
                                .id(("reopen-group", index))
                                .debug_selector(move || format!("reopen-group-{index}"))
                                .role(Role::Group)
                                .aria_label(shown.clone())
                                .gap_2()
                                .items_center()
                                .child(div().flex_none().size_2().rounded_full().bg(fill))
                                .child(shown.clone())
                        })
                        .on_click(move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.reopen_group(&group, window, cx))
                                .ok();
                        }),
                    )
                })
        })
        .into_any_element()
}

/// The top bar. With one pane it holds that pane's tabs.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let active = workspace.panes.active().active();
    let (can_back, can_forward) = (active.can_go_back(), active.can_go_forward());
    let lifted = workspace.lifted_tab(cx);
    let group_lift = workspace.group_lift(cx);
    let single = (workspace.panes.pane_count() == 1).then(|| workspace.panes.panes()[0]);
    let tab_list = single.map(|pane| {
        let row = workspace.tab_row(pane.id);
        tab_list(pane, &row, "Tabs".into(), true, lifted, group_lift, cx).ml_1()
    });
    let closed = crate::store::Settings::get(cx).closed_tab_groups.clone();
    let end_drop = single.map(|pane| pane.id);
    h_flex()
        .id("top-bar")
        .h(TOP_BAR_HEIGHT)
        .w_full()
        .flex_none()
        .pl(LEADING_INSET)
        .pr(px(8.))
        .gap_1()
        .items_center()
        .window_drag_area()
        .child(
            ui::icon_button("toggle-sidebar", IconName::PanelLeft, "Toggle sidebar (⌘B)")
                .small()
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx)),
        )
        .child(
            ui::icon_button("go-back", IconName::ChevronLeft, "Back (⌘[)")
                .small()
                .disabled(!can_back)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(GoBack), cx)),
        )
        .child(
            ui::icon_button("go-forward", IconName::ChevronRight, "Forward (⌘])")
                .small()
                .disabled(!can_forward)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(GoForward), cx)),
        )
        .children(tab_list)
        .child(new_tab_button(&closed, cx))
        // Apart from the tabs, at the bar's trailing edge. A tab dropped
        // here goes after the last one.
        .child(
            div()
                .id("top-bar-end")
                .flex_1()
                .h_full()
                .when_some(end_drop, |end, pane_id| {
                    end.on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                        this.drop_on_row(dragged, pane_id, None, cx)
                    }))
                    .on_drop(cx.listener(
                        move |this, dragged: &DraggedGroup, _, cx| {
                            this.drop_group(dragged, pane_id, None, cx)
                        },
                    ))
                }),
        )
        .child(crate::updater::render(cx))
        .into_any_element()
}

/// A pane's own row of tabs, over its page, once the window is split.
/// `number` counts the panes in reading order, from 1, for its name.
pub(super) fn pane_strip(
    pane: &Pane<AnyView>,
    row: &TabRow,
    number: usize,
    active: bool,
    lifted: Option<(PaneId, u64)>,
    group_lift: Option<GroupLift>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let pane_id = pane.id;
    let list = tab_list(
        pane,
        row,
        format!("Pane {number} tabs").into(),
        active,
        lifted,
        group_lift,
        cx,
    )
    .flex_1();
    h_flex()
        .id(("pane-strip", pane_id))
        .h(dp(STRIP_HEIGHT))
        .w_full()
        .flex_none()
        .items_center()
        .border_b_1()
        .border_color(tokens::border_subtle())
        .child(list)
        .into_any_element()
}
