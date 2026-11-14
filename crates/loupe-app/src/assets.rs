//! The app's asset source: Zed's file-tree icons (see
//! `assets/zed-icons/PROVENANCE.md`) embedded with `include_bytes!`, over
//! gpui-kit's icon catalog, and the interface fonts (see
//! `assets/fonts/PROVENANCE.md`).
//!
//! gpui-kit's `AllAssets` uses rust-embed, which in a debug build reads the
//! SVGs from the machine that compiled the crate. The workspace `Cargo.toml`
//! turns debug assertions off for that crate, so every build embeds them and
//! a debug binary draws its icons on any machine.

use std::borrow::Cow;

use gpui_kit::*;

macro_rules! zed_icons {
    ($($name:literal),* $(,)?) => {
        const ZED_ICONS: &[(&str, &[u8])] = &[
            $((
                concat!("icons/zed/", $name, ".svg"),
                include_bytes!(concat!("../assets/zed-icons/", $name, ".svg")),
            )),*
        ];
    };
}

zed_icons!(
    "audio",
    "book",
    "c",
    "check",
    "chevron_down",
    "chevron_right",
    "code",
    "cpp",
    "css",
    "dash",
    "database",
    "diff",
    "docker",
    "editorconfig",
    "elixir",
    "eslint",
    "file",
    "folder",
    "folder_open",
    "font",
    "git",
    "go",
    "graphql",
    "html",
    "image",
    "info",
    "java",
    "javascript",
    "kotlin",
    "lock",
    "lua",
    "nix",
    "php",
    "prettier",
    "prisma",
    "python",
    "react",
    "ruby",
    "rust",
    "sass",
    "settings",
    "swift",
    "terminal",
    "terraform",
    "toml",
    "typescript",
    "video",
    "vue",
    "yaml",
    "zig",
);

/// Inter (the interface) and JetBrains Mono (code), for
/// `TextSystem::add_fonts`. The weights are the ones the views set: regular,
/// medium, semibold, and bold and italic for Markdown.
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    macro_rules! font {
        ($name:literal) => {
            Cow::Borrowed(include_bytes!(concat!("../assets/fonts/", $name)).as_slice())
        };
    }
    vec![
        font!("Inter-Regular.ttf"),
        font!("Inter-Medium.ttf"),
        font!("Inter-SemiBold.ttf"),
        font!("Inter-Bold.ttf"),
        font!("Inter-Italic.ttf"),
        font!("JetBrainsMono-Regular.ttf"),
        font!("JetBrainsMono-Bold.ttf"),
        font!("JetBrainsMono-Italic.ttf"),
    ]
}

pub const CHEVRON_RIGHT: &str = "icons/zed/chevron_right.svg";
pub const CHEVRON_DOWN: &str = "icons/zed/chevron_down.svg";
pub const FOLDER: &str = "icons/zed/folder.svg";
pub const FOLDER_OPEN: &str = "icons/zed/folder_open.svg";
pub const CHECK: &str = "icons/zed/check.svg";
pub const DASH: &str = "icons/zed/dash.svg";

/// Serves the embedded Zed icons and hands every other path to gpui-kit.
pub struct LoupeAssets;

impl AssetSource for LoupeAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ZED_ICONS.iter().find(|(icon, _)| *icon == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::AllAssets.list(path)?;
        paths.extend(
            ZED_ICONS
                .iter()
                .filter(|(icon, _)| icon.starts_with(path))
                .map(|(icon, _)| SharedString::from(*icon)),
        );
        Ok(paths)
    }
}

/// The Zed icon for a file, from its whole name first and then its
/// extension, or the generic file icon.
pub fn file_icon(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_lowercase();
    let by_name = match lower.as_str() {
        "dockerfile"
        | "containerfile"
        | ".dockerignore"
        | "docker-compose.yml"
        | "docker-compose.yaml"
        | "compose.yml"
        | "compose.yaml" => Some("docker"),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => Some("git"),
        ".editorconfig" => Some("editorconfig"),
        ".prettierrc" | ".prettierignore" => Some("prettier"),
        _ if lower.starts_with("eslint.config.") || lower.starts_with(".eslintrc") => {
            Some("eslint")
        }
        _ if lower.starts_with("prettier.config.") || lower.starts_with(".prettierrc.") => {
            Some("prettier")
        }
        _ => None,
    };
    let key = by_name.unwrap_or_else(|| {
        let extension = lower.rsplit_once('.').map_or("", |(_, ext)| ext);
        match extension {
            "rs" => "rust",
            "ts" | "mts" | "cts" => "typescript",
            "tsx" | "jsx" => "react",
            "js" | "mjs" | "cjs" => "javascript",
            "json" | "jsonc" => "code",
            "md" | "markdown" | "mdx" | "txt" | "pdf" | "doc" | "docx" => "book",
            "toml" => "toml",
            "yml" | "yaml" => "yaml",
            "sql" | "db" | "sqlite" | "csv" => "database",
            "sh" | "bash" | "zsh" | "fish" | "ps1" => "terminal",
            "prisma" => "prisma",
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "avif" => "image",
            "lock" => "lock",
            "html" | "htm" | "xml" | "plist" | "hbs" => "html",
            "css" | "pcss" => "css",
            "scss" | "sass" => "sass",
            "py" => "python",
            "go" | "mod" => "go",
            "java" => "java",
            "kt" => "kotlin",
            "swift" => "swift",
            "rb" => "ruby",
            "php" => "php",
            "c" | "h" => "c",
            "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
            "graphql" | "gql" | "graphqls" => "graphql",
            "ini" | "conf" | "cfg" => "settings",
            "log" => "info",
            "diff" | "patch" => "diff",
            "vue" => "vue",
            "ttf" | "otf" | "woff" | "woff2" => "font",
            "mp3" | "wav" | "ogg" | "flac" | "m4a" => "audio",
            "mp4" | "mov" | "webm" | "mkv" => "video",
            "ex" | "exs" | "heex" => "elixir",
            "lua" => "lua",
            "nix" => "nix",
            "tf" | "tfvars" => "terraform",
            "zig" => "zig",
            _ => "file",
        }
    });
    ZED_ICONS
        .iter()
        .find(|(icon, _)| icon.strip_prefix("icons/zed/") == Some(&format!("{key}.svg")))
        .map_or("icons/zed/file.svg", |(icon, _)| icon)
}

#[cfg(test)]
mod tests {
    use super::{LoupeAssets, ZED_ICONS, file_icon};
    use gpui_kit::AssetSource as _;

    #[test]
    fn files_get_the_zed_icon_for_their_name_or_extension() {
        assert_eq!(file_icon("src/main.rs"), "icons/zed/rust.svg");
        assert_eq!(file_icon("web/App.tsx"), "icons/zed/react.svg");
        assert_eq!(file_icon("api/index.ts"), "icons/zed/typescript.svg");
        assert_eq!(file_icon("Dockerfile"), "icons/zed/docker.svg");
        assert_eq!(file_icon("deploy/compose.yaml"), "icons/zed/docker.svg");
        assert_eq!(file_icon(".gitignore"), "icons/zed/git.svg");
        assert_eq!(file_icon("prisma/schema.prisma"), "icons/zed/prisma.svg");
        assert_eq!(file_icon("README.md"), "icons/zed/book.svg");
        assert_eq!(file_icon("certs/RootCAv10.crt"), "icons/zed/file.svg");
        assert_eq!(file_icon("LICENSE"), "icons/zed/file.svg");
    }

    #[test]
    fn every_embedded_icon_loads_and_unknown_paths_fall_through() {
        for (path, _) in ZED_ICONS {
            let bytes = LoupeAssets.load(path).expect("load").expect("embedded");
            assert!(bytes.starts_with(b"<svg"), "{path}");
        }
        assert!(LoupeAssets.load("icons/zed/nope.svg").is_err());
    }
}
