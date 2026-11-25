use std::path::Path;
use std::process::Command;

/// Writes an executable file that the test is about to run.
///
/// Writing it from this process leaves a write descriptor open on the inode. A
/// test thread that forks at that moment hands the descriptor to its child
/// until the child execs, and `exec` of the script then fails with ETXTBSY
/// ("Text file busy"). A short-lived child process does the write instead, so
/// no descriptor on the file ever exists in this process, and no other thread
/// can leak one.
pub(crate) fn write_executable(path: &Path, contents: &str) {
    let status = Command::new("/bin/sh")
        .args([
            "-c",
            "printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"",
            "sh",
        ])
        .arg(contents)
        .arg(path)
        .status()
        .expect("spawn sh to write an executable");
    assert!(status.success(), "writing {} failed", path.display());
}
