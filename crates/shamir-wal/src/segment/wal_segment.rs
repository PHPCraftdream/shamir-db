//! Append-only, file-backed WAL segment.
//!
//! `WalSegment` is the real append-only file backing the WAL's durability
//! tiers: [`append_batch`](WalSegment::append_batch) does `write()` +
//! userspace flush (level 2), while [`sync`](WalSegment::sync) does
//! `fsync` (level 3). This split is the foundation of the durability
//! contract (see `docs/dev-artifacts/perf/durability-model.md`, "Реализация B").
//!
//! Live production primitive: composed into a [`crate::SegmentSet`]
//! that the repo's [`WalSink::File`]` drives via
//! [`WalGroupCommit`](crate::WalGroupCommit). A `Mem` sink variant
//! mirrors the same interface for in-memory repos / tests.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use shamir_storage::error::{DbError, DbResult};
use tokio::task::spawn_blocking;

use super::frame_reader::{FrameRead, FrameReader, MAX_FRAME_BYTES};
use crate::wal_entry_v2::WalEntryV2;

#[cfg(test)]
type AppendDelay = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[derive(Debug)]
struct WalIoContext {
    context: String,
    source: std::io::Error,
}

impl std::fmt::Display for WalIoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for WalIoContext {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub(crate) fn wal_io_error(context: String, source: std::io::Error) -> DbError {
    DbError::Io(std::io::Error::new(
        source.kind(),
        WalIoContext { context, source },
    ))
}

/// Replay-open retry budget for a transient denial (about 1 s in total).
const OPEN_DENIED_ATTEMPTS: u32 = 50;
const OPEN_DENIED_BACKOFF: std::time::Duration = std::time::Duration::from_millis(20);

/// `PermissionDenied`, or on Windows a sharing violation (os error 32): both
/// are what a scanner or a delete-pending handle produces on a fresh segment.
fn is_transient_open_denial(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
        || (cfg!(windows) && e.raw_os_error() == Some(32))
}

/// Fsync the parent directory of `path` so a freshly-created file's
/// directory entry is durable (audit §1.9). On ext4/xfs, `sync_all()` on
/// the file does NOT guarantee the directory entry survives power loss — a
/// newly-created segment can vanish from the listing, losing every acked
/// write in it. Unix-only; a no-op stub on Windows (where directory fsync
/// is not required for this durability guarantee). Failures are logged but
/// do NOT fail the open: a missing dir-fsync degrades the power-loss window
/// but does not corrupt data (the file's own `sync_all` still applies), and
/// refusing to open a perfectly good segment over a non-fatal dir-fsync
/// would be worse than the degraded window.
#[cfg(unix)]
fn fsync_parent_dir(path: &std::path::Path) -> DbResult<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Ok(()), // no parent (root) — nothing to fsync
    };
    match std::fs::File::open(parent) {
        Ok(dir_f) => {
            if let Err(e) = dir_f.sync_all() {
                log::warn!(
                    "WalSegment dir fsync of {parent:?} failed ({e}); \
                     directory entry durability is not guaranteed on power loss \
                     (audit §1.9) — file data fsync still applies",
                    parent = parent
                );
            }
        }
        Err(e) => {
            log::warn!(
                "WalSegment dir open for fsync of {parent:?} failed ({e}); \
                 directory entry durability is not guaranteed on power loss \
                 (audit §1.9)",
                parent = parent
            );
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn fsync_parent_dir(_path: &std::path::Path) -> DbResult<()> {
    // Windows / non-unix: directory fsync is not required for the durability
    // guarantee that matters here. No-op.
    Ok(())
}

/// Append-only, file-backed WAL segment. Splits durability:
///   - `append_batch` → `write()` to the OS page cache (level 2:
///     survives a process crash, lost only on power loss before `sync`).
///   - `sync`         → `fsync` (level 3: survives power loss).
///
/// The group-commit coordinator normally serializes appends. Direct concurrent
/// callers are serialized by the file mutex, including rollback and accounting.
/// The mutex is held only on a blocking thread inside `spawn_blocking`
/// (`append_batch` / `sync`), NEVER across an `.await`. The `Arc` is mandatory regardless
/// of the lock — `spawn_blocking` needs a `'static` handle, so the file must
/// be reference-counted to cross the closure boundary. (c) Historical bench
/// observation: the WAL-append bench (`benches/wal_append.rs`, baseline
/// `2e3bd51`) recorded file-sink throughput rising with concurrency
/// (7.2K→80.2K appends/s, N=1→64); these raw endpoints are observations
/// only, and any derived scaling ratio is UNVERIFIED. The historical "fsync
/// dominates a durable append ~63×" ratio is likewise UNVERIFIED (no
/// reproducible measurement in this repo); treat fsync as dominant by
/// construction, not by a cited number.
/// The marginal ~10µs/append figure is likewise historical and unverified.
///
/// A single-writer-task rewrite (CAPSTONE, `docs/dev-artifacts/perf/capstone-subplan.md`)
/// would drop the `Mutex` (single ownership becomes type-level) but keep the
/// `Arc` for `spawn_blocking`. It was PROTOTYPED and REVERTED: a permanent
/// writer task mandates a per-append cross-task `oneshot` round-trip, which
/// suspends the executor on EVERY append — whereas the rotating leader drains
/// the in-RAM `Mem` sink synchronously within one poll (no yield). That
/// regressed mem N=1 latency ~+22% (the subplan §0 GO/NO-GO criterion) AND
/// broke an atomicity property the engine commit path relies on (a
/// non-yielding Mem append keeps a commit atomic on a current-thread runtime;
/// the mandatory yield let concurrent SSI committers all validate before any
/// published, defeating "exactly one wins"). So the lock stays, honestly
/// annotated.
pub struct WalSegment {
    path: PathBuf,
    file: Arc<Mutex<File>>,
    next_seq: AtomicU64,
    /// Highest `commit_version` ever appended to this segment
    /// (`fetch_max` on every `append_batch`). A sealed segment is eligible
    /// for truncation only when its maximum is positive and at or below the
    /// durable watermark and its separate retention pin is clear (F6). Init 0.
    ///
    /// Reopen caveat (RV-WAL-48749F97): a bare [`WalSegment::open`] of an
    /// existing non-empty segment starts at 0 with the retention pin clear.
    /// Only frames appended through this handle (plus anything restored by
    /// [`crate::SegmentSet::open`], which replays retained active frames via
    /// `restore_retention`) are reflected. Truncation eligibility is decided
    /// ONLY by `SegmentSet`; never treat the handle-local maximum as a durable
    /// cross-segment watermark.
    max_committed: AtomicU64,
    /// A zero-version or undecodable record permanently pins this segment.
    retention_pin: AtomicBool,
    /// Running count of bytes written through `append_batch` (frame bytes
    /// only). Cheap rotation trigger — avoids a `metadata()` syscall on every
    /// append. Tracks exactly what we wrote; matches the on-disk size for a
    /// freshly created or reopened segment.
    bytes_written: Arc<AtomicU64>,
    /// Quarantine flag — set when a `write_all` or `sync_all` on this segment
    /// failed (audit §1.3). Once poisoned, the segment refuses further appends
    /// so the leader rotates to a brand-new file instead of stranding later
    /// acked commits behind a torn tail. A poisoned segment is never un-poisoned
    /// in-process — recovery on next open rebuilds from disk state.
    poisoned: Arc<AtomicBool>,
    #[cfg(test)]
    append_delay: Mutex<Option<AppendDelay>>,
    #[cfg(test)]
    fail_next_write_after: AtomicU64,
    /// Test-only fault seam: inject `PermissionDenied` at `File::open` for
    /// the next N replay calls (flows through the real permission-policy
    /// branch).
    #[cfg(test)]
    fail_startup_open_deny: std::sync::atomic::AtomicU32,
    /// Test-only fault seam: fail the next post-partial-write rollback
    /// (`set_len` back to the last good boundary).
    #[cfg(test)]
    fail_next_rollback: AtomicBool,
    #[cfg(test)]
    fail_next_sync: AtomicBool,
    #[cfg(test)]
    fail_next_replay_read: AtomicBool,
}

impl WalSegment {
    /// Open (creating if absent) an append-only segment at `path`.
    ///
    /// Reopen watermark caveat (RV-WAL-48749F97): a bare open of an existing
    /// non-empty segment starts with `max_committed = 0` and the retention
    /// pin clear — the file is NOT scanned. The handle-local values cover only
    /// frames appended through this handle plus anything restored by
    /// [`crate::SegmentSet::open`] (which replays retained active frames and
    /// calls `restore_retention`). Truncation eligibility is decided only by
    /// `SegmentSet`; direct-open callers must not use [`Self::max_committed`]
    /// as a durable cross-segment watermark.
    ///
    /// **Audit §1.9 (Linux directory fsync):** when the segment file is
    /// NEWLY created (did not exist before this call), the parent directory
    /// is fsync'd so the directory entry itself is durable. Without this, on
    /// ext4/xfs a freshly-created segment can be missing from the directory
    /// listing after power loss — even Synced-acked writes in it are lost and
    /// replay won't see the file existed. `sync_all()` on the file does NOT
    /// guarantee the directory entry is durable. On Windows this is a no-op
    /// (directory fsync is not required for durability semantics there).
    pub async fn open(path: PathBuf) -> DbResult<Self> {
        let p = path.clone();
        let (file, existing_len) = spawn_blocking(move || -> DbResult<(File, u64)> {
            // Detect NEW creation (file did not exist) so we can fsync the
            // parent directory only when a new entry was actually added.
            let existed_before = p.exists();
            let f = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&p)
                // Preserve the io::ErrorKind across the wrap boundary
                // (RV-WAL-5839FF0D): DbError::Io keeps the typed cause that
                // DbError::Storage(format!) destroyed.
                .map_err(|e| wal_io_error(format!("WalSegment open {p:?}"), e))?;
            // Audit §1.9: on first creation, fsync the parent directory so the
            // directory entry is durable. Without this, a power loss on
            // ext4/xfs can lose the freshly-created segment entirely even
            // though its data was fsync'd. Harmless on reopen (the entry is
            // already durable). No-op on Windows (#[cfg(unix)] gate).
            if !existed_before {
                fsync_parent_dir(&p)?;
            }
            // Seed bytes_written from the on-disk size so a reopened (non-empty)
            // segment reports its true length for rotation decisions.
            let len = f
                .metadata()
                .map_err(|e| wal_io_error(format!("WalSegment open metadata {p:?}"), e))?
                .len();
            Ok((f, len))
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;
        Ok(Self {
            path,
            file: Arc::new(Mutex::new(file)),
            next_seq: AtomicU64::new(0),
            max_committed: AtomicU64::new(0),
            retention_pin: AtomicBool::new(false),
            bytes_written: Arc::new(AtomicU64::new(existing_len)),
            poisoned: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            append_delay: Mutex::new(None),
            #[cfg(test)]
            fail_next_write_after: AtomicU64::new(u64::MAX),
            #[cfg(test)]
            fail_startup_open_deny: std::sync::atomic::AtomicU32::new(0),
            #[cfg(test)]
            fail_next_rollback: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_sync: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_replay_read: AtomicBool::new(false),
        })
    }

    /// Append a batch of encoded payloads in ONE blocking round-trip,
    /// flushing to the OS (level 2) but NOT fsync'ing. Returns the seq
    /// assigned to the LAST entry. Sequence numbers are nonpersistent
    /// per-handle ordinals, restart at zero on reopen, and may have gaps after
    /// failed appends; they are not identifiers for persisted frames. An empty
    /// batch returns the next ordinal instead. Batching amortises the spawn_blocking
    /// hop across all concurrent committers funnelled by the leader.
    ///
    /// Payloads must be [`WalEntryV2::encode`] output. Before sequence/version
    /// accounting or any file mutation, the frame length, header AND body are
    /// validated: a payload with a supported header that fails
    /// [`WalEntryV2::decode`] is rejected with a typed error and nothing is
    /// written or sequenced (a frame that replay would later hard-fail on must
    /// never reach the file). Encoded payloads are limited to 64 MiB, matching
    /// recovery's frame cap; older larger frames fail recovery without being
    /// truncated.
    ///
    /// On a `write_all` failure (e.g. ENOSPC mid-write) the segment is
    /// **quarantined**: the partial bytes are truncated back to the last
    /// good frame boundary (`set_len(pre_batch_offset)`) and `poisoned` is
    /// set so every subsequent append fails fast — forcing the leader to
    /// rotate to a brand-new file instead of stranding later acked commits
    /// behind an unreachable torn tail (audit §1.3).
    pub async fn append_batch(
        &self,
        payloads: Arc<Vec<Vec<u8>>>,
        max_version: u64,
    ) -> DbResult<u64> {
        if self.is_poisoned() {
            return Err(DbError::Storage(format!(
                "WalSegment append refused: segment {:?} is poisoned (prior write/sync failure)",
                self.path
            )));
        }
        let mut payload_max = max_version;
        let mut pinned = false;
        for payload in payloads.iter() {
            if payload.len() > MAX_FRAME_BYTES {
                return Err(DbError::Storage(format!(
                    "WalSegment payload exceeds {MAX_FRAME_BYTES}-byte frame limit"
                )));
            }
            WalEntryV2::validate_frame_header(payload)?;
            match WalEntryV2::decode(payload) {
                Ok(entry) => {
                    payload_max = payload_max.max(entry.commit_version);
                    pinned |= entry.commit_version == 0;
                }
                Err(e) => {
                    // RV-WAL-4470BEF3: a supported header with an undecodable
                    // body would produce a well-formed frame that replay later
                    // hard-fails on. Reject BEFORE sequencing, watermark or
                    // file mutation. Path-free Storage text (3F10E8A1 policy).
                    return Err(DbError::Storage(format!(
                        "WalSegment append rejected: payload is not a decodable WalEntryV2 body ({e})"
                    )));
                }
            }
        }
        if payloads.is_empty() {
            // Still fold the version in — an empty batch carrying a watermark
            // must not regress max_committed (monotonic, fetch_max).
            self.max_committed.fetch_max(max_version, Ordering::AcqRel);
            return Ok(self.next_seq.load(Ordering::Acquire));
        }
        let n = payloads.len() as u64;
        let last_seq = self.next_seq.fetch_add(n, Ordering::AcqRel) + n - 1;
        self.max_committed.fetch_max(payload_max, Ordering::AcqRel);
        // Publish conservatively before dispatch; even a cancelled or failed
        // write must never leave a possibly retained unversioned frame unpinned.
        if pinned {
            self.retention_pin.store(true, Ordering::Release);
        }
        let file = Arc::clone(&self.file);
        let bytes_written = Arc::clone(&self.bytes_written);
        let poisoned = Arc::clone(&self.poisoned);
        let path = self.path.clone();
        #[cfg(test)]
        let delay = self.append_delay.lock().unwrap().take();
        #[cfg(test)]
        let fail_after = self.fail_next_write_after.swap(u64::MAX, Ordering::AcqRel);
        #[cfg(test)]
        let fail_rollback = self.fail_next_rollback.swap(false, Ordering::AcqRel);
        // A cancelled caller detaches this closure; it still completes accounting.
        spawn_blocking(move || -> DbResult<()> {
            // Coalesce all frames into one buffer → a single write() syscall
            // instead of 3N (len header + payload + crc per entry).
            // Frame format is unchanged: [u32 len LE][payload][u32 crc32 LE].
            let total: usize = payloads.iter().map(|p| p.len() + 8).sum();
            let mut buf = Vec::with_capacity(total);
            for p in payloads.iter() {
                let crc = crc32fast::hash(p);
                buf.extend_from_slice(&(p.len() as u32).to_le_bytes());
                buf.extend_from_slice(p);
                buf.extend_from_slice(&crc.to_le_bytes());
            }
            #[cfg(test)]
            if let Some((entered, resume)) = delay {
                let _ = entered.send(());
                let _ = resume.recv();
            }
            let mut f = file.lock().expect("WalSegment file mutex poisoned");
            if poisoned.load(Ordering::Acquire) {
                return Err(DbError::Storage(format!(
                    "WalSegment append refused: segment {path:?} is poisoned (prior write/sync failure)"
                )));
            }
            let pre_batch_offset = bytes_written.load(Ordering::Acquire);
            #[cfg(test)]
            let write_result = if fail_after != u64::MAX {
                let prefix_len = (fail_after as usize).min(buf.len());
                f.write_all(&buf[..prefix_len]).and_then(|()| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::StorageFull,
                        "injected partial WAL write failure",
                    ))
                })
            } else {
                f.write_all(&buf)
            };
            #[cfg(not(test))]
            let write_result = f.write_all(&buf);
            match write_result {
                Ok(()) => {
                    bytes_written.fetch_add(buf.len() as u64, Ordering::AcqRel);
                    Ok(())
                }
                Err(e) => {
                    // Partial write — `write_all` may have written some bytes
                    // before failing (ENOSPC, etc.). Truncate the file back to
                    // the last good frame boundary so no torn frame survives in
                    // the file. We open a fresh write-mode handle for the
                    // truncation: on Windows the append-mode handle the segment
                    // owns lacks GENERIC_WRITE, so `set_len` on it would be
                    // denied. A failure here is catastrophic (the file is in
                    // an unknown state) — log loudly and surface it; the
                    // segment is poisoned below regardless.
                    let do_trunc = || -> std::io::Result<()> {
                        let g = OpenOptions::new().write(true).open(&path)?;
                        g.set_len(pre_batch_offset)
                    };
                    #[cfg(test)]
                    let trunc_res: std::io::Result<()> = if fail_rollback {
                        Err(std::io::Error::other("injected rollback failure"))
                    } else {
                        do_trunc()
                    };
                    #[cfg(not(test))]
                    let trunc_res = do_trunc();
                    if let Err(trunc_err) = trunc_res {
                        log::error!(
                            "WalSegment {path:?}: write_all failed ({e}) AND rollback \
                             set_len({pre_batch_offset}) failed ({trunc_err}) — segment is \
                             in an unknown state; it has been poisoned"
                        );
                    }
                    poisoned.store(true, Ordering::Release);
                    Err(wal_io_error(format!("WalSegment append to {path:?}"), e))
                }
            }
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;
        Ok(last_seq)
    }

    /// Update the reopened segment's watermark from retained frame contents.
    pub(crate) fn restore_retention(&self, max_version: u64, pinned: bool) {
        self.max_committed.fetch_max(max_version, Ordering::AcqRel);
        if pinned {
            self.retention_pin.store(true, Ordering::Release);
        }
    }

    pub(crate) fn retention_pinned(&self) -> bool {
        self.retention_pin.load(Ordering::Acquire)
    }

    /// Highest `commit_version` ever appended to this segment.
    ///
    /// Handle-local only (RV-WAL-48749F97): a bare [`Self::open`] of an
    /// existing non-empty segment starts at 0 — the file is not scanned.
    /// The value covers frames appended through this handle plus anything
    /// restored by [`crate::SegmentSet::open`]. Truncation eligibility is
    /// decided only by `SegmentSet`; do not use this as a durable
    /// cross-segment watermark.
    pub fn max_committed(&self) -> u64 {
        self.max_committed.load(Ordering::Acquire)
    }

    /// Approximate on-disk size in bytes (frame bytes written, seeded from the
    /// file length at open). Read from an atomic counter — no syscall on the
    /// rotation hot path.
    pub fn approx_len_bytes(&self) -> u64 {
        self.bytes_written.load(Ordering::Acquire)
    }

    /// fsync the segment (level 3). Uses `sync_all()` (not `sync_data()`)
    /// because this is a growing append-only file: metadata (file size) must
    /// be flushed alongside data to guarantee the new extent is visible after
    /// power loss on all platforms.
    ///
    /// On an fsync failure the segment is **poisoned** (audit §1.3, the
    /// "fsyncgate" scenario): a failed fsync means we cannot trust the file
    /// is durable, and a later "successful" fsync does not retroactively
    /// persist the in-flight window. The leader reacts to the returned error
    /// by rotating to a new segment rather than retrying on the same fd.
    pub async fn sync(&self) -> DbResult<()> {
        let file = Arc::clone(&self.file);
        let poisoned = Arc::clone(&self.poisoned);
        #[cfg(test)]
        let fail_sync = self.fail_next_sync.swap(false, Ordering::AcqRel);
        let res = spawn_blocking(move || -> DbResult<()> {
            let f = file.lock().expect("WalSegment file mutex poisoned");
            #[cfg(test)]
            let result = if fail_sync {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected WAL sync failure",
                ))
            } else {
                f.sync_all()
            };
            #[cfg(not(test))]
            let result = f.sync_all();
            if result.is_err() {
                poisoned.store(true, Ordering::Release);
            }
            result.map_err(|e| wal_io_error("WalSegment sync".into(), e))
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))?;
        if let Err(ref e) = res {
            log::error!(
                "WalSegment {:?} poisoned after failed fsync: {} — \
                 leader must rotate to a new segment; further appends refused",
                self.path,
                e
            );
        }
        res
    }

    /// Has this segment seen a write or fsync failure? A poisoned segment
    /// refuses further appends; the leader must rotate to a fresh file.
    /// Read-only probe (no lock) — used by the segment-set / group-commit
    /// leader to decide whether to keep appending or force a rotation.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// Crate-test fault hook (RV-WAL-5DFE355F): mark this segment poisoned
    /// so tests can simulate the write/fsync-failure quarantine state.
    /// Production code poisons internally on a write/fsync failure; this
    /// hook is compiled only under `cfg(test)` and is not public API.
    #[cfg(test)]
    pub(crate) fn mark_poisoned(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn delay_next_append(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        *self.append_delay.lock().unwrap() = Some((entered_tx, resume_rx));
        (entered_rx, resume_tx)
    }

    #[cfg(test)]
    pub(crate) fn inject_partial_write_failure(&self, bytes: u64) {
        self.fail_next_write_after.store(bytes, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn inject_sync_failure(&self) {
        self.fail_next_sync.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn inject_replay_read_failure(&self) {
        self.fail_next_replay_read.store(true, Ordering::Release);
    }

    /// Inject a real `io::ErrorKind::PermissionDenied` at the `File::open`
    /// site of `replay_inner` for the next `times` replay calls. The injected
    /// error flows through the EXISTING `is_transient_open_denial` /
    /// `tolerate_permission_denied` classification branch.
    #[cfg(test)]
    pub(crate) fn inject_startup_open_denial(&self, times: u32) {
        self.fail_startup_open_deny.store(times, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn inject_rollback_failure(&self) {
        self.fail_next_rollback.store(true, Ordering::Release);
    }

    /// Repair incomplete trailing bytes left by a partial write. A CRC-bad final
    /// frame is also treated as a crash tail; CRC failure before any suffix is
    /// corruption and returns an error without truncating data.
    /// Called for the active segment on startup; sealed segments are not repaired.
    pub async fn repair_torn_tail(&self) -> DbResult<()> {
        // Read through an independent handle so repeated repair starts at byte zero.
        let path = self.path.clone();
        let repair_offset: Option<u64> = spawn_blocking(move || -> DbResult<Option<u64>> {
            let f = File::open(&path)
                .map_err(|e| wal_io_error(format!("WalSegment repair read {path:?}"), e))?;
            let len = f
                .metadata()
                .map_err(|e| wal_io_error(format!("WalSegment repair metadata {path:?}"), e))?
                .len();
            let mut reader = FrameReader::new(f, len, format!("WalSegment repair read {path:?}"));
            let pos = loop {
                match reader.next()? {
                    FrameRead::End => return Ok(None),
                    FrameRead::Incomplete { offset, .. } => break offset,
                    FrameRead::Frame {
                        offset,
                        end,
                        crc_ok,
                        ..
                    } if !crc_ok => {
                        if end == len {
                            break offset;
                        }
                        return Err(DbError::Storage(format!(
                            "WalSegment repair found complete CRC-bad frame at byte offset {offset} in {path:?}; refusing to truncate possible valid suffix"
                        )));
                    }
                    FrameRead::Frame { .. } => {}
                }
            };
            // Truncate back to the last good frame boundary. Open a fresh
            // write-mode handle: the segment's own handle is append-only,
            // which on Windows lacks GENERIC_WRITE so `set_len` is denied.
            let trunc_res = (|| -> std::io::Result<()> {
                let g = OpenOptions::new().write(true).open(&path)?;
                g.set_len(pos)
            })();
            if let Err(e) = trunc_res {
                return Err(wal_io_error(format!("WalSegment repair set_len {path:?}"), e));
            }
            Ok(Some(pos))
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;
        // If we truncated, keep `bytes_written` consistent with the on-disk
        // length so future appends / rotation see the repaired size.
        if let Some(new_len) = repair_offset {
            self.bytes_written.store(new_len, Ordering::Release);
            log::warn!(
                "WalSegment {:?} self-heal: truncated torn tail back to {} bytes",
                self.path,
                new_len
            );
        }
        Ok(())
    }

    /// Replay directly from a path using this shared corruption/open policy.
    pub(crate) async fn replay_path(
        path: PathBuf,
        sealed: bool,
        tolerate_permission_denied: bool,
        fail_open: bool,
    ) -> DbResult<Vec<WalEntryV2>> {
        Self::replay_inner(path, sealed, tolerate_permission_denied, fail_open, false).await
    }

    /// Replay the segment from the start. Active incomplete tails are tolerated.
    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        #[cfg(test)]
        let fail_open = self
            .fail_startup_open_deny
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n != 0).then(|| n - 1)
            })
            .is_ok();
        #[cfg(not(test))]
        let fail_open = false;
        Self::replay_path(self.path.clone(), false, true, fail_open).await
    }

    /// Replay a SEALED segment, treating a CRC mismatch as a LOUD recovery
    /// error (audit §1.8). A sealed segment is fully fsync'd before sealing
    /// (invariant I4), so a torn tail is impossible by construction — a CRC
    /// mismatch mid-segment means on-disk corruption, not a crash tail.
    /// Silently discarding the valid tail after a corrupt frame (the old
    /// `replay()` behaviour) would hide durable records from recovery; worse,
    /// the frame format `[len][payload][crc]` has no magic/seq, so
    /// resynchronization after a single corrupt frame is impossible. An
    /// operator must decide (restore from backup, run doctor, etc.) — this
    /// is NOT a silent skip.
    ///
    /// `NotFound` and a concurrent-truncation `PermissionDenied` are tolerated;
    /// this policy is for live replay callers, not startup recovery.
    pub async fn replay_sealed(&self) -> DbResult<Vec<WalEntryV2>> {
        #[cfg(test)]
        let fail_open = self
            .fail_startup_open_deny
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n != 0).then(|| n - 1)
            })
            .is_ok();
        #[cfg(not(test))]
        let fail_open = false;
        Self::replay_path(self.path.clone(), true, true, fail_open).await
    }

    /// Replay a segment at STARTUP / open, where `PermissionDenied` must be
    /// a HARD error (audit §2.4). At startup there is no concurrent
    /// `truncate_below`, so a `PermissionDenied` means a real ACL denial or
    /// a file held by an antivirus/backup process — silently treating it as
    /// an empty WAL would skip durable records. `NotFound` is still tolerated
    /// (a sealed segment may have been truncated by a prior clean shutdown).
    pub async fn replay_at_startup(&self) -> DbResult<Vec<WalEntryV2>> {
        #[cfg(test)]
        let fail_open = self
            .fail_startup_open_deny
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n != 0).then(|| n - 1)
            })
            .is_ok();
        #[cfg(not(test))]
        let fail_open = false;
        #[cfg(test)]
        let fail_read = self.fail_next_replay_read.swap(false, Ordering::AcqRel);
        #[cfg(not(test))]
        let fail_read = false;
        Self::replay_inner(self.path.clone(), false, false, fail_open, fail_read).await
    }

    /// Like [`replay_sealed`](Self::replay_sealed) but for the startup path:
    /// `PermissionDenied` is a hard error (audit §2.4), and a CRC mismatch in
    /// a sealed segment is a loud corruption error (audit §1.8).
    pub async fn replay_sealed_at_startup(&self) -> DbResult<Vec<WalEntryV2>> {
        #[cfg(test)]
        let fail_open = self
            .fail_startup_open_deny
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n != 0).then(|| n - 1)
            })
            .is_ok();
        #[cfg(not(test))]
        let fail_open = false;
        Self::replay_path(self.path.clone(), true, false, fail_open).await
    }

    /// Shared replay core.
    ///
    /// - `sealed == true`: a CRC mismatch or incomplete frame is a loud `Err`;
    ///   active incomplete tails remain silently ignored.
    /// - `tolerate_permission_denied == true`: transient denial returns empty;
    ///   startup uses `false`, retries boundedly, then reports persistent denial.
    async fn replay_inner(
        path: PathBuf,
        sealed: bool,
        tolerate_permission_denied: bool,
        fail_open: bool,
        fail_read: bool,
    ) -> DbResult<Vec<WalEntryV2>> {
        spawn_blocking(move || -> DbResult<Vec<WalEntryV2>> {
            #[cfg(not(test))]
            let _ = fail_open;
            let mut attempt = 0u32;
            let f = loop {
                #[cfg(test)]
                let opened = if fail_open {
                    Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                } else {
                    File::open(&path)
                };
                #[cfg(not(test))]
                let opened = File::open(&path);
                match opened {
                    Ok(f) => break f,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Vec::new());
                    }
                    Err(e) if is_transient_open_denial(&e) => {
                        if tolerate_permission_denied {
                            return Ok(Vec::new());
                        }
                        // A just-written segment can be held for a few ms by an
                        // antivirus / backup scanner or a delete-pending handle
                        // on Windows: retry a bounded number of times before
                        // treating the denial as real.
                        attempt += 1;
                        if attempt < OPEN_DENIED_ATTEMPTS {
                            std::thread::sleep(OPEN_DENIED_BACKOFF);
                            continue;
                        }
                        // Audit §2.4: at startup there is no concurrent
                        // truncation, so PermissionDenied is a real ACL denial
                        // or an antivirus/backup-held file — NOT an empty WAL.
                        return Err(wal_io_error(
                            format!("WalSegment replay open {path:?}: PermissionDenied (startup — no concurrent truncation expected; likely ACL denial or file held by another process)"),
                            e,
                        ));
                    }
                    Err(e) => {
                        return Err(wal_io_error(format!("WalSegment replay open {path:?}"), e));
                    }
                }
            };
            #[cfg(test)]
            if fail_read {
                return Err(wal_io_error(
                    format!("WalSegment replay read {path:?}"),
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected WAL replay read failure",
                    ),
                ));
            }
            #[cfg(not(test))]
            let _ = fail_read;
            let length = f
                .metadata()
                .map_err(|e| wal_io_error(format!("WalSegment replay metadata {path:?}"), e))?
                .len();
            let mut reader =
                FrameReader::new(f, length, format!("WalSegment replay read {path:?}"));
            let mut out = Vec::new();
            loop {
                match reader.next()? {
                    FrameRead::End => break,
                    FrameRead::Incomplete { offset, header } => {
                        if sealed {
                            let part = if header { "frame header" } else { "frame" };
                            return Err(DbError::Storage(format!(
                                "WalSegment replay (sealed) {path:?}: incomplete {part} at byte offset {offset}"
                            )));
                        }
                        break;
                    }
                    FrameRead::Frame {
                        offset,
                        payload,
                        crc_ok,
                        ..
                    } => {
                        if !crc_ok {
                            if sealed {
                                return Err(DbError::Storage(format!(
                                    "WalSegment replay (sealed) {path:?}: CRC mismatch at byte offset {offset}"
                                )));
                            }
                            log::warn!(
                                "WalSegment replay: CRC mismatch at byte offset {offset}; discarding remaining data"
                            );
                            break;
                        }
                        out.push(WalEntryV2::decode(&payload)?);
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))?
    }
}
