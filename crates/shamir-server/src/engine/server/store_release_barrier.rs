//! Shutdown barrier: waits until every fjall store under the data dir has
//! dropped its directory lock.
//!
//! After the handle's own references are gone, detached tasks (buffered-store
//! flushers, connection tasks) can keep a store alive for a short while, and
//! fjall releases its `lock` file only when the last handle drops. Probing the
//! lock is the one signal that does not depend on knowing every holder.

use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;

/// fjall's per-database marker files; a directory holding both is a store root.
const LOCK_FILE: &str = "lock";
const VERSION_MARKER: &str = "version";
/// Store roots sit directly under `data_dir` (or one level below it).
const MAX_DEPTH: usize = 4;
const POLL_START: Duration = Duration::from_millis(1);
const POLL_MAX: Duration = Duration::from_millis(25);

/// fjall store roots under `root`; does not descend into a store root.
fn fjall_dirs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if dir.join(LOCK_FILE).is_file() && dir.join(VERSION_MARKER).is_file() {
            found.push(dir);
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    found
}

/// `true` when nobody holds the store's directory lock (or it cannot be probed).
fn is_released(dir: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join(LOCK_FILE))
    else {
        return true;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            true
        }
        Err(TryLockError::WouldBlock) => false,
        Err(TryLockError::Error(_)) => true,
    }
}

/// Wait until no fjall store under `root` is locked. On timeout returns the
/// store roots that are still locked.
pub(crate) async fn wait_stores_released(
    root: &Path,
    timeout: Duration,
) -> Result<(), Vec<PathBuf>> {
    let deadline = Instant::now() + timeout;
    let root = root.to_path_buf();
    let mut pending = tokio::task::spawn_blocking(move || fjall_dirs(&root))
        .await
        .unwrap_or_default();
    let mut delay = POLL_START;
    loop {
        pending = tokio::task::spawn_blocking(move || {
            pending.retain(|dir| !is_released(dir));
            pending
        })
        .await
        .unwrap_or_default();
        if pending.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(pending);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(POLL_MAX);
    }
}
