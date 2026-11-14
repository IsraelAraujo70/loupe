//! GitHub release discovery and staged installation. No credentials are persisted.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const API: &str = "https://api.github.com/repos/IsraelAraujo70/loupe";

#[derive(Clone, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub digest: Option<String>,
    pub size: u64,
}

#[derive(Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into()
}

/// Only stable semver triples participate in automatic updates.
fn stable(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim_start_matches('v').split('.');
    let value = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(value)
}

/// Finds a newer stable release; prereleases can opt into a specific test tag.
pub fn check(token: &str, current: &str, test_tag: Option<&str>) -> Result<Option<Release>> {
    let url = match test_tag {
        Some(tag) => {
            ensure!(
                tag.starts_with('v')
                    && tag
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)),
                "Invalid test tag"
            );
            format!("{API}/releases/tags/{tag}")
        }
        None => format!("{API}/releases/latest"),
    };
    let mut response = match agent()
        .get(&url)
        .header("User-Agent", "Loupe-updater")
        .header("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(_) => bail!("Could not check GitHub releases"),
    };
    let release: Release = response
        .body_mut()
        .read_json()
        .context("Invalid release metadata")?;
    let newer = if test_tag.is_some() {
        release.tag_name.trim_start_matches('v') != current
    } else {
        stable(&release.tag_name)
            .zip(stable(current.split('-').next().unwrap_or(current)))
            .is_some_and(|(next, installed)| {
                next > installed || (next == installed && current.contains('-'))
            })
    };
    Ok((!release.draft && (!release.prerelease || test_tag.is_some()) && newer).then_some(release))
}

fn asset_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("loupe-linux-x86_64"),
        ("macos", "aarch64") => Ok("loupe-macos-aarch64.zip"),
        _ => bail!("No update is available for this platform"),
    }
}

/// Returns the installed binary on Linux or enclosing .app on macOS.
pub fn install_target(executable: &Path) -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        let bundle = executable
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("Run Loupe from an installed .app")?;
        ensure!(
            bundle.extension().is_some_and(|v| v == "app"),
            "Run Loupe from an installed .app"
        );
        Ok(bundle.to_owned())
    } else {
        Ok(executable.to_owned())
    }
}

/// Downloads beside the installation and verifies GitHub's SHA-256 before extraction.
/// A unique staging directory also acts as the install lock. Failed attempts clean it.
pub fn prepare(token: &str, release: &Release, target: &Path) -> Result<PathBuf> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name().unwrap_or(""))
        .context("Release has no asset for this platform")?;
    let digest = asset
        .digest
        .as_deref()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .context("Release asset has no SHA-256 digest")?;
    ensure!(
        asset.size > 0 && asset.size < 512 * 1024 * 1024,
        "Invalid update size"
    );
    let stage = target
        .parent()
        .context("Missing install directory")?
        .join(".loupe-update");
    fs::create_dir(&stage).context("Update already staged, or installation is not writable")?;
    let result = (|| {
        let path = stage.join("download");
        let mut response = agent()
            .get(format!("{API}/releases/assets/{}", asset.id))
            .header("User-Agent", "Loupe-updater")
            .header("Accept", "application/octet-stream")
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(|_| anyhow::anyhow!("Could not download update"))?;
        let mut file = fs::File::create(&path)?;
        let copied = std::io::copy(
            &mut response.body_mut().as_reader().take(asset.size + 1),
            &mut file,
        )?;
        ensure!(copied == asset.size, "Incomplete update download");
        file.sync_all()?;
        let output = if cfg!(target_os = "macos") {
            Command::new("/usr/bin/shasum")
                .args(["-a", "256"])
                .arg(&path)
                .output()?
        } else {
            Command::new("sha256sum").arg(&path).output()?
        };
        ensure!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .next()
                    == Some(digest),
            "Update checksum mismatch"
        );
        if cfg!(target_os = "macos") {
            ensure!(
                Command::new("/usr/bin/ditto")
                    .args(["-x", "-k"])
                    .arg(&path)
                    .arg(&stage)
                    .status()?
                    .success(),
                "Could not extract update"
            );
            ensure!(
                Command::new("/usr/bin/codesign")
                    .args(["--verify", "--deep", "--strict"])
                    .arg(stage.join("Loupe.app"))
                    .status()?
                    .success(),
                "Invalid app signature"
            );
            fs::remove_file(path)?;
        } else {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
        Ok(stage.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Starts a helper that waits for this process to exit, replaces the installation,
/// and restarts it. The helper keeps the previous version until the new one calls
/// [`confirm_start`]; if the new one exits first, the helper puts the previous
/// version back, writes `tag` to `notice` (see [`rolled_back`]), and restarts it.
pub fn install(stage: &Path, target: &Path, tag: &str, notice: &Path) -> Result<()> {
    let source = stage.join(if cfg!(target_os = "macos") {
        "Loupe.app"
    } else {
        "download"
    });
    ensure!(source.exists(), "Staged update is missing");
    if let Some(dir) = notice.parent() {
        fs::create_dir_all(dir)?;
    }
    let script = stage.join("install.sh");
    fs::write(&script, include_str!("install.sh"))?;
    Command::new("/bin/sh")
        .arg(script)
        .arg(std::process::id().to_string())
        .arg(target)
        .arg(stage)
        .arg(source)
        .arg(std::env::consts::OS)
        .arg(tag)
        .arg(notice)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// Tells a waiting install helper that this freshly installed version started
/// well, so it drops the previous version. Call it once the first screen is drawn.
/// Returns whether an install was waiting; without one it does nothing.
pub fn confirm_start(executable: &Path) -> Result<bool> {
    let stage = install_target(executable)?
        .parent()
        .context("Missing install directory")?
        .join(".loupe-update");
    // Only the helper creates `previous`, after the old version quit; a download
    // still being staged by this process has none.
    if !stage.join("previous").exists() {
        return Ok(false);
    }
    fs::write(stage.join("healthy"), b"")?;
    Ok(true)
}

/// The release tag the helper rolled back because it exited before it finished
/// starting, as recorded in `notice` by [`install`]. A later install through the
/// helper clears it; a tag no newer than `current` is stale and ignored.
pub fn rolled_back(notice: &Path, current: &str) -> Option<String> {
    let tag = fs::read_to_string(notice).ok()?;
    let tag = tag.trim();
    let newer = match (stable(tag), stable(current)) {
        (Some(tag), Some(current)) => tag > current,
        _ => !tag.is_empty() && tag.trim_start_matches('v') != current,
    };
    newer.then(|| tag.to_owned())
}

#[cfg(test)]
mod tests {
    use super::stable;
    #[test]
    fn stable_versions_order_numerically_and_exclude_prereleases() {
        assert!(stable("v0.1.10") > stable("v0.1.9"));
        assert_eq!(stable("v0.1.2-rc.1"), None);
        assert_eq!(stable("v0.1.2.3"), None);
        assert_eq!(stable("malformed"), None);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod install_tests {
    use super::{confirm_start, rolled_back};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Child, Command},
        time::{Duration, Instant},
    };

    const OLD: &str = "old";

    /// An installed `loupe` that records "old" when started, a staged replacement
    /// running `new_body` (given the line that records "new"), a stale rollback notice, and the helper started on them.
    struct Install {
        root: PathBuf,
        target: PathBuf,
        stage: PathBuf,
        marker: PathBuf,
        notice: PathBuf,
        helper: Child,
    }

    impl Install {
        fn start(name: &str, new_body: fn(mark_new: &str) -> String) -> Install {
            let root =
                std::env::temp_dir().join(format!("loupe-install-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let stage = root.join(".loupe-update");
            fs::create_dir_all(&stage).unwrap();
            let target = root.join("loupe");
            let source = stage.join("download");
            let marker = root.join("started");
            let notice = root.join("data").join("update-rolled-back");
            // `echo > file` creates the file before it writes, so a reader polling for
            // the file can see it empty. Rename into place so it appears complete.
            let mark = |text: &str| {
                format!(
                    "echo {text} > '{0}.tmp' && mv '{0}.tmp' '{0}'",
                    marker.display()
                )
            };
            fs::write(&target, format!("#!/bin/sh\n{}\n", mark(OLD))).unwrap();
            fs::write(&source, format!("#!/bin/sh\n{}\n", new_body(&mark("new")))).unwrap();
            for path in [&source, &target] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            fs::create_dir_all(notice.parent().unwrap()).unwrap();
            fs::write(&notice, "v0.0.1\n").unwrap();
            let script = stage.join("install.sh");
            fs::write(&script, include_str!("install.sh")).unwrap();
            let helper = Command::new("sh")
                .arg(script)
                .arg("99999999")
                .arg(&target)
                .arg(&stage)
                .arg(&source)
                .arg("linux")
                .arg("v9.9.9")
                .arg(&notice)
                .spawn()
                .unwrap();
            Install {
                root,
                target,
                stage,
                marker,
                notice,
                helper,
            }
        }

        fn started(&self) -> String {
            wait_for(&self.marker);
            fs::read_to_string(&self.marker).unwrap().trim().to_owned()
        }

        /// The helper's result, the version left installed and started, and the
        /// rollback notice.
        fn finish(mut self) -> (bool, String, Option<String>) {
            let ok = self.helper.wait().unwrap().success();
            // The restored version is started asynchronously by the helper.
            let started = self.started();
            assert!(
                !self.stage.exists(),
                "a finished install or rollback must allow another update"
            );
            fs::create_dir(&self.stage).expect("the next update can stage again");
            let installed = fs::read_to_string(&self.target).unwrap();
            assert!(installed.contains(&format!("echo {started} ")));
            let notice = rolled_back(&self.notice, "0.0.1");
            fs::remove_dir_all(&self.root).unwrap();
            (ok, started, notice)
        }
    }

    fn wait_for(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_version_that_confirms_its_start_stays_installed() {
        let mut install = Install::start("healthy", |mark_new| format!("{mark_new}\nexec sleep 5"));
        assert_eq!(install.started(), "new");
        fs::remove_file(&install.marker).unwrap();
        // What the app does once its first frame is drawn.
        assert!(confirm_start(&install.target).unwrap());
        // The helper is done once it accepts the new version; nothing restarts.
        assert!(install.helper.wait().unwrap().success());
        assert!(!install.stage.exists());
        assert!(
            fs::read_to_string(&install.target)
                .unwrap()
                .contains("echo new ")
        );
        assert_eq!(
            rolled_back(&install.notice, "0.0.1"),
            None,
            "a good install clears the notice"
        );
        fs::remove_dir_all(&install.root).unwrap();
    }

    #[test]
    fn a_version_that_fails_to_launch_is_rolled_back() {
        let install = Install::start("exit", |_| "exit 1".into());
        assert_eq!(install.finish(), (false, OLD.into(), Some("v9.9.9".into())));
    }

    #[test]
    fn a_version_that_crashes_after_it_starts_is_rolled_back() {
        // Runs past the three seconds the helper used to wait, then crashes without
        // confirming its start.
        let install = Install::start("crash", |mark_new| {
            format!("{mark_new}\nsleep 4\nkill -SEGV $$")
        });
        assert_eq!(install.started(), "new");
        fs::remove_file(&install.marker).unwrap();
        assert_eq!(install.finish(), (false, OLD.into(), Some("v9.9.9".into())));
    }

    #[test]
    fn a_rolled_back_tag_no_newer_than_the_running_version_is_stale() {
        let notice = std::env::temp_dir().join(format!("loupe-notice-{}", std::process::id()));
        fs::write(&notice, "v0.1.10\n").unwrap();
        assert_eq!(rolled_back(&notice, "0.1.9").as_deref(), Some("v0.1.10"));
        assert_eq!(rolled_back(&notice, "0.1.10"), None);
        assert_eq!(rolled_back(&notice, "0.1.11"), None);
        fs::remove_file(&notice).unwrap();
        assert_eq!(rolled_back(&notice, "0.1.9"), None);
    }

    #[test]
    fn confirming_without_a_waiting_install_does_nothing() {
        let root = std::env::temp_dir().join(format!("loupe-confirm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let stage = root.join(".loupe-update");
        fs::create_dir_all(&stage).unwrap();
        let target = root.join("loupe");
        // A download this process is still staging.
        assert!(!confirm_start(&target).unwrap());
        assert!(!stage.join("healthy").exists());
        fs::remove_dir_all(&stage).unwrap();
        assert!(!confirm_start(&target).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
