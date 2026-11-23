//! The audit chain files of a `data_dir` as the online copy sees them: the
//! name classification boot recovery uses, the fresh listing the copy's
//! audit phase is built on, and that phase's stable-listing condition.
//!
//! The names are a documented layout fact of `audit_appender` — the active
//! `audit.log` plus `audit.log.` + 20 ASCII decimal digits archives — and
//! shamir-sync carries no config for them. Only exact names count: boot
//! recovery ignores every other name in the reserved namespace, so a copy
//! must neither capture such a name as an archive nor skip it as one.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::backup::online::copy_engine::{CopyError, CopyPassResult, SealGate};
use super::backup::online::progress::CancelFlag;

/// File name of the ACTIVE audit chain log at the top of `data_dir`.
pub(crate) const AUDIT_LOG_FILE_NAME: &str = "audit.log";

/// Rotation archives are exactly `audit.log.` + 20 ASCII decimal digits.
const AUDIT_ARCHIVE_PREFIX: &str = "audit.log.";
const AUDIT_ARCHIVE_DIGITS: usize = 20;

/// Upper bound on the audit chain rounds of one data-directory pass. Each
/// round lists the audit files, copies every file it listed and lists them
/// again; a round whose fresh listing names a rotation archive that round did
/// not copy repeats. The bound counts rounds: a stable round, the last one
/// included, succeeds; no stable round within the bound fails the pass typed
/// (never a silently inconsistent copy).
pub(crate) const AUDIT_CAPTURE_ROUNDS: usize = 4;

/// True for the top-level audit chain file names: the active log and its
/// rotation archives.
pub(crate) fn is_audit_log_file_name(name: &OsStr) -> bool {
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
pub(crate) fn list_audit_files(
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
    let mut entries: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    for entry in entries {
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
        let meta = match fs::symlink_metadata(&entry) {
            Ok(meta) => meta,
            Err(_) => continue,
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
pub(crate) fn audit_capture_is_stable(
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

/// Removes the destination's active log when this round's capture has none.
///
/// The data-directory pass runs twice into one destination (pass D, then the
/// snapshot-defining pass E). When the source has no active log at E's audit
/// listing — a rotation renamed it and has not reopened it yet — E's manifest
/// has no entry for it, while the copy pass D made would stay behind and fail
/// the self-verification as an unlisted file. The destination must hold
/// exactly what the round captured, so the earlier copy goes: the archive the
/// rotation produced carries its bytes, and recovery accepts a directory
/// without an active log.
pub(crate) fn remove_uncaptured_active_log(
    dest_root: &Path,
    captured: &CopyPassResult,
) -> Result<(), CopyError> {
    if captured
        .files
        .iter()
        .any(|file| file.rel_path == AUDIT_LOG_FILE_NAME)
    {
        return Ok(());
    }
    let stale = dest_root.join(AUDIT_LOG_FILE_NAME);
    match fs::remove_file(&stale) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CopyError::Io {
            path: stale,
            source,
        }),
    }
}
