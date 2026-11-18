//! Offline backup source locking (brief 47-r2a-3b2 §5, Q5): refuse
//! `backup --from` on a live server before a single byte is copied.
//!
//! Probed lock artifacts under the source `data_dir`:
//! `<root>/.shamir.lock` (server maintenance lock, fs4 exclusive),
//! `<root>/server_meta/lock` and `<root>/shamir_db_meta.redb/lock` (Fjall
//! database locks, std `File`). All are whole-file advisory locks, so a
//! shared advisory lock taken here conflicts with any live holder:
//! [`File::try_lock_shared`] succeeds only where no writer holds it.
//!
//! Per artifact, `File::open` READ-ONLY (the source is never written):
//! NotFound ⇒ that writer never ran, skipped without an `exists()`
//! pre-check; `TryLockError::WouldBlock` ⇒ live holder ⇒ refusal; anything
//! else ⇒ [`BackupError::Io`]. Shared locks still copy as plain bytes and
//! keep a starting server blocked for as long as the returned handles are
//! retained.

use std::fs::{File, TryLockError};
use std::path::Path;

use super::BackupError;

/// Lock artifacts probed for a live server, relative to the source
/// `data_dir` root. Order is irrelevant; all are probed.
const LOCK_ARTIFACTS: [&str; 3] = [
    ".shamir.lock",
    "server_meta/lock",
    "shamir_db_meta.redb/lock",
];

/// Probe the source `data_dir` for a live server and, if it is stopped,
/// return retained shared locks over its lock artifacts (keep the handles
/// alive for the whole copy). A missing source root itself is NOT checked
/// here; `backup()` already classified the source before reaching this
/// helper.
pub(super) fn lock_backup_source(root: &Path) -> Result<Vec<File>, BackupError> {
    let mut held = Vec::new();
    for rel in LOCK_ARTIFACTS {
        let path = root.join(rel);
        let file = match File::open(&path) {
            Ok(file) => file,
            // Absent artifact: that writer never ran; nothing to probe.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(BackupError::Io(context_io(e, &path, "open (read-only)"))),
        };
        match file.try_lock_shared() {
            Ok(()) => held.push(file),
            Err(TryLockError::WouldBlock) => {
                return Err(BackupError::SourceRunning(root.to_path_buf()))
            }
            Err(TryLockError::Error(e)) => {
                return Err(BackupError::Io(context_io(e, &path, "acquire shared lock")))
            }
        }
    }
    Ok(held)
}

/// Rebuild an I/O error with the artifact path and the failing operation
/// named, so [`BackupError::Io`] is actionable without a second diagnostic
/// pass over the source.
fn context_io(e: std::io::Error, path: &Path, operation: &str) -> std::io::Error {
    std::io::Error::new(
        e.kind(),
        format!(
            "backup source lock probe: {operation} {}: {e}",
            path.display()
        ),
    )
}
