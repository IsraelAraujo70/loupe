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
/// and restarts it. Keeps the previous version for rollback on launch failure.
pub fn install(stage: &Path, target: &Path) -> Result<()> {
    let source = stage.join(if cfg!(target_os = "macos") {
        "Loupe.app"
    } else {
        "download"
    });
    ensure!(source.exists(), "Staged update is missing");
    let script = stage.join("install.sh");
    fs::write(&script, include_str!("install.sh"))?;
    Command::new("/bin/sh")
        .arg(script)
        .arg(std::process::id().to_string())
        .arg(target)
        .arg(stage)
        .arg(source)
        .arg(std::env::consts::OS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
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
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    #[test]
    fn replacement_restarts_and_rolls_back_a_failed_launch() {
        for fail in [false, true] {
            let root =
                std::env::temp_dir().join(format!("loupe-install-{}-{fail}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let stage = root.join("stage");
            fs::create_dir(&stage).unwrap();
            let target = root.join("loupe");
            let source = stage.join("download");
            let marker = root.join("started");
            // `echo > file` creates the file before it writes, so a reader polling for the
            // file can see it empty. Rename into place so the marker appears complete.
            let mark = |text: &str| {
                format!(
                    "echo {text} > '{0}.tmp' && mv '{0}.tmp' '{0}'",
                    marker.display()
                )
            };
            let old = format!("#!/bin/sh\n{}\n", mark("old"));
            fs::write(&target, &old).unwrap();
            let new = if fail {
                "#!/bin/sh\nexit 1\n".into()
            } else {
                format!("#!/bin/sh\n{}\nexec sleep 5\n", mark("new"))
            };
            fs::write(&source, &new).unwrap();
            for path in [&source, &target] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let script = root.join("install.sh");
            fs::write(&script, include_str!("install.sh")).unwrap();
            let status = Command::new("sh")
                .arg(script)
                .arg("99999999")
                .arg(&target)
                .arg(&stage)
                .arg(&source)
                .arg("linux")
                .status()
                .unwrap();
            assert_eq!(status.success(), !fail);
            // The helper starts the restored version in the background and exits, so on a
            // busy machine it can take a while to run; the deadline only bounds a real failure.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !marker.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(
                fs::read_to_string(&marker).unwrap().trim(),
                if fail { "old" } else { "new" }
            );
            assert_eq!(
                fs::read_to_string(target).unwrap(),
                if fail { old } else { new }
            );
            assert!(
                !stage.exists(),
                "successful install or rollback must allow another update"
            );
            fs::create_dir(&stage).expect("the next update can stage again");
            fs::remove_dir_all(root).unwrap();
        }
    }
}
