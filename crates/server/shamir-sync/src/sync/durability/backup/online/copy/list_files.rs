//! Deterministic listing of a copy pass, split out of `copy_engine.rs`
//! by the layout rule: the skip rules live here, the copy loop that
//! consumes the listing stays in `copy_engine`.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::super::copy_types::CopyError;
use super::super::progress::CancelFlag;
#[cfg(test)]
use super::test_seams;
use super::SealGate;
use crate::sync::durability::audit_chain_files::is_audit_log_file_name;

fn is_regular_file(meta: &fs::Metadata) -> bool {
    meta.is_file()
}

/// File names skipped in every listing pass, on ALL OSes so the
/// snapshot manifest does not differ across platforms. Both are runtime
/// lock artifacts, not data (restore recreates them on first open):
/// `lock` is fjall's active-store lock artifact — the live engine holds
/// a range lock on it, so on Windows copying it can never succeed while
/// the server is up; `.shamir.lock` is the data-dir maintenance lock
/// (`maintenance_lock.rs`, `fs4` exclusive try-lock, PID inside) held
/// for the whole life of the live server, so the same Windows
/// sharing-violation applies and the file is process state, not data.
const SKIP_FILE_NAMES: &[&str] = &["lock", ".shamir.lock"];

/// Deterministic listing of every regular file under `source_root`
/// (sorted per directory), skipping top-level directories named in
/// `skip_top_level_dirs`, top-level files named in `skip_top_level_files`,
/// any non-regular entry, and files named in [`SKIP_FILE_NAMES`]
/// (`.shamir.lock` and `lock` — runtime lock artifacts, see above).
/// Top-level audit chain files are skipped when `skip_audit_files` is set
/// (phase 1 of [`run_data_dir_copy_pass`]). With a gate, cancel and the
/// clock are checked at every directory iteration — the listing is inside
/// the re-copy budget. Every directory-entry and metadata failure is a
/// typed [`CopyError::Io`]; only an entry that vanished before its
/// metadata read (`NotFound`) is skipped as the documented source race.
///
/// `skip_audit_files` also scopes the AUDIT CLASSIFICATION: only phase 1 of
/// the data-directory pass lists the root whose audit chain files are its own,
/// so only there may an unreadable reserved name take the path-free
/// [`CopyError::AuditListing`] arm. In every other pass (`AuditFiles::InOrder`:
/// WAL directories, store directories, gated re-copies) a file that merely
/// happens to be called `audit.log` is an ordinary file and its failure keeps
/// the path-bearing [`CopyError::Io`] the operator needs.
pub(super) fn list_files(
    source_root: &Path,
    skip_top_level_dirs: &[&str],
    skip_top_level_files: &[&str],
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
    skip_audit_files: bool,
) -> Result<Vec<PathBuf>, CopyError> {
    let mut out = Vec::new();
    let mut stack = vec![source_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        // EVERY directory-entry error is typed, naming the directory:
        // a dropped entry would silently omit a file from the listing.
        let mut entries: Vec<PathBuf> = Vec::new();
        // Failing to OPEN the root keeps the path: the operator needs the
        // directory name to act. Failing to ENUMERATE one of its entries is a
        // different failure (see the arm below).
        let read_dir = fs::read_dir(&dir).map_err(|error| CopyError::Io {
            path: dir.clone(),
            source: error,
        })?;
        for entry in read_dir {
            // Test-only seam: an injected ITERATOR failure for this directory
            // takes the very arm a genuine one takes.
            #[cfg(test)]
            let entry = match test_seams::fire_read_dir_entry(&dir) {
                Some(error) => Err(error),
                None => entry,
            };
            match entry {
                Ok(entry) => entries.push(entry.path()),
                Err(error) => {
                    // An enumeration failure of the data-directory ROOT in
                    // phase 1 could hide a possibly-new rotation archive, so it
                    // is typed path-free there; everywhere else it is the
                    // ordinary path-bearing listing failure.
                    if skip_audit_files && dir.as_path() == source_root {
                        return Err(CopyError::AuditListing { source: error });
                    }
                    return Err(CopyError::Io {
                        path: dir.clone(),
                        source: error,
                    });
                }
            }
        }
        entries.sort();
        for entry in entries {
            if let Some(gate) = gate {
                // Budget checked between individual directory entries.
                gate.check(cancel)?;
            } else if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            // Test-only seam: an injected metadata RESULT for this entry
            // REPLACES the result the listing consumes, so the injected
            // failure travels the same match arm as a genuine one.
            #[cfg(test)]
            let meta_result = match test_seams::fire_symlink_metadata(&entry) {
                Some(result) => result,
                None => fs::symlink_metadata(&entry),
            };
            #[cfg(not(test))]
            let meta_result = fs::symlink_metadata(&entry);
            // Only an entry that VANISHED is tolerated. In phase 1 of the
            // data-directory pass a reserved top-level audit name is the audit
            // listing's own path-free refusal: the phase-1 name exclusion runs
            // AFTER this read.
            let meta = match meta_result {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error)
                    if skip_audit_files
                        && dir.as_path() == source_root
                        && entry.file_name().is_some_and(is_audit_log_file_name) =>
                {
                    return Err(CopyError::AuditListing { source: error });
                }
                Err(error) => {
                    return Err(CopyError::Io {
                        path: entry.clone(),
                        source: error,
                    })
                }
            };
            if meta.is_dir() {
                let at_top = entry.parent() == Some(source_root);
                let name = entry
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if at_top && skip_top_level_dirs.contains(&name.as_str()) {
                    continue;
                }
                stack.push(entry);
            } else if is_regular_file(&meta) {
                let Some(name) = entry.file_name() else {
                    continue;
                };
                if SKIP_FILE_NAMES
                    .iter()
                    .any(|candidate| name == OsStr::new(candidate))
                {
                    continue;
                }
                if skip_audit_files && dir.as_path() == source_root && is_audit_log_file_name(name)
                {
                    continue;
                }
                if !skip_top_level_files.is_empty()
                    && dir.as_path() == source_root
                    && skip_top_level_files
                        .iter()
                        .any(|candidate| name == OsStr::new(candidate))
                {
                    continue;
                }
                out.push(entry);
            }
        }
    }
    out.sort();
    Ok(out)
}
