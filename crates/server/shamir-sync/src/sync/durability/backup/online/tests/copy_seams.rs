//! Body of `copy_engine::test_seams` (kept out of `copy_engine.rs` by
//! the no-inline-test-modules layout rule; reached via
//! `#[cfg(test)] #[path = "tests/copy_seams.rs"]`). See the parent
//! declaration for the scoping and compile-time guarantees.

use std::cell::RefCell;
use std::fs;
use std::io::Read;
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
/// Replaces the `symlink_metadata` RESULT `list_files` consumes for a
/// matching entry path: `Some(result)` supplies it, `None` falls through
/// to the real metadata read. The injected result flows through the very
/// match arm that handles a genuine metadata error, so an injected
/// failure can never take a path the real code does not have. Lets a
/// test prove a directory-entry failure is never silently dropped from
/// the snapshot listing.
pub type MetadataHook = Box<dyn FnMut(&Path) -> Option<std::io::Result<fs::Metadata>> + 'static>;
/// Substitutes `fs::remove_dir_all` in `RunCleanup::drop` for matching
/// owned partial directories: `Some(result)` replaces the real removal,
/// `None` falls through. Lets a test inject a failed partial-tree
/// cleanup without depending on the OS refusing a removal.
pub type RemoveDirAllHook = Box<dyn FnMut(&Path) -> Option<std::io::Result<()>> + 'static>;
/// Substitutes the directory fsync of `manifest::fsync_dest_dir` — the ONE
/// rule the copy passes and BOTH publications (`write_manifest`,
/// `write_backup_meta`) go through — for a matching destination directory:
/// `Some(result)` replaces the real (platform) fsync and is then processed by
/// `tolerate_dir_sync_capability`, exactly like a genuine result; `None` falls
/// through. Lets a test inject a destination-side fsync failure the platform
/// cannot be made to produce, and lets it separate the copy-pass context from
/// the publication context.
pub type FsyncDirHook = Box<dyn FnMut(&Path) -> Option<std::io::Result<()>> + 'static>;
/// Substitutes the publication rename `manifest::write_manifest`
/// performs for a matching destination target: `Some(result)` replaces
/// the real rename, `None` falls through. Lets a test fail the manifest
/// publication at its final step.
pub type ManifestRenameHook = Box<dyn FnMut(&Path) -> Option<std::io::Result<()>> + 'static>;

use crate::sync::durability::backup::online::progress::BackupStage;

thread_local! {
    static AFTER_FIRST_WRITE: RefCell<Option<IoHook>> = RefCell::new(None);
    static AFTER_COPY: RefCell<Option<CopyHook>> = RefCell::new(None);
    static AFTER_CAPTURE: RefCell<Option<CaptureHook>> = RefCell::new(None);
    static AFTER_LISTING: RefCell<Option<ListingHook>> = RefCell::new(None);
    static HASH_FILE: RefCell<Option<HashHook>> = RefCell::new(None);
    static BEFORE_BOUNDARY: RefCell<Option<BoundaryHook>> = RefCell::new(None);
    static MONOTONIC_NOW: RefCell<Option<NowHook>> = RefCell::new(None);
    static SYMLINK_METADATA: RefCell<Option<MetadataHook>> = RefCell::new(None);
    static REMOVE_DIR_ALL: RefCell<Option<RemoveDirAllHook>> = RefCell::new(None);
    static FSYNC_DIR: RefCell<Option<FsyncDirHook>> = RefCell::new(None);
    static MANIFEST_RENAME: RefCell<Option<ManifestRenameHook>> = RefCell::new(None);
}

/// The thread-local slot a [`SeamGuard`] owns.
#[derive(Debug, Clone, Copy)]
enum SeamSlot {
    AfterFirstWrite,
    AfterCopy,
    AfterCapture,
    AfterListing,
    HashFile,
    BeforeBoundary,
    MonotonicNow,
    SymlinkMetadata,
    RemoveDirAll,
    FsyncDir,
    ManifestRename,
}

/// Scoped registration of exactly ONE hook: dropping the guard clears
/// ONLY the slot it installed. Guards are therefore INDEPENDENT — a test
/// can install two faults in one scope and drop them in any order,
/// because no guard's drop clears another guard's hook.
pub struct SeamGuard(SeamSlot);

impl Drop for SeamGuard {
    fn drop(&mut self) {
        match self.0 {
            SeamSlot::AfterFirstWrite => AFTER_FIRST_WRITE.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::AfterCopy => AFTER_COPY.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::AfterCapture => AFTER_CAPTURE.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::AfterListing => AFTER_LISTING.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::HashFile => HASH_FILE.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::BeforeBoundary => BEFORE_BOUNDARY.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::MonotonicNow => MONOTONIC_NOW.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::SymlinkMetadata => SYMLINK_METADATA.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::RemoveDirAll => REMOVE_DIR_ALL.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::FsyncDir => FSYNC_DIR.with(|slot| *slot.borrow_mut() = None),
            SeamSlot::ManifestRename => MANIFEST_RENAME.with(|slot| *slot.borrow_mut() = None),
        }
    }
}

pub fn set_after_first_write(hook: IoHook) -> SeamGuard {
    AFTER_FIRST_WRITE.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::AfterFirstWrite)
}

pub fn set_after_copy(hook: CopyHook) -> SeamGuard {
    AFTER_COPY.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::AfterCopy)
}

pub fn set_after_capture(hook: CaptureHook) -> SeamGuard {
    AFTER_CAPTURE.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::AfterCapture)
}

pub fn set_after_listing(hook: ListingHook) -> SeamGuard {
    AFTER_LISTING.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::AfterListing)
}

pub fn set_hash_file(hook: HashHook) -> SeamGuard {
    HASH_FILE.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::HashFile)
}

pub fn set_before_boundary(hook: BoundaryHook) -> SeamGuard {
    BEFORE_BOUNDARY.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::BeforeBoundary)
}

pub fn set_monotonic_now(hook: NowHook) -> SeamGuard {
    MONOTONIC_NOW.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::MonotonicNow)
}

pub fn set_symlink_metadata(hook: MetadataHook) -> SeamGuard {
    SYMLINK_METADATA.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::SymlinkMetadata)
}

pub fn set_remove_dir_all(hook: RemoveDirAllHook) -> SeamGuard {
    REMOVE_DIR_ALL.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::RemoveDirAll)
}

pub fn set_fsync_dir(hook: FsyncDirHook) -> SeamGuard {
    FSYNC_DIR.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::FsyncDir)
}

pub fn set_manifest_rename(hook: ManifestRenameHook) -> SeamGuard {
    MANIFEST_RENAME.with(|slot| *slot.borrow_mut() = Some(hook));
    SeamGuard(SeamSlot::ManifestRename)
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

/// The registered metadata hook's substitution for one entry path, else
/// `None` (the real `symlink_metadata`). Called only from `list_files`.
pub fn fire_symlink_metadata(path: &Path) -> Option<std::io::Result<fs::Metadata>> {
    SYMLINK_METADATA.with(|slot| slot.borrow_mut().as_mut().and_then(|hook| hook(path)))
}

/// The registered `remove_dir_all` substitution for one owned partial
/// directory, else `None` (the real removal). Called only from
/// `RunCleanup::drop`.
pub fn fire_remove_dir_all(path: &Path) -> Option<std::io::Result<()>> {
    REMOVE_DIR_ALL.with(|slot| slot.borrow_mut().as_mut().and_then(|hook| hook(path)))
}

/// The registered directory-fsync substitution for one destination
/// directory, else `None` (the real platform fsync, or the documented no-op
/// on Windows). Called only from `manifest::fsync_dest_dir`, so the copy
/// passes and the publications share it.
pub fn fire_fsync_dir(path: &Path) -> Option<std::io::Result<()>> {
    FSYNC_DIR.with(|slot| slot.borrow_mut().as_mut().and_then(|hook| hook(path)))
}

/// The registered publication-rename substitution for one destination
/// target, else `None` (the real rename). Called only from
/// `manifest::write_manifest`.
pub fn fire_manifest_rename(target: &Path) -> Option<std::io::Result<()>> {
    MANIFEST_RENAME.with(|slot| slot.borrow_mut().as_mut().and_then(|hook| hook(target)))
}

/// Fjall `file::MAGIC_BYTES` (`b"FJL"`) + `FormatVersion::V3`: the header
/// `restore::fjall_locks::is_fjall_store_dir` requires.
const FJALL_V3_MAGIC: [u8; 4] = [b'F', b'J', b'L', 3];
const FJALL_VERSION_FILE: &str = "version";
const FJALL_KEYSPACES_DIR: &str = "keyspaces";
const FJALL_LOCK_FILE: &str = "lock";

/// The production store fingerprint (`restore::fjall_locks::
/// is_fjall_store_dir`): a `keyspaces` directory AND a `version` header that
/// starts with `FJL\x03`, read bounded to those four magic bytes.
fn is_fjall_store(dir: &Path) -> bool {
    if !dir.join(FJALL_KEYSPACES_DIR).is_dir() {
        return false;
    }
    let mut header = [0u8; 4];
    let genuine = match fs::File::open(dir.join(FJALL_VERSION_FILE))
        .and_then(|mut file| file.read_exact(&mut header))
    {
        Ok(()) => header == FJALL_V3_MAGIC,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            false
        }
        Err(error) => panic!("read the Fjall store version header: {error}"),
    };
    if !genuine {
        return false;
    }
    // An EXISTING artifact must be an ordinary file, exactly as
    // `restore::fjall_locks` requires before it opens a staged store.
    match fs::symlink_metadata(dir.join(FJALL_LOCK_FILE)) {
        Ok(meta) if !meta.is_file() => panic!(
            "the Fjall lock artifact must be a regular file: {}",
            dir.display()
        ),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("read the Fjall lock artifact: {error}"),
    }
    true
}

/// Recreate the empty Fjall `lock` artifact in every Fjall store under `root`.
/// The online producer omits every file named `lock` and pinned Fjall recovery
/// opens it WITHOUT create, so a tree a test copied out of a snapshot gets it
/// back the way the production restore does (`restore/fjall_locks.rs`) before
/// any store in it is opened.
///
/// Reproduced from production: the fingerprint above, the empty `lock` created
/// with `create_new`, the `AlreadyExists` tolerance and the skip of a
/// fingerprinted store's subtree (a Fjall store never nests another one).
/// NOT reproducible here, by design: the typed `RestoreError` of every entry —
/// this is test code and panics instead; the symlink refusal — the online
/// snapshot carries regular files only (`copy_engine::list_files`), so a
/// restored snapshot cannot contain one; and the `stage` bound — the fixture
/// walks the tree it is handed.
pub fn recreate_fjall_locks(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if is_fjall_store(&dir) {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(FJALL_LOCK_FILE))
            {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("recreate the Fjall lock artifact: {error}"),
            }
        } else {
            for entry in fs::read_dir(&dir).expect("read a restored directory") {
                let entry = entry.expect("restored entry");
                if entry.file_type().expect("restored entry type").is_dir() {
                    stack.push(entry.path());
                }
            }
        }
    }
}
