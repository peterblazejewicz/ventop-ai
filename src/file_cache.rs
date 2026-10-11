// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Global thread-safe sysfs file handle cache with error eviction and rebind recovery.
/// Maintains file descriptors across polling ticks to achieve <0.5% CPU overhead.
static CACHE: Mutex<Option<HashMap<String, File>>> = Mutex::new(None);

/// Get the root path for sysfs/procfs/devfs resolution.
/// Reads the environment variable `VENTOP_SYSFS_ROOT`. If set and not empty, uses that path;
/// otherwise defaults to `/`.
pub fn sysfs_root() -> PathBuf {
    match std::env::var("VENTOP_SYSFS_ROOT") {
        Ok(val) if !val.is_empty() => PathBuf::from(val),
        _ => PathBuf::from("/"),
    }
}

/// Resolve a sysfs, procfs, or devfs path relative to the active root (`VENTOP_SYSFS_ROOT` or `/`).
/// If root is `/` or if `path` already starts with `root`, returns `path` unchanged.
/// Otherwise, strips any leading `/` and appends `path` to `root`.
pub fn resolve_sysfs_path<P: AsRef<Path>>(path: P) -> PathBuf {
    let root = sysfs_root();
    resolve_path_with_root(&root, path.as_ref())
}

/// Helper to resolve a path relative to an arbitrary root directory.
pub fn resolve_path_with_root(root: &Path, path: &Path) -> PathBuf {
    if root == Path::new("/") || path.starts_with(root) {
        path.to_path_buf()
    } else {
        // Only redirect sysfs (/sys), procfs (/proc), devfs (/dev), /var, /run
        let is_target = path.starts_with("/sys")
            || path.starts_with("/proc")
            || path.starts_with("/dev")
            || path.starts_with("/var")
            || path.starts_with("/run")
            || path.starts_with("sys")
            || path.starts_with("proc")
            || path.starts_with("dev")
            || path.starts_with("var")
            || path.starts_with("run");

        if is_target {
            match path.strip_prefix("/") {
                Ok(stripped) => root.join(stripped),
                Err(_) => root.join(path),
            }
        } else {
            path.to_path_buf()
        }
    }
}

/// Reset cached file handles.
pub fn reset_file_cache() {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

#[cfg(test)]
pub static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl EnvGuard {
    pub fn set_root(path: &std::path::Path) -> Self {
        let lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_file_cache();
        std::env::set_var("VENTOP_SYSFS_ROOT", path);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("VENTOP_SYSFS_ROOT");
        reset_file_cache();
    }
}

/// Reads a sysfs or procfs file using a cached file descriptor, rewinding to offset 0 on each read.
/// If seek or read encounters an I/O error (e.g. driver rebind, power state change), the invalid
/// descriptor is immediately purged from the cache, and a fresh open is attempted.
pub fn read_cached_file(path: &str) -> std::io::Result<String> {
    let resolved = resolve_sysfs_path(path);
    let resolved_str = resolved.to_string_lossy().to_string();
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);

    match map.entry(resolved_str.clone()) {
        Entry::Occupied(mut entry) => {
            let file = entry.get_mut();
            if file.seek(SeekFrom::Start(0)).is_err() {
                entry.remove();
                return read_fresh_and_cache(map, &resolved_str);
            }
            let mut content = String::new();
            if file.read_to_string(&mut content).is_err() {
                entry.remove();
                return read_fresh_and_cache(map, &resolved_str);
            }
            Ok(content)
        }
        Entry::Vacant(_) => read_fresh_and_cache(map, &resolved_str),
    }
}

/// Helper function to open a file fresh, read its contents, and store the descriptor in cache upon success.
fn read_fresh_and_cache(map: &mut HashMap<String, File>, path: &str) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    map.insert(path.to_string(), file);
    Ok(content)
}

/// Reads a sysfs file using cached descriptor and parses as u32.
pub fn read_cached_u32(path: &str) -> Option<u32> {
    read_cached_file(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Reads a sysfs file using cached descriptor and parses as i32.
pub fn read_cached_i32(path: &str) -> Option<i32> {
    read_cached_file(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_cache() -> std::sync::MutexGuard<'static, ()> {
        let lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_file_cache();
        lock
    }

    #[test]
    fn test_read_cached_file_basic() {
        let _lock = reset_cache();
        let test_dir =
            std::env::temp_dir().join(format!("ventop_fc_test_basic_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&test_dir);
        let file_path = test_dir.join("test_node");
        std::fs::write(&file_path, "hello cached world\n").unwrap();

        let path_str = file_path.to_str().unwrap();
        let content1 = read_cached_file(path_str).unwrap();
        assert_eq!(content1.trim(), "hello cached world");

        let content2 = read_cached_file(path_str).unwrap();
        assert_eq!(content2.trim(), "hello cached world");

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_read_cached_numbers() {
        let _lock = reset_cache();
        let test_dir =
            std::env::temp_dir().join(format!("ventop_fc_test_num_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&test_dir);
        let path_u = test_dir.join("num_u");
        let path_i = test_dir.join("num_i");
        std::fs::write(&path_u, "42000\n").unwrap();
        std::fs::write(&path_i, "-1500\n").unwrap();

        assert_eq!(read_cached_u32(path_u.to_str().unwrap()), Some(42000));
        assert_eq!(read_cached_i32(path_i.to_str().unwrap()), Some(-1500));

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_error_eviction_and_recovery() {
        let _lock = reset_cache();
        let test_dir =
            std::env::temp_dir().join(format!("ventop_fc_test_evict_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&test_dir);
        let file_path = test_dir.join("sensor_node");
        std::fs::write(&file_path, "200\n").unwrap();
        let path_str = file_path.to_str().unwrap();

        // 1. Verify reading non-existent file fails and does not leave a cache entry
        let missing_path = test_dir.join("non_existent");
        assert!(read_cached_file(missing_path.to_str().unwrap()).is_err());
        {
            let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            assert!(!guard
                .as_ref()
                .unwrap()
                .contains_key(missing_path.to_str().unwrap()));
        }

        // 2. Pre-seed cache with a write-only file handle simulating a poisoned/unreadable node
        let broken_handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&file_path)
            .unwrap();
        {
            let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            let map = guard.get_or_insert_with(HashMap::new);
            map.insert(path_str.to_string(), broken_handle);
        }

        // 3. read_cached_file must detect the read failure, evict the broken handle,
        // re-open fresh, and successfully return the content.
        let recovered =
            read_cached_file(path_str).expect("should recover via eviction and re-open");
        assert_eq!(recovered.trim(), "200");

        // 4. Verify subsequent read works using the newly cached working handle
        assert_eq!(read_cached_u32(path_str), Some(200));

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_root_path_resolution() {
        let root = Path::new("/custom/sysfs/root");
        assert_eq!(
            resolve_path_with_root(root, Path::new("/sys/class/thermal")),
            PathBuf::from("/custom/sysfs/root/sys/class/thermal")
        );
        assert_eq!(
            resolve_path_with_root(root, Path::new("sys/class/thermal")),
            PathBuf::from("/custom/sysfs/root/sys/class/thermal")
        );
        // Idempotent if already starts with root
        assert_eq!(
            resolve_path_with_root(root, Path::new("/custom/sysfs/root/sys/devices/system/cpu")),
            PathBuf::from("/custom/sysfs/root/sys/devices/system/cpu")
        );

        // When root is "/", returns path directly
        let default_root = Path::new("/");
        assert_eq!(
            resolve_path_with_root(default_root, Path::new("/sys/class/hwmon")),
            PathBuf::from("/sys/class/hwmon")
        );
    }
}
