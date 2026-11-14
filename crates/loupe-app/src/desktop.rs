//! Linux desktop integration: Loupe ships as one binary, so on launch it registers itself with the
//! desktop the way an installed app would: a `.desktop` entry under `$XDG_DATA_HOME/applications`
//! that starts this binary, and its icon in the `hicolor` theme. The window's app id
//! ([`APP_ID`]) matches the entry, which is how X11 and Wayland docks find the icon.
//!
//! Each file is written only when its contents differ, so a launch after an update in place (or from
//! another path) refreshes `Exec`, and every other launch writes nothing. Failures are reported and
//! ignored: the app works without the entry. It is skipped when `LOUPE_DESKTOP_ENTRY=0`, for a build
//! under a Cargo `target/` directory (`cargo run` would fight the installed binary for `Exec`), and for
//! a path an entry cannot hold (a control character, or a `%`, which desktops read as a field code).

use std::io;
use std::path::{Path, PathBuf};

/// The window's app id on Linux and the name of the desktop entry and icon; the macOS bundle id.
pub const APP_ID: &str = "dev.loupe.app";

const ICON_SVG: &str = include_str!("../../../assets/linux/dev.loupe.app.svg");
const ICON_PNG: &[u8] = include_bytes!("../../../assets/linux/dev.loupe.app.png");

/// Register Loupe with the desktop, best effort. Does nothing outside Linux.
pub fn install() {
    if !cfg!(target_os = "linux")
        || std::env::var_os("LOUPE_DESKTOP_ENTRY").is_some_and(|v| v == "0")
    {
        return;
    }
    let Some(data_home) = data_home() else { return };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("loupe: no desktop entry, cannot find this binary: {error}");
            return;
        }
    };
    if !registrable(&exe) {
        return;
    }
    if let Err(error) = sync(&data_home, &exe) {
        eprintln!(
            "loupe: could not write the desktop entry under {}: {error}",
            data_home.display()
        );
    }
}

/// Whether an entry should start this binary: not a Cargo build, and a path an `Exec` line can hold.
fn registrable(exe: &Path) -> bool {
    let cargo_build = exe
        .parent()
        .and_then(|profile| profile.parent())
        .and_then(|target| target.file_name())
        .is_some_and(|name| name == "target");
    let text = exe.to_string_lossy();
    !cargo_build && !text.contains('%') && !text.chars().any(char::is_control)
}

/// `$XDG_DATA_HOME` if it is absolute (the XDG spec ignores a relative one), or `~/.local/share`.
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// Write the entry and the icons under `data_home`, each only if it differs from what is there.
/// Returns how many files it wrote.
pub fn sync(data_home: &Path, exe: &Path) -> io::Result<usize> {
    let files: [(PathBuf, Vec<u8>); 3] = [
        (
            data_home.join(format!("applications/{APP_ID}.desktop")),
            desktop_entry(exe).into_bytes(),
        ),
        (
            data_home.join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg")),
            ICON_SVG.as_bytes().to_vec(),
        ),
        (
            data_home.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png")),
            ICON_PNG.to_vec(),
        ),
    ];
    let mut written = 0;
    for (path, contents) in files {
        if std::fs::read(&path).ok().as_deref() == Some(contents.as_slice()) {
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, contents)?;
        written += 1;
    }
    Ok(written)
}

/// The desktop entry that starts `exe`.
pub fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Loupe\n\
         Comment=GitHub reviews and Linear issues, in one fast desktop app\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         StartupWMClass={APP_ID}\n\
         Terminal=false\n\
         Categories=Development;\n",
        exec_arg(&exe.to_string_lossy())
    )
}

/// One `Exec` argument, quoted as the Desktop Entry Specification asks: inside double quotes, `"`,
/// `` ` ``, `$`, and `\` are escaped with a backslash, and `%` is doubled because it starts a field code.
fn exec_arg(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    // The entry file is itself parsed for escapes: each backslash we wrote must reach the quoting layer.
    out.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::{APP_ID, desktop_entry, exec_arg, registrable, sync};
    use std::path::Path;

    #[test]
    fn exec_quotes_paths_the_way_the_spec_reads_them() {
        assert_eq!(exec_arg("/home/a/loupe"), r#""/home/a/loupe""#);
        assert_eq!(exec_arg("/opt/my apps/loupe"), r#""/opt/my apps/loupe""#);
        // `$` is escaped for the quoting layer, and that backslash is escaped again for the file.
        assert_eq!(exec_arg("/tmp/$x/100%/loupe"), r#""/tmp/\\$x/100%%/loupe""#);
    }

    #[test]
    fn cargo_builds_and_unholdable_paths_are_not_registered() {
        assert!(registrable(Path::new("/home/a/bin/loupe-linux-x86_64")));
        assert!(!registrable(Path::new("/home/a/loupe/target/debug/loupe")));
        assert!(!registrable(Path::new(
            "/home/a/loupe/target/release/loupe"
        )));
        assert!(!registrable(Path::new("/home/a/100%/loupe")));
        assert!(!registrable(Path::new("/home/a/two\nlines/loupe")));
    }

    #[test]
    fn the_entry_starts_this_binary_with_the_apps_icon() {
        let entry = desktop_entry(Path::new("/home/a/bin/loupe-linux-x86_64"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/home/a/bin/loupe-linux-x86_64\"\n"));
        assert!(entry.contains(&format!("\nIcon={APP_ID}\n")));
        assert!(entry.contains(&format!("\nStartupWMClass={APP_ID}\n")));
    }

    #[test]
    fn sync_writes_once_and_rewrites_when_the_binary_moves() {
        let home = std::env::temp_dir().join(format!("loupe-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(sync(&home, Path::new("/a/loupe")).unwrap(), 3);
        assert_eq!(
            sync(&home, Path::new("/a/loupe")).unwrap(),
            0,
            "a second launch writes nothing"
        );
        assert_eq!(
            sync(&home, Path::new("/b/loupe")).unwrap(),
            1,
            "a moved binary rewrites only the entry"
        );
        let entry =
            std::fs::read_to_string(home.join(format!("applications/{APP_ID}.desktop"))).unwrap();
        assert!(entry.contains("Exec=\"/b/loupe\""));
        assert!(
            home.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png"))
                .is_file()
        );
        std::fs::remove_dir_all(&home).unwrap();
    }
}
