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
#[allow(dead_code)]
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

#[allow(dead_code)]
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

    /// Append a batch to the active segment, then rotate if the active
    /// segment crossed `max_bytes`. Rotation is checked AFTER the whole
    /// batch is written so a batch never straddles two files (simplifies
    /// torn-tail handling and max_version accounting — see §4). Returns the
    /// seq assigned to the last entry in the active segment.
    pub async fn append_batch(&self, payloads: Vec<Vec<u8>>, max_version: u64) -> DbResult<u64> {
        // Clone the active Arc under the lock, then release before awaiting.
        let active = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            Arc::clone(&g.active)
        };
        let last_seq = active.append_batch(payloads, max_version).await?;

        // Rotate iff this segment crossed the threshold. Read len without
        // holding the lock across the await above.
        if active.approx_len_bytes() >= self.max_bytes {
            self.seal_and_rotate().await?;
        }
        Ok(last_seq)
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
        let (sealed_seq, sealed_path, sealed_max, next_seq) = {
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
            )
        };

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
        // Collect the deletable sealed segments under the lock, but defer the
        // unlink (a syscall) to a blocking hop without the lock held.
        let to_delete: Vec<PathBuf> = {
            let g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
            g.sealed
                .iter()
                .filter(|m| m.max_version > 0 && m.max_version <= durable_version)
                .map(|m| m.path.clone())
                .collect()
        };
        if to_delete.is_empty() {
            return Ok(0);
        }

        let paths = to_delete.clone();
        let deleted = spawn_blocking(move || -> DbResult<usize> {
            let mut n = 0usize;
            for p in &paths {
                match remove_file(p) {
                    Ok(()) => n += 1,
                    // Already gone (idempotent re-truncate) — count as done.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => n += 1,
                    Err(e) => {
                        return Err(DbError::Storage(format!(
                            "SegmentSet truncate remove {p:?}: {e}"
                        )))
                    }
                }
            }
            Ok(n)
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;

        // Drop the deleted entries from the sealed list (by path — the set
        // we just unlinked).
        let mut g = self.inner.lock().expect("SegmentSet inner mutex poisoned");
        g.sealed.retain(|m| !to_delete.contains(&m.path));
        Ok(deleted)
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
