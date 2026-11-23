//! Shutdown barrier: waits until every fjall store under the data dir has
//! dropped its directory lock.
//!
//! After the handle's own references are gone, detached tasks (buffered-store
//! flushers, connection tasks) can keep a store alive for a short while, and
//! fjall releases its `lock` file only when the last handle drops. Probing the
//! lock is the one signal that does not depend on knowing every holder.
//!
//! A successful probe is an OBSERVATION, not a lock: it says the directory lock
//! was free at that instant, and another process (or a task that opens the store
//! later) can take it again immediately afterwards. The verdict therefore
//! narrows the relaunch race, it cannot close it — and a probe that could not be
//! taken is UNKNOWN state, never a release.

use shamir_collections::TSet;
use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;

/// fjall's per-database marker files; a directory holding both is a store root.
const LOCK_FILE: &str = "lock";
const VERSION_MARKER: &str = "version";
/// Store roots sit directly under `data_dir` (or one level below it). A walk
/// that would have to go deeper cannot know the store set: that is a GAP.
const MAX_DEPTH: usize = 4;
const POLL_START: Duration = Duration::from_millis(1);
const POLL_MAX: Duration = Duration::from_millis(25);

/// A discovered store root: the name to REPORT plus the directory to PROBE.
///
/// The two are deliberately distinct. On Unix `data_dir/users` can be a
/// directory SYMLINK to the fjall directory the launcher opens by pathname
/// (`FjallUserDirectory::open(config.data_dir.join("users"))`), so the lock
/// lives on the link's TARGET: the probe must be taken there, while the report
/// must name the logical path the operator recognises.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreRoot {
    /// The path reported to the caller (`data_dir/users`).
    logical: PathBuf,
    /// The canonical directory its lock file must be opened in.
    probe: PathBuf,
}

/// What ONE lock probe saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockState {
    /// Nobody holds the directory lock (or the lock file is gone: the store was
    /// closed and removed).
    Unlocked,
    /// Another handle — in this process or in another — holds the lock.
    Held,
    /// The lock could not be probed at all: the state is UNKNOWN.
    ProbeError,
}

/// How far the discovery walk got.
#[derive(Debug)]
enum Discovery {
    /// The walk never ran to a value (its blocking job was starved past the
    /// budget), so no store set is known.
    Unknown,
    /// Every directory under the root was classified: this IS the store set.
    Complete(Vec<StoreRoot>),
    /// The walk could not finish — an unreadable directory, a non-UTF8 entry, a
    /// dangling symlink, or directories below the depth cap — so the store set
    /// is UNKNOWN. A gap is never an empty (⇒ released) set.
    Gap,
}

/// `true` for a fjall store root: `version` is a regular marker file, and the
/// `lock` entry only has to EXIST.
///
/// Requiring `lock` to be a regular file would silently skip a root whose
/// `lock` is unusable (a directory, say) — the probe of exactly such a root is
/// the case that must report [`LockState::ProbeError`] instead.
fn is_store_root(dir: &Path) -> bool {
    dir.join(VERSION_MARKER).is_file() && std::fs::symlink_metadata(dir.join(LOCK_FILE)).is_ok()
}

/// Walk `root` for fjall store roots, FOLLOWING directory symlinks.
///
/// The returned roots carry their logical name and their canonical probe path.
/// A cycle is impossible: a canonical path is walked at most once.
fn discover(root: &Path) -> Discovery {
    let mut found = Vec::new();
    // Canonical paths already walked: a symlink cycle, or a second name for a
    // directory already visited, must not be walked twice.
    let mut visited = TSet::default();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((logical, depth)) = stack.pop() {
        // Resolving the probe path is what follows a directory symlink. It can
        // fail on a dangling or unreadable link, which may have hidden a store
        // root: a GAP, not a skip.
        let Ok(probe) = std::fs::canonicalize(&logical) else {
            return Discovery::Gap;
        };
        if !visited.insert(probe.clone()) {
            continue;
        }
        if is_store_root(&probe) {
            found.push(StoreRoot { logical, probe });
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&probe) else {
            return Discovery::Gap;
        };
        let mut children = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                return Discovery::Gap;
            };
            let name = entry.file_name();
            if name.to_str().is_none() {
                // A store root under an unnameable path could not be reported.
                return Discovery::Gap;
            }
            let child = logical.join(&name);
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => children.push(child),
                Ok(kind) if kind.is_symlink() => match std::fs::metadata(&child) {
                    // Follow only DIRECTORY links; a link to a file is not a
                    // store root and needs no further walk.
                    Ok(meta) if meta.is_dir() => children.push(child),
                    Ok(_) => {}
                    Err(_) => return Discovery::Gap,
                },
                Ok(_) => {}
                Err(_) => return Discovery::Gap,
            }
        }
        if children.is_empty() {
            continue;
        }
        if depth >= MAX_DEPTH {
            // Directories remain below the cap, so a store root may sit below
            // it: a GAP, never a silent truncation of the store set.
            return Discovery::Gap;
        }
        for child in children {
            stack.push((child, depth + 1));
        }
    }
    Discovery::Complete(found)
}

/// One lock observation on a store's canonical directory.
fn probe(dir: &Path) -> LockState {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join(LOCK_FILE))
    {
        Ok(file) => file,
        // The store is gone: nothing can hold its lock.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LockState::Unlocked,
        // Locked by another process with no sharing, permissions changing under
        // us, a `lock` that is not a file — all UNKNOWN, never a release.
        Err(_) => return LockState::ProbeError,
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            LockState::Unlocked
        }
        Err(TryLockError::WouldBlock) => LockState::Held,
        Err(TryLockError::Error(_)) => LockState::ProbeError,
    }
}

/// Probe every root; `None` when ANY probe answered [`LockState::ProbeError`],
/// because a partially observed set could under-report a held lock.
fn observe(roots: &[StoreRoot]) -> Option<Vec<StoreRoot>> {
    let mut held = Vec::new();
    for root in roots {
        match probe(&root.probe) {
            LockState::Unlocked => {}
            LockState::Held => held.push(root.clone()),
            LockState::ProbeError => return None,
        }
    }
    Some(held)
}

/// Discovery and the FIRST lock observation, in one blocking job: a starved
/// blocking pool then costs the budget once instead of twice.
struct FirstLook {
    discovery: Discovery,
    /// The first COMPLETED observation over the discovered set; `None` when a
    /// probe errored or the set was never reached.
    observed: Option<Vec<StoreRoot>>,
}

/// The combined discovery + first probe job.
fn first_look(root: &Path) -> FirstLook {
    match discover(root) {
        Discovery::Complete(roots) => {
            let observed = observe(&roots);
            FirstLook {
                discovery: Discovery::Complete(roots),
                observed,
            }
        }
        // A gap carries no store set, so there is nothing to observe.
        other => FirstLook {
            discovery: other,
            observed: None,
        },
    }
}

/// The reportable (logical) names of these roots.
fn logical(roots: Vec<StoreRoot>) -> Vec<PathBuf> {
    roots.into_iter().map(|root| root.logical).collect()
}

/// The verdict of one bounded release wait.
///
/// The three cases are distinct in what they claim about the locks:
/// - [`Self::Released`]: a COMPLETE discovery found the store set and every
///   store in it answered the probe unlocked (an empty set counts — but only
///   after a discovery that finished).
/// - [`Self::StillLocked`]: at least one store root was OBSERVED holding its
///   directory lock and the budget did not allow another confirmation — the
///   last completed observation is the verdict, even if it is the last one that
///   fitted inside the budget. The paths are LOGICAL store roots.
/// - [`Self::Unverified`]: the store state is UNKNOWN — discovery never finished
///   (an unreadable directory, a symlink that does not resolve, a walk cut by
///   the depth cap, a starved blocking pool), or a lock probe errored. This is
///   never a release.
#[derive(Debug)]
pub(crate) enum ReleaseOutcome {
    /// No store under the root holds its directory lock.
    Released,
    /// These logical store roots were still holding their lock when the budget
    /// expired.
    StillLocked(Vec<PathBuf>),
    /// The store state is UNKNOWN: discovery never produced a complete store
    /// set, or a lock probe could not answer at all (a saturated blocking pool
    /// parks queued jobs, so an unbounded await here would keep the public
    /// shutdown inside this barrier forever). A store that WAS discovered and
    /// then observed locked is [`Self::StillLocked`], never this.
    Unverified,
}

/// Wait until no fjall store under `root` is locked, bounded end to end.
///
/// Every probe await is bounded by the REMAINING budget, so neither discovery
/// nor a lock probe can outlive the timeout on a busy pool; the verdict says
/// which of the three cases happened. The budget is checked BEFORE a probe is
/// submitted, so an already-spent budget never turns a known held set into an
/// `Unverified` verdict: a lock that was observed held is reported as
/// [`ReleaseOutcome::StillLocked`], and only a run that never completed a
/// discovery (or a probe) reports [`ReleaseOutcome::Unverified`].
pub(crate) async fn release_stores(root: &Path, timeout: Duration) -> ReleaseOutcome {
    let deadline = Instant::now() + timeout;
    let root = root.to_path_buf();
    // Discovery and the first observation are one job; the two states stay
    // apart afterwards: `discovery` is fixed by that one walk, while `observed`
    // is only ever replaced by a COMPLETED lock probe.
    let (discovery, observed) = match bounded_probe(deadline, move || first_look(&root)).await {
        Some(FirstLook {
            discovery,
            observed,
        }) => (discovery, observed),
        None => (Discovery::Unknown, None),
    };
    let Discovery::Complete(_roots) = discovery else {
        // Unknown or Gap: the store set is not knowable, so no release claim.
        return ReleaseOutcome::Unverified;
    };
    let mut observed = observed;
    let mut delay = POLL_START;
    loop {
        // The verdict always comes from the last COMPLETED observation.
        let Some(held) = observed.take() else {
            // A probe errored (or discovery never produced a set): UNKNOWN.
            return ReleaseOutcome::Unverified;
        };
        if held.is_empty() {
            return ReleaseOutcome::Released;
        }
        if Instant::now() >= deadline {
            return ReleaseOutcome::StillLocked(logical(held));
        }
        // The probe closure owns the set, so keep a copy: a probe that cannot
        // RUN (a saturated pool, or a budget spent while scheduling) must not
        // discard locks already observed held.
        let fallback = held.clone();
        observed = match bounded_probe(deadline, move || observe(&held)).await {
            Some(observed) => observed,
            None => return ReleaseOutcome::StillLocked(logical(fallback)),
        };
        // A fresh observation is answered at once: an errored one is UNKNOWN,
        // and an empty one is a release — no sleep after either.
        match &observed {
            None => return ReleaseOutcome::Unverified,
            Some(held) if held.is_empty() => return ReleaseOutcome::Released,
            Some(_) => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Re-check at the top of the loop: the verdict is `StillLocked`.
            continue;
        }
        // Capped by the remaining budget: the poll must never overshoot the
        // deadline it is bounded by.
        tokio::time::sleep(delay.min(remaining)).await;
        delay = (delay * 2).min(POLL_MAX);
    }
}

/// Run ONE blocking probe, bounded by `deadline`. `None` when the budget ran out
/// before a pool worker took the job (a saturated pool) or when the job ended
/// without a value; an expired job is ABORTED so queued probe work never
/// outlives the barrier.
async fn bounded_probe<T: Send + 'static>(
    deadline: Instant,
    probe: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = tokio::task::spawn_blocking(move || {
        let _ = tx.send(probe());
    });
    match tokio::time::timeout_at(deadline, rx).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) => None,
        Err(_) => {
            // The budget expired with the job still queued (or stuck): abort it
            // rather than leave an orphan probe running after the verdict.
            job.abort();
            None
        }
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
