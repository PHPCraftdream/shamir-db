//! Scheduled-destination checks of the backup registry, moved out of
//! `registry.rs` (layout): revalidation of the boot-canonical root and the
//! offline-equivalent destination validation shared by operator and
//! scheduled starts. No config, no engine calls: pure path validation plus
//! the private-dir check.

use std::path::Path;

use crate::sync::durability::backup::online::progress::{BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{OnlineBackupError, OnlineBackupFailure};

/// Revalidate the boot-canonical root without creating or repairing it.
/// Check cancellation before blocking preflight and again before handing
/// off to the protocol (which mkdirs the timestamp child before its first
/// cancellation check). Once I/O starts, cancellation is cooperative.
/// Like retention, this assumes trusted operators do not replace the root
/// or its parents concurrently with the run.
pub(in crate::boot::config::diagnostics::backup_online) fn scheduled_preflight(
    to: &Path,
    data_dir: &Path,
    cancel: &CancelFlag,
) -> Result<(), OnlineBackupError> {
    let check_cancel = || {
        if cancel.is_cancelled() {
            Err(OnlineBackupError::Cancelled {
                stage: BackupStage::Preparing,
            })
        } else {
            Ok(())
        }
    };
    let invalid = |error| OnlineBackupError::Failed {
        stage: BackupStage::Preparing,
        source: OnlineBackupFailure::Io(error),
    };
    check_cancel()?;
    let canonical = to.canonicalize().map_err(invalid)?;
    if canonical != to {
        return Err(invalid(std::io::Error::other(
            "scheduled backup destination no longer matches its boot-canonical path",
        )));
    }
    check_cancel()?;
    validate_canonical_destination(to, &canonical, data_dir, true)
        .map_err(|error| invalid(std::io::Error::other(error)))?;
    check_cancel()?;
    shamir_private_dir::verify_private_dir(to).map_err(invalid)?;
    check_cancel()
}

/// Offline-backup-equivalent destination validation: absolute, and its
/// nearest existing canonical ancestor must not be inside (or equal to) the
/// source data dir.
pub(in crate::boot::config::diagnostics::backup_online) fn validate_destination(
    to: &Path,
    data_dir: &Path,
) -> Result<(), super::super::StartBackupError> {
    if !to.is_absolute() {
        return Err(super::super::StartBackupError::InvalidDestination(format!(
            "destination must be absolute: {}",
            to.display()
        )));
    }
    let to_canonical = canonicalize_nearest_existing_ancestor(to)?;
    validate_canonical_destination(to, &to_canonical, data_dir, false)
}

fn validate_canonical_destination(
    to: &Path,
    to_canonical: &Path,
    data_dir: &Path,
    reject_ancestor: bool,
) -> Result<(), super::super::StartBackupError> {
    let source_canonical = data_dir.canonicalize().map_err(|error| {
        super::super::StartBackupError::InvalidDestination(format!(
            "canonicalize source {}: {error}",
            data_dir.display()
        ))
    })?;
    if to_canonical.starts_with(&source_canonical) {
        return Err(super::super::StartBackupError::InvalidDestination(format!(
            "destination is inside (or equal to) the source data dir: {}",
            to.display()
        )));
    }
    if reject_ancestor && source_canonical.starts_with(to_canonical) {
        return Err(super::super::StartBackupError::InvalidDestination(format!(
            "scheduled destination is an ancestor of the source data dir: {}",
            to.display()
        )));
    }
    Ok(())
}

/// Canonicalize `path` if it exists, else walk up to the nearest existing
/// ancestor and append the non-canonical suffix (offline precedent).
fn canonicalize_nearest_existing_ancestor(
    path: &Path,
) -> Result<std::path::PathBuf, super::super::StartBackupError> {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            let mut canonical = current.canonicalize().map_err(|error| {
                super::super::StartBackupError::InvalidDestination(format!(
                    "canonicalize {}: {error}",
                    current.display()
                ))
            })?;
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return Ok(canonical);
        }
        let Some(parent) = current.parent() else {
            return Err(super::super::StartBackupError::InvalidDestination(format!(
                "no existing ancestor for {}",
                path.display()
            )));
        };
        suffix.push(current.file_name().unwrap_or_default().to_os_string());
        current = parent;
    }
}
