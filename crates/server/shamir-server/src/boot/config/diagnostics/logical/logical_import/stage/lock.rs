//! Exclusive ownership of the import: a create-new lock file BESIDE the
//! target, acquired BEFORE the target is inspected, held for the whole run
//! and removed only after the stage discard/publish outcome is resolved.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use super::super::error::LogicalImportError;

pub(crate) struct ImportLock {
    path: PathBuf,
    // Held open (and exclusively created) for the lifetime of the import.
    _file: std::fs::File,
}

impl ImportLock {
    /// Create `<target>.import.lock` with create_new semantics. Any existing
    /// lock (a concurrent or crashed attempt) is a typed refusal. Must be
    /// called before the target directory itself is inspected.
    pub(crate) fn acquire(target: &Path) -> Result<Self, LogicalImportError> {
        let path = lock_path(target);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                LogicalImportError::Lock(format!(
                    "cannot create exclusive lock {}: {e}",
                    path.display()
                ))
            })?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for ImportLock {
    fn drop(&mut self) {
        // The file handle is closed first by field drop order, then the
        // lock file is removed. Best-effort: a leaked lock file refuses the
        // next attempt with a clear message rather than corrupting a target.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_path(target: &Path) -> PathBuf {
    let name = target.file_name().map_or_else(
        || "target.import.lock".to_owned(),
        |n| format!("{}.import.lock", n.to_string_lossy()),
    );
    target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join(name)
}
