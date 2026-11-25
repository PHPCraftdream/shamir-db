//! Export-root validation, exclusive root lock, and stale-temp cleanup for
//! the online export job (brief 56-r3-2b item 4).
//!
//! [`ExportRoot::open`] is the single entry point the job registry calls
//! once, before any export job starts; the returned value is held for the
//! lifetime of the registry so the exclusive root lock survives until ALL
//! jobs are done — a second server pointed at the same export root cannot
//! delete live temp directories of the first one (its `open` fails with
//! [`io::ErrorKind::ResourceBusy`]).
//!
//! Security boundary (rust-intel): the export root is written to by the
//! server but named by an operator, so the path must never alias the data
//! directory (the server would then scan and delete next to its own store)
//! and must never be a symlink: the leaf is refused when it is a
//! symlink/reparse point, and creation goes only through
//! `create_private_dir`, which builds an owner-only directory and never
//! follows a symlink. Reuse of `create_private_dir`/`verify_private_dir`
//! (via the hidden `shamir_db::engine::function` re-export) keeps the
//! Windows DACL work in the one crate that owns its `unsafe` FFI; this file
//! adds NO new dependency and NO unsafe.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use shamir_db::engine::function::{create_private_dir, verify_private_dir};

use super::super::logical_dump::LIVE_STAGE_PREFIX;

/// Exclusive root-lock file name, created inside the export root. It does
/// NOT match the stale-temp pattern ([`LIVE_STAGE_PREFIX`] + exactly 6 ASCII
/// alphanumeric characters), so cleanup never touches it.
const LOCK_FILE_NAME: &str = ".shamir-export-root.lock";

/// Validated, locked export root. Dropping it releases the lock.
pub(crate) struct ExportRoot {
    path: PathBuf,
    /// Keeps the exclusive advisory lock on [`LOCK_FILE_NAME`] alive; the
    /// OS releases it when the file (last handle) is closed.
    _lock: File,
}

impl ExportRoot {
    /// Validate `path` against `data_dir`, create at most ONE missing leaf
    /// through the secure helper, take the exclusive root lock, and remove
    /// stale temp directories of crashed jobs. All jobs must be finished
    /// before this value is dropped.
    pub(crate) fn open(path: &Path, data_dir: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid(format!(
                "export root {} must be an absolute path",
                path.display()
            )));
        }

        // Normalize the OPERATOR-provided spelling before any filesystem
        // inspection: `components().collect::<PathBuf>()` strips a trailing
        // separator and `.` components, so `symlink_metadata` below always
        // inspects the true leaf instead of a trailing-separator spelling
        // (which on some platforms makes the metadata call resolve past the
        // leaf). A `..` component is refused outright: it could make the
        // raw-spelling leaf check and the canonicalized placement check
        // disagree, and an operator has no legitimate reason to spell the
        // export root with `..` (parent/operator trust assumptions are
        // unchanged — the path itself must just name its real leaf).
        let path = path.components().collect::<PathBuf>();
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(invalid(format!(
                "export root {} must not contain a `..` component",
                path.display()
            )));
        }

        // Canonicalize the data directory first: it must exist, and every
        // comparison below happens in canonical form so neither a relative
        // spelling nor any symlink in the DATA path can defeat the checks.
        // `canonicalize` returns a fully normalized path (no trailing
        // separator, no `.`/`..` components) on every platform.
        let data_canon = data_dir.canonicalize().map_err(|e| {
            invalid(format!(
                "data directory {} cannot be canonicalized: {e}",
                data_dir.display()
            ))
        })?;

        // The leaf must not be a symlink (or Windows reparse point). Inspect
        // the FINAL component of the RECONSTRUCTED path with
        // `symlink_metadata` — never through it.
        let leaf_meta = match fs::symlink_metadata(&path) {
            Ok(meta) => Some(meta),
            // Distinguish "missing leaf" from "broken/looping symlink":
            // canonicalize a missing path fails too, so test the parent.
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if let Some(meta) = &leaf_meta {
            if meta.file_type().is_symlink() {
                return Err(invalid(format!(
                    "export root {} is a symlink or reparse point",
                    path.display()
                )));
            }
            if !meta.is_dir() {
                return Err(invalid(format!(
                    "export root {} exists and is not a directory",
                    path.display()
                )));
            }
        }

        // Authoritative leaf check on the RECONSTRUCTED, still-unresolved
        // path BEFORE `canonicalize`: `verify_private_dir` uses
        // `symlink_metadata` + the Windows reparse-bit and DACL checks on
        // this exact spelling, so the Windows verifier sees the LEAF, not a
        // canonicalize-resolved target behind it. All errors here are
        // PermissionDenied from the shared helper; re-map to the same
        // invalid-input error the other leaf refusals use.
        if leaf_meta.is_some() {
            verify_private_dir(&path).map_err(|e| {
                invalid(format!(
                    "export root {} is not an owner-private real directory: {e}",
                    path.display()
                ))
            })?;
        }

        // Canonical root: either canonicalize (existing real dir) or — when
        // the leaf is missing — canonicalize the parent and append the leaf
        // name, so the placement checks below see the same spelling in both
        // cases. Both branches yield a NORMALIZED path: `canonicalize`
        // resolves and strips `.`/`..`/trailing separators, and the missing-
        // leaf branch joins a single plain component onto the canonical
        // parent. The parent must already exist: only ONE missing leaf is
        // ever created. If the leaf is a Windows junction/reparse point that
        // `file_type().is_symlink()` misses, `verify_private_dir` above (on
        // the unresolved spelling) has ALREADY refused the leaf — long before
        // `canonicalize` could resolve a target behind it.
        let root_canon: PathBuf = if leaf_meta.is_some() {
            path.canonicalize()?
        } else {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| {
                    invalid(format!(
                        "export root {} has no parent directory",
                        path.display()
                    ))
                })?;
            let parent_canon = parent.canonicalize().map_err(|e| {
                invalid(format!(
                    "export root parent {} must exist as a canonical directory: {e}",
                    parent.display()
                ))
            })?;
            parent_canon.join(
                path.file_name()
                    .ok_or_else(|| invalid("export root has no file name".to_string()))?,
            )
        };

        // Placement: never inside the data directory, never equal to it, and
        // never an ANCESTOR of it (an ancestor would put the data directory
        // itself under cleanup's scan root). `Path::starts_with` is
        // component-wise on the canonical forms, so `..` and aliasing
        // spellings cannot slip through.
        if root_canon == data_canon {
            return Err(invalid(format!(
                "export root {} must not be the data directory",
                path.display()
            )));
        }
        if root_canon.starts_with(&data_canon) {
            return Err(invalid(format!(
                "export root {} must not be inside the data directory {}",
                path.display(),
                data_dir.display()
            )));
        }
        if data_canon.starts_with(&root_canon) {
            return Err(invalid(format!(
                "export root {} must not contain the data directory {}",
                path.display(),
                data_dir.display()
            )));
        }

        // Create the one missing leaf (or re-verify an existing one) with
        // the shared owner-private helper: owner-only access, symlink AND
        // Windows reparse-point refusal (for the missing-leaf branch this is
        // the authoritative check; the existing-leaf branch was already
        // verified pre-canonicalize above), and — on Windows — a protected
        // DACL with exactly one full-access ACE for the current user SID.
        if leaf_meta.is_none() {
            create_private_dir(&root_canon)?;
        } else {
            // Retained final check on the canonical form: it re-asserts the
            // same owner-private invariants on the resolved spelling (the
            // pre-canonicalize check above is the authoritative leaf check).
            verify_private_dir(&root_canon)?;
        }

        let lock = take_root_lock(&root_canon)?;

        // Exclusive lock held: stale temp directories of crashed jobs can be
        // removed. Only exact `.shamir-export-` + 6 ASCII alphanumeric
        // names, and only real directories (file type from
        // symlink_metadata, never through a symlink); final artifacts and
        // anything else — lock file, operator files, broad prefixes — are
        // left untouched.
        remove_stale_stages(&root_canon)?;

        Ok(Self {
            path: root_canon,
            _lock: lock,
        })
    }

    /// The canonical export root; temp directories and final artifacts live
    /// here.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Create the lock file and take a NON-BLOCKING exclusive advisory lock
/// (`File::try_lock`, std since 1.89, LockFileEx on Windows / flock on
/// Unix). A held lock (another server on the same root) returns
/// [`ErrorKind::ResourceBusy`] — never blocks, never removes the other
/// server's live temp directories.
fn take_root_lock(root: &Path) -> io::Result<File> {
    let lock_path = root.join(LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(io::Error::new(
            ErrorKind::ResourceBusy,
            format!(
                "export root {} is already in use by another server process",
                root.display()
            ),
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// `true` only for a temp-directory name: exactly [`LIVE_STAGE_PREFIX`]
/// followed by exactly 6 ASCII alphanumeric characters (tempfile's random
/// suffix shape). Anything else — final artifacts, the lock file, broad
/// prefixes, longer or shorter suffixes — is not a match.
fn is_stale_stage_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(LIVE_STAGE_PREFIX) else {
        return false;
    };
    suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Remove stale temp directories under the locked root. `symlink_metadata`
/// decides: only real non-reparse directories are removed, a symlink,
/// junction/reparse point or regular-file entry is skipped, and read errors
/// are reported (fail closed).
fn remove_stale_stages(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            // Not valid UTF-8 → cannot match the ASCII pattern; skip.
            continue;
        };
        if !is_stale_stage_name(&name) {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        // `is_dir` alone is not enough on Windows: a junction/reparse point
        // to a directory REPORTS as a directory through `symlink_metadata`,
        // and `remove_dir_all` on it would recurse into the TARGET. Skip any
        // reparse-point entry (std `MetadataExt::file_attributes`, local
        // `FILE_ATTRIBUTE_REPARSE_POINT` constant — no unsafe, no new dep);
        // on non-Windows only symlinks exist and `is_symlink` already
        // excludes them (a real directory is never a symlink).
        if meta.is_dir() && !is_reparse_point(&meta) {
            fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

/// `true` when `meta` (from `symlink_metadata`) carries the Windows
/// reparse-point bit (junction/symlink/mount point). Local constant — same
/// value as the one in `shamir-private-dir` — keeps this file free of any
/// unsafe code and any new dependency.
#[cfg(windows)]
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Non-Windows: `symlink_metadata`'s file type is authoritative there; a
/// plain directory is never a symlink, so nothing extra to detect.
#[cfg(not(windows))]
fn is_reparse_point(_meta: &fs::Metadata) -> bool {
    false
}

fn invalid(message: String) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, message)
}
