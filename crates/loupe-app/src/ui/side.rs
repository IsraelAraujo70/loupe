//! Shared side column primitives: fixed-width scrollable panel on the leading
//! or trailing edge, structured sections with optional counts and actions, and
//! consistent hairline separation.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{scale, tokens};

/// Which edge of the page a side panel sits on; determines which side carries
/// the subtle hairline separator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideEdge {
    Leading,
    Trailing,
}

/// A scrollable, fixed-width side column with a subtle hairline border on its
/// inner edge. Pass a width token from [`crate::theme::scale`] (such as
/// `scale::side_width()`, `scale::side_wide_width()`, or `scale::nav_width()`).
pub fn side_panel(id: impl Into<ElementId>, edge: SideEdge, width: Rems) -> Stateful<Div> {
    let border = tokens::border_subtle();
    v_flex()
        .id(id)
        .flex_none()
        .w(width)
        .h_full()
        .overflow_y_scroll()
        .map(|panel| match edge {
            SideEdge::Leading => panel.border_r_1().border_color(border),
            SideEdge::Trailing => panel.border_l_1().border_color(border),
        })
}

/// One titled section inside a side panel: 24 px heading row (section label
/// and optional count badge) over its body. Build with
/// [`side_section`].
#[derive(IntoElement)]
pub struct SideSection {
    title: SharedString,
    count: Option<usize>,
    children: Vec<AnyElement>,
}

/// Starts a [`SideSection`] with `title`.
pub fn side_section(title: impl Into<SharedString>) -> SideSection {
    SideSection {
        title: title.into(),
        count: None,
        children: Vec::new(),
    }
}

impl SideSection {
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }
}

impl ParentElement for SideSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SideSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .h_6()
                    .gap_2()
                    .items_center()
                    .child(super::section_label(self.title, cx))
                    .when_some(self.count, |row, count| {
                        row.child(super::count_badge(count, cx))
                    }),
            )
            .children(self.children)
    }
}

/// Lays out side sections 24 px apart with a subtle hairline halfway between
/// adjacent sections; the first starts flush at the top of the panel.
pub fn side_sections(sections: Vec<AnyElement>) -> impl Iterator<Item = Div> {
    sections.into_iter().enumerate().map(|(ix, section)| {
        let slot = div().w_full();
        if ix == 0 {
            slot.child(section)
        } else {
            slot.mt_3()
                .pt_3()
                .border_t_1()
                .border_color(tokens::border_subtle())
                .child(section)
        }
    })
}

/// Quiet placeholder value inside an empty side section (such as "None").
pub fn side_empty(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(scale::body())
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
