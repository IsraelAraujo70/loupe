//! Syntax highlighting for code the GitHub screens show outside a markdown
//! body: which grammar a path uses, and styled runs for a few lines of a
//! review thread's code context. The diff viewer highlights its own hunks in
//! the background from the language set on each `DiffFile`; markdown code
//! blocks are highlighted by `TextView`. All three read the theme's
//! `highlight_theme`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::HighlightStyle;
use gpui_kit::component::Rope;
use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};

/// The highlighter language name for a changed file's path, such as
/// `typescript` or `rust`; `text` when no compiled grammar fits.
pub fn language_for_path(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    match name.as_str() {
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".envrc" => return "bash",
        "cargo.lock" | "poetry.lock" | "uv.lock" => return "toml",
        _ => {}
    }
    let Some((_, extension)) = name.rsplit_once('.') else {
        return "text";
    };
    match extension {
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        // The JavaScript grammar parses JSX.
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "rs" => "rust",
        "json" | "jsonc" | "json5" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "mdx" | "markdown" => "markdown",
        "css" | "scss" | "less" => "css",
        "html" | "htm" => "html",
        "sh" | "bash" | "zsh" => "bash",
        "go" => "go",
        "py" | "pyi" => "python",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        _ => "text",
    }
}

/// Styled runs for each of `lines`, highlighted together as one snippet so
/// a construct spanning lines keeps its color. Ranges are byte offsets into
/// each line. Plain text and unknown languages get no runs.
pub fn highlight_lines(
    language: &str,
    lines: &[&str],
    theme: &HighlightTheme,
) -> Vec<Vec<(Range<usize>, HighlightStyle)>> {
    let mut per_line = vec![Vec::new(); lines.len()];
    if language == "text" || lines.is_empty() {
        return per_line;
    }
    thread_local! {
        // Building a highlighter compiles the grammar's queries; keep one per
        // language, as gpui-component's TextView does for code blocks.
        static HIGHLIGHTERS: RefCell<HashMap<String, SyntaxHighlighter>> =
            RefCell::new(HashMap::new());
    }
    let text = lines.join("\n");
    let styles = HIGHLIGHTERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let highlighter = cache
            .entry(language.to_owned())
            .or_insert_with(|| SyntaxHighlighter::new(language));
        // `None` tells the highlighter the whole text changed: a full parse.
        highlighter.update(None, &Rope::from_str(&text), None);
        highlighter.styles(&(0..text.len()), theme)
    });

    let mut starts = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for line in lines {
        starts.push(offset);
        offset += line.len() + 1;
    }
    for (range, style) in styles {
        for (ix, line) in lines.iter().enumerate() {
            let (start, end) = (starts[ix], starts[ix] + line.len());
            let from = range.start.max(start);
            let to = range.end.min(end);
            if from < to {
                per_line[ix].push((from - start..to - start, style));
            }
        }
    }
    per_line
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry};

    use super::{highlight_lines, language_for_path};

    #[test]
    fn extensions_map_to_grammars() {
        let cases = [
            ("src/app.ts", "typescript"),
            ("src/worker.mts", "typescript"),
            ("src/config.cts", "typescript"),
            ("web/Button.tsx", "tsx"),
            ("web/index.js", "javascript"),
            ("web/old.jsx", "javascript"),
            ("scripts/run.mjs", "javascript"),
            ("crates/a/src/lib.rs", "rust"),
            ("package.json", "json"),
            ("tsconfig.jsonc", "json"),
            (".github/workflows/ci.yml", "yaml"),
            ("docker-compose.yaml", "yaml"),
            ("Cargo.toml", "toml"),
            ("Cargo.lock", "toml"),
            ("README.md", "markdown"),
            ("styles/app.scss", "css"),
            ("public/index.html", "html"),
            ("scripts/boat", "text"),
            ("scripts/setup.sh", "bash"),
            ("cmd/main.go", "go"),
            ("tools/gen.py", "python"),
            ("migrations/001.sql", "sql"),
            ("schemas/github.graphql", "graphql"),
            ("LICENSE", "text"),
            ("assets/logo.png", "text"),
            ("SRC/UPPER.TS", "typescript"),
        ];
        for (path, language) in cases {
            assert_eq!(language_for_path(path), language, "{path}");
        }
    }

    /// Every language the mapping can return has a compiled grammar: a
    /// missing `tree-sitter-*` feature on gpui-kit fails here, not silently
    /// in the app as plain text.
    #[test]
    fn every_mapped_language_has_a_grammar() {
        let registry = LanguageRegistry::singleton();
        for language in [
            "typescript",
            "tsx",
            "javascript",
            "rust",
            "json",
            "yaml",
            "toml",
            "markdown",
            "css",
            "html",
            "bash",
            "go",
            "python",
            "sql",
            "graphql",
        ] {
            let config = registry
                .language(language)
                .unwrap_or_else(|| panic!("{language} is not registered"));
            assert!(config.has_grammar(), "{language} has no grammar");
        }
    }

    #[test]
    fn snippet_lines_get_runs_within_each_line() {
        let theme = HighlightTheme::default_dark();
        let lines = ["fn main() {", "    let x = \"hi\";", "}"];
        let runs = highlight_lines("rust", &lines, &theme);
        assert_eq!(runs.len(), 3);
        assert!(!runs[0].is_empty(), "`fn` should be styled");
        for (line, line_runs) in lines.iter().zip(&runs) {
            for (range, _) in line_runs {
                assert!(range.end <= line.len());
            }
        }
        assert!(
            highlight_lines("text", &lines, &theme)
                .iter()
                .all(Vec::is_empty)
        );
    }
}
