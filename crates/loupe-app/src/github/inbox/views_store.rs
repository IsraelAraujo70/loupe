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
}

impl Global for SavedViews {}

fn path() -> PathBuf {
    crate::store::data_dir().join("reviews-views.json")
}

fn saved(cx: &mut App) -> &mut SavedViews {
    if !cx.has_global::<SavedViews>() {
        // Tests run in parallel in one process and share its data
        // directory: each starts on no saved views, not on those another
        // test saved.
        let loaded = if cfg!(test) {
            Loaded::Missing
        } else {
            reviews::load(&path())
        };
        let (file, warning) = match loaded {
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
        cx.set_global(SavedViews { file, warning });
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
    saved(cx).file = next;
    cx.refresh_windows();
    Ok(result)
}
