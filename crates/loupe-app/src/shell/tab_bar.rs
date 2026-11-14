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
//! right-click menu renames, recolors, or ungroups it. A tab joins a group
//! from its own right-click menu, or when it is dropped on the group's name
//! or on one of its tabs; dropped on a tab outside every group, it leaves.
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
use std::cell::Cell;
use std::rc::Rc;

use super::Workspace;
use super::actions::{GoBack, GoForward, NewTab, ToggleSidebar};
use super::panes::{Pane, PaneId, Zone};
use super::tab_groups;
use super::tabs::TabGroup;
use crate::route::Route;
use crate::store::TabGroupColor;
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
const PINNED_TAB_MAX_WIDTH: f32 = 96.;
/// A pane's own row of tabs, once the window is split.
const STRIP_HEIGHT: f32 = 36.;

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

/// The sideways scroll of a pane's row of tabs, and the open tab and row
/// width it last scrolled for.
#[derive(Clone, Default)]
pub(super) struct TabRow {
    scroll: ScrollHandle,
    shown: Rc<Cell<Option<u64>>>,
    width: Rc<Cell<Pixels>>,
}

impl TabRow {
    /// Scrolls the row to the open tab of `pane` when that tab changed, or
    /// the row's width did (the window, the sidebar, or the update pill
    /// beside it), since it last did. Nothing happens before the row's first
    /// layout, when its width is unknown.
    /// `child` is the open tab's place among the row's children, which
    /// counts the groups' names and leaves out hidden tabs.
    fn follow_open_tab(&self, pane: &Pane<AnyView>, child: usize) {
        let open = pane.tabs.active().id;
        let bounds = self.scroll.bounds();
        if bounds == Bounds::default()
            || (self.shown.get() == Some(open) && self.width.get() == bounds.size.width)
        {
            return;
        }
        self.scroll.scroll_to_item(child);
        self.shown.set(Some(open));
        self.width.set(bounds.size.width);
    }

    /// Asks for another frame once the row was laid out at a new width: the
    /// scroll handle learns the width only after this frame's scroll ran.
    fn watch_width(
        &self,
        workspace: EntityId,
    ) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
        let row = self.clone();
        move |_, window, _| {
            if row.scroll.bounds().size.width != row.width.get() {
                window.on_next_frame(move |_, cx| cx.notify(workspace));
            }
        }
    }
}

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

/// The fill of a group's name and line: a color every theme already has.
pub(super) fn group_fill(color: TabGroupColor, cx: &App) -> Hsla {
    match color {
        TabGroupColor::Grey => tokens::draft_fill(),
        TabGroupColor::Blue => cx.theme().primary,
        TabGroupColor::Red => tokens::closed_fill(),
        TabGroupColor::Yellow => tokens::pending(),
        TabGroupColor::Green => tokens::open_fill(),
        TabGroupColor::Purple => tokens::merged_fill(),
    }
}

/// The name of group `group` of `pane`, before its first tab. `hidden` is
/// how many tabs it hides while collapsed.
fn group_label(
    pane: PaneId,
    group: &TabGroup,
    hidden: usize,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = group.id;
    let fill = group_fill(group.color, cx);
    let on_fill = tokens::on_accent();
    let ring = cx.theme().foreground;
    let name: SharedString = group.name.clone().into();
    let collapsed = group.collapsed;
    let color = group.color;
    let workspace = cx.entity().downgrade();
    let state = if collapsed { "collapsed" } else { "expanded" };
    h_flex()
        .id(("tab-group", id))
        .debug_selector(move || format!("tab-group-{pane}-{id}"))
        .role(Role::Button)
        .aria_label(format!("{name} group, {state}"))
        .aria_expanded(!collapsed)
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
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(
            cx.listener(move |this, _, _, cx| {
                this.edit_tabs(pane, |tabs| tabs.toggle_group(id), cx)
            }),
        )
        .drag_over::<DraggedTab>(move |style, _, _, _| style.border_color(ring))
        .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
            this.drop_on_group(dragged, pane, id, cx)
        }))
        .child(div().min_w_0().truncate().child(name.clone()))
        .when(collapsed, |label| {
            label.child(div().flex_none().opacity(0.8).child(hidden.to_string()))
        })
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
        })
        .into_any_element()
}

/// The tabs of `pane`, each group's name before its first tab, without the
/// tabs a collapsed group hides; and the open tab's place among them.
/// `emphasized` is false for the selected tab of a pane that is not the
/// active one, which then keeps its fill but not its color. `lifted` is the
/// pane and the tab being dragged, if any.
fn tabs(
    pane: &Pane<AnyView>,
    emphasized: bool,
    lifted: Option<(PaneId, u64)>,
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
    let workspace = cx.entity().downgrade();
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
            items.push(group_label(pane_id, group, hidden, cx));
        }
        previous_group = tab.group;
        if pane.tabs.is_hidden(index) {
            continue;
        }
        if index == active {
            open_child = items.len();
        }
        let line_color = group.map(|group| group_fill(group.color, cx));
        let element = {
            let route = &tab.current().route;
            let label: SharedString = route.label().into();
            let icon = route_icon(route);
            let selected = index == active;
            let pinned = tab.pinned;
            let id = tab.id;
            let group = SharedString::from(format!("tab-{id}"));
            // A tab dropped here takes this one's place: from the left in
            // its own row it lands to the right of it, otherwise to the left.
            let landing_line = lifted.filter(|(_, tab)| *tab != id).map(|_| {
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
                .when(lifted_index == Some(index), |tab| tab.opacity(0.5))
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
                .child(div().flex_1().min_w_0().truncate().child(label.clone()))
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
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let pane_id = pane.id;
    let (items, open_child) = tabs(pane, emphasized, lifted, cx);
    row.follow_open_tab(pane, open_child);
    h_flex()
        .on_children_prepainted(row.watch_width(cx.entity_id()))
        .id(("tabs", pane_id))
        .debug_selector(move || format!("tabs-{pane_id}"))
        .role(Role::TabList)
        .aria_label(label)
        .min_w_0()
        .overflow_x_scroll()
        .track_scroll(&row.scroll)
        // The row clips what it scrolls; the landing line before its first
        // tab and after its last needs this much room inside it.
        .px_1()
        .gap_1()
        .items_center()
        .children(items)
        .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
            this.drop_on_row(dragged, pane_id, None, cx)
        }))
}

/// The top bar. With one pane it holds that pane's tabs.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let active = workspace.panes.active().active();
    let (can_back, can_forward) = (active.can_go_back(), active.can_go_forward());
    let lifted = workspace.lifted_tab(cx);
    let single = (workspace.panes.pane_count() == 1).then(|| workspace.panes.panes()[0]);
    let tab_list = single.map(|pane| {
        let row = workspace.tab_row(pane.id);
        tab_list(pane, &row, "Tabs".into(), true, lifted, cx).ml_1()
    });
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
        .child(
            div().flex_none().debug_selector(|| "new-tab".into()).child(
                ui::icon_button("new-tab", IconName::Plus, "New tab (⌘T)")
                    .small()
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTab), cx)),
            ),
        )
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
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let pane_id = pane.id;
    let list = tab_list(
        pane,
        row,
        format!("Pane {number} tabs").into(),
        active,
        lifted,
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
