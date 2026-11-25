//! Drawing the files tree like Zed's project panel: dense rows with indent
//! guides, chevrons, file-type icons and git-tinted names, a three-state
//! viewed box, and folders pinned at the top while the list scrolls.
//!
//! Zed's panel code is GPL and was read for behavior only; this is Loupe's
//! own implementation on GPUI's `uniform_list`.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::FilesTab;
use super::header::{ELLIPSIS, cut_name, text_width};
use super::tree::{TreeRow, ViewedMark, sticky_folders};
use crate::assets;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;

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
                .child(if folder_row {
                    let (head, between, tail) = split_label(&view.label);
                    // A folder's name is whole at the tree's opening width;
                    // in a narrower tree it is cut at its end, and the lanes
                    // beside it keep their place.
                    h_flex()
                        .min_w_0()
                        .ml(dp(4.))
                        .text_size(scale::body())
                        .text_color(view.label_color)
                        .flex_shrink(1.)
                        .overflow_hidden()
                        .child(div().flex_none().child(head))
                        .when(!between.is_empty(), |label| {
                            label.child(div().flex_none().text_color(muted).child(between))
                        })
                        .child(div().flex_none().child(tail))
                        .into_any_element()
                } else {
                    name_and_size(
                        view.label.clone(),
                        view.name_from,
                        view.additions,
                        view.deletions,
                    )
                    .text_color(view.label_color)
                    .into_any_element()
                })
                .when(folder_row, |item| {
                    item.child(file_count(view.files))
                        // What is left of a folder's row, so its lanes sit
                        // at the trailing edge like a file's.
                        .child(div().flex_1())
                        .child(
                            // A fixed band at the trailing edge: the size,
                            // then the status letter's lane, each of its
                            // own width and 8 px from the next, so they
                            // form columns down the tree with the files'.
                            h_flex()
                                .flex_none()
                                .ml(dp(LANE_GAP))
                                .gap(dp(LANE_GAP))
                                .child(
                                    h_flex()
                                        .w(dp(STAT_LANE))
                                        .justify_end()
                                        .child(size_label(view.additions, view.deletions)),
                                )
                                .child(status_lane(None)),
                        )
                })
                .when(!folder_row, |item| {
                    item.child(status_lane(view.status).ml(dp(LANE_GAP)))
                }),
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

/// Width of the lane for `+added −removed`.
const STAT_LANE: f32 = 56.;
/// The space between the lanes of a row's trailing band (the size, the
/// status letter, and the viewed box) and after the last one. With the
/// tree at its opening width, a folded folder name still fits whole.
const LANE_GAP: f32 = 8.;

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
/// accessible name, so it is what gives way.
fn fit_file_row(
    name: &str,
    from: usize,
    room: f32,
    size_width: f32,
    width: impl Fn(&str) -> f32,
) -> (String, bool) {
    let whole = width(name);
    if whole + size_width <= room {
        (name.to_owned(), true)
    } else if whole <= room {
        (name.to_owned(), false)
    } else {
        (abbreviate_name(name, from, room, width), false)
    }
}

/// A file's name and, at the trailing edge, its size, in the room its row
/// leaves (`fit_file_row`): the name is cut only when it cannot be whole.
fn name_and_size(name: SharedString, from: usize, additions: u32, deletions: u32) -> Div {
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
                    let (label, with_size) = {
                        let measure = text_width(window, FontWeight::NORMAL);
                        fit_file_row(
                            &name,
                            from,
                            f32::from(bounds.size.width),
                            f32::from(size_box.width + dp(LANE_GAP)),
                            measure,
                        )
                    };
                    let mut text = div().whitespace_nowrap().child(label).into_any_element();
                    let text_box = text.layout_as_root(AvailableSpace::min_size(), window, cx);
                    let middle = |height: Pixels| bounds.top() + (bounds.size.height - height) / 2.;
                    text.prepaint_at(point(bounds.left(), middle(text_box.height)), window, cx);
                    let size = with_size.then(|| {
                        let left = bounds.right() - size_box.width;
                        size.prepaint_at(point(left, middle(size_box.height)), window, cx);
                        size
                    });
                    (text, size)
                },
                |_, (mut text, size), window, cx| {
                    text.paint(window, cx);
                    if let Some(mut size) = size {
                        size.paint(window, cx);
                    }
                },
            )
            .size_full(),
        )
}

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

#[cfg(test)]
mod tests {
    use super::{abbreviate_name, fit_file_row, indent, split_label};

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
        assert_eq!(fit_file_row(name, 0, 30., 9., chars), whole);
        // Too narrow for both: the name stays whole, the size goes.
        assert_eq!(
            fit_file_row(name, 0, 25., 9., chars),
            (name.to_owned(), false)
        );
        assert_eq!(
            fit_file_row(name, 0, 21., 9., chars),
            (name.to_owned(), false)
        );
        // Only a name wider than the whole row is cut.
        assert_eq!(
            fit_file_row(name, 0, 15., 9., chars),
            ("repository_….rs".to_owned(), false)
        );
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
