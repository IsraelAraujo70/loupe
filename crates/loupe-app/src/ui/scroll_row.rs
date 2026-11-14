//! A row that scrolls sideways and keeps its open item in view: a pane's
//! row of tabs, the saved views of a header.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

/// The sideways scroll of a row, and the open item and row width it last
/// scrolled for. Clones share the scroll. Give the row
/// `.overflow_x_scroll().track_scroll(row.handle())`, call [`Self::follow`]
/// while building it, and pass [`Self::watch_width`] to its
/// `on_children_prepainted`.
#[derive(Clone)]
pub struct ScrollRow<K> {
    scroll: ScrollHandle,
    shown: Rc<Cell<Option<K>>>,
    width: Rc<Cell<Pixels>>,
}

impl<K> Default for ScrollRow<K> {
    fn default() -> Self {
        Self {
            scroll: ScrollHandle::default(),
            shown: Rc::default(),
            width: Rc::default(),
        }
    }
}

impl<K: Copy + PartialEq + 'static> ScrollRow<K> {
    pub fn handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// Scrolls the row just enough to show its child at `child` whole when
    /// the open item `open` changed, or the row's width did (the window, the
    /// sidebar, or what sits beside the row), since it last did. Nothing
    /// happens before the row's first layout, when its width is unknown.
    pub fn follow(&self, open: K, child: usize) {
        let bounds = self.scroll.bounds();
        if bounds == Bounds::default()
            || (self.shown.get() == Some(open) && self.width.get() == bounds.size.width)
        {
            return;
        }
        self.scroll.scroll_to_item(child);
        self.shown.set(Some(open));
        self.width.set(bounds.size.width);
    }

    /// Asks `entity` for another frame once the row was laid out at a new
    /// width: the scroll handle learns the width only after this frame's
    /// scroll ran.
    pub fn watch_width(
        &self,
        entity: EntityId,
    ) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
        let row = self.clone();
        move |_, window, _| {
            if row.scroll.bounds().size.width != row.width.get() {
                window.on_next_frame(move |_, cx| cx.notify(entity));
            }
        }
    }
}
