//! Body of `copy_engine::test_seams` (kept out of `copy_engine.rs` by
//! the no-inline-test-modules layout rule; reached via
//! `#[cfg(test)] #[path = "tests/copy_seams.rs"]`). See the parent
//! declaration for the scoping and compile-time guarantees.

use std::cell::RefCell;
use std::path::Path;
use std::time::Instant;

/// Fired inside `copy_once` after every completed `write_all`:
/// returning `Err` injects a genuine mid-file I/O failure AFTER at
/// least one destination write.
pub type IoHook = Box<dyn FnMut(&Path) -> std::io::Result<()> + 'static>;
/// Fired after a file's copy completed (bytes on the destination,
/// synced): used for deterministic post-listing source mutation and
/// between-file cancellation.
pub type CopyHook = Box<dyn FnMut(&Path) + 'static>;
/// Fired once per file, after that file's snapshot entry is final in a
/// copy pass (bytes on the destination, second-pass hash settled): the
/// moment the pass's view of that file is fixed. Used to rendezvous with
/// the real audit appender between two files' captures.
pub type CaptureHook = Box<dyn FnMut(&Path) + 'static>;
/// Overrides `hash_file` for matching paths: returning `Some(result)`
/// supplies the hash, `None` falls through to the real `hash_file`.
/// Used to force a persistent re-pass hash mismatch.
pub type HashHook = Box<dyn FnMut(&Path) -> Option<std::io::Result<(String, u64)>> + 'static>;
/// Fired at a protocol stage boundary, keyed by the stage being
/// entered (C→D: `Copying` — after the WAL passes, before the first
/// store pass; E: `Sealing`; F: `Verifying`; G: `Done` — after
/// successful self-verification, before Done); used to raise
/// cancellation exactly at boundaries no copy-pass hook can reach.
pub type BoundaryHook = Box<dyn Fn(BackupStage) + 'static>;
/// Overrides the gate's monotonic clock for the in-flight deadline
/// check: called with the REAL `Instant::now()` at every gate clock
/// read, returning the instant the gate must treat as now. Default
/// (unregistered) is the identity — the real clock. Lets a test trip
/// the clock cap mid-copy deterministically (no sleeps): shift the
/// reported instant past the deadline after the first chunk lands.
pub type NowHook = Box<dyn FnMut(Instant) -> Instant + 'static>;

use crate::sync::durability::backup::online::progress::BackupStage;

thread_local! {
    static AFTER_FIRST_WRITE: RefCell<Option<IoHook>> = RefCell::new(None);
    static AFTER_COPY: RefCell<Option<CopyHook>> = RefCell::new(None);
    static AFTER_CAPTURE: RefCell<Option<CaptureHook>> = RefCell::new(None);
    static HASH_FILE: RefCell<Option<HashHook>> = RefCell::new(None);
    static BEFORE_BOUNDARY: RefCell<Option<BoundaryHook>> = RefCell::new(None);
    static MONOTONIC_NOW: RefCell<Option<NowHook>> = RefCell::new(None);
}

/// Scoped registration: every hook is cleared when the guard drops.
pub struct SeamGuard;

impl Drop for SeamGuard {
    fn drop(&mut self) {
        AFTER_FIRST_WRITE.with(|slot| *slot.borrow_mut() = None);
        AFTER_COPY.with(|slot| *slot.borrow_mut() = None);
        AFTER_CAPTURE.with(|slot| *slot.borrow_mut() = None);
        HASH_FILE.with(|slot| *slot.borrow_mut() = None);
        BEFORE_BOUNDARY.with(|slot| *slot.borrow_mut() = None);
        MONOTONIC_NOW.with(|slot| *slot.borrow_mut() = None);
    }
}

pub fn set_after_first_write(hook: IoHook) -> SeamGuard {
    AFTER_FIRST_WRITE.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard
}

pub fn set_after_copy(hook: CopyHook) -> SeamGuard {
    AFTER_COPY.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard
}

pub fn set_after_capture(hook: CaptureHook) -> SeamGuard {
    AFTER_CAPTURE.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard
}

pub fn set_hash_file(hook: HashHook) -> SeamGuard {
    HASH_FILE.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard
}

pub fn set_before_boundary(hook: BoundaryHook) -> SeamGuard {
    BEFORE_BOUNDARY.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard
}

pub fn set_monotonic_now(hook: NowHook) -> SeamGuard {
    MONOTONIC_NOW.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard
}

/// The gate's clock read: the registered hook's verdict, else the real
/// instant. Called only from `SealGate`'s clock checks under `cfg(test)`.
pub fn fire_monotonic_now(real: Instant) -> Instant {
    MONOTONIC_NOW.with(|slot| match slot.borrow_mut().as_mut() {
        Some(hook) => hook(real),
        None => real,
    })
}

pub fn fire_after_first_write(source: &Path) -> std::io::Result<()> {
    AFTER_FIRST_WRITE.with(|slot| match slot.borrow_mut().as_mut() {
        Some(hook) => hook(source),
        None => Ok(()),
    })
}

pub fn fire_after_copy(source: &Path) {
    AFTER_COPY.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(source);
        }
    });
}

pub fn fire_after_capture(source: &Path) {
    AFTER_CAPTURE.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(source);
        }
    });
}

pub fn fire_before_boundary(stage: BackupStage) {
    BEFORE_BOUNDARY.with(|slot| {
        if let Some(hook) = slot.borrow().as_ref() {
            hook(stage);
        }
    });
}

pub fn fire_hash_file(path: &Path) -> Option<std::io::Result<(String, u64)>> {
    HASH_FILE.with(|slot| slot.borrow_mut().as_mut().and_then(|hook| hook(path)))
}
