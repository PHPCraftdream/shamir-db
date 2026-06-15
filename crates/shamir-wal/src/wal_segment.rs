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
//! Wired to nothing yet — additive primitive consumed by later stages.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use shamir_storage::error::{DbError, DbResult};
use tokio::task::spawn_blocking;

use crate::wal_entry_v2::WalEntryV2;

/// Append-only, file-backed WAL segment. Splits durability:
///   - `append_batch` → `write()` to the OS page cache (level 2:
///     survives a process crash, lost only on power loss before `sync`).
///   - `sync`         → `fsync` (level 3: survives power loss).
///
/// Single-writer by construction (the group-commit leader is the only
/// appender); the `Mutex<File>` guards the handle for standalone safety
/// and is held ONLY on the blocking thread inside `spawn_blocking`, never
/// across an `.await`.
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
}

#[allow(dead_code)]
impl WalSegment {
    /// Open (creating if absent) an append-only segment at `path`.
    pub async fn open(path: PathBuf) -> DbResult<Self> {
        let p = path.clone();
        let (file, existing_len) = spawn_blocking(move || -> DbResult<(File, u64)> {
            let f = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&p)
                .map_err(|e| DbError::Storage(format!("WalSegment open {p:?}: {e}")))?;
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
        })
    }

    /// Append a batch of encoded payloads in ONE blocking round-trip,
    /// flushing to the OS (level 2) but NOT fsync'ing. Returns the seq
    /// assigned to the LAST entry. Batching amortises the spawn_blocking
    /// hop across all concurrent committers funnelled by the leader.
    pub async fn append_batch(&self, payloads: Vec<Vec<u8>>, max_version: u64) -> DbResult<u64> {
        if payloads.is_empty() {
            // Still fold the version in — an empty batch carrying a watermark
            // must not regress max_committed (monotonic, fetch_max).
            self.max_committed.fetch_max(max_version, Ordering::AcqRel);
            return Ok(self.next_seq.load(Ordering::Acquire));
        }
        let n = payloads.len() as u64;
        let last_seq = self.next_seq.fetch_add(n, Ordering::AcqRel) + n - 1;
        self.max_committed.fetch_max(max_version, Ordering::AcqRel);
        let file = Arc::clone(&self.file);
        let frame_bytes = spawn_blocking(move || -> DbResult<u64> {
            // Coalesce all frames into one buffer → a single write() syscall
            // instead of 3N (len header + payload + crc per entry).
            // Frame format is unchanged: [u32 len LE][payload][u32 crc32 LE].
            let total: usize = payloads.iter().map(|p| p.len() + 8).sum();
            let mut buf = Vec::with_capacity(total);
            for p in &payloads {
                let crc = crc32fast::hash(p);
                buf.extend_from_slice(&(p.len() as u32).to_le_bytes());
                buf.extend_from_slice(p);
                buf.extend_from_slice(&crc.to_le_bytes());
            }
            let mut f = file.lock().expect("WalSegment file mutex poisoned");
            f.write_all(&buf)
                .map_err(|e| DbError::Storage(format!("WalSegment append: {e}")))?;
            // Level 2 (OS page cache) is reached by write_all itself — the
            // write() syscall copies data into kernel buffers. std::fs::File
            // is unbuffered, so there is no userspace buffer to flush.
            Ok(buf.len() as u64)
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;
        self.bytes_written.fetch_add(frame_bytes, Ordering::AcqRel);
        Ok(last_seq)
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
    pub async fn sync(&self) -> DbResult<()> {
        let file = Arc::clone(&self.file);
        spawn_blocking(move || -> DbResult<()> {
            let f = file.lock().expect("WalSegment file mutex poisoned");
            f.sync_all()
                .map_err(|e| DbError::Storage(format!("WalSegment sync: {e}")))
        })
        .await
        .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))?
    }

    /// Replay the segment from the start, returning every intact entry.
    /// Stops at the first torn / corrupt frame (a crash tail) — that is
    /// NOT an error: a partial trailing write is discarded.
    pub async fn replay(&self) -> DbResult<Vec<WalEntryV2>> {
        let path = self.path.clone();
        spawn_blocking(move || -> DbResult<Vec<WalEntryV2>> {
            let mut f = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
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
                    break; // torn tail
                }
                let payload = &buf[pos + 4..pos + 4 + len];
                let crc_stored = u32::from_le_bytes([
                    buf[pos + 4 + len],
                    buf[pos + 5 + len],
                    buf[pos + 6 + len],
                    buf[pos + 7 + len],
                ]);
                if crc32fast::hash(payload) != crc_stored {
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
