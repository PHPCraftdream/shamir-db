#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use shamir_storage::error::DbResult;

use crate::segment_set::SegmentSet;
use crate::wal_entry_v2::WalEntryV2;

/// In-RAM WAL sink — mirrors [`WalSegment`]'s interface but stores frames
/// in a `Vec` instead of a file. Used by in-memory repos so the
/// group-commit write path is identical to disk repos (one code path),
/// while giving genuine in-process replay parity: the buffer lives in the
/// same RAM as the repo and survives a *simulated* same-process "crash"
/// exactly as `InMemoryStore` does.
pub struct MemSink {
    // Sanctioned std::sync::Mutex (CLAUDE.md "Banned in hot paths"): this is
    // the in-memory / test sink only — production durability runs on
    // `WalSink::File` (fsync-backed). Appends are serialised by the
    // group-commit leader, so this lock sees a single writer and is NOT a
    // hot production path. Held only for the O(1) push / clone, never across
    // an `.await`.
    //
    // Each frame is paired with its batch `max_version` so the frame-level
    // GC (`truncate_below`, I7) can drop the frames a sealed segment would
    // drop on disk. `0` means "no versioned record" and pins the frame
    // (never truncated by version, mirrors a sealed segment with
    // `max_version == 0`, I5).
    frames: Mutex<Vec<(u64, Vec<u8>)>>,
    next_seq: AtomicU64,
    /// Highest `commit_version` ever appended (monotonic `fetch_max`).
    /// Mirrors [`WalSegment::max_committed`] so the in-RAM sink carries the
    /// same watermark for future frame-level GC (F6 truncation parity, I7).
    max_committed: AtomicU64,
    /// Test-only fault-injection knob (audit §1.6 / task #531). When armed,
    /// the NEXT `append_batch` fails BEFORE any frame is pushed — a genuine
    /// failure of the real write path (not a simulated rollback), so the
    /// group-commit leader's circuit-breaker fires and `append_many` returns
    /// `Err` while ZERO of the batch's frames survive to a later replay. This
    /// exercises the all-or-nothing atomicity claim `append_many`'s doc makes.
    /// Never compiled into a production build (the `File` hot path is
    /// untouched — it stays an enum, no dyn dispatch).
    #[cfg(test)]
    fail_next_append: AtomicBool,
}

impl MemSink {
    fn new() -> Self {
        Self {
            frames: Mutex::new(Vec::new()),
            next_seq: AtomicU64::new(0),
            max_committed: AtomicU64::new(0),
            #[cfg(test)]
            fail_next_append: AtomicBool::new(false),
        }
    }

    /// Highest `commit_version` ever appended to this in-RAM sink.
    pub fn max_committed(&self) -> u64 {
        self.max_committed.load(Ordering::Acquire)
    }

    /// Test-only: arm the next `append_batch` to fail. The failing call
    /// pushes NO frames (returns before the `frames.extend`), so the batch
    /// leaves zero trace — mirroring a `File` sink quarantining a segment on
    /// a partial write. One-shot: the flag clears itself on the failing call.
    #[cfg(test)]
    pub(crate) fn arm_fail_next_append(&self) {
        self.fail_next_append.store(true, Ordering::Release);
    }
}

impl Default for MemSink {
    fn default() -> Self {
        Self::new()
    }
}

/// WAL storage sink — file-backed (disk repos) or in-RAM (in-memory repos).
/// Enum, not trait: no dyn dispatch on the hot path.
pub enum WalSink {
    /// Real append-only directory of numbered segments (F6b). write() =
    /// level 2, sync_all = level 3. Truncation deletes whole sealed
    /// segments once their data is durable in history.
    File(SegmentSet),
    /// In-RAM Vec-backed segment. append = push, sync = no-op (no fsync in
    /// RAM), replay = decode every stored payload.
    Mem(MemSink),
}

impl WalSink {
    /// Construct a fresh in-memory sink.
    pub fn mem() -> Self {
        Self::Mem(MemSink::default())
    }

    /// Test-only (task #531): arm the next `append_batch` on a `Mem` sink to
    /// fail. Panics on a `File` sink (fault injection is a `Mem`-only knob —
    /// production durability runs on `File` and its design is untouched).
    #[cfg(test)]
    pub(crate) fn arm_fail_next_append(&self) {
        match self {
            Self::Mem(m) => m.arm_fail_next_append(),
            Self::File(_) => panic!("arm_fail_next_append is Mem-only"),
        }
    }

    pub async fn append_batch(&self, payloads: Vec<Vec<u8>>, max_version: u64) -> DbResult<u64> {
        match self {
            Self::File(seg) => seg.append_batch(payloads, max_version).await,
            Self::Mem(m) => {
                // Test-only fault injection (task #531): fail the whole batch
                // BEFORE touching `next_seq` / `frames`, so no partial state
                // survives. A genuine failure of the real Mem write path —
                // the leader sees `is_ok() == false`, trips the circuit
                // breaker, and every caller gets `Err`. Nothing is pushed, so
                // a subsequent replay sees ZERO of the batch's entries.
                #[cfg(test)]
                if m.fail_next_append.swap(false, Ordering::AcqRel) {
                    return Err(shamir_storage::error::DbError::Storage(
                        "MemSink injected append_batch failure (task #531)".into(),
                    ));
                }
                m.max_committed.fetch_max(max_version, Ordering::AcqRel);
                if payloads.is_empty() {
                    return Ok(m.next_seq.load(Ordering::Acquire));
                }
                let n = payloads.len() as u64;
                let last_seq = m.next_seq.fetch_add(n, Ordering::AcqRel) + n - 1;
                // CRC / torn-tail handling is unnecessary in RAM — store the
                // payloads verbatim, each tagged with the batch `max_version`
                // so frame-level GC (`truncate_below`, I7) mirrors a sealed
                // segment's per-segment max_version reclaim.
                let mut frames = m.frames.lock().expect("MemSink frames mutex poisoned");
                frames.extend(payloads.into_iter().map(|p| (max_version, p)));
                Ok(last_seq)
            }
        }
    }

    pub async fn sync(&self) -> DbResult<()> {
        match self {
            Self::File(seg) => seg.sync().await,
            Self::Mem(_) => Ok(()),
        }
    }

    /// Highest `commit_version` ever appended through this sink (monotonic).
    /// Drives F6 segment-level / frame-level truncation.
    pub fn max_committed(&self) -> u64 {
        match self {
            Self::File(seg) => seg.max_committed(),
            Self::Mem(m) => m.max_committed(),
        }
    }

    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        match self {
            Self::File(seg) => seg.replay().await,
            Self::Mem(m) => {
                let frames = m.frames.lock().expect("MemSink frames mutex poisoned");
                let mut out = Vec::with_capacity(frames.len());
                for (_version, payload) in frames.iter() {
                    out.push(WalEntryV2::decode(payload)?);
                }
                Ok(out)
            }
        }
    }

    /// Truncate every record fully durable in history — i.e. whose
    /// `commit_version` is in `(0, durable]`. Returns the count reclaimed
    /// (deleted sealed segments for `File`, dropped frames for `Mem`).
    ///
    /// `File` delegates to [`SegmentSet::truncate_below`] (deletes whole
    /// sealed segments; the active segment and any pin are untouched).
    /// `Mem` drops frames whose batch `max_version` is in `(0, durable]`,
    /// keeping any pinned (`v == 0`) frame and any frame above the durable
    /// watermark (I7). fsync-gate (I2) is the caller's responsibility — it
    /// flushes history before invoking this.
    pub async fn truncate_below(&self, durable: u64) -> DbResult<usize> {
        match self {
            Self::File(segset) => segset.truncate_below(durable).await,
            Self::Mem(m) => {
                let mut frames = m.frames.lock().expect("MemSink frames mutex poisoned");
                let before = frames.len();
                frames.retain(|(v, _)| *v == 0 || *v > durable);
                Ok(before - frames.len())
            }
        }
    }

    /// Cheap probe: is there anything truncatable at `durable`? Gates the
    /// (relatively expensive) history-flush + truncate in the drainer so it
    /// fires only on a segment/frame boundary, never per-commit (I2).
    ///
    /// `File`: any sealed segment with `0 < max_version <= durable`.
    /// `Mem`: any frame with `0 < v <= durable`.
    pub fn has_truncatable(&self, durable: u64) -> bool {
        match self {
            Self::File(segset) => segset.has_truncatable(durable),
            Self::Mem(m) => {
                let frames = m.frames.lock().expect("MemSink frames mutex poisoned");
                frames.iter().any(|(v, _)| *v > 0 && *v <= durable)
            }
        }
    }
}
