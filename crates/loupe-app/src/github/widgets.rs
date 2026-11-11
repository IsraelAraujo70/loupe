//! Small visual pieces the GitHub screens share: state icons and tags, checks
//! and review indicators, labels, diff statistics, and file icons.

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::*;
use loupe_github::{ChecksState, Label, PrState};

use crate::theme::{scale, tokens};
use crate::ui::ChipExt as _;

/// The color GitHub uses for a pull request's state.
pub fn state_color(state: PrState, is_draft: bool) -> Hsla {
    match (state, is_draft) {
        (PrState::Open, true) => tokens::draft(),
        (PrState::Open, false) => tokens::open(),
        (PrState::Merged, _) => tokens::merged(),
        (PrState::Closed, _) => tokens::closed(),
    }
}

fn state_icon_name(state: PrState, is_draft: bool) -> IconName {
    match (state, is_draft) {
        (PrState::Open, true) => IconName::GitPullRequestDraft,
        (PrState::Open, false) => IconName::GitPullRequest,
        (PrState::Merged, _) => IconName::GitMerge,
        (PrState::Closed, _) => IconName::GitPullRequestClosed,
    }
}

/// The pull request glyph for a state, colored like GitHub's. The glyph
/// differs per state, so the state never rests on color alone.
pub fn state_icon(state: PrState, is_draft: bool) -> Icon {
    Icon::new(state_icon_name(state, is_draft))
        .with_size(scale::dp(16.))
        .text_color(state_color(state, is_draft))
}

pub fn state_label(state: PrState, is_draft: bool) -> &'static str {
    match (state, is_draft) {
        (PrState::Open, true) => "Draft",
        (PrState::Open, false) => "Open",
        (PrState::Merged, _) => "Merged",
        (PrState::Closed, _) => "Closed",
    }
}

/// A chip tinted with `color`: faint fill, soft border, colored text.
pub fn tinted_tag(color: Hsla) -> Tag {
    crate::ui::tinted_chip(color)
}

/// The fill of a state pill: the state color darkened until white text on
/// it passes 4.5:1, as on GitHub.
fn state_fill(state: PrState, is_draft: bool) -> Hsla {
    match (state, is_draft) {
        (PrState::Open, true) => tokens::draft_fill(),
        (PrState::Open, false) => tokens::open_fill(),
        (PrState::Merged, _) => tokens::merged_fill(),
        (PrState::Closed, _) => tokens::closed_fill(),
    }
}

/// The state pill of a pull request header: a solid fill with the glyph
/// and label in white.
pub fn state_tag(state: PrState, is_draft: bool) -> Tag {
    let fill = state_fill(state, is_draft);
    let color = tokens::on_accent();
    Tag::custom(fill, color, fill).chip().child(
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Icon::new(state_icon_name(state, is_draft))
                    .with_size(scale::dp(14.))
                    .text_color(color),
            )
            .child(state_label(state, is_draft)),
    )
}

pub fn checks_color(state: ChecksState) -> Hsla {
    match state {
        ChecksState::Pending => tokens::pending(),
        ChecksState::Passing => tokens::open(),
        ChecksState::Failing => tokens::closed(),
    }
}

pub fn checks_icon_name(state: ChecksState) -> IconName {
    match state {
        ChecksState::Pending => IconName::CircleDashed,
        ChecksState::Passing => IconName::CircleCheck,
        ChecksState::Failing => IconName::CircleX,
    }
}

pub fn checks_icon(state: ChecksState) -> Icon {
    Icon::new(checks_icon_name(state))
        .with_size(scale::dp(14.))
        .text_color(checks_color(state))
}

pub fn checks_label(state: ChecksState) -> &'static str {
    match state {
        ChecksState::Pending => "Checks pending",
        ChecksState::Passing => "Checks passing",
        ChecksState::Failing => "Checks failing",
    }
}

/// A repository label in its GitHub color. The color is the label's data,
/// read from GitHub, not a palette choice.
pub fn label_tag(label: &Label) -> Tag {
    let color: Hsla =
        crate::theme::palette::data_color(&label.color).unwrap_or_else(tokens::text_muted);
    Tag::custom(
        color.opacity(0.16),
        color.blend(tokens::text().opacity(0.4)),
        color.opacity(0.3),
    )
    .chip()
    .child(label.name.clone())
}

/// `+12 −3` in the diff colors, 12 px.
pub fn diff_stat(additions: u32, deletions: u32) -> Div {
    h_flex()
        .flex_none()
        .gap_1()
        .text_size(scale::meta())
        .child(
            div()
                .text_color(tokens::diff_added_text())
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_color(tokens::diff_removed_text())
                .child(format!("−{deletions}")),
        )
}

/// A file-type glyph for a path, from its extension or well-known name.
pub fn file_icon(path: &str) -> IconName {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let extension = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    match (name.as_str(), extension) {
        ("dockerfile" | "makefile" | "justfile", _) => IconName::FileCog,
        (_, "lock") | ("cargo.lock" | "package-lock.json", _) => IconName::FileLock,
        (
            _,
            "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "go" | "py" | "rb" | "java" | "kt"
            | "swift" | "c" | "h" | "cpp" | "cc" | "hpp" | "cs" | "php" | "scala" | "ex" | "exs"
            | "zig" | "lua" | "vue" | "svelte" | "graphql" | "sql",
        ) => IconName::FileCode,
        (_, "json" | "jsonc" | "json5") => IconName::FileBraces,
        (_, "toml" | "yaml" | "yml" | "ini" | "env" | "cfg" | "conf" | "xml" | "plist") => {
            IconName::FileCog
        }
        (_, "sh" | "bash" | "zsh" | "fish" | "ps1") => IconName::FileTerminal,
        (_, "md" | "mdx" | "txt" | "rst" | "adoc") => IconName::FileText,
        (_, "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "icns") => {
            IconName::FileImage
        }
        (_, "css" | "scss" | "sass" | "less" | "html" | "htm") => IconName::FileCode,
        (_, "zip" | "tar" | "gz" | "tgz") => IconName::FileArchive,
        _ => IconName::File,
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::assets::IconName;

    use super::file_icon;

    #[test]
    fn file_icons_follow_the_extension() {
        assert_eq!(file_icon("src/main.rs"), IconName::FileCode);
        assert_eq!(file_icon("Cargo.lock"), IconName::FileLock);
        assert_eq!(file_icon("config/app.toml"), IconName::FileCog);
        assert_eq!(file_icon("README.md"), IconName::FileText);
        assert_eq!(file_icon("assets/icon.svg"), IconName::FileImage);
        assert_eq!(file_icon("Dockerfile"), IconName::FileCog);
        assert_eq!(file_icon("LICENSE"), IconName::File);
    }
}
