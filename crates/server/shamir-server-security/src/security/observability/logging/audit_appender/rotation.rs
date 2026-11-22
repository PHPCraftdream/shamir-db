//! Rotation, retention pruning and parent-directory durability for the
//! durable audit appender.
//!
//! Free helpers operating on [`FjallAuditAppender`] internals: the appender
//! keeps lock and gate ownership; this module owns the filesystem sequence
//! of an actual rotation — fsync close → preflighted rename → reopen →
//! flush the new active handle → parent-directory sync → anchor persist →
//! server-meta hook → retention prune — and the prune itself.
//!
//! The prune is metadata-only: retention candidates come from one
//! directory listing (reused for the newest-name floor), are ordered by
//! the strictly increasing name suffix with the boot-authoritative
//! `legacy_archive_order` prefix re-sequenced in, and the age of EVERY
//! candidate — without exception — is taken from the file's modification
//! time, a source that does not depend on the name. No archive body is
//! ever read here: a rotation with nothing to prune costs one directory
//! listing plus one `symlink_metadata` per visited candidate.

use super::{AppenderError, FjallAuditAppender};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

const SECS_PER_DAY: u64 = 86400;

/// Rotation policy plus the boot-derived archive suffix order used by the
/// retention sweep. The public [`super::RotationPolicy`] is unchanged; this
/// private wrapper only carries the extra precomputed order alongside it.
pub(super) struct RotationState {
    pub(super) policy: super::RotationPolicy,
    /// Archive SUFFIX numbers in verified first-record sequence order,
    /// present only when that order is inverted relative to the suffix
    /// names (legacy wall-clock-rolled-back archives) and retention is
    /// enabled; `None` keeps the sweep's ascending-suffix order.
    pub(super) legacy_archive_order: Option<Vec<u64>>,
}

impl RotationState {
    /// Raw-opener rotation policy (`open_{strict,batched}_with_rotation`):
    /// when a cap AND a nonzero retention are both configured, the shared
    /// archive-discovery pass runs exactly ONCE here — at open, outside any
    /// gate — so a later rotation never reads archives. Because a raw
    /// opener has no audit key to authenticate anything, the classification
    /// uses the full bounded first-row read (`raw_ordered_archives_with_suffix`)
    /// instead of the cheap probe, preserving the strict refusals against a
    /// malformed first row with a parseable seq prefix. An invalid,
    /// unreadable or non-regular archive keeps the raw open and all writes
    /// succeeding but suppresses pruning safely (retention forced to 0 with
    /// a warning); the overflow naming policy is unchanged.
    pub(super) fn raw_policy(
        dir: &Path,
        max_size_bytes: Option<u64>,
        initial_size: u64,
        retention_days: u32,
    ) -> Option<Self> {
        let max_size_bytes = max_size_bytes?;
        let order = if retention_days == 0 {
            None
        } else {
            match super::recovery::raw_ordered_archives_with_suffix(dir) {
                Ok(ordered) => ordered
                    .windows(2)
                    .any(|window| window[0].1 > window[1].1)
                    .then(|| ordered.iter().map(|(_, suffix, _)| *suffix).collect()),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "audit archive discovery failed; retention pruning disabled"
                    );
                    return Some(Self {
                        policy: super::RotationPolicy {
                            max_size_bytes,
                            current_size: std::sync::atomic::AtomicU64::new(initial_size),
                            retention_days: 0,
                        },
                        legacy_archive_order: None,
                    });
                }
            }
        };
        Some(Self {
            policy: super::RotationPolicy {
                max_size_bytes,
                current_size: std::sync::atomic::AtomicU64::new(initial_size),
                retention_days,
            },
            legacy_archive_order: order,
        })
    }
}

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

/// One directory listing of the exact rotation archives next to `log_path`
/// as a suffix-keyed map (ascending suffix order by construction),
/// reused for both the newest-name floor and the retention pruning order —
/// no rotation enumerates the directory twice.
///
/// Directory read/enumeration errors propagate (no masked `?`/`flatten`);
/// an exact-shaped rotation name whose 20 digits overflow `u64` is refused
/// via [`AppenderError::Io`] wrapping the fully-qualified
/// `AuditRecoveryError::InvalidRotationName` — the same fatal
/// classification boot recovery applies through
/// `recovery::archive_name`. Only exact archive names count; metadata is
/// never consulted here (classification is by name only).
fn list_archives(log_path: &Path) -> Result<BTreeMap<u64, PathBuf>, AppenderError> {
    let Some(dir) = log_path.parent() else {
        return Ok(BTreeMap::new());
    };
    let mut out = BTreeMap::new();
    let entries = fs::read_dir(dir).map_err(AppenderError::Io)?;
    for entry in entries {
        let path = entry.map_err(AppenderError::Io)?.path();
        match super::recovery::archive_name(&path) {
            Ok(Some(suffix)) => {
                out.insert(suffix, path);
            }
            Ok(None) => {}
            Err(error) => return Err(AppenderError::Io(io::Error::other(error))),
        }
    }
    Ok(out)
}

/// Rotation timestamp: `max(now, newest existing archive + 1)`, or `None`
/// when no healthy name can be minted (the floor or the wall clock lands
/// on `u64::MAX`, which boot recovery refuses). The caller skips the
/// rotation with a warning in that case: the active file stays fsynced,
/// nothing is renamed, no anchor changes.
///
/// Preflight + rename run under the exclusive maintenance (data-directory)
/// lock, so a minted suffix also guarantees `fs::rename` can never
/// overwrite an existing archive (Unix rename would silently clobber;
/// Windows errors anyway).
fn next_archive_suffix(archives: &BTreeMap<u64, PathBuf>) -> Option<u64> {
    let floor = archives
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0)
        .checked_add(1)?;
    let now_unix_nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok())
        .filter(|&now_ns| now_ns != u64::MAX);
    let suffix = match now_unix_nanos {
        Some(now_ns) => now_ns.max(floor),
        None => floor,
    };
    (suffix < u64::MAX).then_some(suffix)
}

/// Rotate the current log file: fsync close, preflight an unused target
/// name (one shared directory listing), rename to
/// `audit.log.<unix_nanos>`, immediately reopen a fresh ACTIVE file into
/// the caller's handle (no write can land in the archived file), flush the
/// new handle, sync the parent directory, persist the appender anchor, run
/// the optional server-meta hook, and only then prune retention. Caller
/// MUST hold the `log_file` mutex (the `&mut File` parameter is the lock
/// guard's deref) and the persistence gate, and must latch the returned
/// error: a failure at or after the rename leaves a rotated file behind
/// that no anchor covers yet, so the appender must go fail-closed. Prune
/// failures alone stay best-effort (housekeeping).
///
/// `legacy_archive_order` — the exact archive SUFFIX numbers of the
/// boot-discovered archives, already sorted by the authoritative
/// first-record sequence (boot recovery is the ordering authority and
/// precomputes this; the slice carries no sequence values). The sweep
/// emits these archives first, in that authoritative order, followed by
/// everything created after boot in ascending suffix order (a legitimate
/// new suffix is always `max(now, newest + 1)`, so post-boot archives
/// strictly follow the boot set in both name and chain position). The
/// just-rotated archive is not yet in the listing the order is built
/// from, so it is never a prune candidate — the newest archive is
/// unconditionally retained with no special case. A pruned suffix is
/// simply absent from future boot lists; no list mutation is needed.
/// Suffixes named here but no longer on disk are skipped; on-disk
/// suffixes not named here follow in suffix order.
pub(super) fn rotate_locked(
    appender: &FjallAuditAppender,
    current: &mut File,
    legacy_archive_order: Option<&[u64]>,
) -> Result<(), AppenderError> {
    current.flush()?;
    current.sync_all()?;
    // Single metadata-only listing: newest-name floor AND prune order, so
    // no plan is computed before the gate/lock is held and no rotation
    // lists the directory twice.
    let archives = list_archives(&appender.log_path)?;
    let Some(ts_ns) = next_archive_suffix(&archives) else {
        tracing::warn!(
            "audit rotation skipped: archive suffix space exhausted at \
             u64::MAX — the active file stays fsynced and open"
        );
        return Ok(());
    };
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
        rot.policy.current_size.store(0, Ordering::Relaxed);
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
        if rot.policy.retention_days != 0 {
            sweep_retention(archives, rot.policy.retention_days, legacy_archive_order);
        }
    }
    Ok(())
}

/// Delete the oldest contiguous prefix of exact rotation archives whose
/// modification time is at least `retention_days` old, always retaining
/// the newest archive (the rotation boundary, absent from the snapshot),
/// stopping at the first not-eligible archive or the first failed unlink,
/// and syncing the parent directory after each successful unlink (warn +
/// stop on sync error). The listing is the one taken under the rotation
/// lock before the rename, passed by value — no second enumeration.
///
/// Runs only inside [`rotate_locked`] — never at appender close; retention
/// is policy, not shutdown work. Ordering and age source:
///
/// * Order: the suffix numbers of `legacy_archive_order` (boot-verified,
///   authoritative first-record sequence order) are pulled out of the
///   suffix-keyed listing first; the remaining entries follow in ascending
///   suffix order.
/// * Age: the age of EVERY candidate, without exception, is the file's
///   modification time (`symlink_metadata` on the candidate only, as it is
///   visited — no bulk stat, no archive body read). `SystemTime::now()` is
///   sampled once per sweep and eligibility is a plain `Duration`
///   comparison against `Duration::from_secs(retention_days * 86400)` —
///   no nanosecond arithmetic, no saturation.
///
/// Known limitation, accepted by the contiguous-prefix rule: an archive
/// created under a future (skewed) clock carries a future modification
/// time permanently, so it — and every strictly later archive — stays
/// pinned until real time passes that timestamp; the configured age bound
/// resumes from there rather than being enforced independently per
/// archive. This preserves contiguity (nothing younger than a retained
/// archive is removed) and always removes the eligible prefix in front of
/// the blocker, but is not a per-archive guarantee.
///
/// Directory reads, classification problems, unreadable candidates and
/// unlink/sync failures log at `warn` and stop the sweep — retention is
/// housekeeping, never a correctness requirement, and never fails a
/// rotation that already anchored. Unreadable, non-regular or
/// unknown-mtime candidates are never removed; the sweep only ever stops,
/// it never upgrades a suspect entry to a deletion.
fn sweep_retention(
    mut archives: BTreeMap<u64, PathBuf>,
    retention_days: u32,
    legacy: Option<&[u64]>,
) {
    let max_age = Duration::from_secs(u64::from(retention_days) * SECS_PER_DAY);
    let now = SystemTime::now();
    // Chain order: boot-verified suffixes by authoritative sequence first,
    // then the post-boot remainder by ascending suffix. A suffix named in
    // `legacy` that is no longer on disk (pruned earlier) is simply absent.
    // (Materialized eagerly: the remove pass borrows the listing mutably
    // before it is consumed into the suffix-ordered remainder.)
    let mut order: Vec<PathBuf> = legacy
        .unwrap_or(&[])
        .iter()
        .filter_map(|suffix| archives.remove(suffix))
        .collect();
    order.extend(archives.into_values());
    for path in order {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            tracing::warn!(
                file = %path.display(),
                "audit retention sweep: candidate metadata unavailable"
            );
            break;
        };
        if !metadata.is_file() {
            tracing::warn!(
                file = %path.display(),
                "audit retention sweep: skipping a non-regular archive entry"
            );
            break;
        }
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(error) => {
                tracing::warn!(
                    %error,
                    file = %path.display(),
                    "audit retention sweep: candidate mtime unknown"
                );
                break;
            }
        };
        // A future mtime (archive created under a skewed clock) is NOT
        // eligible and, for contiguity, blocks everything after it.
        let Ok(age) = now.duration_since(modified) else {
            tracing::warn!(
                file = %path.display(),
                "audit retention sweep: candidate has a future modification time"
            );
            break;
        };
        if age < max_age {
            break;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(
                    file = %path.display(),
                    age_days = age.as_secs() / SECS_PER_DAY,
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
