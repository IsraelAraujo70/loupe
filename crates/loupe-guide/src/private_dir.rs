//! Owner-only temporary directories, removed on drop.

use std::path::{Path, PathBuf};

/// A new directory under the system temp dir, created 0700 (failing if the
/// name exists) and removed with its contents on drop.
#[derive(Debug)]
pub(crate) struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    pub(crate) fn create(prefix: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
