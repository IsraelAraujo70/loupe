//! Shared list layout pieces: container, row, column heading, collapsible group
//! heading, and compact navigation row. Every list aligns its content to the
//! 16 px `content_inset` while letting row hover fills reach 8 px past it.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::scale;

/// Outer padding for a vertical list of [`list_row`]s: 8 px on every side so
/// rows with `px_2()` land their content on the 16 px `content_inset` while
/// hover fills extend 8 px past the text.
pub fn list_container() -> Div {
    v_flex().w_full().p_2()
}

/// One standard list row: density-aware height (`scale::row_height`), 8 px
/// horizontal inset, 12 px lane gap, rounded hover fill, body text size.
pub fn list_row(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let hover = theme.list_hover;
    h_flex()
        .id(id)
        .w_full()
        .h(scale::row_height())
        .px_2()
        .gap_3()
        .items_center()
        .rounded(theme.radius)
        .text_size(scale::body())
        .hover(move |row| row.bg(hover))
}

/// The 24 px column heading over a multi-lane list, sitting inside
/// [`list_container`] on the exact leading edge of its [`list_row`]s.
pub fn list_column_header(cx: &App) -> Div {
    h_flex()
        .w_full()
        .h(scale::column_header_height())
        .px_2()
        .gap_3()
        .items_center()
        .text_size(scale::meta())
        .text_color(cx.theme().muted_foreground)
}

/// A collapsible group heading inside [`list_container`]: chevron, then
/// caller-supplied title, count, and trailing controls.
pub fn list_group_header(id: impl Into<ElementId>, collapsed: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, hover) = (theme.muted_foreground, theme.list_hover);
    h_flex()
        .id(id)
        .role(Role::Button)
        .w_full()
        .h(scale::row_height())
        .px_2()
        .gap_2()
        .items_center()
        .rounded(theme.radius)
        .hover(move |row| row.bg(hover))
        .child(
            h_flex().flex_none().w_4().justify_center().child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .small()
                .text_color(muted),
            ),
        )
}

/// A compact 28 px navigation row for secondary side panels (Settings nav,
/// Actions jobs column).
pub fn nav_row(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (fg, muted, active, hover) = (
        theme.foreground,
        theme.muted_foreground,
        theme.list_active,
        theme.list_hover,
    );
    h_flex()
        .id(id)
        .role(Role::Button)
        .w_full()
        .h(scale::nav_row_height())
        .px_2()
        .gap_2()
        .items_center()
        .rounded(theme.radius)
        .text_size(scale::body())
        .when(selected, move |row| row.bg(active).text_color(fg))
        .when(!selected, move |row| {
            row.text_color(muted)
                .hover(move |style| style.bg(hover).text_color(fg))
        })
}
