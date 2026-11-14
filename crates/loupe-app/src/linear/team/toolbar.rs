//! The team screen's chrome, after t3code's `KanbanViewTabs`,
//! `KanbanFilterBar`, and `KanbanDisplayMenu`: the page header; one toolbar
//! with the saved view tabs, the add-filter menu, search, and the display
//! menu; and, while filters are set, a row of editable chips.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_linear::filter::{FilterClause, SetField, SetOp};
use loupe_linear::views::{
    CompletedWindow, DisplayProperty, Grouping, Layout, Ordering, ViewConfig,
};
use loupe_linear::{IssueSummary, Team};

use super::filter::{self, DATE_PRESETS, FieldOption};
use super::{TeamView, dialogs};
use crate::linear::data::Loadable;
use crate::linear::views_store::{self, Kind};
use crate::linear::widgets;
use crate::shell::OpenOnClick as _;
use crate::theme::scale::dp;
use crate::ui;
use crate::ui::MenuTriggerExt as _;

type Edit = Rc<dyn Fn(&mut ViewConfig)>;
type FilterEdit = Rc<dyn Fn(&mut Vec<FilterClause>)>;

/// A menu entry that edits the filters on screen when picked: the draft's
/// on a view of the app, the local ones on a Linear view.
fn filter_item(
    label: impl Into<SharedString>,
    checked: bool,
    view: &WeakEntity<TeamView>,
    edit: impl Fn(&mut Vec<FilterClause>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    let edit: FilterEdit = Rc::new(edit);
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, _, cx| {
            let edit = edit.clone();
            view.update(cx, |this, cx| {
                this.edit_filters(|clauses| edit(clauses), cx)
            })
            .ok();
        })
}

/// A menu entry that edits the draft when picked.
fn edit_item(
    label: impl Into<SharedString>,
    checked: bool,
    view: &WeakEntity<TeamView>,
    edit: impl Fn(&mut ViewConfig) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    let edit: Edit = Rc::new(edit);
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, _, cx| {
            let edit = edit.clone();
            view.update(cx, |this, cx| this.edit_draft(|draft| edit(draft), cx))
                .ok();
        })
}

/// The values of one set field as checkable entries that toggle each value.
fn set_items(
    menu: PopupMenu,
    field: SetField,
    options: &[FieldOption],
    selected: &[String],
    view: &WeakEntity<TeamView>,
) -> PopupMenu {
    options.iter().fold(menu, |menu, option| {
        let value = option.value.clone();
        menu.item(filter_item(
            option.label.clone(),
            selected.contains(&option.value),
            view,
            move |clauses| filter::toggle_value(clauses, field, &value),
        ))
    })
}

/// Everything the filter menus need, computed once per render.
pub(super) struct FilterOptions {
    by_field: Vec<(SetField, Vec<FieldOption>)>,
}

impl FilterOptions {
    pub(super) fn new(team: &Team, issues: &[IssueSummary]) -> Self {
        Self {
            by_field: SetField::ALL
                .iter()
                .map(|field| (*field, filter::options(*field, team, issues)))
                .collect(),
        }
    }

    #[cfg(test)]
    pub(super) fn from_fields(by_field: Vec<(SetField, Vec<FieldOption>)>) -> Self {
        Self { by_field }
    }

    pub(super) fn of(&self, field: SetField) -> &[FieldOption] {
        self.by_field
            .iter()
            .find(|(known, _)| *known == field)
            .map(|(_, options)| options.as_slice())
            .unwrap_or_default()
    }
}

/// What the lock beside the title of a view says, after the reason its
/// filter cannot be read here.
const LOCKED_VIEW: &str = "The filter of this view stays as Linear has it and is edited there. Filter narrows the view in Loupe, without changing it.";

/// Saved views as `(id, what the caller shows for it)`.
type Views<T> = Vec<(String, T)>;

/// Views shown in the tab row, at most, before the rest go under "More".
const PINNED_VIEWS: usize = 6;
/// What a view tab adds to its label's width: 12 px on each side and the
/// 4 px to the next tab.
const TAB_CHROME: f32 = 28.;
/// The room "More" and "Save as new view" take in the row, with their gaps.
const MORE_WIDTH: f32 = 82.;
const SAVE_AS_WIDTH: f32 = 28.;

/// How many views the row shows when it is `available` px wide: as many of
/// the first [`PINNED_VIEWS`] as fit with what follows them ("More" when
/// some are left out, `trailing` for "Save as new view"), and one at least.
/// `width` is a tab's width for its label. The open view counts with the
/// width it has in the place it takes (see [`pinned_views`]).
fn views_that_fit(
    views: &[(String, SharedString)],
    active: Option<&str>,
    available: f32,
    trailing: f32,
    width: impl Fn(&str) -> f32,
) -> usize {
    let most = views.len().min(PINNED_VIEWS);
    (1..=most)
        .rev()
        .find(|limit| {
            let (pinned, overflow) = pinned_views(views, active, *limit);
            let tabs: f32 = pinned.iter().map(|(_, name)| width(name)).sum();
            let more = if overflow.is_empty() { 0. } else { MORE_WIDTH };
            tabs + more + trailing <= available
        })
        .unwrap_or(most.min(1))
}

/// `views` as the ones in the row and the ones under "More": the first
/// `limit`, with the active view in the last place when it is not among them.
fn pinned_views<T: Clone>(
    views: &[(String, T)],
    active: Option<&str>,
    limit: usize,
) -> (Views<T>, Views<T>) {
    if views.len() <= limit {
        return (views.to_vec(), Vec::new());
    }
    let mut pinned: Vec<_> = views[..limit].to_vec();
    let mut overflow: Vec<_> = views[limit..].to_vec();
    if let Some(position) = overflow
        .iter()
        .position(|(id, _)| Some(id.as_str()) == active)
        && let Some(last) = pinned.last_mut()
    {
        std::mem::swap(last, &mut overflow[position]);
    }
    (pinned, overflow)
}

impl TeamView {
    /// Linear's breadcrumb: the team, then the view on screen with its star
    /// and view menu; Refresh and New issue on the right.
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let team_ready = matches!(self.team, Loadable::Ready(_));
        // Linear keeps the favorites of its own views.
        let remote_id = self
            .page
            .view_id
            .as_deref()
            .filter(|id| views_store::kind(id) != Kind::Legacy);
        let starred = if let Some(id) = remote_id {
            crate::linear::remote_favorites(cx)
                .iter()
                .any(|favorite| crate::linear::favorite_view_id(favorite).as_deref() == Some(id))
        } else {
            crate::store::Settings::get(cx).is_favorite(&self.favorite())
        };
        let star_label = if remote_id.is_some() {
            "Manage favorites in Linear"
        } else if starred {
            "Remove from favorites"
        } else {
            "Add to favorites"
        };
        let count = match &self.issues {
            Loadable::Ready(listed) => Some(self.visible_issues(listed).len()),
            _ => None,
        };
        let star = Button::new("team-favorite")
            .ghost()
            .xsmall()
            .icon(if starred {
                IconName::StarFill
            } else {
                IconName::Star
            })
            .tooltip(star_label)
            .accessibility_label(star_label)
            .disabled(remote_id.is_some())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx)));
        let refresh = Button::new("team-refresh")
            .ghost()
            .small()
            .icon(IconName::RefreshCw)
            .tooltip("Refresh")
            .accessibility_label("Refresh")
            .on_click(cx.listener(|this, _, _, cx| this.reload_all(cx)));
        let new_issue = Button::new("team-new-issue")
            .ghost()
            .small()
            .icon(IconName::Plus)
            .label("New issue")
            .disabled(!team_ready)
            .on_click(cx.listener(|this, _, window, cx| this.open_new_issue(window, cx)));
        let view_name = self
            .page
            .view_id
            .as_deref()
            .and_then(|id| self.views(cx).get(id).map(|saved| saved.name.clone()));
        let header = match view_name {
            Some(view_name) => {
                let route = crate::route::Route::LinearTeam {
                    team_id: self.team_id.clone(),
                    name: self.name.to_string(),
                    view_id: None,
                };
                let open_team = move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                    crate::shell::open(route.clone(), event, window, cx)
                };
                let crumb = h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("team-crumb")
                            .ghost()
                            .xsmall()
                            .label(self.name.clone())
                            .accessibility_label(format!("Team {}", self.name))
                            .on_click(open_team.clone())
                            .on_middle_click(open_team),
                    )
                    .child(div().text_color(cx.theme().muted_foreground).child("›"));
                ui::page_header(view_name).prefix(crumb)
            }
            None => ui::page_header(self.name.clone()),
        };
        let header = header.icon(gpui_kit::assets::IconName::Users);
        let header = match count {
            Some(count) => header.count(count),
            None => header,
        };
        // A lock beside the title says the view's filter stays Linear's;
        // the tooltip says why and what that means. It is a mark, not a
        // control: filtering is the Filter button's job.
        let lock = self.locked.clone().map(|reason| {
            let text: SharedString = format!("{reason} {LOCKED_VIEW}").into();
            let tooltip = text.clone();
            div()
                .id("view-read-only")
                .role(Role::Image)
                .aria_label(text)
                .flex()
                .items_center()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Lock)
                        .size(dp(14.))
                        .text_color(crate::theme::tokens::text_faint()),
                )
        });
        header
            .after_title(star)
            .children_after_title(lock)
            .after_title(self.render_view_menu(cx))
            .action(refresh)
            .action(new_issue)
            .into_any_element()
    }

    /// The "…" menu of the view on screen: rename and delete, which change
    /// the view in Linear. A view still only in Loupe can also be saved to
    /// Linear and moved among its kind. A team's built-in view has no menu.
    fn render_view_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.view_kind();
        if !matches!(kind, Some(Kind::Custom | Kind::Legacy)) {
            return div().into_any_element();
        }
        let legacy = kind == Some(Kind::Legacy);
        let saving = self.saving;
        let views = self.views(cx);
        let active = self.page.view_id.clone();
        let view = cx.weak_entity();
        let menu_view = view.clone();
        let active_for_menu = active.clone();
        let active_name = active
            .as_deref()
            .and_then(|id| views.get(id))
            .map(|saved| saved.name.clone())
            .unwrap_or_default();
        let view_menu = Button::new("view-menu")
            .ghost()
            .small()
            .icon(IconName::Ellipsis)
            .tooltip("View actions")
            .accessibility_label("View actions")
            .menu_trigger()
            .dropdown_menu(move |menu, _, _| {
                let id = active_for_menu.clone();
                let name = active_name.clone();
                let rename = {
                    let (view, id, name) = (menu_view.clone(), id.clone(), name.clone());
                    PopupMenuItem::new("Rename view…")
                        .disabled(id.is_none() || saving)
                        .on_click(move |_, window, cx| {
                            if let Some(id) = id.clone() {
                                dialogs::rename(view.clone(), id, name.clone(), window, cx);
                            }
                        })
                };
                let step = |label: &'static str, step: i32| {
                    let (view, id) = (menu_view.clone(), id.clone());
                    PopupMenuItem::new(label)
                        .disabled(id.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(id) = id.clone() {
                                view.update(cx, |this, cx| this.move_view(&id, step, cx))
                                    .ok();
                            }
                        })
                };
                let delete = {
                    let (view, id, name) = (menu_view.clone(), id.clone(), name.clone());
                    PopupMenuItem::new("Delete view…")
                        .icon(IconName::Delete)
                        .disabled(id.is_none() || saving)
                        .on_click(move |_, window, cx| {
                            if let Some(id) = id.clone() {
                                dialogs::confirm_delete(view.clone(), id, name.clone(), window, cx);
                            }
                        })
                };
                let to_linear = {
                    let (view, id) = (menu_view.clone(), id.clone());
                    PopupMenuItem::new("Save to Linear")
                        .icon(IconName::ArrowUp)
                        .disabled(id.is_none() || saving)
                        .on_click(move |_, _, cx| {
                            if let Some(id) = id.clone() {
                                view.update(cx, |this, cx| this.move_to_linear(&id, cx))
                                    .ok();
                            }
                        })
                };
                let menu = menu.min_w(dp(200.));
                let menu = if legacy {
                    menu.item(to_linear)
                        .item(PopupMenuItem::separator())
                        .item(rename)
                        .item(step("Move left", -1))
                        .item(step("Move right", 1))
                } else {
                    menu.item(rename)
                };
                menu.item(PopupMenuItem::separator()).item(delete)
            });

        view_menu.into_any_element()
    }

    /// The one toolbar of the screen: the views of this team as tabs, then
    /// Filter, search, and Display at its trailing edge.
    pub(super) fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let views = self.views(cx);
        let active = self.page.view_id.clone();
        // A locked view cannot be copied: Loupe cannot read its filter.
        let can_save_as = self.locked.is_none();
        let view = cx.weak_entity();
        // Up to six views sit in the row, as many as its width holds; the
        // rest go under "More". The open view is always in the row, taking
        // the last place when it is further down.
        // A project's issues have no saved views of their own.
        let all: Vec<(String, SharedString)> = if self.is_project() {
            Vec::new()
        } else {
            views
                .for_team(&self.team_id)
                .map(|saved| (saved.id.clone(), SharedString::from(saved.name.clone())))
                .collect()
        };
        // The row's width is the one it was laid out with last; when this
        // frame lays it out with another (a resized window, a folded
        // sidebar), the next frame counts again.
        let strip = self.tabs_strip.bounds().size.width;
        cx.on_next_frame(window, move |this, _, cx| {
            if this.tabs_strip.bounds().size.width != strip {
                cx.notify();
            }
        });
        let limit = if strip > px(0.) {
            let zoom = f32::from(dp(1.));
            let family = cx.theme().font_family.clone();
            let trailing = if can_save_as { SAVE_AS_WIDTH } else { 0. };
            views_that_fit(
                &all,
                active.as_deref(),
                f32::from(strip) / zoom,
                trailing,
                |name| ui::text_width(name, 14., &family, cx) + TAB_CHROME,
            )
        } else {
            PINNED_VIEWS
        };
        let (pinned, overflow) = pinned_views(&all, active.as_deref(), limit);
        let tabs: Vec<AnyElement> = pinned
            .into_iter()
            .enumerate()
            .map(|(ix, (id, name))| {
                let selected = active.as_deref() == Some(id.as_str());
                // Only the open view is in the foreground color, so the row
                // of views reads as one selection among quiet alternatives.
                ui::view_tab(("view-tab", ix), name, selected, cx)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.switch_view(&id, window, cx)),
                    )
                    .into_any_element()
            })
            .collect();
        let more = (!overflow.is_empty()).then(|| {
            let view = view.clone();
            ui::view_tab("view-more", "More", false, cx)
                .dropdown_caret(true)
                .accessibility_label(format!("More views, {}", overflow.len()))
                .menu_trigger()
                .dropdown_menu(move |menu, _, _| {
                    overflow
                        .iter()
                        .fold(menu.min_w(dp(200.)), |menu, (id, name)| {
                            let (id, view) = (id.clone(), view.clone());
                            menu.item(PopupMenuItem::new(name.clone()).on_click(
                                move |_, window, cx| {
                                    view.update(cx, |this, cx| this.switch_view(&id, window, cx))
                                        .ok();
                                },
                            ))
                        })
                })
        });

        let save_as = {
            let view = view.clone();
            Button::new("view-save-as")
                .ghost()
                .small()
                .icon(IconName::Plus)
                .tooltip("Save as new view in Linear…")
                .accessibility_label("Save as new view…")
                .disabled(self.saving)
                .on_click(move |_, window, cx| dialogs::save_as(view.clone(), window, cx))
        };

        let filter_button = self.render_filter_button(cx);
        h_flex()
            .h(dp(widgets::TOOLBAR_HEIGHT))
            .flex_none()
            .pl_1()
            .pr(crate::theme::scale::content_inset())
            .gap_2()
            .border_b_1()
            .border_color(crate::theme::tokens::border_subtle())
            // The views take what the controls beside them leave, and the
            // row shows as many as that holds (`views_that_fit`).
            .child(
                h_flex()
                    .id("view-tabs")
                    .track_scroll(&self.tabs_strip)
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_hidden()
                    .children(tabs)
                    .children(more)
                    .when(can_save_as && !self.is_project(), |row| row.child(save_as)),
            )
            .child(filter_button)
            .child(
                Input::new(&self.search)
                    .small()
                    .flex_none()
                    .w(dp(200.))
                    .cleanable(true)
                    .prefix(IconName::Search),
            )
            .child(display_button(self.page.draft.clone(), &view))
            .into_any_element()
    }

    /// The filters in force as chips, each with its own remove button, then
    /// Clear; at the trailing edge, what to do with a draft that differs
    /// from its saved view. `None` while there is nothing of either: the
    /// screen then has one toolbar.
    pub(super) fn render_filter_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = cx.weak_entity();
        let listed: &[IssueSummary] = match &self.issues {
            Loadable::Ready(listed) => listed,
            _ => &[],
        };
        let chips: Vec<AnyElement> = match &self.team {
            Loadable::Ready(team) => {
                let options = Rc::new(FilterOptions::new(team, listed));
                self.filters()
                    .iter()
                    .enumerate()
                    .filter(|(_, clause)| !matches!(clause, FilterClause::Text { .. }))
                    .map(|(ix, clause)| {
                        chip(
                            ix,
                            clause.clone(),
                            filter::describe(clause, team, listed),
                            options.clone(),
                            &view,
                        )
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let dirty = !self.is_project() && self.page.is_dirty(&self.views(cx));
        if chips.is_empty() && !dirty {
            return None;
        }
        // A team's built-in view is not saved over: its changes become a
        // new view.
        let saves_over = matches!(self.view_kind(), Some(Kind::Custom | Kind::Legacy));
        let saving = self.saving;
        let save_as_view = cx.weak_entity();
        let has_chips = !chips.is_empty();
        let muted = cx.theme().muted_foreground;
        Some(
            h_flex()
                .h(dp(widgets::TOOLBAR_HEIGHT))
                .flex_none()
                .px(crate::theme::scale::content_inset())
                .gap_1p5()
                .border_b_1()
                .border_color(crate::theme::tokens::border_subtle())
                .overflow_hidden()
                .children(chips)
                .when(has_chips, |row| {
                    row.child(
                        Button::new("filters-clear")
                            .ghost()
                            .small()
                            .label("Clear")
                            .accessibility_label("Clear filters")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.local_filters.clear();
                                this.edit_filters(Vec::clear, cx);
                                this.sync_search_box(window, cx);
                            })),
                    )
                })
                .child(div().flex_1())
                .when(dirty, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .mr_1()
                            .child("Unsaved changes"),
                    )
                    .child(
                        Button::new("view-discard")
                            .ghost()
                            .small()
                            .label("Discard")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.discard_draft(window, cx)),
                            ),
                    )
                    .when(saves_over, |row| {
                        row.child(
                            Button::new("view-save")
                                .small()
                                .label("Save")
                                .loading(saving)
                                .disabled(saving)
                                .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
                        )
                    })
                    .when(!saves_over && self.locked.is_none(), |row| {
                        row.child(
                            Button::new("view-save-as-new")
                                .small()
                                .label("Save as new view…")
                                .disabled(saving)
                                .on_click(move |_, window, cx| {
                                    dialogs::save_as(save_as_view.clone(), window, cx)
                                }),
                        )
                    })
                })
                .into_any_element(),
        )
    }

    /// The Filter button and its "Add Filter…" popover, controlled by
    /// `filter_open` so a click from an accessibility client or F opens it.
    fn render_filter_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let menu = self.filter_menu.clone();
        let on_change = cx.weak_entity();
        let on_click = cx.weak_entity();
        Popover::new("filter-popover")
            // The button is toward the trailing edge: the menu's trailing
            // edge sits under it.
            .anchor(Anchor::TopRight)
            .p_1()
            .w(dp(280.))
            .open(self.filter_open)
            .on_open_change(move |open, window, cx| {
                on_change
                    .update(cx, |this, cx| this.set_filter_open(*open, window, cx))
                    .ok();
            })
            .trigger(
                Button::new("filter-open")
                    .outline()
                    .small()
                    .icon(gpui_kit::assets::IconName::ListFilter)
                    .label("Filter")
                    .selected(self.filter_open)
                    .tooltip(if self.locked.is_some() {
                        "Narrow this view in Loupe by assignee, status, priority, labels, and more (F)"
                    } else {
                        "Filter by assignee, status, priority, labels, and more (F)"
                    })
                    .accessibility_label("Filter")
                    .on_click(move |_, window, cx| {
                        on_click
                            .update(cx, |this, cx| this.set_filter_open(true, window, cx))
                            .ok();
                    }),
            )
            .content(move |_, _, _| menu.clone())
            .into_any_element()
    }

    pub(super) fn render_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let created = self.created.as_ref().map(|issue| {
            let identifier = issue.identifier.clone();
            h_flex()
                .h(dp(widgets::TOOLBAR_HEIGHT))
                .px_5()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("Created {} · {}", issue.identifier, issue.title)),
                )
                .child(
                    Button::new("created-open")
                        .ghost()
                        .small()
                        .label("Open")
                        .on_click(move |_, window, cx| {
                            crate::shell::navigate(
                                crate::route::Route::LinearIssue {
                                    identifier: identifier.clone(),
                                },
                                window,
                                cx,
                            );
                        }),
                )
                .child(
                    Button::new("created-dismiss")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .tooltip("Dismiss")
                        .accessibility_label("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.created = None;
                            cx.notify();
                        })),
                )
        });
        let notice = self.notice.as_ref().map(|notice| {
            let color = if notice.is_error {
                theme.danger
            } else {
                theme.muted_foreground
            };
            h_flex()
                .id("view-notice")
                .role(Role::Alert)
                .aria_label(notice.text.clone())
                .min_h(dp(widgets::TOOLBAR_HEIGHT))
                .px_5()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(color)
                        .child(notice.text.clone()),
                )
                .child(
                    Button::new("notice-dismiss")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .tooltip("Dismiss")
                        .accessibility_label("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
        });
        if created.is_none() && notice.is_none() {
            return None;
        }
        Some(
            gpui_kit::component::v_flex()
                .flex_none()
                .children(notice)
                .children(created)
                .into_any_element(),
        )
    }
}

/// One clause as an outlined chip in two halves: the text with a caret,
/// whose menu edits the clause, and an × that removes it.
fn chip(
    ix: usize,
    clause: FilterClause,
    text: String,
    options: Rc<FilterOptions>,
    view: &WeakEntity<TeamView>,
) -> AnyElement {
    let view = view.clone();
    let remove_clause = move |clauses: &mut Vec<FilterClause>| {
        if ix < clauses.len() {
            clauses.remove(ix);
        }
    };
    let remove = {
        let view = view.clone();
        Button::new(("filter-chip-remove", ix))
            .outline()
            .small()
            .w_6()
            .px_0()
            .rounded_l_none()
            // The halves share one border.
            .ml(px(-1.))
            .icon(IconName::Close)
            .tooltip("Remove filter")
            .accessibility_label(format!("Remove filter: {text}"))
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.edit_filters(remove_clause, cx))
                    .ok();
            })
    };
    let edit = Button::new(("filter-chip", ix))
        .outline()
        .small()
        .rounded_r_none()
        .label(text)
        .dropdown_caret(true)
        .menu_trigger()
        .dropdown_menu(move |menu, _, _| {
            let remove =
                filter_item("Remove filter", false, &view, remove_clause).icon(IconName::Close);
            let menu = menu.min_w(dp(200.)).max_h(dp(360.)).scrollable(true);
            let menu = match &clause {
                FilterClause::Set { field, op, values } => {
                    let flip = |label: &'static str, wanted: SetOp| {
                        filter_item(label, *op == wanted, &view, move |clauses| {
                            if let Some(FilterClause::Set { op, .. }) = clauses.get(ix)
                                && *op != wanted
                            {
                                filter::flip_op(clauses, ix);
                            }
                        })
                    };
                    let menu = menu
                        .item(flip("is", SetOp::In))
                        .item(flip("is not", SetOp::NotIn))
                        .item(PopupMenuItem::separator());
                    set_items(menu, *field, options.of(*field), values, &view)
                }
                FilterClause::Date { field, op } => {
                    DATE_PRESETS.iter().fold(menu, |menu, (preset, label)| {
                        let (field, preset) = (*field, *preset);
                        menu.item(filter_item(*label, *op == preset, &view, move |clauses| {
                            filter::set_date(clauses, field, preset)
                        }))
                    })
                }
                FilterClause::Flag { field, value } => {
                    let field = *field;
                    menu.item(filter_item("Yes", *value, &view, move |clauses| {
                        filter::set_flag(clauses, field, true)
                    }))
                    .item(filter_item(
                        "No",
                        !*value,
                        &view,
                        move |clauses| filter::set_flag(clauses, field, false),
                    ))
                }
                FilterClause::Text { .. } => menu,
            };
            menu.item(PopupMenuItem::separator()).item(remove)
        });
    h_flex()
        .flex_none()
        .child(edit)
        .child(remove)
        .into_any_element()
}

/// List or Board, read live from the view so the switch follows each click
/// while the Display menu stays open.
fn layout_switch(view: &WeakEntity<TeamView>, cx: &App) -> AnyElement {
    const LAYOUTS: [(Layout, gpui_kit::assets::IconName, &str); 2] = [
        (Layout::List, gpui_kit::assets::IconName::LayoutList, "List"),
        (
            Layout::Board,
            gpui_kit::assets::IconName::SquareKanban,
            "Board",
        ),
    ];
    let current = view
        .upgrade()
        .map(|view| view.read(cx).page.draft.layout)
        .unwrap_or_default();
    let selected = LAYOUTS
        .iter()
        .position(|(layout, _, _)| *layout == current)
        .unwrap_or_default();
    let view = view.clone();
    h_flex()
        .w_full()
        .py_1()
        .child(ui::segmented(
            "display-layout",
            "Layout",
            LAYOUTS.map(|(_, icon, label)| (Some(icon), label)),
            selected,
            move |index, _, cx| {
                let Some((layout, _, _)) = LAYOUTS.get(index).copied() else {
                    return;
                };
                view.update(cx, |this, cx| {
                    this.edit_draft(|config| config.layout = layout, cx)
                })
                .ok();
            },
        ))
        .into_any_element()
}

/// "Display": layout, grouping, ordering, closed issues, empty columns, and
/// which properties cards and rows show.
fn display_button(draft: ViewConfig, view: &WeakEntity<TeamView>) -> AnyElement {
    let view = view.clone();
    let draft = Rc::new(draft);
    Button::new("display-menu")
        .ghost()
        .small()
        .icon(gpui_kit::assets::IconName::SlidersHorizontal)
        .tooltip("Display")
        .accessibility_label("Display")
        .menu_trigger()
        .dropdown_menu(move |menu, window, cx| {
            let layout_view = view.clone();
            let (group_view, group_draft) = (view.clone(), draft.clone());
            let (order_view, order_draft) = (view.clone(), draft.clone());
            let (closed_view, closed_draft) = (view.clone(), draft.clone());
            let (props_view, props_draft) = (view.clone(), draft.clone());
            let show_empty = draft.show_empty_columns;
            let hidden = draft.hidden_groups.len();
            let reordered = !draft.column_order.is_empty();
            menu.min_w(dp(220.))
                .item(PopupMenuItem::element(move |_, cx| {
                    layout_switch(&layout_view, cx)
                }))
                .item(PopupMenuItem::separator())
                .submenu("Group by", window, cx, move |menu, _, _| {
                    Grouping::ALL.iter().fold(menu, |menu, grouping| {
                        let grouping = *grouping;
                        menu.item(edit_item(
                            grouping.label(),
                            group_draft.group_by == grouping,
                            &group_view,
                            move |config| config.group_by = grouping,
                        ))
                    })
                })
                .submenu("Order by", window, cx, move |menu, _, _| {
                    Ordering::ALL.iter().fold(menu, |menu, order| {
                        let order = *order;
                        menu.item(edit_item(
                            order.label(),
                            order_draft.order_by == order,
                            &order_view,
                            move |config| config.order_by = order,
                        ))
                    })
                })
                .submenu("Closed issues", window, cx, move |menu, _, _| {
                    CompletedWindow::ALL
                        .iter()
                        .fold(menu, |menu, window_choice| {
                            let choice = *window_choice;
                            menu.item(edit_item(
                                choice.label(),
                                closed_draft.completed == choice,
                                &closed_view,
                                move |config| config.completed = choice,
                            ))
                        })
                })
                .item(edit_item(
                    "Show empty columns",
                    show_empty,
                    &view,
                    move |config| config.show_empty_columns = !show_empty,
                ))
                .when(hidden > 0, |menu| {
                    menu.item(edit_item(
                        format!("Show hidden columns ({hidden})"),
                        false,
                        &view,
                        |config| config.hidden_groups.clear(),
                    ))
                })
                .when(reordered, |menu| {
                    menu.item(edit_item("Reset column order", false, &view, |config| {
                        config.column_order.clear()
                    }))
                })
                .item(PopupMenuItem::separator())
                .submenu("Properties", window, cx, move |menu, _, _| {
                    DisplayProperty::ALL.iter().fold(menu, |menu, property| {
                        let property = *property;
                        menu.item(edit_item(
                            property.label(),
                            props_draft.shows(property),
                            &props_view,
                            move |config| match config
                                .properties
                                .iter()
                                .position(|p| *p == property)
                            {
                                Some(index) => {
                                    config.properties.remove(index);
                                }
                                None => config.properties.push(property),
                            },
                        ))
                    })
                })
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::SharedString;

    use super::{pinned_views, views_that_fit};

    fn views(count: usize) -> Vec<(String, usize)> {
        (0..count).map(|ix| (format!("v{ix}"), ix)).collect()
    }

    fn ids(views: &[(String, usize)]) -> Vec<&str> {
        views.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[test]
    fn the_row_shows_as_many_views_as_its_width_holds() {
        let named: Vec<(String, SharedString)> = (0..8)
            .map(|ix| (format!("v{ix}"), SharedString::from(format!("View {ix}"))))
            .collect();
        // Every tab 100 px wide; "More" takes 82 and "Save as" 28.
        let fit = |available: f32, active: Option<&str>| {
            views_that_fit(&named, active, available, 28., |_| 100.)
        };
        assert_eq!(fit(2000., None), 6, "six at most");
        assert_eq!(fit(710., None), 6);
        assert_eq!(fit(709., None), 5);
        assert_eq!(fit(411., None), 3);
        assert_eq!(fit(100., None), 1, "one at least");
        // With every view in the row there is no "More" to make room for.
        let few = &named[..3];
        assert_eq!(views_that_fit(few, None, 328., 28., |_| 100.), 3);
        assert_eq!(views_that_fit(few, None, 327., 28., |_| 100.), 2);
        // The open view counts with its own width in the place it takes.
        let wide_open = |name: &str| if name == "View 7" { 300. } else { 100. };
        assert_eq!(views_that_fit(&named, Some("v7"), 710., 28., wide_open), 4);
    }

    #[test]
    fn the_row_holds_the_first_views_and_always_the_open_one() {
        let (pinned, overflow) = pinned_views(&views(3), Some("v1"), 6);
        assert_eq!(ids(&pinned), ["v0", "v1", "v2"]);
        assert!(overflow.is_empty());

        let (pinned, overflow) = pinned_views(&views(8), Some("v2"), 6);
        assert_eq!(ids(&pinned), ["v0", "v1", "v2", "v3", "v4", "v5"]);
        assert_eq!(ids(&overflow), ["v6", "v7"]);

        // The open view takes the last place; the view it displaces goes
        // under "More" in its stead.
        let (pinned, overflow) = pinned_views(&views(8), Some("v7"), 6);
        assert_eq!(ids(&pinned), ["v0", "v1", "v2", "v3", "v4", "v7"]);
        assert_eq!(ids(&overflow), ["v6", "v5"]);
    }
}
