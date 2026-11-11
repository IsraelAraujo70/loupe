//! The themes the app can show: the built-in ones, embedded from
//! `crates/loupe-app/themes/`, and the user's, one JSON file each under
//! `<data dir>/themes/`. [`Themes`] is the global list; Settings > Appearance
//! creates, imports, duplicates, edits, exports, and deletes the user's.

use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};

use super::palette::ThemeFile;

/// The built-in themes, by id, in the order Settings lists them.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("loupe", include_str!("../../themes/loupe.json")),
    ("grove", include_str!("../../themes/grove.json")),
    ("ocean", include_str!("../../themes/ocean.json")),
    ("ember", include_str!("../../themes/ember.json")),
    ("iris", include_str!("../../themes/iris.json")),
    ("tokyo-night", include_str!("../../themes/tokyo-night.json")),
];

/// The theme a fresh install, or a missing theme, falls back to.
pub const DEFAULT_THEME: &str = "loupe";

/// One theme in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeEntry {
    /// The built-in id, or the user file's name without `.json`.
    pub id: String,
    pub file: ThemeFile,
    /// Built-in themes can be duplicated but not edited or deleted.
    pub built_in: bool,
}

/// Every theme, built-in first, then the user's by name.
#[derive(Clone, Debug, Default)]
pub struct Themes {
    pub entries: Vec<ThemeEntry>,
}

impl Global for Themes {}

fn built_in() -> Vec<ThemeEntry> {
    BUILT_IN
        .iter()
        .map(|(id, json)| ThemeEntry {
            id: (*id).to_owned(),
            // Checked by `palette::tests::built_in_themes_pass_the_contrast_rules`.
            file: ThemeFile::parse(json).expect("a built-in theme parses"),
            built_in: true,
        })
        .collect()
}

/// Where the user's themes live.
pub fn themes_dir() -> PathBuf {
    crate::store::data_dir().join("themes")
}

impl Themes {
    /// The built-in themes and every theme file in `dir` that parses. A file
    /// that does not parse is skipped with a line on stderr.
    pub fn load(dir: &Path) -> Themes {
        let fallback = ThemeFile::parse(BUILT_IN[0].1).expect("a built-in theme parses");
        let mut user: Vec<ThemeEntry> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|item| {
                let path = item.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    return None;
                }
                let id = path.file_stem()?.to_str()?.to_owned();
                let text = std::fs::read_to_string(&path).ok()?;
                match ThemeFile::parse_user(&text, &fallback) {
                    Ok(file) => Some(ThemeEntry {
                        id,
                        file,
                        built_in: false,
                    }),
                    Err(error) => {
                        eprintln!("loupe: skipped theme {}: {error}", path.display());
                        None
                    }
                }
            })
            .filter(|entry| !BUILT_IN.iter().any(|(id, _)| *id == entry.id))
            .collect();
        user.sort_by_key(|entry| entry.file.name.to_lowercase());
        let mut entries = built_in();
        entries.extend(user);
        Themes { entries }
    }

    /// The list from the global, or the built-in themes alone before the app
    /// has loaded the user's.
    pub fn get(cx: &App) -> Themes {
        cx.try_global::<Themes>()
            .cloned()
            .unwrap_or_else(|| Themes {
                entries: built_in(),
            })
    }

    /// The default theme's palette for a color scheme.
    pub fn default_palette(dark: bool) -> super::palette::Palette {
        let json = BUILT_IN
            .iter()
            .find(|(id, _)| *id == DEFAULT_THEME)
            .map(|(_, json)| *json)
            .unwrap_or(BUILT_IN[0].1);
        ThemeFile::parse(json)
            .expect("a built-in theme parses")
            .variant(dark)
            .clone()
    }

    pub fn find(&self, id: &str) -> Option<&ThemeEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The theme `id`, or the default theme when there is none by that id.
    pub fn resolve(&self, id: &str) -> &ThemeEntry {
        self.find(id)
            .or_else(|| self.find(DEFAULT_THEME))
            .or(self.entries.first())
            .expect("the built-in themes are always listed")
    }

    /// A free name for a copy of `name`: "Name copy", then "Name copy 2", …
    pub fn copy_name(&self, name: &str) -> String {
        let taken = |candidate: &str| {
            self.entries
                .iter()
                .any(|entry| entry.file.name == candidate)
        };
        let first = format!("{name} copy");
        if !taken(&first) {
            return first;
        }
        (2..)
            .map(|n| format!("{name} copy {n}"))
            .find(|candidate| !taken(candidate))
            .unwrap_or(first)
    }

    /// A free file id for a theme called `name`: its name in lowercase with
    /// dashes, numbered when a listed theme or any file in `dir` has it (a
    /// file that did not parse is not listed, and must not be written over).
    pub fn free_id(&self, name: &str, dir: &Path) -> String {
        let mut slug: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        let slug = slug.trim_matches('-');
        let base = if slug.is_empty() { "theme" } else { slug };
        let taken = |candidate: &str| {
            self.find(candidate).is_some() || dir.join(format!("{candidate}.json")).exists()
        };
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|candidate| !taken(candidate))
            .unwrap_or_else(|| base.to_owned())
    }
}

/// Writes `file` as the user theme `id` in `dir`.
pub fn save(dir: &Path, id: &str, file: &ThemeFile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{id}.json"));
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, file.to_json())?;
    std::fs::rename(&temporary, &path)
}

/// Removes the user theme `id` from `dir`.
pub fn remove(dir: &Path, id: &str) -> std::io::Result<()> {
    std::fs::remove_file(dir.join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::{BUILT_IN, DEFAULT_THEME, Themes, remove, save};
    use crate::theme::palette::ThemeFile;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("loupe-themes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn user_themes_load_after_the_built_in_ones_and_bad_files_are_skipped() {
        let dir = temp_dir("load");
        let mut file = ThemeFile::parse(BUILT_IN[0].1).unwrap();
        file.name = "Zed".into();
        save(&dir, "zed", &file).unwrap();
        file.name = "Alpha".into();
        save(&dir, "alpha", &file).unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        // A file written before a token existed still loads.
        let mut old: serde_json::Value = serde_json::from_str(&file.to_json()).unwrap();
        old["name"] = "Older".into();
        old["light"]
            .as_object_mut()
            .unwrap()
            .remove("segmented_track");
        std::fs::write(dir.join("older.json"), old.to_string()).unwrap();
        // A user file cannot shadow a built-in theme.
        file.name = "Fake Loupe".into();
        save(&dir, "loupe", &file).unwrap();

        let themes = Themes::load(&dir);
        let ids: Vec<&str> = themes
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        let built_in: Vec<&str> = BUILT_IN.iter().map(|(id, _)| *id).collect();
        assert_eq!(&ids[..built_in.len()], &built_in[..]);
        assert_eq!(&ids[built_in.len()..], ["alpha", "older", "zed"]);
        assert_eq!(themes.find("loupe").unwrap().file.name, "Loupe");
        assert!(themes.find("zed").is_some_and(|entry| !entry.built_in));

        remove(&dir, "zed").unwrap();
        assert!(Themes::load(&dir).find("zed").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_theme_falls_back_to_the_default() {
        let themes = Themes::load(&temp_dir("missing"));
        assert_eq!(themes.resolve("gone").id, DEFAULT_THEME);
        assert_eq!(themes.resolve("grove").id, "grove");
    }

    #[test]
    fn copies_get_free_names_and_ids() {
        let dir = temp_dir("names");
        let mut themes = Themes::load(&dir);
        assert_eq!(themes.copy_name("Loupe"), "Loupe copy");
        assert_eq!(themes.free_id("Loupe", &dir), "loupe-2");
        assert_eq!(themes.free_id("Tokyo  Night!", &dir), "tokyo-night-2");
        assert_eq!(themes.free_id("My theme", &dir), "my-theme");
        assert_eq!(themes.free_id("***", &dir), "theme");
        // A file that does not parse is not listed, but its name is taken.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my-theme.json"), "{ broken").unwrap();
        assert!(Themes::load(&dir).find("my-theme").is_none());
        assert_eq!(themes.free_id("My theme", &dir), "my-theme-2");
        let mut entry = themes.entries[0].clone();
        entry.file.name = "Loupe copy".into();
        entry.id = "loupe-copy".into();
        themes.entries.push(entry);
        assert_eq!(themes.copy_name("Loupe"), "Loupe copy 2");
        assert_eq!(themes.free_id("Loupe copy", &dir), "loupe-copy-2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
