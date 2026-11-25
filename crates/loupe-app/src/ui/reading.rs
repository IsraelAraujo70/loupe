//! The one reading style for Markdown: pull request descriptions, comments,
//! review threads, and Linear issue descriptions and comments all render
//! through [`markdown`], so prose looks the same wherever it is read.
//! GitHub's alerts (`> [!NOTE]`, `> [!WARNING]`, ...) read as callouts
//! there too, as they do on GitHub.

use gpui_kit::assets::IconName;
use gpui_kit::component::text::{
    MarkdownNode, MarkdownParseContext, TextView, TextViewStyle, markdown_ast,
};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::theme::scale::{self, dp};
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
        .markdown_block_parser(parse_alert)
        .markdown_block_renderer(ALERT, render_alert)
        .on_link_click(crate::shell::open_link)
        .text_sm()
        .line_height(rems(1.3125))
}

/// The name of the custom Markdown node an alert parses to.
const ALERT: &str = "github-alert";

/// The five kinds of GitHub alert, by the marker that opens the quote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The kind a quote's first line names: `[!NOTE]` alone on its line, in
    /// any case, as GitHub reads it.
    fn from_marker(line: &str) -> Option<AlertKind> {
        let name = line.trim().strip_prefix("[!")?.strip_suffix(']')?;
        [
            AlertKind::Note,
            AlertKind::Tip,
            AlertKind::Important,
            AlertKind::Warning,
            AlertKind::Caution,
        ]
        .into_iter()
        .find(|kind| kind.title().eq_ignore_ascii_case(name))
    }

    fn title(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }

    /// GitHub's symbol for each kind, from the bundled Lucide set.
    fn icon(self) -> IconName {
        match self {
            AlertKind::Note => IconName::Info,
            AlertKind::Tip => IconName::Lightbulb,
            AlertKind::Important => IconName::MessageSquareWarning,
            AlertKind::Warning => IconName::TriangleAlert,
            AlertKind::Caution => IconName::OctagonAlert,
        }
    }

    /// GitHub's color for each kind, from the theme: blue, green, purple,
    /// amber, red.
    fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            AlertKind::Note => theme.info,
            AlertKind::Tip => theme.success,
            AlertKind::Important => tokens::merged(),
            AlertKind::Warning => theme.warning,
            AlertKind::Caution => theme.danger,
        }
    }
}

/// A GitHub alert: its kind and the Markdown inside it, without the marker.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Alert {
    kind: AlertKind,
    body: SharedString,
}

impl Alert {
    /// The alert a blockquote's source spells, or `None` for an ordinary
    /// quote. The source is the quote as written, `>` markers included.
    fn parse(quote: &str) -> Option<Alert> {
        let mut lines = quote.lines().map(unquote);
        let kind = AlertKind::from_marker(lines.next()?)?;
        let body = lines.collect::<Vec<_>>().join("\n");
        if body.trim().is_empty() {
            return None;
        }
        Some(Alert {
            kind,
            body: body.into(),
        })
    }
}

/// A quoted line without its `>` and the space after it. A lazy
/// continuation line, which has no `>`, stays as written.
fn unquote(line: &str) -> &str {
    match line.trim_start().strip_prefix('>') {
        Some(rest) => rest.strip_prefix(' ').unwrap_or(rest),
        None => line,
    }
}

fn parse_alert(node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
    let markdown_ast::Node::Blockquote(_) = node else {
        return None;
    };
    let source = cx.node_source(node)?;
    let alert = Alert::parse(source)?;
    let text = format!("{}\n{}", alert.kind.title(), alert.body);
    let label = alert.kind.title();
    Some(
        MarkdownNode::new(ALERT, alert)
            .text(text)
            .markdown(source.to_owned())
            .accessibility_label(label),
    )
}

/// An alert as a callout: a bar and a faint wash in the kind's color, its
/// icon and title, then its Markdown in the reading style.
fn render_alert(node: &MarkdownNode, _: &mut Window, cx: &mut App) -> AnyElement {
    let Some(alert) = node.data::<Alert>() else {
        return Empty.into_any_element();
    };
    let color = alert.kind.color(cx);
    let at = node.source_range().map_or(0, |range| range.start);
    v_flex()
        .id(("alert", at))
        .role(Role::Note)
        .aria_label(alert.kind.title())
        .w_full()
        .gap_1()
        .px_3()
        .py_2()
        .border_l_2()
        .border_color(color)
        .bg(color.opacity(0.08))
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(Icon::new(alert.kind.icon()).with_size(scale::icon_sm()))
                .child(alert.kind.title()),
        )
        .child(markdown(("alert-body", at), alert.body.clone()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Alert, AlertKind};

    fn alert(kind: AlertKind, body: &str) -> Option<Alert> {
        Some(Alert {
            kind,
            body: body.into(),
        })
    }

    #[test]
    fn each_github_alert_marker_reads_as_its_kind() {
        for (marker, kind) in [
            ("NOTE", AlertKind::Note),
            ("TIP", AlertKind::Tip),
            ("IMPORTANT", AlertKind::Important),
            ("WARNING", AlertKind::Warning),
            ("CAUTION", AlertKind::Caution),
        ] {
            let quote = format!("> [!{marker}]\n> Read **this** first.");
            assert_eq!(
                Alert::parse(&quote),
                alert(kind, "Read **this** first."),
                "{marker}"
            );
        }
    }

    #[test]
    fn the_marker_is_read_in_any_case() {
        assert_eq!(
            Alert::parse("> [!note]\n> Lower case."),
            alert(AlertKind::Note, "Lower case.")
        );
    }

    #[test]
    fn the_body_keeps_its_paragraphs_lazy_lines_and_nested_quotes() {
        let quote =
            "> [!WARNING]\n> Test data.\nstill the same paragraph\n>\n> > quoted\n>     code";
        assert_eq!(
            Alert::parse(quote),
            alert(
                AlertKind::Warning,
                "Test data.\nstill the same paragraph\n\n> quoted\n    code"
            )
        );
    }

    #[test]
    fn an_ordinary_quote_is_not_an_alert() {
        assert_eq!(Alert::parse("> Just a quote."), None);
        assert_eq!(Alert::parse("> [!NOTE] on the marker's line"), None);
        assert_eq!(Alert::parse("> [!DANGER]\n> Not a GitHub kind."), None);
        assert_eq!(Alert::parse("> [!NOTE]"), None);
    }
}
