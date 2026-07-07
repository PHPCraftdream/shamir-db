//! Segmented WAL sink — a directory of numbered append-only segments.
//!
//! [`SegmentSet`] composes [`WalSegment`]s: at any moment there is exactly
//! one *active* segment (the append tail) plus zero or more *sealed*
//! segments (closed, replay/delete-only). When the active segment crosses
//! `max_bytes` it is sealed and a fresh active segment with the next seq is
//! rotated in. Truncation deletes whole sealed segments once every record
//! in them is durable in history (`max_version <= durable_watermark`).
//!
//! Files are named `NNNNNNNN.wal` (zero-padded 8-digit seq, lexical =
//! chronological). The active segment is never deleted nor rewritten — the
//! append path and the truncation path therefore never touch the same file
//! (zero writer↔truncator coordination, see `docs/perf/f6-subplan.md` §1).
//!
//! PURELY ADDITIVE (F6a): wired into nothing yet — production still runs a
//! single [`WalSegment`] via `WalSink::File`. F6b cuts `repo_instance` over.

use std::fs::remove_file;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use shamir_storage::error::{DbError, DbResult};
use tokio::task::spawn_blocking;

use crate::wal_entry_v2::WalEntryV2;
use crate::wal_segment::WalSegment;

/// Metadata for one sealed (closed) segment. Its file is fully written
/// (no torn tail) and may be replayed or deleted but never appended.
#[derive(Clone)]
struct SealedMeta {
    seq: u64,
    path: PathBuf,
    /// Highest `commit_version` in this segment — the truncation key. A
    /// sealed segment is reclaimable once `max_version <= durable_watermark`.
    /// `0` means "no versioned record" (legacy / non-versioned) and pins the
    /// segment: it is NEVER truncated by version (I5).
    max_version: u64,
}

/// Mutable state guarded by the single coarse `Mutex`. Sealed-list mutation
/// happens only on rotation (the append leader) and truncation — both rare,
/// never on a per-record hot path.
struct Inner {
    sealed: Vec<SealedMeta>,
    active: Arc<WalSegment>,
    active_seq: u64,
}

/// Segmented WAL sink over a directory of numbered segments.
///
/// Concurrency: the sealed list + active handle live behind one
/// **sanctioned** `std::sync::Mutex` (CLAUDE.md "Banned in hot paths").
/// It guards only O(1) metadata edits (clone the active `Arc`, push/scan a
/// short `Vec`, swap the active handle on rotation) — never an I/O syscall,
/// and is NEVER held across an `.await`: every `WalSegment` call takes a
/// cloned `Arc` after the lock is dropped. The single-writer model is the
/// group-commit leader (the sole appender) plus the truncator (rare); they
/// do not contend on a hot path. CAPSTONE replaces this with a
/// single-writer task.
pub struct SegmentSet {
    dir: PathBuf,
    max_bytes: u64,
    inner: Mutex<Inner>,
}

/// Format a segment file name from its seq: `NNNNNNNN.wal` (zero-padded 8).
fn seg_file_name(seq: u64) -> String {
    format!("{seq:08}.wal")
}

/// Parse a segment seq from a `NNNNNNNN.wal` file stem; `None` if the name
/// is not a numeric `.wal` segment.
fn parse_seg_seq(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".wal")?;
    stem.parse::<u64>().ok()
}

impl SegmentSet {
    /// Open (creating if absent) the segment directory `dir`. `max_bytes`
    /// is the seal/rotate threshold (F6b passes
    /// `shamir_tunables::instance_defaults::WAL_SEGMENT_MAX_BYTES`).
    ///
    /// Scans `*.wal`, parses each seq, sorts ascending; the highest seq is
    /// the active segment, the rest are sealed. Each sealed segment's
    /// `max_version` is computed once here via a one-shot `replay` (a rare
    /// open-time cost). An empty directory gets a fresh active `00000000.wal`.
    pub async fn open(dir: PathBuf, max_bytes: u64) -> DbResult<Self> {
        let scan_dir = dir.clone();
        let mut seqs: Vec<u64> = spawn_blocking(move || -> DbResult<Vec<u64>> {
            std::fs::create_dir_all(&scan_dir).map_err(|e| {
                DbError::Storage(format!("SegmentSet create_dir {scan_dir:?}: {e}"))
            })?;
            let mut out = Vec::new();
            for entry in std::fs::read_dir(&scan_dir)
                .map_err(|e| DbError::Storage(format!("SegmentSet read_dir {scan_dir:?}: {e}")))?
            {
                let entry =
                    entry.map_err(|e| DbError::Storage(format!("SegmentSet dir entry: {e}")))?;
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(seq) = parse_seg_seq(name) {
                        out.push(seq);
                    }
                }
            }
            out.sort_unstable();
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;

        if seqs.is_empty() {
            // Fresh directory — create the first active segment.
            let active_seq = 0u64;
            let active_path = dir.join(seg_file_name(active_seq));
            let active = Arc::new(WalSegment::open(active_path).await?);
            return Ok(Self {
                dir,
                max_bytes,
                inner: Mutex::new(Inner {
                    sealed: Vec::new(),
                    active,
                    active_seq,
                }),
            });
        }

        // Highest seq = active; everything below = sealed.
        let active_seq = seqs.pop().expect("non-empty after is_empty check");
        let mut sealed = Vec::with_capacity(seqs.len());
        for seq in seqs {
            let path = dir.join(seg_file_name(seq));
            // Compute max_version once via a one-shot replay (rare, open-time).
            let seg = WalSegment::open(path.clone()).await?;
            let entries = seg.replay().await?;
            let max_version = entries.iter().map(|e| e.commit_version).max().unwrap_or(0);
            sealed.push(SealedMeta {
                seq,
                path,
                max_version,
            });
        }
        let active_path = dir.join(seg_file_name(active_seq));
        let active = Arc::new(WalSegment::open(active_path).await?);
        // Self-heal the active segment (audit §1.3): a prior process crash
        // mid-write (or a partial write that could not be rolled back) may
        // have left a torn tail. Sealed segments are fsync'd before sealing
        // (I4) so a torn tail is impossible there; the active segment is the
        // only one that can have one. Removing it now keeps the next
        // `append_batch` window from being stranded behind it on replay.
        active.repair_torn_tail().await?;

        Ok(Self {
            dir,
            max_bytes,
            inner: Mutex::new(Inner {
                sealed,
                active,
                active_seq,
            }),
        })
    }

    /// Append a batch to the active segment, then rotates if the active
    /// segment crossed `max_bytes`. Rotation is checked AFTER the whole
    /// batch is written so a batch never straddles two files (simplifies
    /// torn-tail handling and max_version accounting — see §4). Returns the
    /// seq assigned to the last entry in the active segment.
    ///
    /// **Poison / fault recovery (audit §1.3):** if the active segment is
    /// already poisoned (a prior write/sync failure) or the append fails
    /// mid-flight, the segment is sealed-as-poisoned and a brand-new active
    /// segment is rotated in — then the batch is retried ONCE on the fresh
    /// file. This breaks the "next leader writes to the same poisoned
    /// segment" bug: instead of retrying on the known-bad fd, we cut over to
    /// a fresh file. A second failure (the new segment also fails) is
    /// surfaced to the caller — we do not loop indefinitely.
    pub async fn append_batch(&self, payloads: Vec<Vec<u8>>, max_version: u64) -> DbResult<u64> {
        // Fast path: clone the active Arc under the lock, then release before awaiting.
        let mut active = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            Arc::clone(&g.active)
        };
        // If the active segment is already poisoned, rotate BEFORE attempting
        // the append (writing to a poisoned segment always fails), and re-fetch
        // the freshly-rotated segment so the append below doesn't immediately
        // fail again on the stale (poisoned) Arc.
        if active.is_poisoned() {
            self.rotate_after_poison().await?;
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            active = Arc::clone(&g.active);
        }
        // `payloads` is wrapped in an `Arc` ONCE here — the retry-after-poison
        // path (rare: only on a write/sync failure) then shares the SAME
        // allocation via a cheap refcount clone, instead of a full deep copy
        // of the batch on every append (the common, non-retry case pays only
        // one Arc::new, no data copy at all).
        let payloads = Arc::new(payloads);
        match active
            .append_batch(Arc::clone(&payloads), max_version)
            .await
        {
            Ok(last_seq) => {
                // Rotate iff this segment crossed the threshold. Read len
                // without holding the lock across the await above.
                if active.approx_len_bytes() >= self.max_bytes {
                    self.seal_and_rotate().await?;
                }
                Ok(last_seq)
            }
            Err(e) => {
                // The append failed (write error → the segment poisoned
                // itself inside WalSegment::append_batch, OR the segment was
                // poisoned by a prior sync failure between our is_poisoned
                // check and our call). Rotate to a fresh segment and retry
                // the batch ONCE so the leader is not stuck on a dead fd.
                log::error!(
                    "SegmentSet append failed on active segment; rotating and \
                     retrying once. Original error: {e}"
                );
                self.rotate_after_poison().await?;
                let active = {
                    let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
                    Arc::clone(&g.active)
                };
                let last_seq = active.append_batch(payloads, max_version).await?;
                // The freshly-rotated segment is brand new and small — no
                // size-based rotation check needed here.
                Ok(last_seq)
            }
        }
    }

    /// Force a rotation because the current active segment is poisoned (a
    /// write/sync failure quarantined it — audit §1.3). Unlike
    /// `seal_and_rotate`, this does NOT fsync the segment being sealed (it is
    /// known-bad; an fsync would either fail again or falsely "succeed"
    /// without persisting the in-flight window) and does NOT require it to be
    /// over the size threshold. The poisoned segment is recorded in the
    /// sealed list with its current `max_version` so it remains replayable
    /// (its intact prefix is still valid data) and is reclaimable by
    /// truncation once durable in history — it simply never accepts new
    /// appends. A subsequent power-loss replays its intact prefix; the torn
    /// tail, if any, was already truncated by `WalSegment::append_batch`'s
    /// rollback (or will be healed by `repair_torn_tail` on next open).
    async fn rotate_after_poison(&self) -> DbResult<()> {
        let (sealed_seq, sealed_max, next_seq) = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            (g.active_seq, g.active.max_committed(), g.active_seq + 1)
        };
        let sealed_path = self.dir.join(seg_file_name(sealed_seq));
        let new_active = Arc::new(WalSegment::open(self.dir.join(seg_file_name(next_seq))).await?);
        let mut g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
        // Re-check: another rotation may have advanced active past us.
        if g.active_seq != sealed_seq {
            return Ok(()); // someone else already rotated
        }
        g.sealed.push(SealedMeta {
            seq: sealed_seq,
            path: sealed_path,
            max_version: sealed_max,
        });
        g.active = new_active;
        g.active_seq = next_seq;
        Ok(())
    }

    /// Seal the current active segment (record its metadata in the sealed
    /// list) and open a fresh active segment with `active_seq + 1`.
    ///
    /// The new segment is opened OUTSIDE the lock (it does I/O); the lock is
    /// re-acquired only to swap the handles. If a concurrent caller already
    /// rotated past `active_seq` while we awaited the open, we abandon our
    /// freshly-opened segment rather than clobber the newer active (the
    /// single-writer leader model means this is not expected, but the guard
    /// keeps the invariant exact).
    async fn seal_and_rotate(&self) -> DbResult<()> {
        // Snapshot the segment to seal under the lock.
        let (sealed_seq, sealed_path, sealed_max, next_seq, sealing) = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            // Guard against double-rotation: only the leader that still sees
            // an over-threshold active rotates it.
            if g.active.approx_len_bytes() < self.max_bytes {
                return Ok(());
            }
            (
                g.active_seq,
                self.dir.join(seg_file_name(g.active_seq)),
                g.active.max_committed(),
                g.active_seq + 1,
                Arc::clone(&g.active),
            )
        };

        // Make the segment being sealed durable on disk BEFORE it leaves the
        // active slot (I4: a sealed segment is fully written — including
        // fsync'd — so a later torn tail can only occur on the active segment,
        // and `SegmentSet::sync` need only fsync the active one). Done outside
        // the lock (it is an fsync syscall). This also underpins truncation
        // crash-safety (F6c): a sealed segment's records are crash-durable, so
        // a power-loss before they are drained to `history` still replays them.
        sealing.sync().await?;

        let new_active = Arc::new(WalSegment::open(self.dir.join(seg_file_name(next_seq))).await?);

        let mut g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
        // Re-check: another rotation may have advanced active past us.
        if g.active_seq != sealed_seq {
            // Already rotated — drop our freshly-opened (empty) segment.
            return Ok(());
        }
        g.sealed.push(SealedMeta {
            seq: sealed_seq,
            path: sealed_path,
            max_version: sealed_max,
        });
        g.active = new_active;
        g.active_seq = next_seq;
        Ok(())
    }

    /// Replay every segment in seq order: all sealed (ascending) then the
    /// active segment last. Byte-identical to a single segment holding the
    /// same data in the same append order.
    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        // Snapshot sealed metadata + active Arc under the lock; do I/O after.
        let (mut sealed, active) = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            (g.sealed.clone(), Arc::clone(&g.active))
        };
        // Replay sealed strictly by seq (chronological). They are pushed in
        // ascending order at seal time, but sorting makes the seq-order
        // invariant explicit and independent of insertion order.
        sealed.sort_unstable_by_key(|m| m.seq);
        let mut out = Vec::new();
        for meta in &sealed {
            let seg = WalSegment::open(meta.path.clone()).await?;
            out.extend(seg.replay().await?);
        }
        out.extend(active.replay().await?);
        Ok(out)
    }

    /// Delete every sealed segment wholly durable in history — i.e. whose
    /// `max_version` is in `(0, durable_version]`. Returns the count deleted.
    ///
    /// The active segment is NEVER touched (I3). A sealed segment with
    /// `max_version == 0` is a pin (legacy / non-versioned records, I5) and
    /// is never deleted by version. Each `remove_file` is an atomic FS op;
    /// replay of the survivors after truncation is idempotent (I6).
    pub async fn truncate_below(&self, durable_version: u64) -> DbResult<usize> {
        // CLAIM-then-delete: under the lock, collect the deletable sealed
        // segments AND remove them from the sealed list in the SAME critical
        // section. This makes truncation safe against a CONCURRENT truncator
        // (the background drainer's interval pass racing an explicit
        // `drain_all`): a second caller no longer sees these entries in
        // `sealed`, so it never targets the same files — no double-`remove_file`
        // race (which on Windows surfaces as `PermissionDenied`, not
        // `NotFound`). The unlink syscall then runs OUTSIDE the lock. If an
        // unlink fails, the file lingers on disk but is no longer tracked;
        // `SegmentSet::open` re-scans the directory on reopen and replays it
        // idempotently (I6), so a leaked file is harmless, never a data loss.
        let to_delete: Vec<PathBuf> = {
            let mut g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            let mut claimed = Vec::new();
            g.sealed.retain(|m| {
                if m.max_version > 0 && m.max_version <= durable_version {
                    claimed.push(m.path.clone());
                    false // claimed for deletion — drop from sealed now
                } else {
                    true
                }
            });
            claimed
        };
        if to_delete.is_empty() {
            return Ok(0);
        }

        let paths = to_delete;
        let deleted = spawn_blocking(move || -> DbResult<usize> {
            let mut n = 0usize;
            for p in &paths {
                match remove_file(p) {
                    Ok(()) => n += 1,
                    // Already gone (idempotent re-truncate) — count as done.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => n += 1,
                    // Windows: a concurrent `replay()` on another drainer may
                    // still hold this sealed segment open for reading, so the
                    // unlink returns `PermissionDenied` ("Access is denied",
                    // os error 5) instead of succeeding. The entry is ALREADY
                    // claimed (removed from `sealed` above), so we do not retry
                    // or fail: the file lingers on disk but is untracked, and
                    // `SegmentSet::open` re-scans the directory on reopen and
                    // replays it idempotently (I6) — never a data loss. Count it
                    // as reclaimed (it will not be re-targeted).
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        n += 1;
                    }
                    Err(e) => {
                        return Err(DbError::Storage(format!(
                            "SegmentSet truncate remove {p:?}: {e}"
                        )))
                    }
                }

                // F6c crash seam — `wal_mid_delete`. This crate cannot reach
                // the engine's `maybe_crash` (no dependency edge), so the seam
                // is self-contained: debug-only, env-gated, and a bare
                // `process::abort()` AFTER the first successful unlink but
                // BEFORE the rest. Why a bare abort is the correct mid-delete
                // crash: by the drainer's I2 ordering, `flush_all_history` ran
                // (and fsync'd) `history` up to the truncation watermark BEFORE
                // calling `truncate_below`, so every record in every segment we
                // are unlinking is already durable in `history`. Killing the
                // process here leaves a torn delete-set — some segments gone,
                // some surviving — yet zero data loss: `SegmentSet::open` on
                // reopen picks up whatever survived and replay re-materializes
                // it idempotently (the deleted segments' data is in `history`).
                // Needs >= 2 truncatable segments to exercise (the abort fires
                // after #1, before #2).
                #[cfg(debug_assertions)]
                {
                    if std::env::var("SHAMIR_TEST_CRASH_AFTER").as_deref() == Ok("wal_mid_delete") {
                        std::process::abort();
                    }
                }
            }
            Ok(n)
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;

        // The claimed entries were already removed from `sealed` above (under
        // the lock), so there is nothing more to retract here.
        Ok(deleted)
    }

    /// Cheap probe (under the lock, no I/O): is any sealed segment
    /// reclaimable at `durable_version` — i.e. has `0 < max_version <=
    /// durable_version`? The drainer gates the (more expensive)
    /// history-flush + `truncate_below` on this so truncation work fires
    /// only on a segment boundary, never per-commit (I2). The active
    /// segment is never considered (I3); a `max_version == 0` segment is a
    /// pin and never reclaimable (I5).
    pub fn has_truncatable(&self, durable_version: u64) -> bool {
        let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
        g.sealed
            .iter()
            .any(|m| m.max_version > 0 && m.max_version <= durable_version)
    }

    /// fsync the active segment (sealed segments are already fully on disk).
    pub async fn sync(&self) -> DbResult<()> {
        let active = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            Arc::clone(&g.active)
        };
        active.sync().await
    }

    /// Highest `commit_version` across all segments (sealed + active) — for
    /// parity with [`WalSegment::max_committed`].
    pub fn max_committed(&self) -> u64 {
        let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
        let sealed_max = g.sealed.iter().map(|m| m.max_version).max().unwrap_or(0);
        sealed_max.max(g.active.max_committed())
    }
}
