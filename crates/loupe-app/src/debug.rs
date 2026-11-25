//! Launch overrides for evaluating the UI from scripts (`scripts/ux-shot.sh`).
//! Debug builds only: a release binary ignores every variable here.

fn var(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// `LOUPE_DEBUG_WINDOW=1916x1030`: the main window's size in logical pixels.
pub fn window_size() -> Option<(f32, f32)> {
    let value = var("LOUPE_DEBUG_WINDOW")?;
    let (width, height) = value.trim().split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// `LOUPE_DEBUG_OPEN=loupe://pr/<owner>/<repo>/<n>`: a page to open on launch,
/// in any form the system can hand to Loupe.
pub fn open_url() -> Option<String> {
    var("LOUPE_DEBUG_OPEN")
}

/// `LOUPE_DEBUG_PR_TAB=guide`: the tab a pull request page opens on, by its
/// title in any case.
pub fn pr_tab() -> Option<String> {
    var("LOUPE_DEBUG_PR_TAB")
}
