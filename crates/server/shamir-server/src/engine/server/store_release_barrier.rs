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

/// One lock re-check over a discovered store set: keep only the roots whose
/// directory lock is still held.
fn still_locked(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.retain(|dir| !is_released(dir));
    dirs
}

/// The verdict of one bounded release wait.
///
/// The three cases are distinct in what they claim about the locks:
/// - [`Self::Released`]: every store under the root answered the probe unlocked.
/// - [`Self::StillLocked`]: at least one store root was OBSERVED holding its
///   directory lock and the budget did not allow another confirmation — the
///   last observation is the verdict, even if it is the last one that fitted
///   inside the budget.
/// - [`Self::Unverified`]: the store state is UNKNOWN — discovery itself never
///   produced a store set, so there is nothing to name.
#[derive(Debug)]
pub(crate) enum ReleaseOutcome {
    /// No store under the root holds its directory lock.
    Released,
    /// These store roots were still holding their lock when the budget expired.
    StillLocked(Vec<PathBuf>),
    /// The store state is UNKNOWN: the discovery probe never ran to a verdict
    /// inside the budget (a saturated blocking pool parks queued jobs, so an
    /// unbounded await here would keep the public shutdown inside this barrier
    /// forever), so no store root was ever discovered. A store that WAS
    /// discovered and then observed locked is [`Self::StillLocked`], never this.
    Unverified,
}

/// Wait until no fjall store under `root` is locked, bounded end to end.
///
/// Every probe await is bounded by the REMAINING budget, so neither discovery
/// nor a lock probe can outlive the timeout on a busy pool; the verdict says
/// which of the three cases happened. The budget is checked BEFORE a probe is
/// submitted, so an already-spent budget never turns a known held set into an
/// `Unverified` verdict: a lock that was observed held is reported as
/// [`ReleaseOutcome::StillLocked`], and only a run that never discovered the
/// stores at all reports [`ReleaseOutcome::Unverified`].
pub(crate) async fn release_stores(root: &Path, timeout: Duration) -> ReleaseOutcome {
    let deadline = Instant::now() + timeout;
    let root = root.to_path_buf();
    // `None` = discovery has not produced a store set yet, which is exactly the
    // "unknown" state; `Some(vec![])` = a discovered set with nothing locked.
    let mut known: Option<Vec<PathBuf>> = bounded_probe(deadline, move || fjall_dirs(&root)).await;
    let mut delay = POLL_START;
    loop {
        // The known set is re-checked at the TOP of every iteration, so the
        // verdict is always derived from the freshest observation and never
        // from a probe that could not run.
        let Some(held) = known.take() else {
            return ReleaseOutcome::Unverified;
        };
        if held.is_empty() {
            return ReleaseOutcome::Released;
        }
        if Instant::now() >= deadline {
            return ReleaseOutcome::StillLocked(held);
        }
        // The probe closure owns the set, so keep a copy for the fallback: a
        // probe that cannot run (budget spent while scheduling, or a saturated
        // pool) must not discard the locks already observed held.
        let fallback = held.clone();
        known = match bounded_probe(deadline, move || still_locked(held)).await {
            Some(held) => Some(held),
            None => return ReleaseOutcome::StillLocked(fallback),
        };
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
