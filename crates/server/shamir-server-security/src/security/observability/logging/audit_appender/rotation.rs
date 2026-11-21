//! Rotation, retention pruning and parent-directory durability for the
//! durable audit appender.
//!
//! Free helpers operating on [`FjallAuditAppender`] internals: the appender
//! keeps lock and gate ownership; this module owns the filesystem sequence
//! of an actual rotation — fsync close → preflighted rename → reopen →
//! flush the new active handle → parent-directory sync → anchor persist →
//! server-meta hook → retention prune — and the prune itself.

use super::{AppenderError, FjallAuditAppender};
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::atomic::Ordering;

const NANOS_PER_SEC: u64 = 1_000_000_000;
const SECS_PER_DAY: u64 = 86400;

/// Make the rename of a rotation (or an unlink) durable in the containing
/// directory.
///
/// Unix: the parent directory entry is fsync'd strictly and the error is
/// propagated. The file data itself is already covered by the caller's
/// `sync_all` on the writable handle. Windows has no equivalent directory
/// flush: this is a no-op (`Ok(())`) — directory-entry durability is NOT
/// equivalently guaranteed there.
#[cfg(unix)]
pub(super) fn sync_parent(log_path: &Path) -> io::Result<()> {
    let dir = log_path
        .parent()
        .ok_or_else(|| io::Error::other("audit log path has no parent directory"))?;
    File::open(dir)?.sync_all()
}

/// Windows variant of [`sync_parent`] — see there. Windows has no
/// supported directory-handle fsync, so this is always `Ok(())`:
/// directory-entry durability is NOT guaranteed there (the caller's
/// `sync_all` on the writable handle already covers the file data).
#[cfg(not(unix))]
pub(super) fn sync_parent(_log_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Current UNIX time in nanoseconds, `None` before the epoch or on u64
/// overflow — never a lossy `u128 as u64` truncation.
fn now_unix_nanos() -> Option<u64> {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    )
    .ok()
}

/// Newest archive suffix present next to `log_path`.
///
/// Directory read/enumeration errors propagate (no masked `?`/`flatten`);
/// an exact-shaped rotation name whose 20 digits overflow `u64` is refused
/// via [`AppenderError::Io`] wrapping the fully-qualified
/// `AuditRecoveryError::InvalidRotationName`. Only exact archive names
/// count.
fn max_archive_suffix(log_path: &Path) -> Result<Option<u64>, AppenderError> {
    let Some(dir) = log_path.parent() else {
        return Ok(None);
    };
    let mut max: Option<u64> = None;
    let entries = fs::read_dir(dir).map_err(AppenderError::Io)?;
    for entry in entries {
        let path = entry.map_err(AppenderError::Io)?.path();
        match super::recovery::archive_name(&path) {
            Ok(Some(suffix)) => max = Some(max.map_or(suffix, |m: u64| m.max(suffix))),
            Ok(None) => {}
            Err(error) => return Err(AppenderError::Io(io::Error::other(error))),
        }
    }
    Ok(max)
}

/// Rotation timestamp (finding 3): `max(now, newest existing archive + 1)`.
///
/// The newest-suffix floor is `checked_add(1)` — on overflow the rotation
/// is refused rather than saturating back onto an existing name.
///
/// Preflight + rename run under the exclusive maintenance (data-directory)
/// lock, so the refusal also guarantees `fs::rename` can never overwrite
/// an existing archive (Unix rename would silently clobber; Windows
/// errors anyway).
fn next_archive_suffix(log_path: &Path) -> Result<u64, AppenderError> {
    let floor = match max_archive_suffix(log_path)? {
        Some(max) => max
            .checked_add(1)
            .ok_or_else(|| AppenderError::Io(io::Error::other("audit archive suffix overflow")))?,
        None => 1,
    };
    Ok(match now_unix_nanos() {
        Some(now_ns) => now_ns.max(floor),
        None => floor,
    })
}

/// Rotate the current log file: fsync close, preflight an unused target
/// name, rename to `audit.log.<unix_nanos>`, immediately reopen a fresh
/// ACTIVE file into the caller's handle (no write can land in the archived
/// file), flush the new handle, sync the parent directory, persist the
/// appender anchor, run the optional server-meta hook, and only then prune
/// retention. Caller MUST hold the `log_file` mutex (the `&mut File`
/// parameter is the lock guard's deref) and the persistence gate, and must
/// latch the returned error: a failure at or after the rename leaves a
/// rotated file behind that no anchor covers yet, so the appender must go
/// fail-closed. Prune failures alone stay best-effort (housekeeping).
pub(super) fn rotate_locked(
    appender: &FjallAuditAppender,
    current: &mut File,
) -> Result<(), AppenderError> {
    current.flush()?;
    current.sync_all()?;
    let ts_ns = next_archive_suffix(&appender.log_path)?;
    let mut rotated = appender.log_path.clone();
    let stem = rotated
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audit_log.log".to_string());
    rotated.set_file_name(format!("{stem}.{ts_ns:020}"));
    // Preflight (under the maintenance lock): never overwrite an archive.
    if rotated.try_exists().map_err(AppenderError::Io)? {
        return Err(AppenderError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("rotation target already exists: {}", rotated.display()),
        )));
    }
    fs::rename(&appender.log_path, &rotated)?;
    // Reopen the ACTIVE file into the caller's handle before anything else
    // can write, so no byte can ever land in the archived file.
    *current = FjallAuditAppender::open_log(&appender.log_path)?;
    if let Some(rot) = &appender.rotation {
        rot.current_size.store(0, Ordering::Relaxed);
    }
    // (7) Flush the newly reopened writable ACTIVE handle (once, inside
    // `dir_sync` — meaningful on Windows, where a read-only handle sync
    // proves nothing) and make the rename's directory entry durable BEFORE
    // any anchor (fjall or server meta) is persisted over the new layout.
    appender.dir_sync(&appender.log_path, current)?;
    // Anchor, then optional server-meta hook, then prune: pruning may
    // delete the oldest archives, so both trusted anchors must already
    // cover the retained history.
    appender.persist_anchor_and_meta()?;
    tracing::info!(rotated = %rotated.display(), "audit log rotated");

    if let Some(rot) = &appender.rotation {
        if rot.retention_days != 0 {
            sweep_retention(&appender.log_path, rot.retention_days);
        }
    }
    Ok(())
}

/// Delete the oldest contiguous prefix of exact rotation archives whose
/// embedded nanos timestamp is at least `retention_days` old, always
/// retaining the newest archive, stopping at the first young archive or the
/// first failed unlink, and syncing the parent directory after each
/// successful unlink (warn + stop on sync error).
///
/// Runs only inside [`rotate_locked`] — never at appender close; retention
/// is policy, not shutdown work. Ordering comes from
/// `recovery::ordered_archives` (first-record sequence), so the age walk is
/// safe after wall-clock rollback: a future-dated or young archive ends the
/// prefix. **Never fails**: directory reads, classification problems and
/// unlink/sync failures log at `warn` and stop the sweep — retention is
/// housekeeping, not a correctness requirement.
pub(super) fn sweep_retention(log_path: &Path, retention_days: u32) {
    let max_age_ns = (retention_days as u64)
        .saturating_mul(SECS_PER_DAY)
        .saturating_mul(NANOS_PER_SEC);
    let Some(now_ns) = now_unix_nanos() else {
        return; // clock before epoch — give up silently
    };
    let Some(dir) = log_path.parent() else {
        return;
    };
    let archives = match super::recovery::ordered_archives(dir) {
        Ok(archives) => archives,
        Err(error) => {
            tracing::warn!(%error, "audit retention sweep: failed to order archives");
            return;
        }
    };
    // Retain the newest archive; only an oldest contiguous prefix is
    // eligible for deletion.
    let mut oldest = archives;
    let Some(_newest) = oldest.pop() else {
        return;
    };
    for path in oldest {
        let file_ns = match super::recovery::archive_name(&path) {
            Ok(Some(file_ns)) => file_ns,
            Ok(None) => continue, // not an exact archive name — never touched
            Err(error) => {
                tracing::warn!(%error, "audit retention sweep: unclassifiable archive");
                break;
            }
        };
        // Future-dated archive (clock skew) — never delete, stop the prefix.
        let Some(age_ns) = now_ns.checked_sub(file_ns) else {
            break;
        };
        if age_ns < max_age_ns {
            break;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(
                    file = %path.display(),
                    age_days = age_ns / (SECS_PER_DAY * NANOS_PER_SEC),
                    "audit retention sweep: deleted old rotated file"
                );
                // Durability of each unlink before touching the next
                // archive; a sync error stops the sweep at `warn`.
                if let Err(error) = sync_parent(&path) {
                    tracing::warn!(
                        %error,
                        "audit retention sweep: directory sync after unlink failed"
                    );
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    file = %path.display(),
                    "audit retention sweep: failed to delete old rotated file"
                );
                break;
            }
        }
    }
}
