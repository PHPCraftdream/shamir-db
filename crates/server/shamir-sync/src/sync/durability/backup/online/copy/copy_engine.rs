//! Share-tolerant recursive copy with a snapshot-defining hash pass
//! (design `online-backup.md` 4.2, steps C/D/E). WAL directories go
//! through the same engine; "WAL before stores" is the protocol's
//! ordering decision (step C). The snapshot root directory itself is
//! created atomically by the protocol (step A), never here.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::manifest::{self, BACKUP_META_FILE_NAME};
use super::progress::CancelFlag;
use super::protocol::SealRecopyCap;
use crate::sync::durability::audit_chain_files::{
    audit_capture_is_stable, list_audit_files, AUDIT_CAPTURE_ROUNDS,
};
use crate::sync::durability::backup::MANIFEST_FILE_NAME;

use self::audit_rounds::{
    active_log_reappearance_is_unsafe, destination_archive_names, unlisted_destination_audit_file,
    UnlistedAudit, ACTIVE_REOPEN_ROUNDS,
};
use self::list_files::list_files;
pub use super::copy_types::{CopiedFile, CopyError, CopyOptions, CopyPassResult};

/// The pre-publication destination guard of the data-directory pass.
#[path = "audit_rounds.rs"]
mod audit_rounds;
/// The deterministic listing of a pass (sibling file).
#[path = "list_files.rs"]
mod list_files;

/// Fixed reused buffer for streaming copies and hashes.
const STREAM_BUFFER_SIZE: usize = 1024 * 1024;

/// Cooperative seal-re-copy gate (design Q12): bounds ONE repo's seal
/// re-copy (WAL dir + store dir) by wall-clock deadline and cumulative
/// copied bytes. The gate travels through the whole gated pass —
/// listing, per-chunk writes, bounded sharing-retry sleeps (clamped to
/// the deadline), and around each directory fsync — so the cap is
/// enforced cooperatively DURING the work, not only at completion. It
/// cannot preempt a blocking syscall: the deadline is a cooperative
/// bound checked at every engine loop point, never a detached kill.
/// `bytes` accumulates every byte actually written to the destination,
/// including the bytes of sharing-retry re-attempts and of the
/// hash-mismatch re-copies, across BOTH of the repo's gated passes
/// (WAL + store share one gate).
#[derive(Debug)]
pub(crate) struct SealGate {
    deadline: Option<Instant>,
    max_bytes: Option<u64>,
    bytes: Cell<u64>,
}

impl SealGate {
    /// Gate for one repo's seal re-copy; `started` is the per-repo
    /// monotonic instant taken immediately BEFORE the seal barrier, so
    /// the barrier's own latency is inside the budget. A configured
    /// clock cap beyond the platform's instant range is a typed refusal
    /// — never a silent unlimited clock.
    pub(crate) fn new(cap: &SealRecopyCap, started: Instant) -> Result<Self, CopyError> {
        let deadline = match cap.max_seconds {
            Some(seconds) => match started.checked_add(Duration::from_secs(seconds)) {
                Some(deadline) => Some(deadline),
                None => {
                    return Err(CopyError::RecopyCap(format!(
                        "configured clock cap {seconds}s overflows the platform clock"
                    )));
                }
            },
            None => None,
        };
        Ok(Self {
            deadline,
            max_bytes: cap.max_bytes,
            bytes: Cell::new(0),
        })
    }

    /// The gate's clock boundary, handed to the port's
    /// `seal_point_before` so an async server seal honours the same
    /// deadline.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn check_clock(&self) -> Result<(), CopyError> {
        if self.deadline.is_some_and(|deadline| gate_now() >= deadline) {
            return Err(CopyError::RecopyCap(
                "seal re-copy exceeded its wall-clock deadline".to_string(),
            ));
        }
        Ok(())
    }

    /// Cooperative check at an engine loop point: cancellation first,
    /// then the clock.
    pub(crate) fn check(&self, cancel: &CancelFlag) -> Result<(), CopyError> {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        self.check_clock()
    }

    /// Checked BEFORE writing each chunk: the clock, then the byte cap
    /// against the cumulative counter plus this chunk (saturating — a
    /// huge chunk cannot wrap past the cap).
    fn before_write(&self, chunk: u64) -> Result<(), CopyError> {
        self.check_clock()?;
        if let Some(max) = self.max_bytes {
            let written = self.bytes.get();
            // Saturating subtraction: correct even at max = u64::MAX.
            if chunk > max.saturating_sub(written) {
                return Err(CopyError::RecopyCap(format!(
                    "repo seal re-copy reached {written}+{chunk} bytes, cap {max}"
                )));
            }
        }
        Ok(())
    }

    /// Charged BEFORE the write lands: a partial or failed write keeps
    /// the charge (conservative — bytes that reached the destination
    /// file, and bytes a retry will re-attempt, both count against the
    /// cap). Saturating.
    fn charge(&self, chunk: u64) {
        self.bytes.set(self.bytes.get().saturating_add(chunk));
    }

    /// Sharing-retry sleep clamped to the deadline: never oversleeps
    /// past the gate, and refuses to sleep at all once past it.
    fn sleep(&self, delay: Duration) -> Result<(), CopyError> {
        match self.deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(gate_now());
                if remaining.is_zero() {
                    return self.check_clock();
                }
                std::thread::sleep(delay.min(remaining));
            }
            None => std::thread::sleep(delay),
        }
        Ok(())
    }
}

/// Windows sharing violations retry on BOTH sides: a source open/read
/// (`Io`) and a destination create/write/fsync (`Dest`) are equally
/// transient while another process holds the file.
fn is_sharing_violation(error: &CopyError) -> bool {
    let source = match error {
        CopyError::Io { source, .. } | CopyError::Dest { source, .. } => source,
        CopyError::Cancelled
        | CopyError::RecopyCap(_)
        | CopyError::AuditFilesUnstable { .. }
        | CopyError::AuditListing { .. }
        | CopyError::AuditArchiveLost => {
            return false;
        }
    };
    matches!(source.raw_os_error(), Some(32) | Some(33))
}

/// The gate's monotonic now: the test clock seam under `cfg(test)`
/// (registered via `test_seams::set_monotonic_now`), the real clock
/// otherwise. Guarded by cfg attributes on statements, not `cfg!()` —
/// the seam module would fail to typecheck in a release build.
fn gate_now() -> Instant {
    #[cfg(test)]
    let now = test_seams::fire_monotonic_now(Instant::now());
    #[cfg(not(test))]
    let now = Instant::now();
    now
}

/// Every engine I/O failure is typed `CopyError::Io` naming the path it
/// was operating on — never string-erased through `io::Error::other`.
/// Destination-side writes use [`dest_at`] instead, so a destination
/// failure is never read as a vanished source. The typed `Dest` variant is
/// used for destination create/write/fsync failures; creating the snapshot
/// child itself is separately typed by the protocol.
fn io_at(path: &Path, error: std::io::Error) -> CopyError {
    CopyError::Io {
        path: path.to_path_buf(),
        source: error,
    }
}

/// Every destination-side write failure is typed `CopyError::Dest`
/// naming the DESTINATION path it was writing — never the source, whose
/// `Io { NotFound }` is the tolerated vanished-file race.
fn dest_at(path: &Path, error: std::io::Error) -> CopyError {
    CopyError::Dest {
        path: path.to_path_buf(),
        source: error,
    }
}

fn hex_encode(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{:02x}", byte);
    }
    out
}

/// Test-only injection seams for the failure-injection suite (the
/// module body lives in `tests/copy_seams.rs`). Compiled ONLY under
/// `#[cfg(test)]` — no release build carries any of this code, and no
/// production behaviour reads it. Each hook is a scoped thread-local
/// cleared when its OWN guard drops — a guard never clears another
/// hook's slot, so independently registered faults coexist and can be
/// dropped in any order — and no state can leak between tests; the
/// protocol runs on the caller's thread, so the hooks observe exactly
/// the copy/hash calls of the run under test.
#[cfg(test)]
#[path = "../tests/copy_seams.rs"]
pub(crate) mod test_seams;

/// Stream-hash a file, returning lowercase-hex SHA-256 and size.
pub fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    hash_file_inner(path, &CancelFlag::default(), None).map_err(|error| match error {
        CopyError::Io { source, .. } => source,
        // Unreachable without a gate; kept typed rather than
        // string-erased.
        other => std::io::Error::other(other.to_string()),
    })
}

fn hash_file_inner(
    path: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<(String, u64), CopyError> {
    #[cfg(test)]
    {
        if let Some(result) = test_seams::fire_hash_file(path) {
            return result.map_err(|error| io_at(path, error));
        }
    }
    let mut file = fs::File::open(path).map_err(|error| io_at(path, error))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let read = file.read(&mut buf).map_err(|error| io_at(path, error))?;
        if read == 0 {
            // Clock and cancel checked on EOF too: the pass ends inside
            // the budget or not at all.
            if let Some(gate) = gate {
                gate.check(cancel)?;
            } else if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            break;
        }
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    Ok((hex_encode(&hasher.finalize()), size))
}

/// Copy `source` to `dest` through the fixed buffer, hashing the bytes
/// written. Windows sharing violations are retried with a bounded
/// delay (clamped to the gate's deadline). Only `CopyError::Io` and
/// `CopyError::Dest` sharing violations retry; caps and cancellation
/// propagate typed.
fn copy_with_retry(
    source: &Path,
    dest: &Path,
    cancel: &CancelFlag,
    options: &CopyOptions,
    gate: Option<&SealGate>,
) -> Result<(String, u64), CopyError> {
    let mut attempt = 0u32;
    loop {
        match copy_once(source, dest, cancel, gate) {
            Ok(result) => return Ok(result),
            Err(error) if is_sharing_violation(&error) => {
                attempt += 1;
                if attempt >= options.share_retry_attempts {
                    return Err(error);
                }
                match gate {
                    Some(gate) => {
                        gate.sleep(Duration::from_millis(options.share_retry_delay_ms))?
                    }
                    None => std::thread::sleep(Duration::from_millis(options.share_retry_delay_ms)),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn copy_once(
    source: &Path,
    dest: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<(String, u64), CopyError> {
    let mut input = fs::File::open(source).map_err(|error| io_at(source, error))?;
    let mut output = fs::File::create(dest).map_err(|error| dest_at(dest, error))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut size = 0u64;
    loop {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let read = input.read(&mut buf).map_err(|error| io_at(source, error))?;
        if read == 0 {
            // Clock and cancel checked on EOF too.
            if let Some(gate) = gate {
                gate.check(cancel)?;
            } else if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            break;
        }
        if let Some(gate) = gate {
            // Byte cap checked and CHARGED before the chunk lands: a
            // failed or partial write keeps the charge (conservative).
            gate.before_write(read as u64)?;
            gate.charge(read as u64);
        }
        output
            .write_all(&buf[..read])
            .map_err(|error| dest_at(dest, error))?;
        #[cfg(test)]
        test_seams::fire_after_first_write(source).map_err(|error| io_at(source, error))?;
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    if let Some(gate) = gate {
        // Before and after the file fsync.
        gate.check(cancel)?;
    }
    output.sync_all().map_err(|error| dest_at(dest, error))?;
    if let Some(gate) = gate {
        gate.check(cancel)?;
    }
    Ok((hex_encode(&hasher.finalize()), size))
}

/// Fsync one destination directory; a failure is [`CopyError::Dest`] naming
/// the DIRECTORY (a destination failure, never a source one) and always
/// propagates — never swallowed. [`manifest::fsync_dest_dir`] owns the ONE
/// tolerated-filesystem-capability rule of the online copy AND the test seam
/// that substitutes its result, so the copy passes and the two publications
/// (`manifest::write_manifest`, `manifest::write_backup_meta`) can never
/// disagree about which directory-fsync failures are tolerated.
fn fsync_dir_fallible(dir: &Path) -> Result<(), CopyError> {
    manifest::fsync_dest_dir(dir).map_err(|error| dest_at(dir, error))
}

/// One copy pass `source_root` -> `dest_root` (design 4.2 steps C/D/E):
/// deterministic listing, per-file copy with bounded sharing-violation
/// retry, then a second hash pass over every SOURCE file that defines the
/// snapshot. A source file changed between copy and hash is re-copied up
/// to twice more; one still mismatching fails the pass. A source file
/// deleted mid-copy is excluded from the result. Files named in
/// [`list_files`]'s skip rules (`.shamir.lock`, `lock`) and files under any of
/// `exclude_dirs` (absolute source paths) are left out entirely.
pub fn run_copy_pass(
    source_root: &Path,
    dest_root: &Path,
    skip_top_level_dirs: &[&str],
    exclude_dirs: &[PathBuf],
    cancel: &CancelFlag,
    options: &CopyOptions,
) -> Result<CopyPassResult, CopyError> {
    run_copy_pass_inner(
        source_root,
        dest_root,
        &PassRules {
            skip_top_level_dirs,
            skip_top_level_files: &[],
            audit_files: AuditFiles::InOrder,
            exclude_dirs,
            cancel,
            options,
            gate: None,
            preauth: &BTreeMap::new(),
        },
    )
}

/// Data-dir pass: everything outside `exclude_dirs`, skipping the private
/// `cache` subtree and manifest files, in TWO phases.
///
/// Phase 1 captures every other pass file, including `audit_checkpoint/` and
/// `server_meta/`, but not `audit.log` or its rotation archives. Phase 2 lists
/// and captures the audit chain only after phase 1, archives in ascending name
/// order before the active log. This ordering ensures anchors only name bytes
/// already durable and present before phase 2's listing: checkpoint flushes
/// before writing its row; rotation fsyncs, renames, reopens, dir-syncs, then
/// persists both anchors. The log is append-only and rotation only adds an
/// archive. Thus phase 2 sees bytes covered by captured anchors, including an
/// archive created between phase 1 listing and anchor capture. A single early
/// listing could miss that archive.
///
/// Phase 2 re-lists after each capture and repeats if an archive appeared;
/// each round recopies every listed file because rotation can move active-log
/// bytes into a new archive. There are at most four unstable rounds plus three
/// confirmed active-log reopen attempts: seven capture rounds per pass, a
/// count bound, not a time bound. A stable round completes only if the
/// destination has no unlisted audit file, except an eligible active-log-only
/// leftover can trigger reopen. Exhausting unstable rounds fails with typed
/// [`CopyError::AuditFilesUnstable`]. The manifest describes the accepted
/// round; counters include work across all rounds, including retries.
/// NOTHING is deleted from the destination. An uncaptured reserved audit file
/// from an earlier round or pass yields typed, path-free
/// [`CopyError::AuditArchiveLost`] before manifest write. An unlisted archive
/// always refuses, even with an active-log leftover. That leftover is
/// retryable only if the accepted round proves a new archive since the prior
/// stable listing; same-name replacement alone is insufficient. A new archive
/// name does not prove chain continuity. Phase-1 anchors may name those bytes,
/// and inventory self-verification would also refuse. Cleanup only attempts
/// to remove the whole run directory; removal can fail and leave it behind.
/// A later eligible scheduler tick may start a fresh run; same-run retry is
/// not guaranteed. A source vanishing during corrective re-copy or re-hash is
/// MISSING for that round: rotation moved the active log to a new archive,
/// which stability checking re-lists for the next round. Other passes still
/// fail with typed I/O errors for vanished sources.
///
/// Retention may delete an archive between listing and copy; a vanished file
/// is excluded like any deleted source. A retained archive captured by the
/// accepted round does not trigger `AuditArchiveLost`. Unsupported active-log
/// reappearance can trigger it without a leftover copy: the reappearance guard
/// runs before the leftover scan. Retention never removes the newest
/// archive, which alone can hold the tail of a just-persisted anchor. A needed
/// prefix already gone before capture remains the documented limit; boot
/// validates the saved anchor against the copied chain.
///
/// KNOWN LIMIT: live `server_meta` is copied file by file in phase 1, outside
/// every seal, and is not a consistent-point snapshot. The copier's hash, size,
/// and inventory self-check never opens the store. Phase F validation runs via
/// `OnlineBackupPort::validate_copied_data_dir`, whose default is a no-op and
/// which only the server adapter implements. That adapter opens a disposable
/// clone covering copied `server_meta`, the `audit_checkpoint` store and copied
/// audit files, then validates the copied anchor against the copied audit chain
/// before publication; this can detect a mixed image, but cannot recover missing
/// source bytes. The copier itself does not repair, validate, or delete files;
/// boot remains the last resort for copies made without the hook, including older
/// servers and manual copies.
/// [`CopyError::AuditArchiveLost`] guards audit files only.
pub(super) fn run_data_dir_copy_pass(
    source_root: &Path,
    dest_root: &Path,
    exclude_dirs: &[PathBuf],
    cancel: &CancelFlag,
    options: &CopyOptions,
) -> Result<CopyPassResult, CopyError> {
    let rules = PassRules {
        skip_top_level_dirs: &["cache"],
        skip_top_level_files: &[MANIFEST_FILE_NAME, BACKUP_META_FILE_NAME],
        audit_files: AuditFiles::AfterAnchor,
        exclude_dirs,
        cancel,
        options,
        gate: None,
        preauth: &BTreeMap::new(),
    };
    // Phase 1: every other file, both anchor stores included.
    let mut result = run_copy_pass_inner(source_root, dest_root, &rules)?;
    // Phase 2: the audit chain files, listed only now. Two BOUNDED budgets
    // drive the loop: unstable rounds (an archive keeps appearing) and the
    // rotation window's reopen attempts (the active name is absent between the
    // appender's rename and its immediate reopen).
    let mut unstable_rounds = 0usize;
    let mut reopen_attempts = 0usize;
    // Preserve the pass-entry destination archive set: pass D may have copied
    // archives that pass E's first stable round must compare against when
    // proving a genuine rotation behind a stale destination active log.
    let mut previous_stable_archives = Some(destination_archive_names(dest_root, cancel)?);
    // Retain rotation evidence across the rename-to-reopen window: the archive
    // may be observed in the stable round that still omits audit.log, while the
    // active log reappears only in the following stable round.
    let mut reopen_archive_evidence = false;
    // Track the active-log presence of each stable source listing separately
    // from the destination archive baseline: a same-name active file can
    // reappear with replacement bytes and no archive evidence.
    let mut previous_stable_active_present: Option<bool> = None;
    loop {
        if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let audit = run_copy_pass_inner(
            source_root,
            dest_root,
            &PassRules {
                audit_files: AuditFiles::Only,
                ..rules
            },
        )?;
        // The audit phase's counters include work from every capture round,
        // including unstable rounds and rounds followed by a retry: a source
        // that vanishes inside one round's corrective re-copy is counted
        // missing in that round, even when a later accepted round defines the
        // snapshot.
        let round_archives: BTreeSet<String> = audit
            .files
            .iter()
            .filter(|file| file.rel_path != "audit.log")
            .map(|file| file.rel_path.clone())
            .collect();
        result.changed_files += audit.changed_files;
        result.missing_files += audit.missing_files;
        if audit_capture_is_stable(source_root, &audit, cancel)? {
            let current_active_present =
                audit.files.iter().any(|file| file.rel_path == "audit.log");
            let saw_new_archive = previous_stable_archives.as_ref().is_some_and(|previous| {
                round_archives
                    .iter()
                    .any(|archive| !previous.contains(archive))
            });
            if active_log_reappearance_is_unsafe(
                previous_stable_active_present,
                current_active_present,
                reopen_archive_evidence,
            ) {
                if cancel.is_cancelled() {
                    return Err(CopyError::Cancelled);
                }
                return Err(CopyError::AuditArchiveLost);
            }
            previous_stable_archives = Some(round_archives);
            previous_stable_active_present = Some(current_active_present);
            // Nothing is deleted from the destination: a reserved audit name
            // this round did not capture — copied by pass D or by an earlier
            // unstable round — refuses the pass before the manifest exists.
            if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            match unlisted_destination_audit_file(dest_root, &audit.files, cancel)? {
                None => {
                    if cancel.is_cancelled() {
                        return Err(CopyError::Cancelled);
                    }
                    result.files.extend(audit.files);
                    return Ok(result);
                }
                Some(UnlistedAudit::Archive) => {
                    if cancel.is_cancelled() {
                        return Err(CopyError::Cancelled);
                    }
                    return Err(CopyError::AuditArchiveLost);
                }
                Some(UnlistedAudit::ActiveLog) => {
                    // Give the active name a bounded chance to reopen, even
                    // without archive evidence. Evidence is accumulated only
                    // from rounds in which the active name remains missing;
                    // the transition guard rejects an unsupported reappearance.
                    reopen_archive_evidence |= saw_new_archive;
                    reopen_attempts += 1;
                    if reopen_attempts > ACTIVE_REOPEN_ROUNDS {
                        if cancel.is_cancelled() {
                            return Err(CopyError::Cancelled);
                        }
                        return Err(CopyError::AuditArchiveLost);
                    }
                    if cancel.is_cancelled() {
                        return Err(CopyError::Cancelled);
                    }
                    continue;
                }
            }
        }
        unstable_rounds += 1;
        if unstable_rounds >= AUDIT_CAPTURE_ROUNDS {
            if cancel.is_cancelled() {
                return Err(CopyError::Cancelled);
            }
            return Err(CopyError::AuditFilesUnstable {
                rounds: AUDIT_CAPTURE_ROUNDS,
            });
        }
    }
}

/// How one pass treats the audit chain files of its source root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditFiles {
    /// Ordinary pass (repo, WAL, gated re-copy): audit files are captured
    /// with everything else, in sorted order.
    InOrder,
    /// Data-directory phase 1: the audit chain files are left to phase 2,
    /// which runs only after this pass has captured every anchor store.
    AfterAnchor,
    /// Data-directory phase 2: the audit chain files ARE the listing,
    /// archives first and the active log last.
    Only,
}

/// Engine-internal pass rules shared by the copy loops (keeps argument
/// lists short).
struct PassRules<'a> {
    skip_top_level_dirs: &'a [&'a str],
    skip_top_level_files: &'a [&'a str],
    exclude_dirs: &'a [PathBuf],
    cancel: &'a CancelFlag,
    options: &'a CopyOptions,
    gate: Option<&'a SealGate>,
    preauth: &'a BTreeMap<String, (String, u64)>,
    /// Which audit chain files this pass captures (see [`AuditFiles`]).
    audit_files: AuditFiles,
}

/// Same pass, GATED and INCREMENTAL (seal re-copy): `gate` bounds the
/// run cooperatively (listing, per-chunk writes, retry sleeps, fsync
/// surroundings, hash chunks) and switches the copy to incremental:
/// the first passes already copied the whole WAL/store, so for every
/// file the gate first compares the SOURCE against the ACTUAL
/// DESTINATION content by size + digest (never mtimes, never the
/// previous pass's returned hash, which may mismatch the destination
/// by design before self-verification) and skips the write when they
/// already agree; destination digests authenticated outside the hold
/// are reused by path. The gate is closed by the seal barrier for this
/// repo, so the compared source is stable; the snapshot-defining pass
/// still runs over authentic destination-backed content. Gated passes
/// never skip top-level dirs and never exclude (the protocol scopes
/// them per repo).
pub(crate) fn run_copy_pass_gated(
    source_root: &Path,
    dest_root: &Path,
    cancel: &CancelFlag,
    options: &CopyOptions,
    gate: &SealGate,
    preauth: &BTreeMap<String, (String, u64)>,
) -> Result<CopyPassResult, CopyError> {
    run_copy_pass_inner(
        source_root,
        dest_root,
        &PassRules {
            skip_top_level_dirs: &[],
            skip_top_level_files: &[],
            audit_files: AuditFiles::InOrder,
            exclude_dirs: &[],
            cancel,
            options,
            gate: Some(gate),
            preauth,
        },
    )
}

/// Hash every file under `dest_root` (same listing rules as a copy
/// pass) into rel-path -> (digest, size). Run OUTSIDE the seal hold by
/// the protocol so the gated re-copy never re-reads unchanged
/// destinations under the hold; the map is a digest/size authentication
/// of ACTUAL destination content — never mtimes. Clock/cancel bounded
/// (a gate may be passed: reads count against the clock, not the byte
/// cap).
pub(crate) fn authenticate_dest(
    dest_root: &Path,
    cancel: &CancelFlag,
    gate: Option<&SealGate>,
) -> Result<BTreeMap<String, (String, u64)>, CopyError> {
    // A missing destination root authenticates as EMPTY: nothing is
    // proven about any file, so every file goes through hash-under-hold
    // (or a fresh copy).
    if fs::symlink_metadata(dest_root)
        .map(|meta| !meta.is_dir())
        .unwrap_or(true)
    {
        return Ok(BTreeMap::new());
    }
    let listing = list_files(dest_root, &[], &[], cancel, gate, false)?;
    let mut out = BTreeMap::new();
    for path in listing {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let rel = match path.strip_prefix(dest_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let rel_slash = rel
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let (digest, size) = hash_file_inner(&path, cancel, gate)?;
        out.insert(rel_slash, (digest, size));
    }
    Ok(out)
}

fn run_copy_pass_inner(
    source_root: &Path,
    dest_root: &Path,
    rules: &PassRules<'_>,
) -> Result<CopyPassResult, CopyError> {
    let listing = match rules.audit_files {
        AuditFiles::Only => list_audit_files(source_root, rules.cancel, rules.gate)?,
        AuditFiles::AfterAnchor => {
            let listing = list_files(
                source_root,
                rules.skip_top_level_dirs,
                rules.skip_top_level_files,
                rules.cancel,
                rules.gate,
                true,
            )?;
            // Test-only seam: the pass listing is fixed and it names no
            // audit chain file; the anchor stores are captured from here on.
            #[cfg(test)]
            test_seams::fire_after_listing(source_root);
            listing
        }
        AuditFiles::InOrder => list_files(
            source_root,
            rules.skip_top_level_dirs,
            rules.skip_top_level_files,
            rules.cancel,
            rules.gate,
            false,
        )?,
    };
    let &PassRules {
        exclude_dirs,
        cancel,
        options,
        gate,
        preauth,
        ..
    } = rules;
    // Last entry: the file's stored digest is already AUTHENTICATED
    // against the actual destination under the closed gate (source and
    // destination hashed equal), so the second hash pass must not
    // re-hash it — one comparison, no third hash of unchanged files.
    let mut copied: Vec<(String, PathBuf, PathBuf, String, u64, bool)> = Vec::new();
    let mut changed_files = 0u64;
    let mut missing_files = 0u64;
    let mut synced_dirs: Vec<PathBuf> = Vec::new();
    for source_path in listing {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        if exclude_dirs.iter().any(|dir| source_path.starts_with(dir)) {
            continue;
        }
        let rel = match source_path.strip_prefix(source_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let rel_slash = rel
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let dest_path = dest_root.join(rel);
        if let Some(parent) = dest_path.parent() {
            // A parent-directory failure is a DESTINATION error and
            // always propagates: it can never be a vanished source.
            fs::create_dir_all(parent).map_err(|error| dest_at(parent, error))?;
            if !synced_dirs.contains(&parent.to_path_buf()) {
                synced_dirs.push(parent.to_path_buf());
            }
        }
        // Incremental seal re-copy: under the closed gate, compare the
        // source against the ACTUAL destination content (size + digest;
        // authentic, never mtime-based, never the previous pass's
        // returned hash which may mismatch the destination by design
        // before self-verification). Destination digests authenticated
        // before the hold are reused by path; only a destination absent
        // from that map (new/changed file) is hashed under the hold.
        // Only changed or new files are written and hash-verified. A
        // source file already gone is missing, exactly as an uncopyable
        // file was before; caps and cancellation surface typed.
        if let Some(gate) = gate {
            let source_digest = match hash_file_inner(&source_path, cancel, Some(gate)) {
                Ok(digest) => digest,
                Err(CopyError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    missing_files += 1;
                    continue;
                }
                Err(other) => return Err(other),
            };
            let dest_digest = match preauth.get(&rel_slash) {
                Some(authenticated) => Some(authenticated.clone()),
                None => match hash_file_inner(&dest_path, cancel, Some(gate)) {
                    Ok(digest) => Some(digest),
                    Err(CopyError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        None
                    }
                    Err(other) => return Err(other),
                },
            };
            if let Some(dest_digest) = dest_digest {
                if source_digest == dest_digest {
                    copied.push((
                        rel_slash,
                        source_path,
                        dest_path,
                        source_digest.0,
                        source_digest.1,
                        true,
                    ));
                    continue;
                }
            }
        }
        let copy = match copy_with_retry(&source_path, &dest_path, cancel, options, gate) {
            Ok(copy) => copy,
            Err(CopyError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                missing_files += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        #[cfg(test)]
        test_seams::fire_after_copy(&source_path);
        copied.push((rel_slash, source_path, dest_path, copy.0, copy.1, false));
    }
    for dir in &synced_dirs {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        }
        fsync_dir_fallible(dir)?;
        if let Some(gate) = gate {
            gate.check(cancel)?;
        }
    }
    if let Some(gate) = gate {
        gate.check(cancel)?;
    }
    // A pass that copied nothing (an empty WAL directory) never created
    // its destination root: there are no new entries to make durable.
    if dest_root.is_dir() {
        fsync_dir_fallible(dest_root)?;
    }
    if let Some(gate) = gate {
        gate.check(cancel)?;
    }

    // Second hash pass: defines the snapshot (design 4.2 step D) —
    // except for files whose stored digest was already authenticated
    // against the actual destination under the closed gate: there was
    // no copy race on that branch, and step-F self-verification still
    // re-hashes the actual destination.
    let mut files = Vec::with_capacity(copied.len());
    for (rel_slash, source_path, dest_path, copy_hash, copy_size, preverified) in copied {
        if let Some(gate) = gate {
            gate.check(cancel)?;
        } else if cancel.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let (hash, size) = if preverified {
            // Stored digest already authenticated against the actual
            // destination under the closed gate — no third hash.
            (copy_hash, copy_size)
        } else {
            let (pass_hash, pass_size) = match hash_file_inner(&source_path, cancel, gate) {
                Ok(result) => result,
                Err(CopyError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    missing_files += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if pass_hash == copy_hash && pass_size == copy_size {
                (pass_hash, pass_size)
            } else {
                // Corrective re-copy, bounded (design 4.2 D).
                changed_files += 1;
                let recopied = recopy_until_stable(
                    &source_path,
                    &dest_path,
                    cancel,
                    options,
                    gate,
                    rules.audit_files == AuditFiles::Only,
                    (pass_hash, pass_size),
                )?;
                let Some(settled) = recopied else {
                    // The source vanished between its copy and this
                    // corrective copy: in the audit chain phase that
                    // rotation renamed the active log into a NEW archive,
                    // which the pass's stability check re-lists and the
                    // next round captures, so the file is MISSING from
                    // this round, not a failure.
                    missing_files += 1;
                    continue;
                };
                settled
            }
        };
        #[cfg(test)]
        test_seams::fire_after_capture(&source_path);
        files.push(CopiedFile {
            rel_path: rel_slash,
            sha256: hash,
            size_bytes: size,
        });
    }
    Ok(CopyPassResult {
        files,
        changed_files,
        missing_files,
    })
}

/// Bounded corrective re-copy of one file whose snapshot hash differed from
/// the hash of its copy: copy it again and hash the source again, at most
/// twice, accepting the copy once the two agree. Returns the LAST pair —
/// exactly what the inline loop returned before, so a persistent mismatch
/// still surfaces at the run's self-verification, never here.
///
/// `tolerate_vanished` is set ONLY for the audit chain phase
/// (`AuditFiles::Only`): there a source that vanished before the corrective
/// copy or its re-hash is the rotation that renamed the active log into a new
/// archive, and `Ok(None)` reports it MISSING so the pass's stability check
/// re-lists. Every other pass keeps the old behaviour: a vanished source
/// fails the pass with its typed `NotFound`.
fn recopy_until_stable(
    source_path: &Path,
    dest_path: &Path,
    cancel: &CancelFlag,
    options: &CopyOptions,
    gate: Option<&SealGate>,
    tolerate_vanished: bool,
    mismatched: (String, u64),
) -> Result<Option<(String, u64)>, CopyError> {
    let mut hash = mismatched.0;
    let mut size = mismatched.1;
    for _ in 0..2 {
        let recopy = match copy_with_retry(source_path, dest_path, cancel, options, gate) {
            Ok(copy) => copy,
            Err(CopyError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound && tolerate_vanished =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let rehash = match hash_file_inner(source_path, cancel, gate) {
            Ok(result) => result,
            Err(CopyError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                if tolerate_vanished {
                    return Ok(None);
                }
                return Err(CopyError::Io {
                    path: source_path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "source vanished during re-copy",
                    ),
                });
            }
            Err(error) => return Err(error),
        };
        let matches_copy = rehash.0 == recopy.0 && rehash.1 == recopy.1;
        hash = rehash.0;
        size = rehash.1;
        if matches_copy {
            break;
        }
    }
    Ok(Some((hash, size)))
}
