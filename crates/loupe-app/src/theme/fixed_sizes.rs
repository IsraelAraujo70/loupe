//! Layout scale guard: screen code never writes loose numeric `px(...)`
//! sizes. Sizes in screens come from `theme::scale` (which zooms via `rem` or
//! `scale::dp`), leaving raw `px(...)` only for 0/0.5/1/2 px hairlines and
//! offsets, and for OS window chrome anchored to the macOS traffic lights.

use std::path::Path;

/// Files that legitimately hold OS window-chrome constants or define `scale`.
const CHROME_AND_SCALE_FILES: [&str; 7] = [
    "theme.rs",
    "theme/fixed_sizes.rs",
    "main.rs",
    "ui.rs",
    "shell/mod.rs",
    "shell/sidebar.rs",
    "shell/tab_bar.rs",
];

/// Whether `line` calls the free `px(...)` constructor with a numeric literal
/// larger than a 2 px hairline/offset (skipping method calls like `.px(...)`
/// and line comments).
fn writes_loose_px(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    let bytes = code.as_bytes();
    let mut from = 0;
    while let Some(found) = code[from..].find("px(") {
        let at = from + found;
        let before = at.checked_sub(1).map(|ix| bytes[ix]);
        let is_free_call =
            !before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
        if is_free_call && let Some(end) = code[at + 3..].find(')') {
            let raw = code[at + 3..at + 3 + end].trim();
            if let Ok(value) = raw.trim_end_matches("f32").parse::<f32>()
                && value.abs() > 2.0
            {
                return true;
            }
        }
        from = at + 3;
    }
    false
}

/// Lines of `source` outside `#[cfg(test)]` modules, with their 1-based line
/// numbers preserved.
fn non_test_lines(source: &str) -> Vec<(usize, &str)> {
    let mut kept = Vec::new();
    let mut lines = source.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        if line.trim() != "#[cfg(test)]" {
            kept.push((index + 1, line));
            continue;
        }
        let Some((_, item)) = lines.next() else {
            break;
        };
        if item.trim_end().ends_with(';') || !item.trim_end().ends_with('{') {
            continue;
        }
        let indent = item.len() - item.trim_start().len();
        let close = format!("{}}}", " ".repeat(indent));
        for (_, line) in lines.by_ref() {
            if line == close || line.strip_suffix(';') == Some(close.as_str()) {
                break;
            }
        }
    }
    kept
}

fn loose_px_in_screens(src: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![src.to_path_buf()];
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
            let rel = path
                .strip_prefix(src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if CHROME_AND_SCALE_FILES.contains(&rel.as_str()) {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read a source file");
            for (line_no, line) in non_test_lines(&source) {
                if writes_loose_px(line) {
                    found.push(format!("{}:{}: {}", rel, line_no, line.trim()));
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn no_loose_px_literal_in_screen_code() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = loose_px_in_screens(&src);
    assert!(
        found.is_empty(),
        "use theme::scale tokens or scale::dp(...) instead of raw px(...) in screens; found:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_catches_loose_px_and_allows_hairlines_and_tokens() {
    for line in [
        ".w(px(400.))",
        ".rounded(px(9999.))",
        ".max_h(px(420.))",
        ".left(px(-7.))",
        "svg_icon(IconName::Search, px(16.), muted)",
    ] {
        assert!(writes_loose_px(line), "should flag: {line}");
    }
    for line in [
        ".h(px(1.))",
        ".w(px(2.))",
        ".min(px(0.))",
        ".px(scale::content_inset())",
        ".w(scale::dialog_sm())",
        ".w(dp(400.))",
        "// 16 px inset: px(16.)",
    ] {
        assert!(!writes_loose_px(line), "should allow: {line}");
    }
}
