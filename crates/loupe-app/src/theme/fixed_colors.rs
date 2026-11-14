//! Issue #41's rule: no color is written in the code. Every color comes from
//! a theme file (`crates/loupe-app/themes/*.json` or the user's), so this
//! test fails on a hex string or an `rgb`/`rgba`/`hsla` literal anywhere in
//! `src/`, comments included. Test modules may hold fixtures (label colors
//! as GitHub and Linear send them) and are skipped.

use std::path::Path;

/// Whether `line` writes a color: a call to one of GPUI's or
/// gpui-component's color constructors (`rgb`, `rgba`, `hsla`, `white`,
/// `red`, `opaque_grey`, the palette helpers such as `blue_500` or `gray`),
/// an `Hsla { .. }` or `Rgba { .. }` literal, `#` and six or eight hex
/// digits, or a quoted `"#fff"` / `"#ffff"`.
fn writes_a_color(line: &str) -> bool {
    let calls = [
        "rgb(",
        "rgba(",
        "hsla(",
        "hsl(",
        "white()",
        "black()",
        "red()",
        "blue()",
        "green()",
        "yellow()",
        "opaque_grey(",
        "transparent_white()",
        "gray(",
        "Hsla {",
        "Rgba {",
    ];
    let bytes = line.as_bytes();
    for call in calls {
        let mut from = 0;
        while let Some(found) = line[from..].find(call) {
            let at = from + found;
            let before = at.checked_sub(1).map(|ix| bytes[ix]);
            // `transparent_black()` and words that end in these are not calls
            // to them, and `-> Hsla {` is a return type, not a literal.
            let return_type = line[..at].trim_end().ends_with("->");
            if !before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') && !return_type {
                return true;
            }
            from = at + call.len();
        }
    }
    if palette_helper(line) {
        return true;
    }
    line.char_indices().any(|(ix, c)| {
        if c != '#' {
            return false;
        }
        let digits = line[ix + 1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        let next = line[ix + 1 + digits..].chars().next();
        let ends = !next.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let quoted = line[..ix].ends_with('"');
        ends && (matches!(digits, 6 | 8) || (quoted && matches!(digits, 3 | 4)))
    })
}

/// gpui-component's Tailwind helpers: a color name, `_`, a shade of two or
/// three digits, `()`, as in `blue_500()`.
fn palette_helper(line: &str) -> bool {
    const NAMES: [&str; 22] = [
        "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime",
        "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia",
        "pink", "rose",
    ];
    NAMES.iter().any(|name| {
        let prefix = format!("{name}_");
        line.match_indices(&prefix).any(|(at, _)| {
            let before = at.checked_sub(1).map(|ix| line.as_bytes()[ix]);
            if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return false;
            }
            let rest = &line[at + prefix.len()..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            matches!(digits, 2 | 3) && rest[digits..].starts_with("()")
        })
    })
}

/// `source` without its `#[cfg(test)]` modules.
fn without_test_modules(source: &str) -> String {
    let mut kept = String::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "#[cfg(test)]" {
            kept.push_str(line);
            kept.push('\n');
            continue;
        }
        // Skip the attribute's item: up to its closing brace at the same
        // indentation, or the line itself when it ends in `;`.
        let Some(item) = lines.next() else { break };
        if item.trim_end().ends_with(';') || !item.trim_end().ends_with('{') {
            continue;
        }
        let indent = item.len() - item.trim_start().len();
        let close = format!("{}}}", " ".repeat(indent));
        for line in lines.by_ref() {
            // `}` closes a module or a function; `};` a const or a static.
            if line == close || line.strip_suffix(';') == Some(close.as_str()) {
                break;
            }
        }
    }
    kept
}

/// Every line under `dir` that writes a color, as `path:line: text`.
fn fixed_colors(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in std::fs::read_dir(&dir).expect("read src").flatten() {
            let path = item.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read a source file");
            // This file names the patterns it looks for; the palette module
            // is the color math (parsing hex, mixing, contrast) and builds
            // `Hsla`/`Rgba` from the theme's values.
            if path.ends_with("theme/fixed_colors.rs") || path.ends_with("theme/palette.rs") {
                continue;
            }
            for (index, line) in without_test_modules(&source).lines().enumerate() {
                if writes_a_color(line) {
                    found.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn no_color_is_written_in_the_code() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = fixed_colors(&src);
    assert!(
        found.is_empty(),
        "colors belong in a theme file (crates/loupe-app/themes); found:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_sees_every_way_of_writing_a_color() {
    for line in [
        "Hsla::from(rgb(0x0e0f11))",
        ".bg(rgba(0xffffff0a))",
        "linear_color_stop(hsla(0., 0., 0., 0.35), 0.)",
        "color: \"#e2e2e2\".into(),",
        "// the window is #0e0f11",
        "\"#ffffff1c\"",
        ".text_color(white())",
        ".bg(gpui::black())",
        ".bg(red())",
        ".bg(opaque_grey(0.5, 1.))",
        ".text_color(blue_500())",
        ".bg(gray(200))",
        "let c = Hsla { h: 0.5, s: 1., l: 0.5, a: 1. };",
        "let shade = |color: Hsla, factor: f32| Hsla {",
        "Rgba { r: 1., g: 0., b: 0., a: 1. }",
        "Rgba::try_from(\"#fff\")",
        "parse(\"#ffff\")",
    ] {
        assert!(writes_a_color(line), "{line}");
    }
    for line in [
        ".border_color(transparent_black())",
        "#[derive(Clone)]",
        "issue #41",
        "see #123456789",
        "pub fn state_color(state: PrState) -> Hsla {",
        "tokens::open()",
        "let shade = |mut color: Hsla, factor: f32| {",
        "a_blue_500()",
        "issue #123 and #fff9 in prose",
        "Some(value)",
    ] {
        assert!(!writes_a_color(line), "{line}");
    }
    let source =
        "fn a() {}\n#[cfg(test)]\nmod tests {\n    const C: &str = \"#888888\";\n}\nfn b() {}\n";
    assert_eq!(without_test_modules(source), "fn a() {}\nfn b() {}\n");
    // A test-only const closes with `};` and the rest of the file is read.
    let source = "#[cfg(test)]\nconst X: S = S {\n    c: 1,\n};\nfn c() { rgb(0x123456) }\n";
    assert_eq!(without_test_modules(source), "fn c() { rgb(0x123456) }\n");
}
