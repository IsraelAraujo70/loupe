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
use super::tree::{TreeRow, ViewedMark, sticky_folders};
use crate::assets;
use crate::theme::scale::{self, dp};
use crate::theme::tokens;

/// Zed's dense project panel row.
pub const ROW_HEIGHT: f32 = 22.;
/// Zed's default `project_panel.indent_size`.
const INDENT: f32 = 20.;
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
    pub label_color: Hsla,
    pub icon: &'static str,
    /// The icon's color: muted, or the state color of a folder whose files
    /// are all new or all deleted.
    pub icon_color: Hsla,
    pub additions: u32,
    pub deletions: u32,
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
                .left(dp(ROW_PADDING + level as f32 * INDENT + ICON_SIZE / 2. - 1.))
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
                .when_some(view.folder, |item, expanded| item.aria_expanded(expanded))
                .flex_1()
                .min_w_0()
                .h_full()
                .pl(dp(ROW_PADDING + view.depth as f32 * INDENT - 1.))
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
                .child({
                    let (head, between, tail) = split_label(&view.label);
                    let folder = folder_row;
                    h_flex()
                        .min_w_0()
                        .ml(dp(4.))
                        .text_size(scale::body())
                        .text_color(view.label_color)
                        // A file's stem gives way and its extension
                        // stays. A folder's name is whole at the tree's
                        // opening width; in a narrower tree it is cut at
                        // its end, and the lanes beside it keep their place.
                        .when(folder, |label| label.flex_shrink(1.).overflow_hidden())
                        .when(!folder, |label| label.flex_1())
                        .child(
                            div()
                                .when(folder, |part| part.flex_none())
                                .when(!folder, |part| part.min_w_0().truncate())
                                .child(head),
                        )
                        .when(!between.is_empty(), |label| {
                            label.child(div().flex_none().text_color(muted).child(between))
                        })
                        .child(div().flex_none().child(tail))
                })
                // What is left of a folder's row, so its lanes sit at the
                // trailing edge like a file's.
                .when(folder_row, |item| item.child(div().flex_1()))
                .child(
                    // One fixed band at the trailing edge of every row:
                    // the size, the status letter, then the viewed box
                    // after it, each in a lane of its own width and 8 px
                    // from the next. The band never shrinks, so the three
                    // form columns down the tree whatever the name beside
                    // them.
                    h_flex()
                        .flex_none()
                        .ml(dp(8.))
                        .gap(dp(LANE_GAP))
                        .text_size(scale::label())
                        .child(
                            h_flex()
                                .w(dp(STAT_LANE))
                                .justify_end()
                                .gap(dp(3.))
                                .child(
                                    div()
                                        .text_color(tokens::diff_added_text())
                                        .child(format!("+{}", view.additions)),
                                )
                                .child(
                                    div()
                                        .text_color(tokens::diff_removed_text())
                                        .child(format!("−{}", view.deletions)),
                                ),
                        )
                        .child(
                            div()
                                .w(dp(12.))
                                .text_center()
                                .font_weight(FontWeight::SEMIBOLD)
                                .children(
                                    view.status.map(|(letter, color)| {
                                        div().text_color(color).child(letter)
                                    }),
                                ),
                        ),
                ),
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

/// A label as its first part, what stands between, and its last part. A
/// folded path of three or more folders shows its first and last whole with
/// "/…/" between: the first is what differs between two such rows
/// (development, production) and the last is the folder the rows under it
/// belong to. A file name is its stem and its extension. The row's tooltip
/// and accessible name have the whole path.
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
        _ => match label.rfind('.') {
            Some(dot) if dot > 0 => {
                let (stem, extension) = label.split_at(dot);
                (stem.to_owned().into(), none(), extension.to_owned().into())
            }
            _ => (label.to_owned().into(), none(), none()),
        },
    }
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
    use super::split_label;

    fn parts(label: &str) -> String {
        let (head, between, tail) = split_label(label);
        format!("{head}|{between}|{tail}")
    }

    #[test]
    fn a_label_keeps_the_ends_that_tell_rows_apart() {
        assert_eq!(
            parts("development/sa-east-1/compute/ecs/hooli"),
            "development|/…/|hooli"
        );
        assert_eq!(parts("src/shell"), "src/||shell");
        assert_eq!(parts("terragrunt.hcl"), "terragrunt||.hcl");
        assert_eq!(parts("accounts"), "accounts||");
        assert_eq!(parts(".gitignore"), ".gitignore||");
    }
}
