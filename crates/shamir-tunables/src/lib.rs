//! Centralized tunable constants and runtime-overridable knobs for ShamirDB.
//!
//! One home for build-time tunables, organized by their owner level in the
//! (future) Instance→Repo→Table→Store config cascade. Today these are plain
//! `const`s (change = edit here + rebuild + benchmark via /opti); a later
//! phase promotes selected knobs to a runtime cascade where these become
//! the defaults.

pub mod runtime;

#[cfg(test)]
mod tests;

use std::time::Duration;

/// Knobs whose natural owner is the storage backend / store level.
pub mod store_defaults {
    /// Batch size for foreground / read-path full scans and index backfill
    /// (latency-sensitive; larger batch = fewer round-trips).
    pub const FULL_SCAN_BATCH: usize = 1000;
    /// Batch size for background maintenance scans — gc / vacuum / purge /
    /// migration drain / metadata prefix scans (smaller batch keeps memory
    /// and CPU spikes modest, avoids starving foreground work).
    pub const MAINT_SCAN_BATCH: usize = 256;
    /// Batch size for version-log history range reads (get_at slow path,
    /// history-of, seek-latest) — small bounded reads.
    pub const HISTORY_SCAN_BATCH: usize = 64;
}

/// Knobs whose natural owner is the instance / deployment level.
pub mod instance_defaults {
    use super::Duration;

    /// Initial capacity for transport frame / scratch byte buffers.
    pub const IO_FRAME_BUFFER_CAP: usize = 4096;

    /// Poll/backoff interval for server housekeeping loops (sleep between
    /// non-blocking checks).
    pub const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(50);

    /// Maximum number of requests in-flight concurrently per connection.
    ///
    /// Controls the size of the per-connection semaphore (reader back-pressure)
    /// and the mpsc channel capacity to the writer task.
    ///
    /// * `1` → lock-step (identical to the old sequential loop).
    /// * Default `32` → up to 32 pipelined requests before the reader stalls.
    pub const CONN_MAX_IN_FLIGHT: usize = 32;

    /// Number of consecutive push failures before a subscription bridge
    /// declares the consumer "slow" and tears down the subscription.
    pub const SLOW_CONSUMER_THRESHOLD: u32 = 100;

    /// Maximum number of journal events to backfill when a subscription
    /// resumes from a specific `from_version`.
    pub const JOURNAL_BACKFILL_LIMIT: usize = 10_000;

    /// Number of commits between automatic interner checkpoint persists.
    ///
    /// After A5 the interner is no longer persisted synchronously on each
    /// commit — the WAL carries the delta for crash recovery. A background
    /// checkpoint every N commits flushes accumulated deltas to the
    /// interner's durable chunk store, advancing the high-water mark so
    /// Phase 7 WAL truncation can proceed for entries whose deltas are
    /// now covered. Lower N = more frequent I/O; higher N = more WAL
    /// entries retained before truncation (recovery-time cost, not
    /// correctness).
    pub const INTERNER_CHECKPOINT_INTERVAL: u64 = 64;
}
