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

/// The verdict of one bounded release wait.
#[derive(Debug)]
pub(crate) enum ReleaseOutcome {
    /// No store under the root holds its directory lock.
    Released,
    /// These store roots still held their lock when the budget expired.
    StillLocked(Vec<PathBuf>),
    /// The probe itself could not run to a verdict inside the budget: the
    /// blocking pool never took the discovery/probe job (a saturated pool parks
    /// queued jobs, so an unbounded await here would keep the public shutdown
    /// inside this barrier forever). Distinct from [`Self::StillLocked`],
    /// because nothing is known about the stores.
    Unverified,
}

/// Wait until no fjall store under `root` is locked, bounded end to end.
///
/// Every probe await is bounded by the REMAINING budget, so neither discovery
/// nor a lock probe can outlive the timeout on a busy pool; the verdict says
/// which of the three cases happened.
pub(crate) async fn release_stores(root: &Path, timeout: Duration) -> ReleaseOutcome {
    let deadline = Instant::now() + timeout;
    let root = root.to_path_buf();
    let mut pending = match bounded_probe(deadline, move || fjall_dirs(&root)).await {
        Some(pending) => pending,
        None => return ReleaseOutcome::Unverified,
    };
    let mut delay = POLL_START;
    loop {
        pending = match bounded_probe(deadline, move || {
            pending.retain(|dir| !is_released(dir));
            pending
        })
        .await
        {
            Some(pending) => pending,
            None => return ReleaseOutcome::Unverified,
        };
        if pending.is_empty() {
            return ReleaseOutcome::Released;
        }
        if Instant::now() >= deadline {
            return ReleaseOutcome::StillLocked(pending);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(POLL_MAX);
    }
}

/// Run ONE blocking probe, bounded by `deadline`. `None` when the budget ran out
/// before a pool worker took the job (a saturated pool) or when the job ended
/// without a value.
async fn bounded_probe<T: Send + 'static>(
    deadline: Instant,
    probe: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(probe());
    });
    match tokio::time::timeout_at(deadline, rx).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Fail-closed wrapper for the callers that must not publish while the lock
/// state is UNKNOWN: `Unverified` is reported as a failure of the given root.
pub(crate) async fn wait_stores_released(
    root: &Path,
    timeout: Duration,
) -> Result<(), Vec<PathBuf>> {
    match release_stores(root, timeout).await {
        ReleaseOutcome::Released => Ok(()),
        ReleaseOutcome::StillLocked(locked) => Err(locked),
        ReleaseOutcome::Unverified => Err(vec![root.to_path_buf()]),
    }
}
