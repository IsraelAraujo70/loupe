//! Shared list layout pieces: container, row, column heading, collapsible group
//! heading, and compact navigation row. Every list aligns its content to the
//! 16 px `content_inset` while letting row hover fills reach 8 px past it.

use std::ops::RangeInclusive;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{scale, tokens};

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

/// The row under a list's keyboard cursor, while the list holds the focus.
///
/// Keyboard focus is the one state drawn as a ring: a 1 px accent line
/// inside the row's edge, over the hover fill. Hover alone is the fill, and
/// a selection keeps its own mark (a fill or a check), so focus, hover, and
/// selection never read alike. The row also claims the accessibility focus
/// (`aria_active_descendant`), so a screen reader follows the cursor while
/// the key focus stays on the list; that needs the row's own role and a
/// focused ancestor with a role.
pub trait ListRowFocus {
    fn keyboard_focus(self, focused: bool, cx: &App) -> Self;
}

impl ListRowFocus for Stateful<Div> {
    fn keyboard_focus(self, focused: bool, cx: &App) -> Self {
        if !focused {
            return self;
        }
        let theme = cx.theme();
        self.relative()
            .bg(theme.list_hover)
            .aria_active_descendant()
            .child(focus_ring(theme.radius))
    }
}

/// The keyboard focus ring of a row: a 1 px accent line inside its edge,
/// following its corners.
fn focus_ring(radius: Pixels) -> Div {
    div()
        .absolute()
        .inset_0()
        .rounded(radius)
        .border_1()
        .border_color(tokens::accent())
}

/// A row that acts on a click (a sidebar link, a heading that folds its
/// section), put in the window's Tab order.
///
/// Tab and Shift+Tab stop on it, Enter and Space click it (GPUI's keyboard
/// click), and while it holds the focus it has `fill` and the ring of
/// [`ListRowFocus`], drawn inside `radius`. A press does not take the
/// focus, as a button's does not, so a click leaves the keys where they
/// were. `key` names the row's focus among its siblings for as long as it
/// is drawn.
pub trait TabStopRow: Sized {
    fn tab_stop_row(
        self,
        key: impl Into<ElementId>,
        fill: Hsla,
        radius: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Self;
}

impl TabStopRow for Stateful<Div> {
    fn tab_stop_row(
        self,
        key: impl Into<ElementId>,
        fill: Hsla,
        radius: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let focus = tab_stop_focus(key, window, cx);
        let focused = focus.is_focused(window);
        self.track_focus(&focus)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .when(focused, |row| {
                row.relative().bg(fill).child(focus_ring(radius))
            })
    }
}

/// The focus of the [`TabStopRow`] drawn with `key`.
pub fn tab_stop_focus(key: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(key, cx, |_, cx| cx.focus_handle().tab_stop(true))
        .read(cx)
        .clone()
}

/// Where a list's keyboard cursor goes next among `rows`, the lines on
/// screen in order: one step from `current`, staying at either end. With no
/// cursor yet, or one whose line left the list, it starts at the first row
/// going down and at the last going up.
pub fn step_cursor<T: PartialEq + Clone>(rows: &[T], current: Option<&T>, down: bool) -> Option<T> {
    let last = rows.len().checked_sub(1)?;
    let at = current.and_then(|row| rows.iter().position(|known| known == row));
    let next = match (at, down) {
        (Some(at), true) => (at + 1).min(last),
        (Some(at), false) => at.saturating_sub(1),
        (None, true) => 0,
        (None, false) => last,
    };
    rows.get(next).cloned()
}

/// Scrolls `scroll` the least that shows the whole of the children
/// `children` of the list that `rows` tracks, where the last frame drew
/// them. The list tracks its children with a handle of its own
/// (`track_scroll` on an element that does not scroll), so a row is found
/// where it is whatever the heights above it: a card, an open log, a
/// heading. Passing a group's heading with its first row keeps the heading
/// in sight when the cursor comes up to that row.
pub fn scroll_to_rows(scroll: &ScrollHandle, rows: &ScrollHandle, children: RangeInclusive<usize>) {
    let (Some(first), Some(last)) = (
        rows.bounds_for_item(*children.start()),
        rows.bounds_for_item(*children.end()),
    ) else {
        return;
    };
    let viewport = scroll.bounds();
    let mut offset = scroll.offset();
    if first.top() < viewport.top() {
        offset.y += viewport.top() - first.top();
    } else if last.bottom() > viewport.bottom() {
        offset.y -= last.bottom() - viewport.bottom();
    }
    scroll.set_offset(offset);
}

/// Hairline along the bottom of a `relative()` [`list_row`], inset to the row's
/// content edge so the hover fill extends past it while wide tables stay easy
/// to scan across columns.
pub fn list_row_divider(cx: &App) -> Div {
    div()
        .absolute()
        .left_2()
        .right_2()
        .bottom_0()
        .h(px(1.))
        .bg(cx.theme().border.opacity(0.45))
}

/// The 24 px column heading over a multi-lane list, sitting inside
/// [`list_container`] on the exact leading edge of its [`list_row`]s.
pub fn list_column_header(cx: &App) -> Div {
    h_flex()
        .w_full()
        .h(scale::column_header_height())
        .px_2()
        .mb_1()
        .gap_3()
        .items_center()
        .border_b_1()
        .border_color(tokens::border_subtle())
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

/// What a list's view tests share.
#[cfg(test)]
pub(crate) mod test_support {
    use gpui_kit::*;

    /// Opens the view `build` makes alone in a test window, with the list keys
    /// (J, K, Enter) bound where the app binds them, for a list's view tests.
    pub(crate) fn open_list<V: Render>(
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> (VisualTestContext, Entity<V>) {
        use crate::shell::actions::{OpenSelected, SelectNext, SelectPrevious};
        use crate::shell::menus::LIST_KEYS;

        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(crate::store::Settings::default());
            cx.set_global(crate::services::Services::default());
            cx.set_global(crate::services::Accounts::default());
            cx.bind_keys([
                KeyBinding::new("j", SelectNext, Some(LIST_KEYS)),
                KeyBinding::new("k", SelectPrevious, Some(LIST_KEYS)),
                KeyBinding::new("enter", OpenSelected, Some(LIST_KEYS)),
            ]);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                build,
            )
            .expect("open the window")
        });
        (VisualTestContext::from_window(handle, cx), view)
    }
}
