//! Isolated, hardened temporary checkout for read-only PR inspection.
//!
//! Each checkout lives in a fresh temp directory, fetches only the requested PR
//! commit with git hooks/symlinks/submodules disabled, strips `.git/` and any
//! symlinks before handing the directory to the agent, marks the tree read-only
//! on Unix, and deletes the directory unconditionally on [`Drop`].

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Maximum wall-clock time permitted for a single `git` invocation.
const GIT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Error)]
pub enum CheckoutError {
    #[error("Invalid repository reference `{0}`")]
    InvalidRepo(String),
    #[error("Invalid commit SHA `{0}`")]
    InvalidCommit(String),
    #[error("Cancelled while preparing repository checkout")]
    Cancelled,
    #[error("Git operation timed out")]
    TimedOut,
    #[error("Failed to create temporary checkout directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git checkout failed: {0}")]
    Git(String),
    #[error("Checked-out commit `{actual}` did not match expected PR head `{expected}`")]
    CommitMismatch { expected: String, actual: String },
}

/// An isolated read-only temporary checkout that is always removed on drop.
#[derive(Debug)]
pub struct TempCheckout {
    dir: PathBuf,
    read_only_applied: bool,
}

impl TempCheckout {
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Creates a hardened temporary checkout of `commit` from a local git repo.
    /// Useful for tests and offline verification.
    pub fn from_local_repo(source_repo: &Path, commit: &str) -> Result<Self, CheckoutError> {
        validate_commit_sha(commit)?;
        let mut checkout = Self::create_empty()?;
        let source = source_repo
            .to_str()
            .ok_or_else(|| CheckoutError::Git("non-UTF8 source repository path".into()))?;
        run_git(
            &checkout.dir,
            &["init", "-q"],
            &[],
            true,
            &AtomicBool::new(false),
        )?;
        run_git(
            &checkout.dir,
            &["fetch", "--depth=1", "--no-tags", source, commit],
            &[],
            true,
            &AtomicBool::new(false),
        )?;
        run_git(
            &checkout.dir,
            &["checkout", "--detach", "-q", "FETCH_HEAD"],
            &[],
            true,
            &AtomicBool::new(false),
        )?;
        checkout.verify_head(commit)?;
        checkout.harden()?;
        Ok(checkout)
    }

    /// Creates a hardened temporary checkout for GitHub pull request `repo#pr_number`
    /// at `head_oid`. Auth tokens are passed only to `git` via environment config
    /// and never persisted into `.git/config` or exposed to the agent.
    pub fn from_github_pr(
        repo: &str,
        pr_number: u64,
        head_oid: &str,
        github_token: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Self, CheckoutError> {
        validate_repo_slug(repo)?;
        validate_commit_sha(head_oid)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(CheckoutError::Cancelled);
        }

        let mut checkout = Self::create_empty()?;
        let remote_url = format!("https://github.com/{repo}.git");
        let auth_env = github_auth_env(github_token);

        run_git(&checkout.dir, &["init", "-q"], &[], false, cancel)?;

        // Try fetching the exact head SHA first (works even for merged PRs whose ref moved),
        // then fall back to `refs/pull/<n>/head` (works for forks where the SHA isn't advertised).
        let pr_ref = format!("refs/pull/{pr_number}/head");
        let fetch_sha = run_git(
            &checkout.dir,
            &["fetch", "--depth=1", "--no-tags", &remote_url, head_oid],
            &auth_env,
            false,
            cancel,
        );
        if fetch_sha.is_err() {
            if cancel.load(Ordering::Relaxed) {
                return Err(CheckoutError::Cancelled);
            }
            run_git(
                &checkout.dir,
                &["fetch", "--depth=1", "--no-tags", &remote_url, &pr_ref],
                &auth_env,
                false,
                cancel,
            )?;
        }

        run_git(
            &checkout.dir,
            &["checkout", "--detach", "-q", "FETCH_HEAD"],
            &[],
            false,
            cancel,
        )?;
        checkout.verify_head(head_oid)?;
        checkout.harden()?;
        Ok(checkout)
    }

    fn create_empty() -> Result<Self, CheckoutError> {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let pid = std::process::id();
        for _ in 0..32 {
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let dir = std::env::temp_dir().join(format!("loupe-guide-{pid}-{nanos}-{seq}"));
            // Atomic `create_dir` refuses pre-existing directories (prevents /tmp pre-creation).
            match fs::create_dir(&dir) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
                    }
                    return Ok(Self {
                        dir,
                        read_only_applied: false,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(CheckoutError::Io(err)),
            }
        }
        Err(CheckoutError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create unique temporary directory",
        )))
    }

    fn verify_head(&self, expected: &str) -> Result<(), CheckoutError> {
        let output = hardened_git(&self.dir, false)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !output.status.success() {
            return Err(CheckoutError::Git("could not read checked-out HEAD".into()));
        }
        let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !actual.starts_with(expected) && !expected.starts_with(&actual) {
            return Err(CheckoutError::CommitMismatch {
                expected: expected.to_owned(),
                actual,
            });
        }
        Ok(())
    }

    /// Strips `.git/` and any symlinks, then locks permissions to owner-only read-only on Unix.
    fn harden(&mut self) -> Result<(), CheckoutError> {
        let git_dir = self.dir.join(".git");
        if git_dir.exists() {
            fs::remove_dir_all(&git_dir)?;
        }
        remove_all_symlinks(&self.dir)?;
        #[cfg(unix)]
        {
            // Mark before mutating permissions so Drop still unlocks a partially locked tree.
            self.read_only_applied = true;
            set_tree_writable(&self.dir, false)?;
        }
        Ok(())
    }
}

impl Drop for TempCheckout {
    fn drop(&mut self) {
        if !self.dir.exists() {
            return;
        }
        #[cfg(unix)]
        if self.read_only_applied {
            let _ = set_tree_writable(&self.dir, true);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Captures a deterministic snapshot of all files (relative paths + contents)
/// inside `dir`, used by security tests to prove nothing was created, deleted,
/// or modified.
pub fn snapshot_tree(dir: &Path) -> std::io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut out = BTreeMap::new();
    collect_snapshot(dir, dir, &mut out)?;
    Ok(out)
}

fn collect_snapshot(
    root: &Path,
    current: &Path,
    out: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            out.insert(rel, format!("symlink:{}", target.display()).into_bytes());
        } else if meta.is_dir() {
            collect_snapshot(root, &path, out)?;
        } else if meta.is_file() {
            out.insert(rel, fs::read(&path)?);
        }
    }
    Ok(())
}

fn validate_repo_slug(repo: &str) -> Result<(), CheckoutError> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(CheckoutError::InvalidRepo(repo.to_owned()));
    };
    let valid_part = |part: &str| {
        !part.is_empty()
            && !part.starts_with('-')
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if valid_part(owner) && valid_part(name) && !name.contains('/') {
        Ok(())
    } else {
        Err(CheckoutError::InvalidRepo(repo.to_owned()))
    }
}

fn validate_commit_sha(sha: &str) -> Result<(), CheckoutError> {
    if (7..=40).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(CheckoutError::InvalidCommit(sha.to_owned()))
    }
}

fn hardened_git(dir: &Path, allow_file_protocol: bool) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.symlinks=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            if allow_file_protocol {
                "protocol.file.allow=always"
            } else {
                "protocol.file.allow=never"
            },
        ]);
    cmd
}

fn run_git(
    dir: &Path,
    args: &[&str],
    extra_env: &[(String, String)],
    allow_file_protocol: bool,
    cancel: &AtomicBool,
) -> Result<(), CheckoutError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CheckoutError::Cancelled);
    }
    let mut cmd = hardened_git(dir, allow_file_protocol);
    cmd.args(args);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + GIT_TIMEOUT;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CheckoutError::Cancelled);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CheckoutError::TimedOut);
        }
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let output = child.wait_with_output()?;
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            // Never echo Authorization headers if git somehow printed config.
            let safe = redact_auth(&stderr);
            return Err(CheckoutError::Git(if safe.is_empty() {
                format!("git exited with status {status}")
            } else {
                safe
            }));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn github_auth_env(token: Option<&str>) -> Vec<(String, String)> {
    let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    let basic = base64_encode(format!("x-access-token:{token}").as_bytes());
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        (
            "GIT_CONFIG_KEY_0".into(),
            "http.https://github.com/.extraHeader".into(),
        ),
        (
            "GIT_CONFIG_VALUE_0".into(),
            format!("AUTHORIZATION: basic {basic}"),
        ),
    ]
}

fn redact_auth(text: &str) -> String {
    text.lines()
        .filter(|line| !line.to_ascii_lowercase().contains("authorization:"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn remove_all_symlinks(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            fs::remove_file(&path)?;
        } else if meta.is_dir() {
            remove_all_symlinks(&path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_tree_writable(dir: &Path, writable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if writable {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            set_tree_writable(&path, writable)?;
        } else if meta.is_file() {
            // Owner-only permissions prevent other local users on shared Linux hosts from reading private repos.
            let mode = if writable { 0o600 } else { 0o400 };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        }
    }
    if !writable {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o500))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::{
        CheckoutError, TempCheckout, base64_encode, snapshot_tree, validate_commit_sha,
        validate_repo_slug,
    };

    #[test]
    fn validates_repo_slugs_and_commit_shas_against_option_injection() {
        assert!(validate_repo_slug("IsraelAraujo70/loupe").is_ok());
        assert!(validate_repo_slug("-evil/repo").is_err());
        assert!(validate_repo_slug("owner/-evil").is_err());
        assert!(validate_repo_slug("owner/..").is_err());
        assert!(validate_repo_slug("owner/a/b").is_err());
        assert!(validate_repo_slug("https://evil.example/a/b").is_err());

        assert!(validate_commit_sha("0123456789abcdef0123456789abcdef01234567").is_ok());
        assert!(validate_commit_sha("--upload-pack=sh").is_err());
        assert!(validate_commit_sha("short").is_err());
        assert!(validate_commit_sha("zzzzzzz").is_err());
    }

    #[test]
    fn base64_encodes_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(
            base64_encode(b"x-access-token:abc"),
            "eC1hY2Nlc3MtdG9rZW46YWJj"
        );
    }

    #[test]
    fn local_checkout_strips_git_enforces_read_only_and_cleans_up_on_drop() {
        let src = std::env::temp_dir().join(format!("loupe-src-repo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&src);
        fs::create_dir_all(&src).unwrap();

        let git = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&src)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };

        git(&["init", "-q"]);
        fs::write(src.join("hello.txt"), "hello from commit\n").unwrap();
        git(&["add", "hello.txt"]);
        git(&["commit", "-q", "-m", "initial"]);

        let sha_out = Command::new("git")
            .current_dir(&src)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_owned();

        let checkout_path;
        {
            let checkout = TempCheckout::from_local_repo(&src, &sha).expect("checkout succeeds");
            checkout_path = checkout.path().to_path_buf();
            assert!(!checkout_path.join(".git").exists(), ".git must be removed");
            assert_eq!(
                fs::read_to_string(checkout_path.join("hello.txt")).unwrap(),
                "hello from commit\n"
            );

            let snap = snapshot_tree(&checkout_path).unwrap();
            assert_eq!(snap.len(), 1);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let dir_mode = fs::metadata(&checkout_path).unwrap().permissions().mode() & 0o777;
                assert_eq!(dir_mode, 0o500, "temp dir must be owner read-only (0500)");
                let file_mode = fs::metadata(checkout_path.join("hello.txt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(
                    file_mode, 0o400,
                    "checked-out file must be owner read-only (0400)"
                );
                let write_attempt = fs::write(checkout_path.join("pwned.txt"), "nope");
                assert!(
                    write_attempt.is_err(),
                    "read-only temp checkout must reject writes"
                );
            }
        }
        assert!(
            !checkout_path.exists(),
            "temp checkout directory must be deleted on Drop"
        );

        let err = TempCheckout::from_local_repo(&src, "0000000000000000000000000000000000000000")
            .unwrap_err();
        assert!(matches!(err, CheckoutError::Git(_)));

        let _ = fs::remove_dir_all(&src);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_global_filters_hooks_auth_redaction_and_commit_mismatch_are_enforced() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("loupe-sec-guards-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let src = base.join("repo");
        let fake_home = base.join("home");
        let marker = base.join("filter-ran");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&fake_home).unwrap();

        // Plant a malicious global gitconfig filter (simulating git-lfs smudge / arbitrary filter).
        fs::write(
            fake_home.join(".gitconfig"),
            format!(
                "[filter \"evil\"]\n\tsmudge = sh -c 'touch {} && cat'\n",
                marker.display()
            ),
        )
        .unwrap();

        let git = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&src)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };

        git(&["init", "-q"]);
        fs::write(src.join(".gitattributes"), "*.bin filter=evil\n").unwrap();
        fs::write(src.join("data.bin"), "payload\n").unwrap();
        symlink("/etc/passwd", src.join("leak.txt")).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "symlink and filter attr"]);

        let sha_out = Command::new("git")
            .current_dir(&src)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_owned();

        // Run hardened_git explicitly under fake_home to verify GIT_CONFIG_GLOBAL=/dev/null blocks global filters.
        let dest = base.join("dest");
        fs::create_dir_all(&dest).unwrap();
        let run_with_home = |args: &[&str]| {
            let status = super::hardened_git(&dest, true)
                .env("HOME", &fake_home)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "hardened git {args:?} failed");
        };
        run_with_home(&["init", "-q"]);
        // Plant a repo post-checkout hook to verify core.hooksPath=/dev/null blocks it.
        let hook_marker = base.join("hook-ran");
        let hooks_dir = dest.join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("post-checkout");
        fs::write(
            &hook_path,
            format!("#!/bin/sh\ntouch {}\n", hook_marker.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        run_with_home(&["fetch", "--depth=1", src.to_str().unwrap(), &sha]);
        run_with_home(&["checkout", "--detach", "-q", "FETCH_HEAD"]);

        assert!(
            !hook_marker.exists(),
            "core.hooksPath=/dev/null must block repo post-checkout hooks"
        );
        assert!(
            !marker.exists(),
            "global git filter from HOME/.gitconfig must never execute during checkout"
        );
        let leak_meta = fs::symlink_metadata(dest.join("leak.txt")).unwrap();
        assert!(
            !leak_meta.file_type().is_symlink(),
            "core.symlinks=false must materialize symlinks as plain files"
        );

        // Verify remove_all_symlinks deletes OS symlinks directly too.
        let sym_dir = base.join("sym-dir");
        fs::create_dir_all(&sym_dir).unwrap();
        symlink("/etc/passwd", sym_dir.join("escape")).unwrap();
        super::remove_all_symlinks(&sym_dir).unwrap();
        assert!(!sym_dir.join("escape").exists());

        // Verify verify_head catches commit mismatch.
        let temp = TempCheckout {
            dir: dest.clone(),
            read_only_applied: false,
        };
        let mismatch = temp.verify_head("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(matches!(
            mismatch,
            Err(CheckoutError::CommitMismatch { .. })
        ));
        std::mem::forget(temp);

        // Verify redact_auth strips Authorization lines.
        let redacted =
            super::redact_auth("fatal: error\nAuthorization: basic c2VjcmV0\nmore details");
        assert!(!redacted.contains("c2VjcmV0"));
        assert!(redacted.contains("fatal: error"));
        assert!(redacted.contains("more details"));

        let _ = fs::remove_dir_all(&base);
    }
}
