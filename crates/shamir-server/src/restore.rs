//! Offline restore of a `backup::backup` snapshot back into a server's
//! `data_dir` (RI-11).
//!
//! Strictly offline (server-stopped) restore, mirroring the existing
//! stop-and-copy backup model — PITR / WAL-archive / logical dump are out
//! of scope (beta roadmap).
//!
//! Procedure:
//! 1. **Liveness probe** (unless `force`): if `data_dir/server_meta`
//!    exists, attempt to open it via [`ServerMetaStore::open_or_init`] —
//!    the same store the real server opens at boot
//!    (`server_launcher.rs:133`). fjall already takes an exclusive OS-level
//!    advisory file lock per keyspace directory on open, so a lock-
//!    contention error (`MetaError::Fjall(fjall::Error::Locked)`) means a
//!    real server process is running against `data_dir` — refuse. ANY
//!    other open error (corruption, etc.) also refuses by default
//!    (fail-closed: a probe failure could equally mean genuine corruption,
//!    not liveness). `--force` skips this probe entirely.
//! 2. **Verify the SOURCE snapshot's manifest** (`backup::verify_manifest`)
//!    BEFORE touching `data_dir` at all. A checksum mismatch or missing
//!    manifest aborts the whole restore with no side effects.
//! 3. **Copy to a TEMPORARY sibling directory** (same filesystem as
//!    `data_dir`'s parent — required for the atomic rename in step 4).
//! 4. **Atomic swap**: rename the CURRENT `data_dir` (if it exists) to a
//!    `.pre_restore_backup_<timestamp>` sibling (preserved, NOT deleted —
//!    the explicit rollback path), then rename the temp dir into place as
//!    the new `data_dir`. A failure in the second rename triggers a
//!    best-effort rollback of the first.
//! 5. **Invalidate sessions**: open the restored `users` store and call
//!    `invalidate_all_tickets(now_ns)` so no resumption ticket issued
//!    before the restore point can still resume against the restored
//!    state, then release it.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::backup::{self, copy_dir_recursive, BackupError};
use crate::server_meta::{MetaError, ServerMetaStore};
use crate::user_directory::FjallUserDirectory;

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("restore io: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "server appears to be running against this data_dir ({0}) — stop it first, \
         or pass --force if you are certain it is not"
    )]
    ServerRunning(PathBuf),
    #[error(
        "liveness probe of {0} failed (not a lock-contention error — could indicate \
         corruption): {1}; pass --force to bypass this check"
    )]
    ProbeFailed(PathBuf, MetaError),
    #[error("snapshot manifest verification failed: {0}")]
    ManifestVerification(#[from] BackupError),
    #[error(
        "atomic swap partially failed: pre-restore backup at {pre_restore_backup} \
         and NEW data at {temp_dir} both exist, but {data_dir} could not be \
         reconstructed automatically ({source}) — operator must manually rename \
         one of these two directories to {data_dir}"
    )]
    SwapPartialFailure {
        data_dir: PathBuf,
        pre_restore_backup: PathBuf,
        temp_dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("opening restored user directory failed: {0}")]
    UserDirectory(#[from] fjall::Error),
    #[error("invalidating tickets in restored user directory failed: {0}")]
    Invalidate(#[from] shamir_connect::common::error::Error),
}

/// Outcome of a successful restore.
#[derive(Debug)]
pub struct RestoreReport {
    /// Number of files copied from the snapshot into the new `data_dir`.
    pub files_restored: u64,
    /// Total bytes copied.
    pub bytes_restored: u64,
    /// Number of user accounts whose `tickets_invalid_before_ns` was bumped.
    pub users_invalidated: usize,
    /// The `.pre_restore_backup_<timestamp>` sibling directory holding the
    /// PRE-restore `data_dir` contents (`None` if there was no pre-existing
    /// `data_dir` to preserve — e.g. restoring into a fresh target).
    pub pre_restore_backup: Option<PathBuf>,
}

/// Restore `from` (a `backup::backup` snapshot directory) into `data_dir`.
///
/// See the module doc for the full step-by-step procedure. `force` skips
/// the liveness probe in step 1 — use only when certain no server process
/// holds `data_dir`.
///
/// `from` and `data_dir` MUST be on the same filesystem as `data_dir`'s
/// parent directory, since step 3/4 relies on `fs::rename` being atomic
/// (a cross-filesystem rename would silently fall back to copy+delete on
/// some platforms, or fail outright — this function does not attempt to
/// paper over that; `fs::rename`'s own error surfaces as-is).
pub fn restore(from: &Path, data_dir: &Path, force: bool) -> Result<RestoreReport, RestoreError> {
    // ---- Step 1: liveness probe ----
    if !force {
        probe_not_running(data_dir)?;
    }

    // ---- Step 2: verify the SOURCE snapshot's manifest, before touching
    // data_dir at all. ----
    backup::verify_manifest(from)?;

    // ---- Step 3: copy to a temporary sibling directory ----
    let stamp = restore_timestamp();
    let parent = data_dir.parent().unwrap_or(data_dir);
    let temp_dir = parent.join(format!(
        "{}.restore_tmp_{stamp}",
        dir_name(data_dir, "data_dir")
    ));
    if temp_dir.exists() {
        return Err(RestoreError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "temporary restore dir already exists: {}",
                temp_dir.display()
            ),
        )));
    }
    fs::create_dir_all(&temp_dir)?;
    let mut bytes = 0u64;
    let mut files = 0u64;
    copy_dir_recursive(from, &temp_dir, &mut bytes, &mut files)?;

    // ---- Step 4: atomic swap ----
    let pre_restore_backup = if data_dir.exists() {
        let backup_sibling = parent.join(format!(
            "{}.pre_restore_backup_{stamp}",
            dir_name(data_dir, "data_dir")
        ));
        fs::rename(data_dir, &backup_sibling)?;
        match fs::rename(&temp_dir, data_dir) {
            Ok(()) => Some(backup_sibling),
            Err(e) => {
                // Best-effort rollback: put the pre-restore backup back so
                // data_dir is not left missing.
                if fs::rename(&backup_sibling, data_dir).is_err() {
                    return Err(RestoreError::SwapPartialFailure {
                        data_dir: data_dir.to_path_buf(),
                        pre_restore_backup: backup_sibling,
                        temp_dir,
                        source: e,
                    });
                }
                return Err(RestoreError::SwapPartialFailure {
                    data_dir: data_dir.to_path_buf(),
                    pre_restore_backup: backup_sibling,
                    temp_dir,
                    source: e,
                });
            }
        }
    } else {
        fs::rename(&temp_dir, data_dir)?;
        None
    };

    // ---- Step 5: invalidate sessions in the RESTORED data ----
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let users_invalidated = {
        let users = FjallUserDirectory::open(data_dir.join("users"))?;
        let n = users.invalidate_all_tickets(now_ns)?;
        drop(users); // release fjall's lock before returning
        n
    };

    Ok(RestoreReport {
        files_restored: files,
        bytes_restored: bytes,
        users_invalidated,
        pre_restore_backup,
    })
}

/// Attempt to open `data_dir/server_meta` (mirrors `server_launcher.rs`'s
/// own boot-time open) purely as a liveness probe. `Ok(())` on success
/// (store opened and was immediately dropped, releasing the lock) or when
/// `data_dir/server_meta` does not exist yet (nothing to restore over — a
/// fresh target). `Err` on lock contention (server running) or any other
/// open failure (fail-closed).
fn probe_not_running(data_dir: &Path) -> Result<(), RestoreError> {
    let meta_dir = data_dir.join("server_meta");
    if !meta_dir.exists() {
        return Ok(()); // fresh target — nothing to probe
    }
    match ServerMetaStore::open_or_init(&meta_dir) {
        Ok(store) => {
            drop(store); // release the lock immediately
            Ok(())
        }
        Err(MetaError::Fjall(fjall::Error::Locked)) => {
            Err(RestoreError::ServerRunning(data_dir.to_path_buf()))
        }
        Err(other) => Err(RestoreError::ProbeFailed(data_dir.to_path_buf(), other)),
    }
}

/// `YYYYMMDD_HHMMSS` UTC, matching `backup::utc_timestamp`'s format (kept
/// separate rather than reusing it directly since that helper is
/// `pub(crate)` in `backup.rs` and this needs the identical shape here).
fn restore_timestamp() -> String {
    backup::utc_timestamp()
}

/// Directory name (final path component) of `path`, falling back to
/// `default` if it cannot be determined (e.g. `path` is `/` or empty).
fn dir_name(path: &Path, default: &str) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| default.to_string())
}
