//! Drawing the files tree like Zed's project panel: dense rows with indent
//! guides, chevrons, file-type icons and git-tinted names, a three-state
//! viewed box, and folders pinned at the top while the list scrolls.
//!
//! Zed's panel code is GPL and was read for behavior only; this is Loupe's
//! own implementation on GPUI's `uniform_list`.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::header::{ELLIPSIS, cut_name, text_width};
use super::model::{Missing, is_viewed, split_path, status_letter};
use super::tree::{
    Facet, GitTint, LOW_SIGNAL, NOT_COVERED, RowKind, TreeRow, ViewedMark, active_folder,
    ancestors, distinct_starts, file_tint, folder_tint, is_group, split_group, step_of,
    sticky_folders, viewed_mark,
};
use super::{FilesTab, LOW_SIGNAL_HINT, LOW_SIGNAL_ICON, tint_color};
use crate::theme::scale::{self, dp};
use crate::theme::tokens;
use crate::ui::MenuTriggerExt as _;
use crate::{assets, ui};

/// Zed's dense project panel row.
pub const ROW_HEIGHT: f32 = 22.;
/// Zed's default `project_panel.indent_size`, for the first levels.
const INDENT: f32 = 20.;
/// How many levels take the full indent.
const FULL_INDENT_LEVELS: usize = 3;
/// The indent of each level past those, so a file deep in a large pull
/// request keeps the room for its whole name.
const DEEP_INDENT: f32 = 10.;
/// The row's inner padding before the first indent level.
const ROW_PADDING: f32 = 8.;
const ICON_SIZE: f32 = 16.;

/// Everything one row draws, computed before the list asks for it.
#[derive(Clone)]
pub struct RowView {
    pub path: SharedString,
    pub depth: usize,
    /// `Some(expanded)` for a folder.
    pub folder: Option<bool>,
    pub label: SharedString,
    /// Where a file's name stops sharing its start with a sibling's
    /// (`tree::distinct_starts`).
    pub name_from: usize,
    pub label_color: Hsla,
    pub icon: &'static str,
    /// The icon's color: muted, or the state color of a folder whose files
    /// are all new or all deleted.
    pub icon_color: Hsla,
    pub additions: u32,
    pub deletions: u32,
    /// How many files a folder holds; 0 for a file.
    pub files: usize,
    /// A file's status letter and its color.
    pub status: Option<(&'static str, Hsla)>,
    pub viewed: ViewedMark,
    pub selected: bool,
    pub focused: bool,
    /// One entry per indent level: whether that level's guide belongs to
    /// the folder holding the selection.
    pub guides: Vec<bool>,
    pub tooltip: SharedString,
    pub accessible_name: SharedString,
    /// Open review threads on the file, or on the files under the folder.
    pub threads: usize,
    /// A group of the Guide order: a step's (its number, from 1) or
    /// [`NOT_COVERED`]'s (`Some(None)`), whose label is a title, not a path.
    pub guide_group: Option<Option<usize>>,
}

/// One tree row. `sticky` rows are the pinned copies drawn over the list.
pub fn row(
    view: &RowView,
    ix: usize,
    sticky: bool,
    tab: &WeakEntity<FilesTab>,
    cx: &App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let id_prefix = if sticky { "tree-sticky" } else { "tree-row" };
    let click_path = view.path.clone();
    let viewed_path = view.path.clone();
    let click_tab = tab.clone();
    let viewed_tab = tab.clone();
    let tooltip = view.tooltip.clone();
    let folder_row = view.folder.is_some();
    let badge_id = format!("{id_prefix}-threads");
    let threads = (view.threads > 0).then(|| (badge_id.clone(), ix, view.threads));
    let background = if view.selected {
        tokens::glass_selected()
    } else if sticky {
        tokens::surface()
    } else {
        transparent_black()
    };
    h_flex()
        .id(ElementId::NamedInteger(id_prefix.into(), ix as u64))
        .relative()
        .flex_none()
        .w_full()
        .h(dp(ROW_HEIGHT))
        .pr(dp(LANE_GAP))
        .gap(dp(LANE_GAP))
        .items_center()
        .bg(background)
        .border_1()
        .border_color(if view.selected && view.focused {
            tokens::accent()
        } else {
            transparent_black()
        })
        .when(!view.selected, |row| {
            row.hover(|style| style.bg(tokens::glass_hover()))
        })
        .children(view.guides.iter().enumerate().map(|(level, active)| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(dp(ROW_PADDING + indent(level) + ICON_SIZE / 2. - 1.))
                .w(px(1.))
                .bg(if *active {
                    tokens::indent_guide_active()
                } else {
                    tokens::indent_guide()
                })
        }))
        .child(
            h_flex()
                .id(ElementId::NamedInteger(
                    format!("{id_prefix}-item").into(),
                    ix as u64,
                ))
                .role(Role::TreeItem)
                .aria_label(view.accessible_name.clone())
                .aria_selected(view.selected)
                .when(view.selected && view.focused, |item| {
                    item.aria_active_descendant()
                })
                .when_some(view.folder, |item, expanded| item.aria_expanded(expanded))
                .flex_1()
                .min_w_0()
                .h_full()
                .pl(dp(ROW_PADDING + indent(view.depth) - 1.))
                .gap(dp(2.))
                .items_center()
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(move |_, window, cx| {
                    click_tab
                        .update(cx, |tab, cx| {
                            tab.click_tree_row(&click_path, sticky, window, cx)
                        })
                        .ok();
                })
                .child(match view.folder {
                    Some(expanded) => icon(
                        if expanded {
                            assets::CHEVRON_DOWN
                        } else {
                            assets::CHEVRON_RIGHT
                        },
                        muted,
                    )
                    .into_any_element(),
                    None => div().flex_none().size(dp(ICON_SIZE)).into_any_element(),
                })
                .child(icon(view.icon, view.icon_color))
                .child(if let Some(step) = view.guide_group {
                    guide_group_label(view, step, muted).into_any_element()
                } else if folder_row {
                    folder_label(view, muted, threads).into_any_element()
                } else {
                    name_and_size(
                        view.label.clone(),
                        view.name_from,
                        view.additions,
                        view.deletions,
                        threads,
                    )
                    .text_color(view.label_color)
                    .into_any_element()
                })
                .when(view.guide_group.is_some() && view.threads > 0, |item| {
                    item.child(thread_badge(&badge_id, ix, view.threads).ml(dp(LANE_GAP)))
                })
                .child(status_lane(view.status).ml(dp(LANE_GAP))),
        )
        .child(viewed_box(
            ElementId::NamedInteger(format!("{id_prefix}-viewed").into(), ix as u64),
            view.viewed,
            if view.folder.is_some() {
                format!("Viewed folder {}", view.path)
            } else {
                format!("Viewed {}", view.path)
            },
            move |_, cx| {
                viewed_tab
                    .update(cx, |tab, cx| tab.toggle_row_viewed(&viewed_path, cx))
                    .ok();
            },
        ))
        .into_any_element()
}

/// The space between the lanes of a row's trailing band (the size, the
/// status letter, and the viewed box) and after the last one. With the
/// tree at its opening width, a folded folder name still fits whole.
const LANE_GAP: f32 = 8.;

/// The space between a file's name and its open threads' badge.
const BADGE_GAP: f32 = 6.;

/// A folder's label as its first part, what stands between, and its last
/// part. A folded path of three or more folders shows its first and last
/// whole with "/…/" between: the first is what differs between two such rows
/// (development, production) and the last is the folder the rows under it
/// belong to. The row's tooltip and accessible name have the whole path.
fn split_label(label: &str) -> (SharedString, SharedString, SharedString) {
    let none = SharedString::default;
    let folders: Vec<&str> = label.split('/').collect();
    match folders.as_slice() {
        [first, _, .., last] => (
            (*first).to_owned().into(),
            "/…/".into(),
            (*last).to_owned().into(),
        ),
        [first, last] => (
            format!("{first}/").into(),
            none(),
            (*last).to_owned().into(),
        ),
        _ => (label.to_owned().into(), none(), none()),
    }
}

/// A file name that fits `max`, as `width` measures text. A name too long to
/// show whole drops the start it shares with a sibling, up to `from`
/// (`…repository.ts`), so the part that tells the two apart stays; what
/// still does not fit loses the end of its stem and keeps its extension, as
/// a name of its own does (`…repos….ts`, `foo-audit.inte….ts`).
fn abbreviate_name(name: &str, from: usize, max: f32, width: impl Fn(&str) -> f32) -> String {
    if width(name) <= max {
        return name.to_owned();
    }
    if from > 0 && from < name.len() {
        let rest = &name[from..];
        let led = |text: &str| width(&format!("{ELLIPSIS}{text}"));
        if led(rest) <= max {
            return format!("{ELLIPSIS}{rest}");
        }
        let cut = cut_name(rest, max, &led);
        if cut.len() > ELLIPSIS.len() {
            return format!("{ELLIPSIS}{cut}");
        }
    }
    cut_name(name, max, &width)
}

/// The left edge of a row's content at `depth`, past the row's padding.
fn indent(depth: usize) -> f32 {
    let full = depth.min(FULL_INDENT_LEVELS);
    full as f32 * INDENT + (depth - full) as f32 * DEEP_INDENT
}

/// What a file row shows in `room`, as `width` measures text: the whole
/// name with its size `size_width` beside it, then the whole name alone,
/// and only when the name alone does not fit, the name cut as
/// `abbreviate_name` cuts it. The size is also in the row's tooltip and
/// accessible name, so it is what gives way. A row with open threads keeps
/// their badge, `badge` wide, and its size, and cuts the name instead: the
/// files under discussion are the ones whose size the reader weighs.
fn fit_file_row(
    name: &str,
    from: usize,
    room: f32,
    badge: f32,
    size_width: f32,
    width: impl Fn(&str) -> f32,
) -> (String, bool) {
    let whole = width(name);
    if whole + badge + size_width <= room {
        return (name.to_owned(), true);
    }
    let name_room = room - badge - size_width;
    if badge > 0. && name_room >= width(ELLIPSIS) {
        return (abbreviate_name(name, from, name_room, width), true);
    }
    let room = room - badge;
    if whole <= room {
        (name.to_owned(), false)
    } else {
        (abbreviate_name(name, from, room, width), false)
    }
}

/// A file's name, its open threads' badge right after it, and at the
/// trailing edge its size, in the room its row leaves (`fit_file_row`), so
/// the size stays in the column of the other rows. `threads` is the badge's
/// id prefix, the row, and the count.
fn name_and_size(
    name: SharedString,
    from: usize,
    additions: u32,
    deletions: u32,
    threads: Option<(String, usize, usize)>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .ml(dp(4.))
        .text_size(scale::body())
        .child(
            canvas(
                move |bounds, window, cx| {
                    let mut size = size_label(additions, deletions).into_any_element();
                    let size_box = size.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let mut badge = threads.as_ref().map(|(id, ix, count)| {
                        let mut badge = thread_badge(id, *ix, *count).into_any_element();
                        let badge_box =
                            badge.layout_as_root(AvailableSpace::min_size(), window, cx);
                        (badge, badge_box)
                    });
                    let badge_width = badge
                        .as_ref()
                        .map_or(px(0.), |(_, badge_box)| badge_box.width + dp(BADGE_GAP));
                    let (label, with_size) = {
                        let measure = text_width(window, FontWeight::NORMAL);
                        fit_file_row(
                            &name,
                            from,
                            f32::from(bounds.size.width),
                            f32::from(badge_width),
                            f32::from(size_box.width + dp(LANE_GAP)),
                            measure,
                        )
                    };
                    let mut text = div().whitespace_nowrap().child(label).into_any_element();
                    let text_box = text.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let middle = |height: Pixels| bounds.top() + (bounds.size.height - height) / 2.;
                    text.prepaint_at(point(bounds.left(), middle(text_box.height)), window, cx);
                    if let Some((badge, badge_box)) = badge.as_mut() {
                        let left = bounds.left() + text_box.width + dp(BADGE_GAP);
                        badge.prepaint_at(point(left, middle(badge_box.height)), window, cx);
                    }
                    let size = with_size.then(|| {
                        let left = bounds.right() - size_box.width;
                        size.prepaint_at(point(left, middle(size_box.height)), window, cx);
                        size
                    });
                    (text, badge.map(|(badge, _)| badge), size)
                },
                |_, (mut text, badge, size), window, cx| {
                    text.paint(window, cx);
                    if let Some(mut badge) = badge {
                        badge.paint(window, cx);
                    }
                    if let Some(mut size) = size {
                        size.paint(window, cx);
                    }
                },
            )
            .size_full(),
        )
}

/// Which of a folder row's file count and size fit in `room` beside its
/// whole label, as (count, size): the size gives way first, then the count,
/// so the name is the last thing a narrow tree loses.
fn fit_folder_row(label: f32, count: f32, size: f32, room: f32) -> (bool, bool) {
    if label + count + size <= room {
        (true, true)
    } else if label + count <= room {
        (true, false)
    } else {
        (false, false)
    }
}

/// A folder's label, its file count and its open threads' badge after it,
/// and its size at the trailing edge, in the room its row leaves
/// (`fit_folder_row`, where the badge stays with the label). A label wider
/// than the whole room is cut at its end. `threads` is as in
/// [`name_and_size`].
fn folder_label(view: &RowView, muted: Hsla, threads: Option<(String, usize, usize)>) -> Div {
    let label = view.label.clone();
    let color = view.label_color;
    let (files, additions, deletions) = (view.files, view.additions, view.deletions);
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .ml(dp(4.))
        .text_size(scale::body())
        .text_color(color)
        .child(
            canvas(
                move |bounds, window, cx| {
                    let room = f32::from(bounds.size.width);
                    let (head, between, tail) = split_label(&label);
                    let mut name = h_flex()
                        .whitespace_nowrap()
                        .child(head.clone())
                        .when(!between.is_empty(), |name| {
                            name.child(div().text_color(muted).child(between.clone()))
                        })
                        .child(tail.clone())
                        .into_any_element();
                    let name_box = name.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let mut count = file_count(files).into_any_element();
                    let count_box = count.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let mut size = size_label(additions, deletions).into_any_element();
                    let size_box = size.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let mut badge = threads.as_ref().map(|(id, ix, open)| {
                        let mut badge = thread_badge(id, *ix, *open).into_any_element();
                        let badge_box =
                            badge.layout_as_root(AvailableSpace::min_size(), window, cx);
                        (badge, badge_box)
                    });
                    let badge_width = badge.as_ref().map_or(0., |(_, badge_box)| {
                        f32::from(badge_box.width + dp(BADGE_GAP))
                    });
                    let name_width = f32::from(name_box.width);
                    let (with_count, with_size) = fit_folder_row(
                        name_width + badge_width,
                        f32::from(count_box.width),
                        f32::from(size_box.width + dp(LANE_GAP)),
                        room,
                    );
                    let mut name_box = name_box;
                    if name_width + badge_width > room {
                        let text = format!("{head}{between}{tail}");
                        let cut: SharedString = {
                            let measure = text_width(window, FontWeight::NORMAL);
                            cut_name(&text, room - badge_width, &measure).into()
                        };
                        name = div().whitespace_nowrap().child(cut).into_any_element();
                        name_box = name.layout_as_root(AvailableSpace::min_size(), window, cx);
                    }
                    let middle = |height: Pixels| bounds.top() + (bounds.size.height - height) / 2.;
                    name.prepaint_at(point(bounds.left(), middle(name_box.height)), window, cx);
                    let count = with_count.then(|| {
                        let left = bounds.left() + name_box.width;
                        count.prepaint_at(point(left, middle(count_box.height)), window, cx);
                        count
                    });
                    if let Some((badge, badge_box)) = badge.as_mut() {
                        let count_width = if with_count { count_box.width } else { px(0.) };
                        let left = bounds.left() + name_box.width + count_width + dp(BADGE_GAP);
                        badge.prepaint_at(point(left, middle(badge_box.height)), window, cx);
                    }
                    let size = with_size.then(|| {
                        let left = bounds.right() - size_box.width;
                        size.prepaint_at(point(left, middle(size_box.height)), window, cx);
                        size
                    });
                    (name, count, badge.map(|(badge, _)| badge), size)
                },
                |_, (mut name, count, badge, size), window, cx| {
                    name.paint(window, cx);
                    for mut part in count.into_iter().chain(badge).chain(size) {
                        part.paint(window, cx);
                    }
                },
            )
            .size_full(),
        )
}

/// A step of the Guide order: its number, muted, then its title cut at its
/// end, and how many files it holds. [`NOT_COVERED`] has no number.
fn guide_group_label(view: &RowView, step: Option<usize>, muted: Hsla) -> Div {
    h_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .ml(dp(4.))
        .gap(dp(4.))
        .items_center()
        .text_size(scale::body())
        .text_color(view.label_color)
        .children(step.map(|step| {
            div()
                .flex_none()
                .text_color(muted)
                .child(format!("{step}."))
        }))
        .child(
            div()
                .min_w_0()
                .whitespace_nowrap()
                .truncate()
                .child(view.label.clone()),
        )
        .child(file_count(view.files).ml(dp(2.)))
}

/// How many review threads on a row are still open: a speech bubble and the
/// count in the accent, since an open thread waits on someone.
fn thread_badge(id: &str, ix: usize, threads: usize) -> Stateful<Div> {
    h_flex()
        .id(ElementId::NamedInteger(id.to_owned().into(), ix as u64))
        .role(Role::Status)
        .aria_label(open_threads_label(threads))
        .flex_none()
        .gap(dp(2.))
        .items_center()
        .text_size(scale::label())
        .text_color(tokens::accent())
        .child(icon(THREAD_ICON, tokens::accent()).size(dp(12.)))
        .child(threads.to_string())
}

/// "1 open thread", "3 open threads".
pub fn open_threads_label(threads: usize) -> String {
    match threads {
        1 => "1 open thread".to_owned(),
        threads => format!("{threads} open threads"),
    }
}

const THREAD_ICON: &str = "icons/message-square.svg";
const STEP_ICON: &str = "icons/book-open.svg";
const NOT_COVERED_ICON: &str = "icons/circle-dashed.svg";
/// What the group of the files no step cites holds.
const NOT_COVERED_HINT: &str = "changed files no step of the guide cites";

/// `+added −removed`, in the diff's colors.
fn size_label(additions: u32, deletions: u32) -> Div {
    h_flex()
        .flex_none()
        .gap(dp(3.))
        .text_size(scale::label())
        .whitespace_nowrap()
        .child(
            div()
                .text_color(tokens::diff_added_text())
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_color(tokens::diff_removed_text())
                .child(format!("−{deletions}")),
        )
}

/// The lane of a file's status letter; a folder's stays empty so the
/// columns line up.
fn status_lane(status: Option<(&'static str, Hsla)>) -> Div {
    div()
        .flex_none()
        .w(dp(12.))
        .text_size(scale::label())
        .text_center()
        .font_weight(FontWeight::SEMIBOLD)
        .children(status.map(|(letter, color)| div().text_color(color).child(letter)))
}

/// How many files a folder holds, so a closed folder says how much is in
/// it. The row's accessible name says it too.
fn file_count(files: usize) -> Div {
    div()
        .flex_none()
        .ml(dp(6.))
        .px(dp(5.))
        .rounded(dp(4.))
        .bg(tokens::glass_hover())
        .text_size(scale::label())
        .text_color(tokens::text_muted())
        .child(files.to_string())
}

fn icon(path: &'static str, color: Hsla) -> Svg {
    svg()
        .flex_none()
        .path(path)
        .size(dp(ICON_SIZE))
        .text_color(color)
}

/// A checkbox with Zed's look and a third, mixed state for a folder whose
/// files are only partly viewed.
fn viewed_box(
    id: ElementId,
    mark: ViewedMark,
    accessible_name: String,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (toggled, tooltip) = match mark {
        ViewedMark::All => (Toggled::True, "Mark as not viewed"),
        ViewedMark::Some => (Toggled::Mixed, "Mark all as viewed"),
        ViewedMark::None => (Toggled::False, "Mark as viewed"),
    };
    let filled = mark != ViewedMark::None;
    div()
        .id(id)
        .role(Role::CheckBox)
        .aria_label(accessible_name)
        .aria_toggled(toggled)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(dp(14.))
        .rounded(dp(3.))
        .border_1()
        .border_color(if filled {
            tokens::accent()
        } else {
            tokens::text_faint()
        })
        .when(filled, |this| this.bg(tokens::accent()))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .on_click(move |_, window, cx| on_click(window, cx))
        .when(filled, |this| {
            this.child(
                svg()
                    .path(if mark == ViewedMark::All {
                        assets::CHECK
                    } else {
                        assets::DASH
                    })
                    .size(dp(12.))
                    .text_color(tokens::text()),
            )
        })
}

/// Folders pinned over the top of the list, drawn by the list itself so
/// they follow the scroll offset frame by frame.
pub struct StickyFolders {
    pub rows: Rc<Vec<TreeRow>>,
    pub views: Rc<Vec<RowView>>,
    pub tab: WeakEntity<FilesTab>,
}

impl UniformListDecoration for StickyFolders {
    fn compute(
        &self,
        visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        _item_count: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(sticky) = sticky_folders(&self.rows, visible_range.start) else {
            return div().into_any_element();
        };
        let count = sticky.rows.len();
        // The last pinned folder slides up under the others as the row
        // below it, its last one, leaves the top.
        let drift = if sticky.drifting {
            let anchor_bottom = item_height * (sticky.anchor + 1) as f32;
            (anchor_bottom + scroll_offset.y - item_height * count as f32).min(px(0.))
        } else {
            px(0.)
        };
        let (rest, drifting) = if sticky.drifting {
            (&sticky.rows[..count - 1], sticky.rows.last())
        } else {
            (&sticky.rows[..], None)
        };
        let width = bounds.size.width;
        let bottom = item_height * count as f32 + drift;
        // The list lays decorations out at the top of its scrolled content,
        // so the stack moves down by the scroll distance to sit in view.
        let scrolled = -scroll_offset.y;
        div()
            .relative()
            .w(width)
            .h(scrolled + bottom)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(scrolled)
                    .w(width)
                    .h(bottom.max(px(0.)))
                    .occlude()
                    .children(drifting.map(|&ix| {
                        div()
                            .absolute()
                            .left_0()
                            .w(width)
                            .top(item_height * (count - 1) as f32 + drift)
                            .child(row(&self.views[ix], ix, true, &self.tab, cx))
                    }))
                    .children(rest.iter().enumerate().map(|(position, &ix)| {
                        div()
                            .absolute()
                            .left_0()
                            .w(width)
                            .top(item_height * position as f32)
                            .child(row(&self.views[ix], ix, true, &self.tab, cx))
                    }))
                    .child(div().absolute().left_0().w(width).top(bottom).h(dp(6.)).bg(
                        linear_gradient(
                            180.,
                            linear_color_stop(tokens::edge_shadow(), 0.),
                            linear_color_stop(tokens::edge_shadow().opacity(0.), 1.),
                        ),
                    )),
            )
            .into_any_element()
    }
}

impl FilesTab {
    pub(super) fn row_views(&self, rows: &[TreeRow], focused: bool, cx: &App) -> Vec<RowView> {
        let selected = self
            .selected
            .as_deref()
            .and_then(|path| self.shown(rows, path));
        let active = selected.and_then(|ix| active_folder(rows, ix));
        let starts = distinct_starts(rows);
        let threads = self.open_thread_counts();
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        rows.iter()
            .enumerate()
            .map(|(ix, row)| {
                let guides = ancestors(rows, ix)
                    .into_iter()
                    .map(|folder| Some(folder) == active)
                    .collect();
                let path = SharedString::from(row.path.clone());
                let is_selected = selected == Some(ix);
                match &row.kind {
                    RowKind::Dir(dir) => {
                        let files = dir.files.iter().map(|&ix| &self.files[ix]);
                        let viewed = viewed_mark(files.clone().map(|file| is_viewed(file.viewed)));
                        let open: usize = files
                            .clone()
                            .map(|file| threads.get(file.path.as_str()).copied().unwrap_or(0))
                            .sum();
                        // A folder that is all new or all deleted says so on
                        // its icon; any other folder's icon stays muted.
                        let icon_color = match folder_tint(files.map(|file| file.status)) {
                            GitTint::Created => tokens::open(),
                            GitTint::Deleted => tokens::closed(),
                            GitTint::Modified | GitTint::Renamed => muted,
                        };
                        let count = dir.files.len();
                        let noun = if count == 1 { "file" } else { "files" };
                        let size = format!("+{} −{}", dir.additions, dir.deletions);
                        let group = row.parent.is_none() && is_group(&row.path);
                        let step = group.then(|| step_of(&row.path)).flatten();
                        let guide_group = match (step, row.path == NOT_COVERED) {
                            (Some(step), _) => Some(Some(step + 1)),
                            (None, true) => Some(None),
                            (None, false) => None,
                        };
                        // A folder in a group is named for the folder it
                        // mirrors.
                        let (shown_path, section) = match split_group(&row.path) {
                            Some((group, inner)) => {
                                let section = match step_of(group) {
                                    Some(step) => format!(" (step {})", step + 1),
                                    None => format!(" ({})", group.to_lowercase()),
                                };
                                (inner, section)
                            }
                            None => (row.path.as_str(), String::new()),
                        };
                        let threads_note = match open {
                            0 => String::new(),
                            open => format!(", {}", open_threads_label(open)),
                        };
                        let (tooltip, accessible_name) = if let Some(step) = step {
                            (
                                format!("{}. {} · {count} {noun} · {size}", step + 1, dir.label),
                                format!(
                                    "Guide step {}: {}, {count} {noun}, {size}",
                                    step + 1,
                                    dir.label
                                ),
                            )
                        } else if row.path == LOW_SIGNAL && group {
                            (
                                format!(
                                    "{LOW_SIGNAL} · {count} {noun} · {size} · {LOW_SIGNAL_HINT}"
                                ),
                                format!("{LOW_SIGNAL} group, {count} {noun}, {size}"),
                            )
                        } else if row.path == NOT_COVERED && group {
                            (
                                format!(
                                    "{NOT_COVERED} · {count} {noun} · {size} · {NOT_COVERED_HINT}"
                                ),
                                format!("{NOT_COVERED} group, {count} {noun}, {size}"),
                            )
                        } else if group {
                            (
                                format!("{} · {count} {noun} · {size}", row.path),
                                format!("{} group, {count} {noun}, {size}", row.path),
                            )
                        } else {
                            (
                                format!("{shown_path}{section} · {count} {noun} · {size}"),
                                format!("Folder {shown_path}{section}, {count} {noun}, {size}"),
                            )
                        };
                        RowView {
                            depth: row.depth,
                            folder: Some(dir.expanded),
                            label: dir.label.clone().into(),
                            name_from: 0,
                            // Names stay neutral; the status letter carries
                            // the color.
                            label_color: if viewed == ViewedMark::All {
                                muted
                            } else {
                                foreground
                            },
                            icon: if step.is_some() {
                                STEP_ICON
                            } else if guide_group.is_some() {
                                NOT_COVERED_ICON
                            } else if row.path == LOW_SIGNAL && group {
                                LOW_SIGNAL_ICON
                            } else if dir.expanded {
                                assets::FOLDER_OPEN
                            } else {
                                assets::FOLDER
                            },
                            icon_color: if group { muted } else { icon_color },
                            additions: dir.additions,
                            deletions: dir.deletions,
                            files: count,
                            status: None,
                            viewed,
                            selected: is_selected,
                            focused,
                            guides,
                            tooltip: format!("{tooltip}{}", threads_note.replacen(',', " ·", 1))
                                .into(),
                            accessible_name: format!("{accessible_name}{threads_note}").into(),
                            path,
                            threads: open,
                            guide_group,
                        }
                    }
                    RowKind::File { index } => {
                        let file = &self.files[*index];
                        let viewed = is_viewed(file.viewed);
                        let tint = tint_color(file_tint(file.status));
                        let open = threads.get(file.path.as_str()).copied().unwrap_or(0);
                        let missing = match self.missing.get(&file.path) {
                            Some(Missing::Omitted) => " · diff not loaded",
                            Some(Missing::Unreadable) => " · diff not shown",
                            None => "",
                        };
                        let kind = self
                            .low
                            .get(&file.path)
                            .map(|kind| format!(", {}", kind.label(file)))
                            .unwrap_or_default();
                        let threads_note = match open {
                            0 => String::new(),
                            open => format!(", {}", open_threads_label(open)),
                        };
                        RowView {
                            depth: row.depth,
                            folder: None,
                            label: split_path(&file.path).1.to_owned().into(),
                            name_from: starts[ix],
                            label_color: if viewed { muted } else { foreground },
                            icon: assets::file_icon(&file.path),
                            icon_color: muted,
                            additions: file.additions,
                            deletions: file.deletions,
                            files: 0,
                            status: Some((status_letter(file.status), tint)),
                            viewed: if viewed {
                                ViewedMark::All
                            } else {
                                ViewedMark::None
                            },
                            selected: is_selected,
                            focused,
                            guides,
                            tooltip: format!(
                                "{} · +{} −{}{}{}{}",
                                file.path,
                                file.additions,
                                file.deletions,
                                kind.replacen(',', " ·", 1),
                                threads_note.replacen(',', " ·", 1),
                                missing
                            )
                            .into(),
                            accessible_name: format!(
                                "File {}{kind}, +{} −{}{threads_note}",
                                file.path, file.additions, file.deletions
                            )
                            .into(),
                            path,
                            threads: open,
                            guide_group: None,
                        }
                    }
                }
            })
            .collect()
    }

    /// Under the filter's text: Filter, with the facets that narrow the
    /// tree and the diff; Group, for folders or the Guide order; and Next
    /// open thread. The facets set follow as chips, each with its ×, and
    /// Clear.
    pub(super) fn tree_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = cx.entity().downgrade();
        let muted = cx.theme().muted_foreground;
        let counts = self.facet_counts();
        let set = self.tree.facets.clone();
        let has_guide = !self.tree.guide.is_empty();
        let in_commit = self.commit.is_some();
        let filter = {
            let tab = tab.clone();
            let set = set.clone();
            Button::new("files-tree-filter")
                .debug_selector(|| "files-tree-filter".into())
                .ghost()
                .xsmall()
                .icon(IconName::ListFilter)
                .label("Filter")
                .dropdown_caret(true)
                .selected(!set.is_empty())
                .tooltip("Show only some files in the tree and the diff")
                .accessibility_label("Filter files by state")
                .menu_trigger()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    Facet::ALL
                        .into_iter()
                        .fold(menu.min_w(scale::dp(220.)), |menu, facet| {
                            let count = counts.get(&facet).copied().unwrap_or(0);
                            let on = set.contains(&facet);
                            let unavailable = match facet {
                                Facet::CitedByGuide if !has_guide => Some("no guide yet"),
                                Facet::HasThreads if in_commit => Some("not in a commit"),
                                _ => None,
                            };
                            let label = match unavailable {
                                Some(why) => format!("{} ({why})", facet.label()),
                                None => format!("{} · {count}", facet.label()),
                            };
                            let tab = tab.clone();
                            menu.item(
                                PopupMenuItem::new(label)
                                    .checked(on)
                                    .disabled(unavailable.is_some() && !on)
                                    .on_click(move |_, _, cx| {
                                        tab.update(cx, |tab, cx| tab.toggle_facet(facet, cx)).ok();
                                    }),
                            )
                        })
                })
        };
        let guide_order = self.tree.guide_order && has_guide;
        let group = {
            let tab = tab.clone();
            Button::new("files-tree-group")
                .debug_selector(|| "files-tree-group".into())
                .ghost()
                .xsmall()
                .icon(if guide_order {
                    IconName::BookOpen
                } else {
                    IconName::ListTree
                })
                .label(if guide_order {
                    "Guide order"
                } else {
                    "Folders"
                })
                .dropdown_caret(true)
                .tooltip("Group the files by folder or by the steps of the guide")
                .accessibility_label(if guide_order {
                    "Group files: Guide order"
                } else {
                    "Group files: Folders"
                })
                .menu_trigger()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let pick = |label: &str, on: bool, disabled: bool| {
                        let tab = tab.clone();
                        PopupMenuItem::new(label.to_owned())
                            .checked(guide_order == on)
                            .disabled(disabled)
                            .on_click(move |_, _, cx| {
                                tab.update(cx, |tab, cx| tab.set_guide_order(on, cx)).ok();
                            })
                    };
                    menu.min_w(scale::dp(220.))
                        .item(pick("Folders", false, false))
                        .item(if has_guide {
                            pick("Guide order", true, false)
                        } else {
                            pick("Guide order (no guide yet)", true, true)
                        })
                })
        };
        let stops = self.open_thread_stops(cx);
        let next_thread = {
            let position = super::tree::next_stop(&stops, self.tree.last_thread.as_ref())
                .filter(|_| self.tree.last_thread.is_some());
            let tooltip = match (stops.len(), position) {
                (0, _) if in_commit => "Threads belong to the whole pull request".to_owned(),
                (0, _) => "No open threads in the files shown".to_owned(),
                (total, Some(next)) => format!("Next open thread ({} of {total})", next + 1),
                (total, None) => format!("Next open thread (1 of {total})"),
            };
            Button::new("files-next-thread")
                .debug_selector(|| "files-next-thread".into())
                .ghost()
                .xsmall()
                .icon(IconName::MessageSquare)
                .label(stops.len().to_string())
                .tooltip(tooltip)
                .accessibility_label(format!(
                    "Next open thread, {}",
                    open_threads_label(stops.len())
                ))
                .disabled(stops.is_empty())
                .on_click(cx.listener(|tab, _, _, cx| tab.next_open_thread(cx)))
        };
        let chips: Vec<AnyElement> = set
            .iter()
            .map(|&facet| {
                ui::chip_button(
                    SharedString::from(format!("files-facet-{}", facet.label())),
                    format!("Remove filter: {}", facet.label()),
                    cx,
                )
                .tooltip(|window, cx| Tooltip::new("Remove filter").build(window, cx))
                .on_click(cx.listener(move |tab, _, _, cx| tab.toggle_facet(facet, cx)))
                .child(
                    h_flex().gap_1().items_center().child(facet.label()).child(
                        Icon::new(IconName::Close)
                            .with_size(scale::icon_xs())
                            .text_color(muted),
                    ),
                )
                .into_any_element()
            })
            .collect();
        v_flex_bar()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(filter)
                    .child(group)
                    .child(div().flex_1())
                    .child(next_thread),
            )
            .when(!chips.is_empty(), |bar| {
                bar.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1p5()
                        .items_center()
                        .children(chips)
                        .child(
                            Button::new("files-facets-clear")
                                .ghost()
                                .xsmall()
                                .label("Clear")
                                .accessibility_label("Clear filters")
                                .on_click(cx.listener(|tab, _, _, cx| tab.clear_facets(cx))),
                        ),
                )
            })
            .into_any_element()
    }
}

/// The tree's bar on the toolbar's 16 px edge, its rows 4 px apart.
fn v_flex_bar() -> Div {
    div().flex().flex_col().flex_none().gap_1().px_4().pb_1()
}

#[cfg(test)]
mod tests {
    use super::{abbreviate_name, fit_file_row, fit_folder_row, indent, split_label};

    fn parts(label: &str) -> String {
        let (head, between, tail) = split_label(label);
        format!("{head}|{between}|{tail}")
    }

    /// One unit per character, so a width is a character count.
    fn chars(text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn a_label_keeps_the_ends_that_tell_rows_apart() {
        assert_eq!(
            parts("development/sa-east-1/compute/ecs/hooli"),
            "development|/…/|hooli"
        );
        assert_eq!(parts("src/shell"), "src/||shell");
        assert_eq!(parts("accounts"), "accounts||");
        assert_eq!(parts("my.config"), "my.config||");
    }

    #[test]
    fn a_name_that_fits_is_whole() {
        let name = "collaborator.repository.ts";
        assert_eq!(abbreviate_name(name, 13, chars(name), chars), name);
        assert_eq!(abbreviate_name(name, 0, 40., chars), name);
    }

    #[test]
    fn siblings_with_one_start_keep_the_part_that_differs() {
        let names = [
            "collaborator.controller.ts",
            "collaborator.repository.ts",
            "collaborator.service.ts",
        ];
        let cut: Vec<String> = names
            .iter()
            .map(|name| abbreviate_name(name, 13, 15., chars))
            .collect();
        assert_eq!(cut, ["…controller.ts", "…repository.ts", "…service.ts"]);
        // Narrower still, the part that differs leads and the extension stays.
        let cut: Vec<String> = names
            .iter()
            .map(|name| abbreviate_name(name, 13, 9., chars))
            .collect();
        assert_eq!(cut, ["…cont….ts", "…repo….ts", "…serv….ts"]);
    }

    #[test]
    fn the_size_gives_way_before_the_name() {
        let name = "repository_context.rs";
        let whole = (name.to_owned(), true);
        assert_eq!(fit_file_row(name, 0, 30., 0., 9., chars), whole);
        // Too narrow for both: the name stays whole, the size goes.
        assert_eq!(
            fit_file_row(name, 0, 25., 0., 9., chars),
            (name.to_owned(), false)
        );
        assert_eq!(
            fit_file_row(name, 0, 21., 0., 9., chars),
            (name.to_owned(), false)
        );
        // Only a name wider than the whole row is cut.
        assert_eq!(
            fit_file_row(name, 0, 15., 0., 9., chars),
            ("repository_….rs".to_owned(), false)
        );
    }

    #[test]
    fn a_row_with_open_threads_cuts_its_name_before_its_size() {
        let name = "repository_context.rs";
        assert_eq!(
            fit_file_row(name, 0, 34., 4., 9., chars),
            (name.to_owned(), true)
        );
        // The badge and the size stay; the name gives way.
        assert_eq!(
            fit_file_row(name, 0, 30., 4., 9., chars),
            ("repository_co….rs".to_owned(), true)
        );
        // Too narrow for even a cut name beside both: the size goes.
        assert_eq!(
            fit_file_row(name, 0, 13., 4., 9., chars),
            ("repos….rs".to_owned(), false)
        );
    }

    #[test]
    fn a_folder_keeps_its_name_before_its_size_and_count() {
        assert_eq!(fit_folder_row(10., 3., 6., 19.), (true, true));
        assert_eq!(fit_folder_row(10., 3., 6., 18.), (true, false));
        assert_eq!(fit_folder_row(10., 3., 6., 12.), (false, false));
        assert_eq!(fit_folder_row(10., 3., 6., 8.), (false, false));
    }

    #[test]
    fn deep_levels_indent_less() {
        assert_eq!(indent(0), 0.);
        assert_eq!(indent(3), 60.);
        assert_eq!(indent(5), 80.);
    }

    #[test]
    fn a_name_of_its_own_loses_the_end_of_its_stem() {
        assert_eq!(
            abbreviate_name("foo-audit.interceptor.spec.ts", 0, 18., chars),
            "foo-audit.inte….ts"
        );
        // With no room for the extension, the part that differs still leads.
        assert_eq!(
            abbreviate_name("collaborator.repository.ts", 13, 4., chars),
            "…re…"
        );
        assert_eq!(abbreviate_name("Makefile", 0, 5., chars), "Make…");
        assert_eq!(abbreviate_name("Makefile", 0, 0., chars), "");
    }
}
