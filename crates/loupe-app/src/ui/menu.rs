//! The row both filter menus (Reviews and the Linear team) draw, so the two
//! menus share one geometry and one way to show focus and selection.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::scale::{self, dp};

/// What a menu row shows besides its label.
pub struct MenuRow {
    pub icon: Option<IconName>,
    /// The value is part of the filter: a check at the trailing edge.
    pub checked: bool,
    /// The row opens a page of values: a chevron at the trailing edge.
    pub opens: bool,
    /// The row has the keyboard focus: the only state that fills the row.
    pub focused: bool,
    /// What the row already contributes to the filter, at the trailing
    /// edge in the muted color: the values chosen for a field.
    pub detail: Option<SharedString>,
}

/// One 28 px menu row: 12 px side padding, 8 px gaps, a 16 px icon. The
/// fill means focus (keyboard or pointer) and nothing else; a value that is
/// part of the filter carries a check in the foreground color. The caller
/// adds `on_click`.
pub fn menu_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    label: impl Into<SharedString>,
    row: MenuRow,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, foreground, fill) = (theme.muted_foreground, theme.foreground, theme.accent);
    let glyph = |icon: IconName, size: f32, color: Hsla| {
        svg()
            .path(icon.path())
            .flex_none()
            .size(dp(size))
            .text_color(color)
    };
    h_flex()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(name)
        .h(scale::nav_row_height())
        .px_3()
        .gap_2()
        .items_center()
        .rounded(theme.radius)
        .text_size(scale::body())
        .when(row.focused, |el| el.bg(fill))
        .hover(move |el| el.bg(fill))
        .children(row.icon.map(|icon| glyph(icon, 16., muted)))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .children(row.detail.map(|detail| {
            div()
                .flex_none()
                .max_w(dp(120.))
                .truncate()
                .text_size(scale::meta())
                .text_color(muted)
                .child(detail)
        }))
        .when(row.checked, |el| {
            el.child(glyph(IconName::Check, 16., foreground))
        })
        .when(row.opens, |el| {
            el.child(glyph(IconName::ChevronRight, 14., muted))
        })
}
