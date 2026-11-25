//! The header of the file at the top of the diff, kept there while that
//! file's lines scroll under it, so a long file never leaves the screen
//! without its name.
//!
//! The diff component keeps its scroll position to itself, so the file at the
//! top is read from where the file headers it draws land, frame by frame.

use gpui_kit::{Bounds, Pixels};

/// Where a file's header was drawn in one frame: the file's place in the
/// diff and the top of the header's text, from the top of the diff on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderAt {
    pub file: usize,
    pub top: f32,
}

/// The headers the diff drew, and the file at its top.
#[derive(Default)]
pub struct HeaderWatch {
    /// The headers of the frame being drawn.
    drawn: Vec<HeaderAt>,
    /// The headers of the last frame that ended.
    last: Vec<HeaderAt>,
    /// The part of the diff on screen.
    viewport: Bounds<Pixels>,
    top: Option<usize>,
}

impl HeaderWatch {
    /// A header of `file` drawn at `bounds` in the diff on screen at `viewport`.
    pub fn record(&mut self, file: usize, bounds: Bounds<Pixels>, viewport: Bounds<Pixels>) {
        self.viewport = viewport;
        self.drawn.retain(|header| header.file != file);
        self.drawn.push(HeaderAt {
            file,
            top: f32::from(bounds.top() - viewport.top()),
        });
    }

    /// Ends a frame, once the diff drew its headers: the file at the top before
    /// it and after it. `inset` is how far a header's text sits below the top
    /// of its row.
    pub fn finish_frame(&mut self, inset: f32) -> (Option<usize>, Option<usize>) {
        let mut drawn = std::mem::take(&mut self.drawn);
        drawn.sort_by(|a, b| a.top.total_cmp(&b.top));
        let before = self.top;
        self.top = top_file(
            &drawn,
            &self.last,
            before,
            f32::from(self.viewport.size.height),
            inset,
        );
        self.last = drawn;
        (before, self.top)
    }

    /// After a jump the headers cannot follow, such as to a line in the middle
    /// of a long file.
    pub fn jumped_to(&mut self, file: usize) {
        self.top = Some(file);
        self.last.clear();
    }

    /// The headers of the last frame that ended, top first.
    pub fn headers(&self) -> &[HeaderAt] {
        &self.last
    }

    pub fn viewport(&self) -> Bounds<Pixels> {
        self.viewport
    }
}

/// The file whose lines are at the top of the diff, `height` tall on screen.
///
/// With a header in view the answer is exact: the file of the first header
/// once its text reached the top (within `inset`), else the file before it.
/// With none in view the screen lies inside one long file, the one the last
/// frame's headers left it in: the last of them when it took less scrolling
/// for that header to leave at the top than for the first to leave at the
/// bottom, else the file that was at the top.
pub fn top_file(
    drawn: &[HeaderAt],
    last: &[HeaderAt],
    top: Option<usize>,
    height: f32,
    inset: f32,
) -> Option<usize> {
    if let Some(first) = drawn.first() {
        return if first.top <= inset {
            Some(first.file)
        } else {
            first.file.checked_sub(1)
        };
    }
    match (last.first(), last.last()) {
        (Some(first), Some(last)) if last.top < height - first.top => Some(last.file),
        _ => top,
    }
}

/// Where the pinned header of `file` goes: the top of its row, `row_height`
/// tall, from the top of the diff, or `None` while the file's own header is in
/// its place, its text not yet under the top edge. The next file's header pushes it up as it arrives, as a pinned
/// folder slides away in the files tree.
pub fn pinned_top(drawn: &[HeaderAt], file: usize, row_height: f32, inset: f32) -> Option<f32> {
    if drawn
        .iter()
        .any(|header| header.file == file && header.top >= 0.)
    {
        return None;
    }
    let push = drawn
        .iter()
        .find(|header| header.file > file)
        .map_or(0., |next| next.top - inset - row_height);
    Some(push.min(0.))
}

#[cfg(test)]
mod tests {
    use super::{HeaderAt, pinned_top, top_file};

    const INSET: f32 = 5.;
    const HEIGHT: f32 = 600.;

    fn at(file: usize, top: f32) -> HeaderAt {
        HeaderAt { file, top }
    }

    #[test]
    fn a_header_in_view_tells_the_file_at_the_top() {
        // The first header at the top: its own file.
        assert_eq!(top_file(&[at(3, 4.)], &[], None, HEIGHT, INSET), Some(3));
        assert_eq!(top_file(&[at(3, -20.)], &[], None, HEIGHT, INSET), Some(3));
        // Lower down: the lines above it are the file before.
        assert_eq!(
            top_file(&[at(3, 200.), at(4, 400.)], &[], None, HEIGHT, INSET),
            Some(2)
        );
        // Above the first file's header there is nothing.
        assert_eq!(top_file(&[at(0, 40.)], &[], None, HEIGHT, INSET), None);
    }

    #[test]
    fn inside_a_long_file_the_last_frame_decides() {
        // Scrolling down: the last header left at the top.
        assert_eq!(
            top_file(&[], &[at(3, -10.), at(4, 30.)], Some(3), HEIGHT, INSET),
            Some(4)
        );
        // Scrolling up: the first header left at the bottom, so the screen is
        // still in the file that was at the top.
        assert_eq!(
            top_file(&[], &[at(4, 570.)], Some(3), HEIGHT, INSET),
            Some(3)
        );
        // No header for a while: the file stays.
        assert_eq!(top_file(&[], &[], Some(7), HEIGHT, INSET), Some(7));
    }

    #[test]
    fn the_pinned_header_shows_once_its_own_slides_under_and_is_pushed_by_the_next() {
        // The file's header is in place at the top: nothing to pin.
        assert_eq!(pinned_top(&[at(3, INSET)], 3, 32., INSET), None);
        // The first file's header has no border above it.
        assert_eq!(pinned_top(&[at(0, INSET - 1.)], 0, 32., INSET), None);
        // Sliding under the top, or gone: pinned at the top.
        assert_eq!(pinned_top(&[at(3, -4.)], 3, 32., INSET), Some(0.));
        assert_eq!(pinned_top(&[], 3, 32., INSET), Some(0.));
        assert_eq!(pinned_top(&[at(4, 300.)], 3, 32., INSET), Some(0.));
        // The next header within one row of the top pushes it up.
        assert_eq!(pinned_top(&[at(4, 25.)], 3, 32., INSET), Some(-12.));
    }
}
