//! Fail-closed guard of the data-directory pass, run BEFORE the manifest is
//! published: the destination run directory must hold no reserved audit chain
//! file the accepted round did not capture.

use std::fs;
use std::path::Path;

use super::super::copy_types::{CopiedFile, CopyError};
use crate::sync::durability::audit_chain_files::is_audit_log_file_name;

/// Refuses the pass when the destination run directory holds a reserved audit
/// chain file (`audit.log` or `audit.log.` + 20 ASCII digits) that the accepted
/// (manifest-defining) round did not capture.
///
/// Whichever pass or round copied it, the copy stays: this check NEVER deletes
/// anything. The cases it covers are exactly the leftovers a rotation or a
/// retention sweep creates under a running copy — pass D copies an archive
/// that retention prunes before pass E lists the directory, an unstable round
/// copies an archive the accepted round no longer sees, a rotation renames the
/// active log and the accepted round has no active log at all. Publishing then
/// either describes a file set the destination contradicts or drops bytes a
/// captured anchor may name, and the run's own inventory self-verification
/// would refuse the snapshot after the fact anyway (that refusal is SAFE: the
/// protocol's cleanup guard removes the whole run directory and the scheduler
/// retries on its next period). This check moves the same refusal BEFORE the
/// manifest is written and makes it typed and path-free
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
pub(super) fn refuse_unlisted_destination_audit_file(
    dest_root: &Path,
    accepted: &[CopiedFile],
) -> Result<(), CopyError> {
    let entries = fs::read_dir(dest_root).map_err(|source| CopyError::Dest {
        path: dest_root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| CopyError::Dest {
            path: dest_root.to_path_buf(),
            source,
        })?;
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
        if accepted.iter().any(|file| file.rel_path == name) {
            continue;
        }
        return Err(CopyError::AuditArchiveLost);
    }
    Ok(())
}
