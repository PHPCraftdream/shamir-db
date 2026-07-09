//! Append-only, file-backed WAL segment.
//!
//! The existing [`crate::WalManager`] is KV-backed — it writes markers
//! into a `Store` and inherits whatever durability the backend chooses
//! (level 1: in-process buffer, or level 3: write + fsync). It cannot
//! express level 2 (data in the OS page cache, surviving a *process*
//! crash but lost on power loss).
//!
//! `WalSegment` is the real append-only file that separates those two
//! tiers: [`append_batch`](WalSegment::append_batch) does `write()` +
//! userspace flush (level 2), while [`sync`](WalSegment::sync) does
//! `fsync` (level 3). This split is the foundation of the durability
//! contract (see `docs/perf/durability-model.md`, "Реализация B").
//!
//! Live production primitive: composed into a [`crate::SegmentSet`]
//! that the repo's [`WalSink::File`]` drives via
//! [`WalGroupCommit`](crate::WalGroupCommit). A `Mem` sink variant
//! mirrors the same interface for in-memory repos / tests.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use shamir_storage::error::{DbError, DbResult};
use tokio::task::spawn_blocking;

use crate::wal_entry_v2::WalEntryV2;

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
/// Single-writer BY CONSTRUCTION: the sole appender is the WAL group-commit
/// coordinator's drain path (one window at a time — see
/// `wal_group_commit.rs`). The `Arc<Mutex<File>>` is therefore UNCONTENDED on
/// the hot path; it is retained, not removed, on purpose. (a) it is held
/// ONLY on the blocking thread inside `spawn_blocking` (`append_batch` /
/// `sync`), NEVER across an `.await`. (b) the `Arc` is mandatory regardless
/// of the lock — `spawn_blocking` needs a `'static` handle, so the file must
/// be reference-counted to cross the closure boundary. (c) MEASURED
/// non-bottleneck: the WAL-append contention bench (`benches/wal_append.rs`,
/// baseline `2e3bd51`) shows file-sink throughput SCALING with concurrency
/// (7.2K→80.2K appends/s, N=1→64) exactly like the lockless mem sink — a
/// lock-bound path would flatten. fsync dominates a durable append ~63×; the
/// marginal ~10µs/append is `spawn_blocking` + `Notify`, not this sub-µs lock.
///
/// A single-writer-task rewrite (CAPSTONE, `docs/perf/capstone-subplan.md`)
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
#[allow(dead_code)]
pub struct WalSegment {
    path: PathBuf,
    file: Arc<Mutex<File>>,
    next_seq: AtomicU64,
    /// Highest `commit_version` ever appended to this segment
    /// (`fetch_max` on every `append_batch`). Drives segment-level
    /// truncation: a sealed segment whose `max_committed <= durable_watermark`
    /// is wholly durable in history and may be deleted (F6). Init 0.
    max_committed: AtomicU64,
    /// Running count of bytes written through `append_batch` (frame bytes
    /// only). Cheap rotation trigger — avoids a `metadata()` syscall on every
    /// append. Tracks exactly what we wrote; matches the on-disk size for a
    /// freshly created segment (does not account for a pre-existing file's
    /// bytes, which is fine — rotation only needs a monotonic growth signal).
    bytes_written: AtomicU64,
    /// Quarantine flag — set when a `write_all` or `sync_all` on this segment
    /// failed (audit §1.3). Once poisoned, the segment refuses further appends
    /// so the leader rotates to a brand-new file instead of stranding later
    /// acked commits behind a torn tail. A poisoned segment is never un-poisoned
    /// in-process — recovery on next open rebuilds from disk state.
    poisoned: AtomicBool,
}

#[allow(dead_code)]
impl WalSegment {
    /// Open (creating if absent) an append-only segment at `path`.
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
                .map_err(|e| DbError::Storage(format!("WalSegment open {p:?}: {e}")))?;
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
                .map_err(|e| DbError::Storage(format!("WalSegment open metadata {p:?}: {e}")))?
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
            bytes_written: AtomicU64::new(existing_len),
            poisoned: AtomicBool::new(false),
        })
    }

    /// Append a batch of encoded payloads in ONE blocking round-trip,
    /// flushing to the OS (level 2) but NOT fsync'ing. Returns the seq
    /// assigned to the LAST entry. Batching amortises the spawn_blocking
    /// hop across all concurrent committers funnelled by the leader.
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
        if payloads.is_empty() {
            // Still fold the version in — an empty batch carrying a watermark
            // must not regress max_committed (monotonic, fetch_max).
            self.max_committed.fetch_max(max_version, Ordering::AcqRel);
            return Ok(self.next_seq.load(Ordering::Acquire));
        }
        let n = payloads.len() as u64;
        let last_seq = self.next_seq.fetch_add(n, Ordering::AcqRel) + n - 1;
        self.max_committed.fetch_max(max_version, Ordering::AcqRel);
        // The last-known-good byte offset BEFORE this batch — the boundary to
        // roll back to if `write_all` fails partway through `buf`. Tracked
        // incrementally by this struct; read here (not under the file lock) so
        // the truncation target is a known frame boundary from a prior
        // successful append.
        let pre_batch_offset = self.bytes_written.load(Ordering::Acquire);
        let file = Arc::clone(&self.file);
        let path = self.path.clone();
        let frame_bytes = spawn_blocking(move || -> DbResult<u64> {
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
            let mut f = file.lock().expect("WalSegment file mutex poisoned");
            match f.write_all(&buf) {
                Ok(()) => Ok(buf.len() as u64),
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
                    drop(f);
                    let trunc_res = (|| -> std::io::Result<()> {
                        let g = OpenOptions::new().write(true).open(&path)?;
                        g.set_len(pre_batch_offset)
                    })();
                    if let Err(trunc_err) = trunc_res {
                        log::error!(
                            "WalSegment {path:?}: write_all failed ({e}) AND rollback \
                             set_len({pre_batch_offset}) failed ({trunc_err}) — segment is \
                             in an unknown state; it has been poisoned"
                        );
                    }
                    Err(DbError::Storage(format!(
                        "WalSegment append to {path:?}: {e}"
                    )))
                }
            }
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))?;
        match frame_bytes {
            Ok(n) => {
                self.bytes_written.fetch_add(n, Ordering::AcqRel);
                Ok(last_seq)
            }
            Err(e) => {
                // Quarantine: no future append may touch this file. The leader
                // reacts by rotating to a fresh segment.
                self.poisoned.store(true, Ordering::Release);
                log::error!(
                    "WalSegment {:?} poisoned after failed append: {} — \
                     leader must rotate to a new segment; further appends refused",
                    self.path,
                    e
                );
                Err(e)
            }
        }
    }

    /// Highest `commit_version` ever appended to this segment.
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
        let res = spawn_blocking(move || -> DbResult<()> {
            let f = file.lock().expect("WalSegment file mutex poisoned");
            f.sync_all()
                .map_err(|e| DbError::Storage(format!("WalSegment sync: {e}")))
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))?;
        if let Err(ref e) = res {
            self.poisoned.store(true, Ordering::Release);
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

    /// Mark this segment poisoned. Production code does this internally on a
    /// write/fsync failure; exposed (pub(crate)-ish) for tests that simulate
    /// a fault by setting the flag directly.
    pub fn mark_poisoned(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    /// Repair a torn trailing frame left in the file by a partial write that
    /// could not be rolled back at the time (e.g. a process crash mid-write, or
    /// a fault-injection test that manually appends a torn tail). Walks the
    /// file's frames in order; on the first torn or CRC-mismatched frame it
    /// truncates the file back to the byte offset just before that frame. A
    /// file with no torn tail is a no-op. Idempotent.
    ///
    /// This is the **self-heal** path (audit §1.3): it removes the torn tail
    /// so a subsequent `append_batch` window is not stranded behind it on
    /// replay. Called by `SegmentSet::open` after reopening an existing
    /// segment so the file is in a known-good state before the leader appends.
    /// Distinct from the **in-process rollback** inside `append_batch` (which
    /// runs synchronously when a `write_all` on a freshly-opened segment
    /// fails) — that path handles the case where the segment is already open
    /// and the failed write is on the same fd; this path handles the case
    /// where the torn tail was left by a prior process / write attempt.
    pub async fn repair_torn_tail(&self) -> DbResult<()> {
        // Read the file under the existing append-mode handle to find the
        // repair point. Truncation is done via a fresh write-mode handle below
        // (Windows: an append-mode handle lacks the GENERIC_WRITE needed for
        // SetEndOfFile; opening with `.write(true)` is portable).
        let path = self.path.clone();
        let file = Arc::clone(&self.file);
        let repair_offset: Option<u64> = spawn_blocking(move || -> DbResult<Option<u64>> {
            let mut buf = Vec::new();
            {
                let mut f = file.lock().expect("WalSegment file mutex poisoned");
                f.read_to_end(&mut buf)
                    .map_err(|e| DbError::Storage(format!("WalSegment repair read: {e}")))?;
            }
            let mut pos = 0usize;
            while pos + 4 <= buf.len() {
                let len = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
                    as usize;
                let frame_end = pos + 4 + len + 4;
                if frame_end > buf.len() {
                    break; // torn tail — repair point found
                }
                let payload = &buf[pos + 4..pos + 4 + len];
                let crc_stored = u32::from_le_bytes([
                    buf[pos + 4 + len],
                    buf[pos + 5 + len],
                    buf[pos + 6 + len],
                    buf[pos + 7 + len],
                ]);
                if crc32fast::hash(payload) != crc_stored {
                    break; // corrupt frame — repair point found
                }
                pos = frame_end;
            }
            if pos == buf.len() {
                return Ok(None); // file is already clean — no-op
            }
            // Truncate back to the last good frame boundary. Open a fresh
            // write-mode handle: the segment's own handle is append-only,
            // which on Windows lacks GENERIC_WRITE so `set_len` is denied.
            let trunc_res = (|| -> std::io::Result<()> {
                let g = OpenOptions::new().write(true).open(&path)?;
                g.set_len(pos as u64)
            })();
            if let Err(e) = trunc_res {
                return Err(DbError::Storage(format!(
                    "WalSegment repair set_len {path:?}: {e}"
                )));
            }
            Ok(Some(pos as u64))
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

    /// Replay the segment from the start, returning every intact entry.
    /// Stops at the first torn / corrupt frame (a crash tail) — that is
    /// NOT an error: a partial trailing write is discarded.
    ///
    /// A missing file (`NotFound`) AND a Windows delete-pending file
    /// (`PermissionDenied` / OS error 5) both return `Ok(Vec::new())`:
    /// this method is called from `SegmentSet::replay` over a sealed-list
    /// SNAPSHOT, and a concurrent `truncate_below` can unlink one of the
    /// snapshot's paths between the snapshot capture and our open here.
    /// That truncation only happens for segments whose `max_version`
    /// reached the durable watermark — i.e. every entry of that segment is
    /// already durable in `history`. Skipping the file on replay therefore
    /// loses no data: the surviving segments cover the still-undurable
    /// tail, and the deleted one's data lives in history.
    ///
    /// On Windows a freshly-unlinked file enters "delete pending" state
    /// while any handle (including the truncator's own `remove_file`
    /// in-flight) is still being released; new opens against its path
    /// return `ERROR_ACCESS_DENIED` (os error 5) instead of `NotFound`,
    /// so the two error kinds must be treated symmetrically here. This
    /// mirrors the symmetric tolerance in `SegmentSet::truncate_below`
    /// (which counts a `PermissionDenied` unlink as reclaimed because a
    /// concurrent replay may still hold the file open).
    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        self.replay_inner(false, true).await
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
    /// `NotFound` / `PermissionDenied` are still tolerated (the concurrent-
    /// truncate race described on `replay()` applies equally to sealed
    /// segments).
    pub async fn replay_sealed(&self) -> DbResult<Vec<WalEntryV2>> {
        self.replay_inner(true, true).await
    }

    /// Replay a segment at STARTUP / open, where `PermissionDenied` must be
    /// a HARD error (audit §2.4). At startup there is no concurrent
    /// `truncate_below`, so a `PermissionDenied` means a real ACL denial or
    /// a file held by an antivirus/backup process — silently treating it as
    /// an empty WAL would skip durable records. `NotFound` is still tolerated
    /// (a sealed segment may have been truncated by a prior clean shutdown).
    pub async fn replay_at_startup(&self) -> DbResult<Vec<WalEntryV2>> {
        self.replay_inner(false, false).await
    }

    /// Like [`replay_sealed`](Self::replay_sealed) but for the startup path:
    /// `PermissionDenied` is a hard error (audit §2.4), and a CRC mismatch in
    /// a sealed segment is a loud corruption error (audit §1.8).
    pub async fn replay_sealed_at_startup(&self) -> DbResult<Vec<WalEntryV2>> {
        self.replay_inner(true, false).await
    }

    /// Shared replay core.
    ///
    /// - `sealed == true`: a CRC mismatch is a loud `Err` (corruption in a
    ///   fully-fsync'd sealed segment — audit §1.8), not a silent warn+break.
    ///   A torn TRAILING frame is still a silent break for both (crash tail).
    /// - `tolerate_permission_denied == true`: `PermissionDenied` returns
    ///   `Ok(vec![])` (the Windows delete-pending race during a concurrent
    ///   `truncate_below`). When `false` (startup/open), `PermissionDenied`
    ///   is a hard error (audit §2.4) — a real ACL denial or antivirus-held
    ///   file must not silently become "empty WAL".
    async fn replay_inner(
        &self,
        sealed: bool,
        tolerate_permission_denied: bool,
    ) -> DbResult<Vec<WalEntryV2>> {
        let path = self.path.clone();
        spawn_blocking(move || -> DbResult<Vec<WalEntryV2>> {
            let mut f = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Vec::new());
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    if tolerate_permission_denied {
                        return Ok(Vec::new());
                    }
                    // Audit §2.4: at startup there is no concurrent
                    // truncation, so PermissionDenied is a real ACL denial
                    // or an antivirus/backup-held file — NOT an empty WAL.
                    return Err(DbError::Storage(format!(
                        "WalSegment replay open {path:?}: PermissionDenied \
                         (startup — no concurrent truncation expected; likely \
                         ACL denial or file held by another process): {e}"
                    )));
                }
                Err(e) => return Err(DbError::Storage(format!("WalSegment replay open: {e}"))),
            };
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .map_err(|e| DbError::Storage(format!("WalSegment replay read: {e}")))?;
            let mut out = Vec::new();
            let mut pos = 0usize;
            while pos + 4 <= buf.len() {
                let len = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
                    as usize;
                let frame_end = pos + 4 + len + 4;
                if frame_end > buf.len() {
                    break; // torn tail — always a silent break (crash tail)
                }
                let payload = &buf[pos + 4..pos + 4 + len];
                let crc_stored = u32::from_le_bytes([
                    buf[pos + 4 + len],
                    buf[pos + 5 + len],
                    buf[pos + 6 + len],
                    buf[pos + 7 + len],
                ]);
                if crc32fast::hash(payload) != crc_stored {
                    if sealed {
                        // Audit §1.8: a sealed segment is fully fsync'd
                        // (I4), so a CRC mismatch is disk corruption, not
                        // a crash tail. Silently discarding the valid tail
                        // hides durable records. The frame format has no
                        // magic/seq for resync, so this is a hard error —
                        // an operator must decide. (Adding magic+seq for
                        // single-frame-skip resync is a deferred follow-up:
                        // it requires a WAL format version bump.)
                        return Err(DbError::Storage(format!(
                            "WalSegment replay (sealed) {:?}: CRC mismatch at byte \
                             offset {pos} — on-disk corruption in a fully-fsync'd \
                             sealed segment; the valid tail after this frame cannot \
                             be trusted (no magic/seq for resync). Operator action \
                             required (restore from backup / run doctor).",
                            path
                        )));
                    }
                    log::warn!(
                        "WalSegment replay: CRC mismatch at byte offset {pos} \
                         (full frame present but payload corrupted); \
                         discarding this frame and all remaining data"
                    );
                    break;
                }
                out.push(WalEntryV2::decode(payload)?);
                pos = frame_end;
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))?
    }
}
