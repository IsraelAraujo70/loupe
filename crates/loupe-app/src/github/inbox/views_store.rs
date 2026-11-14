//! Loupe's saved Reviews views, kept in `reviews-views.json` in the data
//! directory. Loaded once per run on first use; every change is written
//! back at once.

use std::path::PathBuf;

use gpui_kit::{App, Global, SharedString};
use loupe_github::reviews::{self, Loaded, ViewsFile};

struct SavedViews {
    file: ViewsFile,
    /// Why the file on disk could not be used, until the user sees it once.
    warning: Option<SharedString>,
    /// Whether the file on disk could not be used this run.
    recovered: bool,
}

impl Global for SavedViews {}

fn path() -> PathBuf {
    crate::store::data_dir().join("reviews-views.json")
}

fn saved(cx: &mut App) -> &mut SavedViews {
    if !cx.has_global::<SavedViews>() {
        let (file, warning): (ViewsFile, Option<SharedString>) = match reviews::load(&path()) {
            Loaded::Read(file) => (file, None),
            Loaded::Missing => (ViewsFile::default(), None),
            Loaded::Recovered { reason } => (
                ViewsFile::default(),
                Some(
                    format!(
                        "Saved views could not be read ({reason}). A copy is in reviews-views.json.bak."
                    )
                    .into(),
                ),
            ),
        };
        let recovered = warning.is_some();
        cx.set_global(SavedViews {
            file,
            warning,
            recovered,
        });
    }
    cx.global_mut::<SavedViews>()
}

/// A copy of every saved view.
pub fn snapshot(cx: &mut App) -> ViewsFile {
    saved(cx).file.clone()
}

/// The saved views, once loaded: a Reviews page loads them when it opens.
/// For reads that have no `&mut App`, such as a tab's title.
pub fn loaded(cx: &App) -> Option<&ViewsFile> {
    cx.try_global::<SavedViews>().map(|saved| &saved.file)
}

/// Whether the views come from the user's file (or there was none yet),
/// not from an empty set that stands in for a file that could not be used.
pub fn readable(cx: &mut App) -> bool {
    !saved(cx).recovered
}

/// The load warning, once.
pub fn take_warning(cx: &mut App) -> Option<SharedString> {
    saved(cx).warning.take()
}

/// Applies `change` to a copy and writes it; only a successful write replaces
/// the views in memory, so a failed save never looks saved.
pub fn update<R>(
    cx: &mut App,
    change: impl FnOnce(&mut ViewsFile) -> R,
) -> Result<R, SharedString> {
    let mut next = saved(cx).file.clone();
    let result = change(&mut next);
    reviews::save(&path(), &next)
        .map_err(|error| SharedString::from(format!("Couldn’t save views: {error}")))?;
    let saved = saved(cx);
    saved.file = next;
    // The file on disk is the user's again.
    saved.recovered = false;
    cx.refresh_windows();
    Ok(result)
}
