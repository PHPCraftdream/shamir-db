//! Fail-closed guard of the data-directory pass, run BEFORE the manifest is
//! published: the destination run directory must hold no reserved audit chain
//! file the accepted round did not capture.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use super::super::copy_types::{CopiedFile, CopyError};
use crate::sync::durability::audit_chain_files::{
    is_audit_active_log_name, is_audit_log_file_name,
};
use crate::sync::durability::backup::online::progress::CancelFlag;

/// Extra capture rounds the data-directory pass may run to observe the ACTIVE
/// log a rotation reopened. `rotate_locked` renames `audit.log` and reopens it
/// a moment later, so a listing inside that window leaves the destination with
/// an earlier active-log copy the accepted round cannot list; master simply
/// removed that copy, this pass never deletes, so it retries only when a new
/// archive proves the rotation. Up to three extra active-reopen attempts are
/// allowed. A fixed count, not a configuration key: the window is the
/// appender's own rename-to-reopen gap, not a tunable.
pub(super) const ACTIVE_REOPEN_ROUNDS: usize = 3;

/// Whether a stable-round transition is safe to accept. An active log
/// reappearing after a stable round omitted it must be backed by rotation
/// evidence retained from a stable round that also omitted the active log;
/// an archive first observed in the reappearance round does not prove that
/// the intervening missing-active round was a rotation window. Otherwise the
/// same active name could hide replacement bytes and a lost chain tail.
pub(super) fn active_log_reappearance_is_unsafe(
    previous_active_present: Option<bool>,
    current_active_present: bool,
    missing_round_had_rotation_evidence: bool,
) -> bool {
    previous_active_present == Some(false)
        && current_active_present
        && !missing_round_had_rotation_evidence
}

#[cfg(test)]
mod continuity_policy_tests {
    use super::active_log_reappearance_is_unsafe;

    #[test]
    fn active_reappearance_requires_evidence_from_missing_active_round() {
        // A same-name active file can reappear with replacement bytes.
        assert!(active_log_reappearance_is_unsafe(Some(false), true, false));
        // A rotation observed while the active log was missing is retained
        // across retries and permits its eventual reappearance.
        assert!(!active_log_reappearance_is_unsafe(Some(false), true, true));
        // No reappearance is not this continuity violation.
        assert!(!active_log_reappearance_is_unsafe(
            Some(false),
            false,
            false
        ));
        // The first stable round has no previous source-round state.
        assert!(!active_log_reappearance_is_unsafe(None, true, false));
    }
}

/// Which reserved audit chain file the destination holds that the accepted
/// round did not capture. Archives have priority over the active log: any
/// unlisted archive refuses immediately, even if `audit.log` is also left
/// over. Only an active-log-only leftover may be retried, and only when a new
/// archive proves a rotation since the prior stable listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnlistedAudit {
    /// A rotation archive (`audit.log.` + 20 ASCII digits).
    Archive,
    /// The active `audit.log` — the rotation window, which may close by itself.
    ActiveLog,
}

/// Snapshot the exact reserved archive names already present at pass entry.
/// This is read-only and supplies the baseline when a later pass first sees a
/// stale active-log copy; otherwise its first stable round has no prior round
/// from which to prove that a rotation added an archive.
pub(super) fn destination_archive_names(
    dest_root: &Path,
    cancel: &CancelFlag,
) -> Result<BTreeSet<String>, CopyError> {
    if cancel.is_cancelled() {
        return Err(CopyError::Cancelled);
    }
    let entries = fs::read_dir(dest_root).map_err(|source| CopyError::Dest {
        path: dest_root.to_path_buf(),
        source,
    })?;
    let mut archives = BTreeSet::new();
    for entry in entries {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let path = entry
            .map_err(|source| CopyError::Dest {
                path: dest_root.to_path_buf(),
                source,
            })?
            .path();
        let Some(name) = path.file_name() else {
            continue;
        };
        if is_audit_log_file_name(name) && !is_audit_active_log_name(name) {
            if let Some(name) = name.to_str() {
                archives.insert(name.to_owned());
            }
        }
    }
    if cancel.is_cancelled() {
        return Err(CopyError::Cancelled);
    }
    Ok(archives)
}

/// Names the reserved audit chain file (`audit.log` or `audit.log.` + 20 ASCII
/// digits) the destination run directory holds and the accepted
/// (manifest-defining) round did not capture. If the leftover is an archive,
/// refuse immediately even when an active-log leftover is also present. Only
/// an active-log-only leftover may receive up to [`ACTIVE_REOPEN_ROUNDS`]
/// retries, regardless of archive evidence; archive evidence from missing-
/// active rounds governs whether a later same-name reappearance is safe. A
/// same-name replacement without such evidence remains refused. The name check proves
/// neither chain continuity nor archive contents. This check NEVER deletes
/// anything; cleanup attempts to remove the whole run directory but may fail
/// and leave it behind. A later eligible tick may start a fresh run; no same-run
/// scheduler retry is guaranteed. Publishing then either describes a file set
/// the destination contradicts or drops bytes a captured anchor may name, and
/// self-verification would refuse the snapshot after the fact anyway.
///
/// This check moves that refusal BEFORE the manifest is written and makes it
/// typed and path-free
/// ([`CopyError::AuditArchiveLost`], token `copy-audit-archive-lost`).
///
/// Phase 1 of this pass excludes reserved audit names by construction
/// (`list_files`' skip rule), so the accepted audit round is the complete set
/// of reserved names the snapshot lists. Only exact reserved names at the top
/// of the run directory are considered; every other destination entry is left
/// to self-verification.
///
/// A needed prefix that was already gone BEFORE any pass captured it remains
/// the documented pre-existing limit (see the guide).
pub(super) fn unlisted_destination_audit_file(
    dest_root: &Path,
    accepted: &[CopiedFile],
    cancel: &CancelFlag,
) -> Result<Option<UnlistedAudit>, CopyError> {
    if cancel.is_cancelled() {
        return Err(CopyError::Cancelled);
    }
    let entries = fs::read_dir(dest_root).map_err(|source| CopyError::Dest {
        path: dest_root.to_path_buf(),
        source,
    })?;
    if cancel.is_cancelled() {
        return Err(CopyError::Cancelled);
    }
    let mut paths = Vec::new();
    let mut entries = entries.into_iter();
    loop {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let Some(entry) = entries.next() else {
            break;
        };
        paths.push(
            entry
                .map_err(|source| CopyError::Dest {
                    path: dest_root.to_path_buf(),
                    source,
                })?
                .path(),
        );
    }
    #[cfg(test)]
    let paths = super::test_seams::fire_destination_audit_order(dest_root, paths);
    let mut saw_active_log = false;
    let mut saw_archive = false;
    for path in paths {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        if !is_audit_log_file_name(name) {
            continue;
        }
        let Some(name) = name.to_str() else {
            continue;
        };
        if accepted.iter().any(|file| file.rel_path == name) {
            continue;
        }
        if is_audit_active_log_name(OsStr::new(name)) {
            saw_active_log = true;
        } else {
            saw_archive = true;
        }
    }
    if cancel.is_cancelled() {
        return Err(CopyError::Cancelled);
    }
    if saw_archive {
        Ok(Some(UnlistedAudit::Archive))
    } else if saw_active_log {
        Ok(Some(UnlistedAudit::ActiveLog))
    } else {
        Ok(None)
    }
}
