//! Scheduled-run retention publication and adoption provenance (moved out
//! of `registry.rs` for layout): marker-failure accounting, the bounded
//! in-memory adoption provenance ([`PendingAdoption`] inside
//! [`PendingAdoptions`]), and capture of manifest bytes after protocol
//! success.
//!
//! Manifest capture (R2d-FIX-C round 3, reviewer P2): after the protocol
//! returns, one bounded read captures the manifest bytes and decoded creation
//! time; a replacement before capture is therefore what gets recorded. The
//! digest checks that the manifest still matches at the scan and late check;
//! tagging also requires an untagged copy, while a matching entry that already
//! bears this owner's tag is reported as adopted without a new tag. This does
//! NOT prove payload-tree
//! or directory identity: replacing either beneath unchanged manifest bytes
//! is not detected, so the root remains single-writer by contract.
//!
//! All derivation happens BEFORE any record lock is taken: no filesystem
//! I/O runs under the registry's `RunRecord` lock. The provenance set
//! snapshot and the record/retire bookkeeping run under that existing
//! lock in the caller; a read or decode failure records no provenance
//! (fail closed: the copy stays `untouched` by selection) and never
//! fails the run.

use std::collections::BTreeSet;
use std::io::Read;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::cell::RefCell;

use sha2::{Digest, Sha256};

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::online::protocol::{BackupReport, OnlineBackupError};
use crate::sync::durability::backup::MANIFEST_FILE_NAME;

use super::super::metrics as backup_metrics;
use super::retention::{prune, RetentionOutcome};

/// Upper bound for one raw manifest read, shared by the retention
/// preflight and the completion-time capture (single cap, module doc).
pub(in crate::boot::config::diagnostics::backup_online) const MAX_MANIFEST: u64 = 16 << 20;

// Test-only one-shot retention pause armed on the CURRENT thread (the
// runner thread). It fires at the START of [`scheduled_retention_pass`] —
// inside the retention step, AFTER the copy outcome was published — so a
// shutdown test can hold the real worker inside retention.
#[cfg(test)]
thread_local! {
    static RETENTION_PAUSE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Clears an unfired [`set_scheduled_retention_pause`] when dropped; a pause
/// that fires removes itself from the slot. Zero-sized.
#[cfg(test)]
pub(crate) struct ScheduledRetentionPauseGuard;

#[cfg(test)]
impl Drop for ScheduledRetentionPauseGuard {
    fn drop(&mut self) {
        RETENTION_PAUSE.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Arm ONE retention pause on the CURRENT thread. The runner thread arms it
/// from the per-run post-run callback, so the thread-local is visible to the
/// retention pass that runs next on the same thread. Never process-global.
#[cfg(test)]
pub(crate) fn set_scheduled_retention_pause(
    pause: impl FnOnce() + 'static,
) -> ScheduledRetentionPauseGuard {
    RETENTION_PAUSE.with(|slot| *slot.borrow_mut() = Some(Box::new(pause)));
    ScheduledRetentionPauseGuard
}

/// Fire and consume the current thread's one-shot retention pause, if any.
#[cfg(test)]
fn fire_scheduled_retention_pause() {
    let pause = RETENTION_PAUSE.with(|slot| slot.borrow_mut().take());
    if let Some(pause) = pause {
        pause();
    }
}

// Test-only one-shot retention END notification armed on the CURRENT thread
// (the runner thread): fires at the END of [`scheduled_retention_pass`],
// AFTER [`backup_metrics::note_retention`] published the pass's counters —
// the pause seam's counterpart, so a fixture can wait for the retention
// handshake instead of a sleep or a shutdown.
#[cfg(test)]
thread_local! {
    static RETENTION_END: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Clears an unfired [`set_scheduled_retention_end`] when dropped; a hook
/// that fires removes itself from the slot. Zero-sized.
#[cfg(test)]
pub(crate) struct ScheduledRetentionEndGuard;

#[cfg(test)]
impl Drop for ScheduledRetentionEndGuard {
    fn drop(&mut self) {
        RETENTION_END.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Arm ONE retention END notification on the CURRENT thread. The runner
/// thread arms it from the per-run post-run callback (where the pause seam is
/// armed too), so the thread-local is visible to the retention pass that runs
/// next on the same thread. Never process-global.
#[cfg(test)]
pub(crate) fn set_scheduled_retention_end(
    end: impl FnOnce() + 'static,
) -> ScheduledRetentionEndGuard {
    RETENTION_END.with(|slot| *slot.borrow_mut() = Some(Box::new(end)));
    ScheduledRetentionEndGuard
}

/// Fire and consume the current thread's one-shot retention end hook, if any.
#[cfg(test)]
fn fire_scheduled_retention_end() {
    let end = RETENTION_END.with(|slot| slot.borrow_mut().take());
    if let Some(end) = end {
        end();
    }
}

/// Upper bound of the in-memory adoption provenance list (module doc).
/// Dropping the OLDEST record on overflow keeps the bound without
/// unbounding the newest outputs; a dropped record simply means that copy
/// stays `untouched` by selection — never an adoption of something
/// unrecorded.
const MAX_PENDING_ADOPTIONS: usize = 64;

/// Manifest-byte provenance for one successful scheduled output published
/// while marker provisioning failed: root, owner, name, decoded manifest
/// creation time, and SHA-256 of the manifest bytes observed by the bounded
/// post-protocol capture. A replacement before capture is therefore the
/// bytes recorded. Adoption requires the same manifest bytes/time at scan
/// and late check; an untagged copy is tagged, and a copy already bearing
/// this owner's tag is reported without a rewrite. This does not bind payload
/// contents or directory identity, which remain protected by the
/// single-writer root contract.
struct PendingAdoption {
    root: PathBuf,
    owner: String,
    name: String,
    created_at_unix_ns: u128,
    manifest_digest: String,
}

/// A plain, non-reparse regular file (Windows reparse points excluded via
/// the raw attributes — `Metadata::is_file()` alone does not exclude every
/// reparse tag).
#[cfg(windows)]
fn is_plain_file(meta: &std::fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    meta.is_file() && meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(windows))]
fn is_plain_file(meta: &std::fs::Metadata) -> bool {
    meta.is_file()
}

/// Bounded adoption provenance carried across runs inside the registry's
/// `RunRecord` (module doc): one helper per bookkeeping move, so the
/// caller keeps the existing `RunRecord` lock and this type stays
/// lock-free.
#[derive(Default)]
pub(in crate::boot::config::diagnostics::backup_online) struct PendingAdoptions {
    items: Vec<PendingAdoption>,
}

impl PendingAdoptions {
    /// The exact (name, creation time, digest) triples THIS root + owner
    /// recorded — the adoptable set for one retention pass.
    pub(in crate::boot::config::diagnostics::backup_online) fn recorded_for(
        &self,
        root: &Path,
        owner: &str,
    ) -> BTreeSet<(String, u128, String)> {
        self.items
            .iter()
            .filter(|item| item.root == root && item.owner == owner)
            .map(|item| {
                (
                    item.name.clone(),
                    item.created_at_unix_ns,
                    item.manifest_digest.clone(),
                )
            })
            .collect()
    }

    /// Record one freshly captured manifest identity (name, creation time
    /// and digest), evicting the OLDEST record at [`MAX_PENDING_ADOPTIONS`]
    /// (module doc).
    pub(in crate::boot::config::diagnostics::backup_online) fn record(
        &mut self,
        root: &Path,
        owner: &str,
        name: &str,
        created_at_unix_ns: u128,
        manifest_digest: String,
    ) {
        if self.items.len() >= MAX_PENDING_ADOPTIONS {
            let dropped = self.items.remove(0);
            tracing::warn!(
                name = %dropped.name,
                "adoption provenance is full; the oldest recorded copy stays \
                 untouched by selection"
            );
        }
        self.items.push(PendingAdoption {
            root: root.to_path_buf(),
            owner: owner.to_owned(),
            name: name.to_owned(),
            created_at_unix_ns,
            manifest_digest,
        });
    }

    /// Drop exactly the reported (name, creation time, digest) TRIPLES a
    /// repaired pass tagged (the ownership tag is durable by then,
    /// whatever the rest of the pass did). Matching the triple, not the
    /// bare name, keeps a different output that later occupies a recorded
    /// name in the provenance list instead of silently retiring it.
    pub(in crate::boot::config::diagnostics::backup_online) fn retire_adopted(
        &mut self,
        root: &Path,
        owner: &str,
        adopted: &[(String, u128, String)],
    ) {
        if adopted.is_empty() {
            return;
        }
        self.items.retain(|item| {
            !(item.root == root
                && item.owner == owner
                && adopted.contains(&(
                    item.name.clone(),
                    item.created_at_unix_ns,
                    item.manifest_digest.clone(),
                )))
        });
    }
}

/// Outcome of the pre-publish marker provisioning.
pub(in crate::boot::config::diagnostics::backup_online) enum MarkerProvisioning {
    /// Not a scheduled run, or the preflight already failed: retention is
    /// not accounted at all for such a run (no metric increment).
    NotAttempted,
    Provisioned,
    /// The pass counts as a retention failure (same outcome shape and
    /// metric as a failed prune), independent of the copy outcome. The
    /// error itself is logged at the provisioning site.
    Failed,
}

/// One bounded read of the raw manifest bytes of a finished snapshot
/// (module doc). `None` on any metadata, open or short-read failure, on a
/// non-plain file (symlinks and Windows reparse points excluded via
/// `symlink_metadata` plus the raw reparse attribute), or when the
/// manifest exceeds [`MAX_MANIFEST`] — the caller records no
/// provenance for that run (fail closed).
pub(in crate::boot::config::diagnostics::backup_online) fn bounded_manifest_bytes(
    dest_dir: &Path,
) -> Option<Vec<u8>> {
    let path = dest_dir.join(MANIFEST_FILE_NAME);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !is_plain_file(&meta) || meta.len() > MAX_MANIFEST {
        return None;
    }
    // Take exactly the observed length (plus one sentinel byte) so a size
    // change between the metadata and the read cannot yield a torn digest
    // silently: the exact-length guard below fails closed instead.
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(meta.len() + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 == meta.len()).then_some(bytes)
}

/// Lowercase hex SHA-256 of the exact manifest bytes (module doc).
pub(in crate::boot::config::diagnostics::backup_online) fn manifest_digest_hex(
    raw: &[u8],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    hex::encode(hasher.finalize())
}

/// The adoption identity captured from ONE bounded read (module doc): the
/// manifest's own creation time, decoded from the same captured bytes the
/// digest was taken over; `None` when the bytes are not a decodable
/// manifest.
pub(in crate::boot::config::diagnostics::backup_online) fn manifest_capture(
    raw: &[u8],
) -> Option<(u128, String)> {
    let created_at_unix_ns = std::str::from_utf8(raw)
        .ok()
        .and_then(|raw| manifest::decode(raw).ok())
        .map(|decoded| decoded.created_at_unix_ns)?;
    Some((created_at_unix_ns, manifest_digest_hex(raw)))
}

/// One scheduled run's retention decision (module doc). Marker-error
/// accounting and adoption scanning for a finished run — fail-closed
/// under a broken marker, silent when the marker was never attempted.
/// Pure: the caller performs the completion capture, snapshots the
/// adoptable provenance set under the existing `RunRecord` lock and does
/// the record/retire bookkeeping under it afterwards; no filesystem I/O
/// or locking happens here beyond the scan itself.
pub(in crate::boot::config::diagnostics::backup_online) fn scheduled_retention_pass(
    root: &Path,
    owner: &str,
    policy: &BackupRetentionConfig,
    cancel: &CancelFlag,
    outcome: &Result<BackupReport, OnlineBackupError>,
    marker: &MarkerProvisioning,
    recorded: &BTreeSet<(String, u128, String)>,
) -> RetentionOutcome {
    // Test-only: a shutdown test parks the real worker HERE, after the copy
    // outcome was published and inside the retention step.
    #[cfg(test)]
    fire_scheduled_retention_pause();
    let result = match (outcome, marker) {
        (_, MarkerProvisioning::Failed) => RetentionOutcome {
            failed: 1,
            ..RetentionOutcome::default()
        },
        (Ok(report), MarkerProvisioning::Provisioned) => {
            // A cancel observed after successful protocol completion and
            // before this pass starts tagging, with a healthy marker, skips
            // the scan: the completed copy remains untagged and unrecorded.
            // (A cancel after tagging began leaves the copy tagged.) The
            // operator must verify and manually
            // move or retire it; no later pass can infer it. A marker
            // failure observed before cancellation is still accounted in the
            // branch above.
            if cancel.is_cancelled() {
                RetentionOutcome::default()
            } else {
                // Only THIS root + THIS owner's exact recorded (name,
                // creation time, manifest digest) triples are adoptable:
                // bounded in-memory provenance, never a discovery
                // heuristic.
                prune(root, &report.dest_dir, owner, policy, cancel, recorded)
            }
        }
        // Not attempted (preflight failure, non-scheduled run), or nothing
        // published: no retention accounting.
        (_, MarkerProvisioning::NotAttempted) | (Err(_), MarkerProvisioning::Provisioned) => {
            RetentionOutcome::default()
        }
    };
    backup_metrics::note_retention(result.deleted, result.failed, result.untouched);
    // Lock contention on this attempt, not an error and not retried by this
    // pass: debug only.
    if result.skipped_in_use > 0 {
        tracing::debug!(
            skipped = result.skipped_in_use,
            "scheduled retention skipped snapshots in use by readers"
        );
    }
    // Test-only: the retention-END handshake fires AFTER the counters were
    // published above, so a waiter observes this pass's result. A panic in the
    // pass never reaches here (the registry accounts it as one failure).
    #[cfg(test)]
    fire_scheduled_retention_end();
    result
}
