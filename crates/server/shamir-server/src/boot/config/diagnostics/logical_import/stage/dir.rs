//! Attempt-owned sibling stage on the same volume as the target, mirroring
//! the `logical_dump` staging conventions with a distinct prefix. A stale
//! stage from a crashed attempt is removed only while the import lock is
//! held. Publication is ONE same-volume rename of the stage onto the absent
//! (or just-removed empty) target; a platform or volume that cannot do it
//! fails closed with a typed error.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::Builder;

use super::super::error::LogicalImportError;

const STAGE_PREFIX: &str = ".logical-import-";

pub(crate) struct ImportStage {
    path: PathBuf,
    published: bool,
}

impl ImportStage {
    /// Create a uniquely named stage directory beside the target. Requires
    /// the import lock to be held (so stale-stage cleanup is exclusive).
    pub(crate) fn prepare(target: &Path) -> Result<Self, LogicalImportError> {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        remove_stale(parent)?;
        let directory = Builder::new()
            .prefix(STAGE_PREFIX)
            .tempdir_in(parent)
            .map_err(|e| stage_err(format!("cannot create stage in {}: {e}", parent.display())))?;
        // Detach: the stage must survive until publish/discard; TempDir's
        // automatic cleanup would race the rename commit point.
        let path = directory.keep();
        Ok(Self {
            path,
            published: false,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The rename is the publication commit point. On Windows the target
    /// must not exist (an existing empty target is removed first by the
    /// caller through `prepare_target_name`); `fs::rename` never replaces.
    pub(crate) fn publish(mut self, target: &Path) -> Result<(), LogicalImportError> {
        match fs::rename(&self.path, target) {
            Ok(()) => {
                self.published = true;
                Ok(())
            }
            Err(e) => Err(LogicalImportError::Publish(format!(
                "same-volume rename of {} onto {} failed: {e}",
                self.path.display(),
                target.display()
            ))),
        }
    }

    /// Remove the stage after a failed or cancelled attempt. Nothing of the
    /// attempt is left behind (the lock file is removed by `ImportLock`).
    pub(crate) fn discard(self) -> Result<(), LogicalImportError> {
        if self.published {
            return Ok(());
        }
        fs::remove_dir_all(&self.path)
            .map_err(|e| stage_err(format!("cannot discard stage {}: {e}", self.path.display())))
    }
}

/// Refuse an existing non-empty target (callers inspect the target only
/// after acquiring the lock). An existing empty directory is removed so the
/// publish rename has an absent name.
pub(crate) fn prepare_target_name(target: &Path) -> Result<(), LogicalImportError> {
    match fs::symlink_metadata(target) {
        Err(_) => Ok(()),
        Ok(meta) if meta.is_dir() => {
            let mut entries = fs::read_dir(target).map_err(|e| {
                LogicalImportError::Target(format!(
                    "cannot inspect existing target {}: {e}",
                    target.display()
                ))
            })?;
            if entries.next().is_some() {
                return Err(LogicalImportError::Target(format!(
                    "target {} exists and is not empty",
                    target.display()
                )));
            }
            fs::remove_dir(target).map_err(|e| {
                LogicalImportError::Target(format!(
                    "cannot remove empty target {}: {e}",
                    target.display()
                ))
            })
        }
        Ok(_) => Err(LogicalImportError::Target(format!(
            "target {} exists and is not a directory",
            target.display()
        ))),
    }
}

fn remove_stale(parent: &Path) -> Result<(), LogicalImportError> {
    let entries = fs::read_dir(parent).map_err(|e| {
        stage_err(format!(
            "cannot read stage parent {}: {e}",
            parent.display()
        ))
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(STAGE_PREFIX) {
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path).map_err(|e| {
                    stage_err(format!("cannot remove stale stage {}: {e}", path.display()))
                })?;
            }
        }
    }
    Ok(())
}

fn stage_err(message: String) -> LogicalImportError {
    LogicalImportError::Stage(message)
}
