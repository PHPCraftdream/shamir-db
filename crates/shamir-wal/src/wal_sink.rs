use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use shamir_storage::error::DbResult;

use crate::wal_entry_v2::WalEntryV2;
use crate::wal_segment::WalSegment;

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
    frames: Mutex<Vec<Vec<u8>>>,
    next_seq: AtomicU64,
}

impl MemSink {
    fn new() -> Self {
        Self {
            frames: Mutex::new(Vec::new()),
            next_seq: AtomicU64::new(0),
        }
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
    /// Real append-only file. write() = level 2, sync_all = level 3.
    File(WalSegment),
    /// In-RAM Vec-backed segment. append = push, sync = no-op (no fsync in
    /// RAM), replay = decode every stored payload.
    Mem(MemSink),
}

impl WalSink {
    /// Construct a fresh in-memory sink.
    pub fn mem() -> Self {
        Self::Mem(MemSink::default())
    }

    pub async fn append_batch(&self, payloads: Vec<Vec<u8>>) -> DbResult<u64> {
        match self {
            Self::File(seg) => seg.append_batch(payloads).await,
            Self::Mem(m) => {
                if payloads.is_empty() {
                    return Ok(m.next_seq.load(Ordering::Acquire));
                }
                let n = payloads.len() as u64;
                let last_seq = m.next_seq.fetch_add(n, Ordering::AcqRel) + n - 1;
                // CRC / torn-tail handling is unnecessary in RAM — store the
                // payloads verbatim.
                let mut frames = m.frames.lock().expect("MemSink frames mutex poisoned");
                frames.extend(payloads);
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

    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        match self {
            Self::File(seg) => seg.replay().await,
            Self::Mem(m) => {
                let frames = m.frames.lock().expect("MemSink frames mutex poisoned");
                let mut out = Vec::with_capacity(frames.len());
                for payload in frames.iter() {
                    out.push(WalEntryV2::decode(payload)?);
                }
                Ok(out)
            }
        }
    }
}
