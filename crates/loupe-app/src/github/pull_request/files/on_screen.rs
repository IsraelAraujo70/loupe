//! A review thread or the composer under a diff line, kept on the part of the
//! diff on screen.
//!
//! While long lines scroll sideways, the component draws a line's annotations
//! inside the rows that scroll, as wide as the longest line. A card there
//! would scroll away with the code and could run past the panel's edge. As on
//! GitHub, the card takes the width on screen instead and stays at its leading
//! edge while the lines move under it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ParentElement as _, Pixels, Style, Styled as _, Window, div, point, px, relative,
};

/// The width the cards of one diff get, shared by all of them: the part of
/// the diff on screen, as the last frame measured it. Zero until a card has
/// been drawn.
pub type CardWidth = Rc<Cell<Pixels>>;

/// `card` in the annotation slot of a line, `fade` clear of the trailing
/// edge, where the panel fades while long lines scroll.
pub fn on_screen(card: impl IntoElement, width: CardWidth, fade: Pixels) -> OnScreen {
    let laid_out = width.get();
    OnScreen {
        child: div()
            .when(laid_out > px(0.), |slot| slot.w(laid_out))
            .child(card)
            .into_any_element(),
        laid_out,
        width,
        fade,
    }
}

pub struct OnScreen {
    child: AnyElement,
    /// The width the card is laid out at in this frame, zero for the slot's.
    laid_out: Pixels,
    width: CardWidth,
    fade: Pixels,
}

impl IntoElement for OnScreen {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for OnScreen {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = self.child.request_layout(window, cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        (window.request_layout(style, [child], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        slot: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // The clip of the rows that scroll sideways is the part of the diff
        // on screen; the component insets the slot 12 px from the row.
        let screen = window.content_mask().bounds;
        let inset = window.rem_size() * 0.75;
        let width = (screen.size.width - inset * 2. - self.fade)
            .min(slot.size.width)
            .max(px(0.));
        // The width is known only once the frame is laid out, so a new one
        // (the window or the tree resized) takes effect in the next frame.
        if self.width.replace(width) != width {
            window.on_next_frame(|window, _| window.refresh());
        }
        let card = if self.laid_out > px(0.) {
            self.laid_out
        } else {
            slot.size.width
        };
        let shift = (screen.left() + inset - slot.left())
            .min(slot.size.width - card)
            .max(px(0.));
        window.with_element_offset(point(shift, px(0.)), |window| {
            self.child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
