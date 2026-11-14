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
    // UI tests run in parallel in one process, which has one data
    // directory: each test thread keeps its own views, so a test never
    // shows the view tabs another one saved.
    if cfg!(test) {
        let thread = format!("{:?}", std::thread::current().id());
        let thread: String = thread.chars().filter(char::is_ascii_digit).collect();
        return crate::store::data_dir().join(format!("reviews-views-{thread}.json"));
    }
    crate::store::data_dir().join("reviews-views.json")
}

fn saved(cx: &mut App) -> &mut SavedViews {
    if !cx.has_global::<SavedViews>() {
        let (file, warning) = match reviews::load(&path()) {
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
