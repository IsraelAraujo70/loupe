//! The one reading style for Markdown: pull request descriptions, comments,
//! review threads, and Linear issue descriptions and comments all render
//! through [`markdown`], so prose looks the same wherever it is read.

use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::*;

use crate::theme::scale::dp;
use crate::theme::tokens;

/// The widest a column of prose gets: about 90 characters of 14 px Inter.
pub const READING_WIDTH: f32 = 760.;
/// A code block taller than this scrolls inside itself.
const MAX_CODE_HEIGHT: f32 = 360.;

/// Body text is 14 / 21 px with 16 px between paragraphs. Headings step
/// 18, 16, 15, 14 px, always under
/// the page's 20 px title. Code blocks are padded, capped in height, and
/// scroll on their own; tables scroll instead of squeezing their cells.
fn reading_style() -> TextViewStyle {
    let mut code = StyleRefinement::default()
        .max_h(dp(MAX_CODE_HEIGHT))
        .px(dp(12.))
        .py(dp(10.));
    code.overflow.y = Some(Overflow::Scroll);
    let mut table = StyleRefinement::default();
    table.overflow.x = Some(Overflow::Scroll);
    TextViewStyle::default()
        .paragraph_gap(rems(1.))
        .heading_font_size(|level, _| match level {
            1 => dp(18.),
            2 => dp(16.),
            3 => dp(15.),
            _ => dp(14.),
        })
        .code_block(code)
        .table(table)
        .table_cell(StyleRefinement::default().px(dp(10.)).py(dp(6.)))
}

/// A selectable Markdown view of `text` in the reading style. Its links go
/// through `shell::open_link`: a pull request or a Linear issue opens in
/// Loupe, anything else in the browser.
pub fn markdown(id: impl Into<ElementId>, text: impl Into<SharedString>) -> TextView {
    TextView::markdown(id, text)
        .style(reading_style())
        // Inline code is a small box of its own: 4 px of side padding on a
        // fill that differs from the surface under the prose.
        .inline_code_style(
            StyleRefinement::default()
                .px(dp(4.))
                .rounded(dp(4.))
                .bg(tokens::code_inline()),
        )
        .selectable(true)
        .on_link_click(crate::shell::open_link)
        .text_sm()
        .line_height(rems(1.3125))
}
