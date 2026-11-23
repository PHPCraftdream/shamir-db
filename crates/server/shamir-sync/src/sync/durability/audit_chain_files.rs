//! The audit chain files of a `data_dir` as the online copy sees them: the
//! name classification boot recovery uses, the fresh listing the copy's
//! audit phase is built on, and that phase's stable-listing condition.
//!
//! The names are a documented layout fact of `audit_appender` — the active
//! `audit.log` plus `audit.log.` + 20 ASCII decimal digits archives — and
//! shamir-sync carries no config for them. Only exact names count: boot
//! recovery ignores every other name in the reserved namespace, so a copy
//! must neither capture such a name as an archive nor skip it as one. A
//! per-entry listing failure is typed ([`CopyError::AuditListing`]) rather
//! than a silent omission, so an unreadable entry can never hide a possibly
//! new rotation archive.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::backup::online::copy_engine::{CopyError, CopyPassResult, SealGate};
use super::backup::online::progress::CancelFlag;

/// File name of the ACTIVE audit chain log at the top of `data_dir`.
/// Private: every caller lives in this module (the copy engine uses
/// `is_audit_log_file_name`, never the raw name).
const AUDIT_LOG_FILE_NAME: &str = "audit.log";

/// Rotation archives are exactly `audit.log.` + 20 ASCII decimal digits.
const AUDIT_ARCHIVE_PREFIX: &str = "audit.log.";
const AUDIT_ARCHIVE_DIGITS: usize = 20;

/// Upper bound on the audit chain rounds of one data-directory pass. Each
/// round lists the audit files, copies every file it listed and lists them
/// again; a round whose fresh listing names a rotation archive that round did
/// not copy repeats. The bound counts rounds: a stable round, the last one
/// included, succeeds; no stable round within the bound fails the pass typed
/// (never a silently inconsistent copy).
pub(super) const AUDIT_CAPTURE_ROUNDS: usize = 4;

/// True for the top-level audit chain file names: the active log and its
/// rotation archives.
pub(super) fn is_audit_log_file_name(name: &OsStr) -> bool {
    name == OsStr::new(AUDIT_LOG_FILE_NAME)
        || name.to_str().is_some_and(|name| {
            name.strip_prefix(AUDIT_ARCHIVE_PREFIX)
                .is_some_and(|suffix| {
                    suffix.len() == AUDIT_ARCHIVE_DIGITS
                        && suffix.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
}

/// The audit chain files of a data directory, listed FRESH: the rotation
/// archives in ascending suffix order first, the ACTIVE log last (the order
/// both the copy loop and the hash pass follow, so the manifest's file order
/// stays deterministic). Only exact archive names, only regular files;
/// cancellation and the gate are checked per entry.
///
/// Every enumeration and metadata failure is a typed
/// [`CopyError::AuditListing`] — which carries no path, the copy's error
/// vocabulary being path-free — except an entry that VANISHED between the
/// listing and the metadata read (`NotFound`), which is the
/// retention/rotation race and is skipped. Never a silent omission: an
/// unreadable entry could be a possibly-new rotation archive.
pub(super) fn list_audit_files(
    source_root: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<Vec<PathBuf>, CopyError> {
    let mut archives = Vec::new();
    let mut active = None;
    let entries = fs::read_dir(source_root).map_err(|source| CopyError::Io {
        path: source_root.to_path_buf(),
        source,
    })?;
    // EVERY enumeration error is typed: a dropped entry would silently omit
    // an audit chain file from this listing — and this is the ONE listing
    // both the capture round and the stability check use.
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| CopyError::AuditListing { source })?;
        paths.push(entry.path());
    }
    paths.sort();
    for entry in paths {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let Some(name) = entry.file_name() else {
            continue;
        };
        if !is_audit_log_file_name(name) {
            continue;
        }
        // Test-only seam: an injected metadata RESULT for this entry
        // REPLACES the result the listing consumes, so the injected failure
        // travels the same match arm as a genuine one (the seam
        // `copy_engine::list_files` uses).
        #[cfg(test)]
        let meta_result =
            match super::backup::online::copy_engine::test_seams::fire_symlink_metadata(&entry) {
                Some(result) => result,
                None => fs::symlink_metadata(&entry),
            };
        #[cfg(not(test))]
        let meta_result = fs::symlink_metadata(&entry);
        // Only an entry that VANISHED between the listing and this read is
        // skipped (the retention/rotation race, exactly as in
        // `copy_engine::list_files`); every other failure is typed, never a
        // silent omission of a possibly-new archive.
        let meta = match meta_result {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(CopyError::AuditListing { source }),
        };
        if !meta.is_file() {
            continue;
        }
        if name == OsStr::new(AUDIT_LOG_FILE_NAME) {
            active = Some(entry);
        } else {
            archives.push(entry);
        }
    }
    archives.extend(active);
    Ok(archives)
}

/// True when a FRESH listing of the audit chain files names no rotation
/// archive this capture round did not copy — the stable condition of a
/// data-directory pass's phase 2. The active log is always tolerated: a round
/// that listed it captured it, and the anchors were captured before any audit
/// listing, so bytes appended later are only a longer tail; a rotation landing
/// after a round forces another round, in which its archive is copied.
pub(super) fn audit_capture_is_stable(
    source_root: &Path,
    captured: &CopyPassResult,
    cancel: &CancelFlag,
) -> Result<bool, CopyError> {
    let listing = list_audit_files(source_root, cancel, None)?;
    Ok(listing.iter().all(|path| {
        path.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                name == AUDIT_LOG_FILE_NAME
                    || captured.files.iter().any(|file| file.rel_path == name)
            })
    }))
}

/// Removes the destination's audit chain files the ACCEPTED round did not
/// capture.
///
/// The data-directory pass runs twice into one destination (pass D, then the
/// snapshot-defining pass E) and phase 2 itself may run several rounds. A
/// reserved audit file copied by an earlier pass or an earlier (unstable)
/// round can be gone from the source by the accepted round — a rotation
/// renamed the active log, or retention deleted an archive the writer pruned
/// — while its copy stays behind and fails self-verification as an unlisted
/// file. The destination must hold exactly what the accepted round captured,
/// so every reserved audit name that round did not list goes: the bytes of a
/// rotation live in its archive, and recovery accepts a directory without an
/// active log.
///
/// Only exact reserved names at the top of `dest_root` are considered; every
/// other destination entry is left alone.
pub(super) fn reconcile_destination_audit_files(
    dest_root: &Path,
    captured: &CopyPassResult,
) -> Result<(), CopyError> {
    let entries = fs::read_dir(dest_root).map_err(|source| CopyError::Io {
        path: dest_root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| CopyError::AuditListing { source })?;
        let path = entry.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        if !is_audit_log_file_name(name) {
            continue;
        }
        let Some(name) = name.to_str() else {
            continue;
        };
        if captured.files.iter().any(|file| file.rel_path == name) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CopyError::Io { path, source });
            }
        }
    }
    Ok(())
}
