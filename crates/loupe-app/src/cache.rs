//! Bounded snapshots of domain data. Credentials and API clients never enter here.
use std::collections::HashMap;
use std::fs::{self, FileTimes, OpenOptions};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Serialize, de::DeserializeOwned};

const LIMIT: u64 = 200 * 1024 * 1024;

#[derive(Default)]
struct Cache {
    memory: HashMap<String, (Vec<u8>, SystemTime)>,
    revisions: HashMap<String, u64>,
}

// The full key in the envelope rejects filename hash collisions.
fn filename(key: &str) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    format!(
        "{}{:016x}",
        if key.starts_with("offline/") {
            "identity-"
        } else {
            ""
        },
        hash.finish()
    )
}

fn shared() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn directory() -> PathBuf {
    crate::store::data_dir().join("review-cache-v1")
}

/// Reads a snapshot and marks it recently used in both tiers.
pub fn get<T: DeserializeOwned>(key: &str) -> Option<T> {
    let mut cache = shared().lock().ok()?;
    read(&mut cache, &directory(), key)
}

fn read<T: DeserializeOwned>(cache: &mut Cache, dir: &std::path::Path, key: &str) -> Option<T> {
    let now = SystemTime::now();
    if let Some((bytes, used)) = cache.memory.get_mut(key) {
        *used = now;
        touch(&dir.join(filename(key)), now);
        return serde_json::from_slice::<(String, T)>(bytes)
            .ok()
            .map(|(_, value)| value);
    }
    let path = dir.join(filename(key));
    let bytes = fs::read(&path).ok()?;
    let (stored_key, value): (String, T) = serde_json::from_slice(&bytes).ok()?;
    if stored_key != key {
        return None;
    }
    touch(&path, now);
    cache.memory.insert(key.into(), (bytes, now));
    trim_memory(cache, LIMIT);
    Some(value)
}

fn touch(path: &std::path::Path, now: SystemTime) {
    if let Ok(file) = OpenOptions::new().write(true).open(path) {
        let _ = file.set_times(FileTimes::new().set_modified(now));
    }
}

/// Stores only serializable domain values, atomically replacing the disk snapshot.
pub fn put<T: Serialize>(key: &str, value: &T) {
    let Ok(mut cache) = shared().lock() else {
        return;
    };
    write(&mut cache, &directory(), key, value);
}

fn write<T: Serialize>(cache: &mut Cache, dir: &std::path::Path, key: &str, value: &T) {
    let Ok(bytes) = serde_json::to_vec(&(key, value)) else {
        return;
    };
    if bytes.len() as u64 > LIMIT {
        return;
    }
    if fs::create_dir_all(dir).is_ok() {
        let path = dir.join(filename(key));
        let temp = path.with_extension("tmp");
        if fs::write(&temp, &bytes).is_ok() {
            let _ = fs::rename(temp, path);
        }
        trim_disk(dir, LIMIT);
    }
    cache.memory.insert(key.into(), (bytes, SystemTime::now()));
    trim_memory(cache, LIMIT);
}

/// Applies a confirmed field change to the latest snapshot, preserving other tabs' writes.
pub fn update<T: Serialize + DeserializeOwned>(key: &str, change: impl FnOnce(&mut T)) {
    let Ok(mut cache) = shared().lock() else {
        return;
    };
    let dir = directory();
    if let Some(mut value) = read::<T>(&mut cache, &dir, key) {
        change(&mut value);
        *cache.revisions.entry(key.into()).or_default() += 1;
        write(&mut cache, &dir, key, &value);
    }
}

fn trim_memory(cache: &mut Cache, limit: u64) {
    let mut size: u64 = cache
        .memory
        .values()
        .map(|(bytes, _)| bytes.len() as u64)
        .sum();
    while size > limit {
        let Some(key) = cache
            .memory
            .iter()
            .filter(|(key, _)| !key.starts_with("offline/"))
            .min_by_key(|(_, (_, used))| *used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        if let Some((bytes, _)) = cache.memory.remove(&key) {
            size -= bytes.len() as u64;
        }
    }
}

fn trim_disk(dir: &std::path::Path, limit: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    entry.path(),
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                )
            })
        })
        .collect();
    let mut size: u64 = files.iter().map(|(_, len, _)| *len).sum();
    files.sort_by_key(|(_, _, used)| *used);
    for (path, len, _) in files {
        if size <= limit {
            break;
        }
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("identity-"))
        {
            continue;
        }
        if fs::remove_file(path).is_ok() {
            size -= len;
        }
    }
}

/// Account-scoped keys prevent a different signed-in user seeing old private data.
pub fn github_key(cx: &gpui_kit::App, resource: &str) -> Option<String> {
    match &crate::services::Accounts::get(cx).github {
        crate::services::Connection::Connected(viewer) => {
            Some(format!("gh/{}/{resource}", viewer.login))
        }
        _ => None,
    }
}

/// Removes an identity snapshot on explicit sign-out.
pub fn remove(key: &str) {
    if let Ok(mut cache) = shared().lock() {
        cache.memory.remove(key);
        let _ = fs::remove_file(directory().join(filename(key)));
    }
}

/// Marks a local write so an earlier read cannot replace its result.
pub fn invalidate_reads(key: &str) {
    if let Ok(mut cache) = shared().lock() {
        *cache.revisions.entry(key.into()).or_default() += 1;
    }
}

/// The local-write generation a background read belongs to.
pub fn revision(key: &str) -> u64 {
    shared()
        .lock()
        .ok()
        .and_then(|cache| cache.revisions.get(key).copied())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Cache, filename, read, touch, trim_disk, trim_memory};
    use std::fs;
    use std::time::{Duration, SystemTime};

    #[test]
    fn disk_snapshot_survives_memory_loss_and_rejects_wrong_key_or_corruption() {
        let dir = std::env::temp_dir().join(format!("loupe-cache-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let key = "gh/alice/pr/1/head-a";
        let path = dir.join(filename(key));
        fs::write(&path, serde_json::to_vec(&(key, vec![1, 2, 3])).unwrap()).unwrap();
        let mut cache = Cache::default();
        assert_eq!(read::<Vec<i32>>(&mut cache, &dir, key), Some(vec![1, 2, 3]));
        assert_eq!(
            read::<Vec<i32>>(&mut cache, &dir, "gh/bob/pr/1/head-a"),
            None
        );
        cache.memory.clear();
        fs::write(&path, b"corrupted").unwrap();
        assert_eq!(read::<Vec<i32>>(&mut cache, &dir, key), None);
        fs::write(
            &path,
            serde_json::to_vec(&("another-key", vec![9])).unwrap(),
        )
        .unwrap();
        assert_eq!(read::<Vec<i32>>(&mut cache, &dir, key), None);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn disk_lru_obeys_byte_limit_and_touches_on_read() {
        let dir = std::env::temp_dir().join(format!("loupe-lru-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old");
        let recent = dir.join("recent");
        let identity = dir.join("identity-viewer");
        fs::write(&identity, b"me").unwrap();
        touch(&identity, SystemTime::UNIX_EPOCH);
        fs::write(&old, b"123456").unwrap();
        fs::write(&recent, b"123456").unwrap();
        touch(&old, SystemTime::UNIX_EPOCH);
        trim_disk(&dir, 8);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(identity.exists());
        fs::remove_file(identity).unwrap();
        fs::remove_file(recent).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn public_cache_survives_a_process_restart() {
        let dir = std::env::temp_dir().join(format!("loupe-restart-{}", std::process::id()));
        for phase in ["write", "read"] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "cache::tests::public_cache_child", "--ignored"])
                .env("LOUPE_DATA_DIR", &dir)
                .env("LOUPE_CACHE_TEST_PHASE", phase)
                .status()
                .unwrap();
            assert!(status.success(), "cache phase {phase} failed");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "launched in two isolated processes by the restart test"]
    fn public_cache_child() {
        let key = "gh/fixture/pr/1/head-a";
        if std::env::var("LOUPE_CACHE_TEST_PHASE").unwrap() == "write" {
            super::put(key, &vec![1, 2, 3]);
            assert_eq!(super::get::<Vec<i32>>(key), Some(vec![1, 2, 3]));
            super::put("viewed", &vec![false, false]);
            super::update::<Vec<bool>>("viewed", |files| files[0] = true);
            super::update::<Vec<bool>>("viewed", |files| files[1] = true);
        } else {
            assert_eq!(super::get::<Vec<i32>>(key), Some(vec![1, 2, 3]));
            assert_eq!(super::get::<Vec<bool>>("viewed"), Some(vec![true, true]));
        }
    }

    #[test]
    fn keys_do_not_alias() {
        assert_ne!(filename("a/b"), filename("a-b"));
        assert_ne!(filename("pr/1/head-a"), filename("pr/1/head-b"));
    }

    #[test]
    fn least_recently_used_is_evicted() {
        let mut cache = Cache::default();
        let now = SystemTime::now();
        cache
            .memory
            .insert("old".into(), (vec![1; 6], now - Duration::from_secs(1)));
        cache.memory.insert("new".into(), (vec![2; 6], now));
        trim_memory(&mut cache, 6);
        assert!(!cache.memory.contains_key("old"));
        assert!(cache.memory.contains_key("new"));
    }
}
