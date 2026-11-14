//! Presentation pieces shared by every front: the page header, avatars,
//! empty, loading and error states. The shell front owns this file; other
//! fronts call it and keep their own helpers in their own modules. Rules:
//! docs/DESIGN.md.

use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::avatar::Avatar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::empty::{
    Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle,
};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Selectable, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{scale, tokens};

mod menu;
mod reading;
mod segmented;
pub use menu::{MenuRow, menu_row};
pub use reading::{READING_WIDTH, markdown};
pub use segmented::segmented;

/// Height of every page header and of the sidebar header beside it. It is
/// window chrome: it lines up with the macOS traffic lights, so it stays in
/// points rather than following the `rem` scale.
pub const HEADER_HEIGHT: Pixels = px(44.);

/// Window chrome behavior for a title strip. The window sets
/// `app_owns_titlebar_drag` because macOS ignores GPUI drag areas, so the app
/// moves the window itself: a press that then moves drags the window, and a
/// double-click zooms or minimizes it, as the user's macOS setting says.
/// Presses a child already handled (buttons and inputs call
/// `prevent_default`) are left to that child.
pub trait WindowDragArea {
    fn window_drag_area(self) -> Self;
}

impl WindowDragArea for Stateful<Div> {
    fn window_drag_area(self) -> Self {
        self.window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |event, window, cx| {
                if window.default_prevented() {
                    return;
                }
                let double_click = event.click_count == 2;
                cx.set_global(DragPress(!double_click));
                if double_click {
                    window.titlebar_double_click();
                }
            })
            .on_mouse_up(MouseButton::Left, |_, _, cx| {
                cx.set_global(DragPress(false))
            })
            .on_mouse_move(|event, window, cx| {
                let pressed = cx.try_global::<DragPress>().is_some_and(|press| press.0);
                if pressed && event.pressed_button == Some(MouseButton::Left) {
                    cx.set_global(DragPress(false));
                    window.start_window_move();
                }
            })
    }
}

/// A left press on a drag area that has not moved or been released yet. There
/// is one pointer, so one flag serves every drag area.
struct DragPress(bool);

impl Global for DragPress {}

/// The width of `text` set at `size` px in `family`, in px at 100% zoom: the
/// sum of its glyphs' advances, which is what a line of it draws but for
/// kerning. For layout that must know a text's width before it is laid out.
pub fn text_width(text: &str, size: f32, family: &SharedString, cx: &App) -> f32 {
    let system = cx.text_system();
    let id = system.resolve_font(&font(family.clone()));
    text.chars()
        .map(|ch| {
            system
                .advance(id, px(size), ch)
                .map_or(size * 0.6, |advance| f32::from(advance.width))
        })
        .sum()
}

/// "now", "5m", "3h", "2d", "4w", "7mo", "2y", like GitHub's compact ages.
pub fn relative_time(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - at).num_seconds().max(0);
    let (value, unit) = match seconds {
        s if s < 60 => return "now".into(),
        s if s < 3_600 => (s / 60, "m"),
        s if s < 86_400 => (s / 3_600, "h"),
        s if s < 604_800 => (s / 86_400, "d"),
        s if s < 2_592_000 => (s / 604_800, "w"),
        s if s < 31_536_000 => (s / 2_592_000, "mo"),
        s => (s / 31_536_000, "y"),
    };
    format!("{value}{unit}")
}

/// A round avatar loaded from a remote URL. While the image loads, when it
/// fails, or when `url` is empty it shows a neutral person icon.
pub fn avatar(url: impl Into<SharedString>, size: Pixels) -> impl IntoElement {
    base_avatar(url.into(), size)
}

/// Like [`avatar`], with the person's name: the fallback shows their initials
/// on an identity color instead of the generic icon.
pub fn avatar_named(
    url: impl Into<SharedString>,
    name: impl Into<SharedString>,
    size: Pixels,
) -> impl IntoElement {
    base_avatar(url.into(), size).name(name)
}

fn base_avatar(url: SharedString, size: Pixels) -> Avatar {
    let avatar = Avatar::new().with_size(size);
    if url.is_empty() {
        avatar
    } else {
        avatar.src(url)
    }
}

/// The one header row of a screen: icon, title, count, then
/// actions on the right; back and forward live in the top bar. 44 px, a window drag
/// area. Build it with [`page_header`].
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    icon: Option<IconName>,
    prefix: Option<AnyElement>,
    count: Option<usize>,
    after_title: Vec<AnyElement>,
    actions: Vec<AnyElement>,
}

/// The one count badge: 18 px tall, 6 px side padding, a 9 px radius, a
/// neutral fill. Every count beside a title, a tab, or a row uses it.
pub fn count_badge(count: impl std::fmt::Display, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .h(rems(1.125))
        .min_w(rems(1.125))
        .px_1p5()
        .justify_center()
        .rounded(px(9.))
        .bg(theme.secondary)
        .text_size(scale::meta())
        .line_height(rems(1.))
        .text_color(theme.muted_foreground)
        .child(count.to_string())
}

/// The one chip geometry: 20 px tall, 12 px text, 8 px side padding, a pill.
/// Labels, states, counts, and review decisions all use it, whatever size
/// the caller set on the [`Tag`].
pub trait ChipExt {
    fn chip(self) -> Self;
}

impl ChipExt for Tag {
    fn chip(self) -> Self {
        self.h_5()
            .px_2()
            .py_0()
            .flex_none()
            .text_size(scale::meta())
            .rounded(px(9999.))
    }
}

/// A neutral chip: counts, "bot", "Author".
pub fn chip() -> Tag {
    Tag::secondary().chip()
}

/// A chip tinted with `color`: faint fill, colored text, soft border. For
/// states and decisions, where the color carries the meaning.
pub fn tinted_chip(color: Hsla) -> Tag {
    Tag::custom(color.opacity(0.14), color, color.opacity(0.3)).chip()
}

/// A chip that is a control: [`chip`]'s geometry and colors, a hover state,
/// and a 24 px hit area around its 20 px pill that adds nothing to the line
/// it sits on. Clicks on it stay on it, so it can sit inside a row or a card
/// that opens something else. With [`DropdownMenu`] it opens a menu, and
/// keeps its hover look while the menu is open. Build it with
/// [`chip_button`].
#[derive(IntoElement)]
pub struct ChipButton {
    hit: Stateful<Div>,
    pill: Div,
    raised: StyleRefinement,
}

/// Starts a [`ChipButton`]. `name` is what a screen reader announces.
pub fn chip_button(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    cx: &App,
) -> ChipButton {
    let theme = cx.theme();
    let raised = StyleRefinement::default()
        .bg(theme.secondary_hover)
        .border_color(theme.muted_foreground.opacity(0.45));
    let hover = raised.clone();
    ChipButton {
        hit: h_flex()
            .id(id)
            .group(CHIP_BUTTON_GROUP)
            .role(Role::Button)
            .aria_label(name.into())
            .flex_none()
            .h_6()
            .my(rems(-0.125)),
        pill: h_flex()
            .h_5()
            .px_2()
            .border_1()
            .rounded(px(9999.))
            .text_size(scale::meta())
            .line_height(relative(1.25))
            .bg(theme.secondary)
            .text_color(theme.secondary_foreground)
            .border_color(theme.border)
            .group_hover(CHIP_BUTTON_GROUP, move |style| style.refined(hover)),
        raised,
    }
}

const CHIP_BUTTON_GROUP: &str = "chip-button";

impl ParentElement for ChipButton {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.pill.extend(elements);
    }
}

impl Styled for ChipButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.hit.style()
    }
}

impl InteractiveElement for ChipButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.hit.interactivity()
    }
}

impl StatefulInteractiveElement for ChipButton {}

impl Selectable for ChipButton {
    fn selected(mut self, selected: bool) -> Self {
        if selected {
            self.pill.style().refine(&self.raised);
        }
        self
    }

    fn is_selected(&self) -> bool {
        false
    }
}

impl DropdownMenu for ChipButton {}

impl RenderOnce for ChipButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.hit.child(self.pill)
    }
}

/// The height of a labeled action in a header or a form: 28 px, between
/// the kit's small (24) and medium (32), so a row of actions shares one
/// height whatever their variant.
pub trait ControlSize {
    fn control(self) -> Self;
}

impl ControlSize for Button {
    fn control(self) -> Self {
        self.small().h(rems(1.75)).px_2p5()
    }
}

/// One tab of a row of saved views: 32 px tall with 12 px of side padding.
/// The open view is filled and in the foreground color; the others are
/// muted, so the row reads as one selection among quiet alternatives.
pub fn view_tab(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .h_8()
        .px_3()
        .label(label)
        .selected(selected)
        .when(!selected, |tab| tab.text_color(cx.theme().muted_foreground))
}

/// For a [`Button`] that opens a dropdown menu.
pub trait MenuTriggerExt {
    /// Lets accessibility clients open the menu. The menu opens on mouse
    /// down, which GPUI's accessibility Click simulates, but a button
    /// exposes Click only when it has a click handler; this one does nothing.
    fn menu_trigger(self) -> Self;
}

impl MenuTriggerExt for Button {
    fn menu_trigger(self) -> Self {
        self.on_click(|_, _, _| {})
    }
}

/// Starts a [`PageHeader`] for `title`.
pub fn page_header(title: impl Into<SharedString>) -> PageHeader {
    PageHeader {
        title: title.into(),
        icon: None,
        prefix: None,
        count: None,
        after_title: Vec::new(),
        actions: Vec::new(),
    }
}

impl PageHeader {
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// An element drawn before the title in place of the icon, such as a
    /// pull request's state tag.
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Adds an element right after the title (and count), such as Linear's
    /// star and view menu; call it again for more, left to right.
    pub fn after_title(mut self, element: impl IntoElement) -> Self {
        self.after_title.push(element.into_any_element());
        self
    }

    /// Like [`Self::after_title`], for an element that may be absent.
    pub fn children_after_title(mut self, element: Option<impl IntoElement>) -> Self {
        self.after_title
            .extend(element.map(IntoElement::into_any_element));
        self
    }

    /// Adds an action on the right; call it again for more, left to right.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (tokens::border_subtle(), theme.muted_foreground);
        h_flex()
            .id("page-header")
            .h(HEADER_HEIGHT)
            .flex_none()
            .w_full()
            .px(scale::content_inset())
            .gap_2()
            .border_b_1()
            .border_color(border)
            .window_drag_area()
            .when_some(self.icon, |row, icon| {
                row.child(
                    svg()
                        .path(icon.path())
                        .flex_none()
                        .size_4()
                        .text_color(muted),
                )
            })
            .when_some(self.prefix, |row, prefix| {
                row.child(div().flex_none().child(prefix))
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::title())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title),
            )
            .when_some(self.count, |row, count| row.child(count_badge(count, cx)))
            .children(self.after_title)
            .child(div().flex_1().min_w_4())
            .child(h_flex().flex_none().gap_1().children(self.actions))
    }
}

/// The secondary toolbar under a page header: 40 px, content inset, 1 px
/// bottom border. Tabs, filters, and view switchers go here.
pub fn toolbar() -> Div {
    h_flex()
        .h(scale::toolbar_height())
        .flex_none()
        .w_full()
        .px(scale::content_inset())
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(tokens::border_subtle())
}

/// A section label, the heading of a group of controls or facts (a side
/// column's section, a group of settings, the sidebar's groups): 13 px,
/// semibold, muted, in the case it was written.
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> Div {
    group_title(text).text_color(cx.theme().muted_foreground)
}

/// A group title, the heading of a group of content (a section of a list,
/// a board column, a card): 13 px, semibold, in the text color. One step
/// under the page title (14 px) and one over a section label, which has
/// its size and weight in the muted color.
pub fn group_title(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .text_size(scale::body())
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// The card surface: 1 px border, 8 px radius, 12 px padding.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .p(scale::card_padding())
        .gap_2()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
}

/// An icon-only action that sits in a row of 28 px outlined buttons: the
/// same outline and height, 28 px square. `name` is its tooltip and the name
/// a screen reader announces.
pub fn outline_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    name: impl Into<SharedString>,
) -> Button {
    let name: SharedString = name.into();
    Button::new(id)
        .outline()
        .control()
        .w(rems(1.75))
        .px_0()
        .icon(icon)
        .tooltip(name.clone())
        .accessibility_label(name)
}

/// A quiet icon-only toolbar button, 24 px with a 16 px icon. `name` is both
/// its tooltip and the name a screen reader announces.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    name: impl Into<SharedString>,
) -> Button {
    let name: SharedString = name.into();
    Button::new(id)
        .ghost()
        .small()
        .icon(icon)
        .tooltip(name.clone())
        .accessibility_label(name)
}

/// A one-line empty state inside a section or side column: muted icon and
/// text, left-aligned with the section's rows.
pub fn empty_line(icon: IconName, text: impl Into<SharedString>, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap_2()
        .items_center()
        .text_size(scale::meta())
        .text_color(muted)
        .child(
            svg()
                .path(icon.path())
                .flex_none()
                .size_3p5()
                .text_color(muted),
        )
        .child(text.into())
}

/// A centered empty state: icon, one line, optional detail and action. Build
/// it with [`empty_state`].
#[derive(IntoElement)]
pub struct EmptyState {
    icon: IconName,
    title: SharedString,
    detail: SharedString,
    action: Option<AnyElement>,
}

/// Starts an [`EmptyState`]. Pass an empty `detail` to show only the title.
pub fn empty_state(
    icon: IconName,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> EmptyState {
    EmptyState {
        icon,
        title: title.into(),
        detail: detail.into(),
        action: None,
    }
}

impl EmptyState {
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mut header = EmptyHeader::new()
            .media(EmptyMedia::new().child(svg().path(self.icon.path()).size_5().text_color(muted)))
            .title(EmptyTitle::new().child(self.title));
        if !self.detail.is_empty() {
            header = header.description(EmptyDescription::new().child(self.detail));
        }
        let mut empty = Empty::new().header(header);
        if let Some(action) = self.action {
            empty = empty.content(EmptyContent::new().child(action));
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .child(empty)
    }
}

/// `count` loading rows shaped like a list row: status dot, title, trailing
/// meta. Use it only while content is really loading.
pub fn skeleton_rows(count: usize) -> impl IntoElement {
    const TITLE_WIDTHS: [f32; 4] = [0.55, 0.4, 0.65, 0.48];
    v_flex().w_full().children((0..count).map(|index| {
        h_flex()
            .h(scale::row_height())
            .px(scale::content_inset())
            .gap_3()
            .child(Skeleton::new().size_4().rounded_full())
            .child(
                Skeleton::new()
                    .h_3()
                    .w(relative(TITLE_WIDTHS[index % TITLE_WIDTHS.len()])),
            )
            .child(div().flex_1())
            .child(Skeleton::new().secondary().h_3().w_10())
    }))
}

/// An inline error with the message and a Retry button.
pub fn error_banner(
    message: impl Into<SharedString>,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let message: SharedString = message.into();
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(Alert::error("error-banner", message).flex_1())
        .child(
            Button::new("error-banner-retry")
                .outline()
                .small()
                .icon(IconName::RefreshCw)
                .label("Retry")
                .on_click(on_retry),
        )
}

#[cfg(test)]
mod tests {
    use super::relative_time;
    use chrono::DateTime;

    #[test]
    fn compact_ages() {
        let now = DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .to_utc();
        let ago = |seconds| relative_time(now - chrono::Duration::seconds(seconds), now);
        assert_eq!(ago(10), "now");
        assert_eq!(ago(5 * 60), "5m");
        assert_eq!(ago(3 * 3_600), "3h");
        assert_eq!(ago(2 * 86_400), "2d");
        assert_eq!(ago(15 * 86_400), "2w");
        assert_eq!(ago(400 * 86_400), "1y");
    }
}
