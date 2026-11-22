//! Body of `copy_engine::test_seams` (kept out of `copy_engine.rs` by
//! the no-inline-test-modules layout rule; reached via
//! `#[cfg(test)] #[path = "tests/copy_seams.rs"]`). See the parent
//! declaration for the scoping and compile-time guarantees.

use std::cell::RefCell;
use std::fs;
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
/// Fired once per data-directory pass, after that pass's file listing is
/// fixed and BEFORE the first file is captured: a test rendezvous point
/// between the listing and the anchor-store capture.
pub type ListingHook = Box<dyn FnMut(&Path) + 'static>;
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
    static AFTER_LISTING: RefCell<Option<ListingHook>> = RefCell::new(None);
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
        AFTER_LISTING.with(|slot| *slot.borrow_mut() = None);
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

pub fn set_after_listing(hook: ListingHook) -> SeamGuard {
    AFTER_LISTING.with(|slot| *slot.borrow_mut() = Some(hook));
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

pub fn fire_after_listing(source: &Path) {
    AFTER_LISTING.with(|slot| {
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

/// Recreate the empty Fjall `lock` artifact in every Fjall store under `root`
/// (a directory holding a `version` file and a `keyspaces` subdirectory). The
/// online producer omits every file named `lock` and pinned Fjall recovery
/// opens it WITHOUT create, so a tree a test copied out of a snapshot gets it
/// back the way the production restore does (`restore/fjall_locks.rs`) before
/// any store in it is opened.
pub fn recreate_fjall_locks(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir.join("version").is_file() && dir.join("keyspaces").is_dir() {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join("lock"))
            {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("recreate the Fjall lock artifact: {error}"),
            }
            continue;
        }
        for entry in fs::read_dir(&dir).expect("read a restored directory") {
            let entry = entry.expect("restored entry");
            if entry.file_type().expect("restored entry type").is_dir() {
                stack.push(entry.path());
            }
        }
    }
}
