//! Loupe's saved Reviews views, kept in `reviews-views.json` in the data
//! directory. Loaded once per run on first use; every change is written
//! back at once.

use std::path::PathBuf;

use gpui_kit::{App, Global, SharedString};
use loupe_github::reviews::{self, Loaded, ViewsFile};

struct SavedViews {
    file: ViewsFile,
    /// Where the views are read from and written to.
    path: PathBuf,
    /// Why the file on disk could not be used, until the user sees it once.
    warning: Option<SharedString>,
}

impl Global for SavedViews {}

/// `reviews-views.json` in the data directory. Tests run in parallel in one
/// process and share its data directory, so each test app gets a file of
/// its own: none reads views another saved, and no two race on one file.
fn path() -> PathBuf {
    let name = if cfg!(test) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        format!(
            "reviews-views-{}.json",
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    } else {
        "reviews-views.json".to_owned()
    };
    crate::store::data_dir().join(name)
}

fn saved(cx: &mut App) -> &mut SavedViews {
    if !cx.has_global::<SavedViews>() {
        let path = path();
        let (file, warning) = match reviews::load(&path) {
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
        cx.set_global(SavedViews {
            file,
            path,
            warning,
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
    let saved = saved(cx);
    let mut next = saved.file.clone();
    let result = change(&mut next);
    reviews::save(&saved.path, &next)
        .map_err(|error| SharedString::from(format!("Couldn’t save views: {error}")))?;
    saved.file = next;
    cx.refresh_windows();
    Ok(result)
}
