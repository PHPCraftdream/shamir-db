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

    /// Maximum number of concurrently-active subscriptions per connection.
    ///
    /// Each active subscription owns a spawned bridge task plus one
    /// broadcast receiver per subscribed repo (finding 2b-i). Without a cap
    /// a single connection can spawn unbounded bridge tasks / receivers and
    /// exhaust runtime resources. `Subscribe` ops beyond this many active
    /// subscriptions on one connection are rejected. `256` is generous for
    /// legitimate reactive workloads while bounding the fan-out.
    pub const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 256;

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

    /// F6 — maximum size of a single WAL segment file before it is sealed
    /// and a fresh active segment is rotated in.
    ///
    /// The WAL is a directory of numbered segments (`NNNNNNNN.wal`); the
    /// active segment accepts appends until it crosses this threshold, at
    /// which point it is sealed (closed, replay/delete-only) and a new
    /// active segment opens. Truncation deletes whole sealed segments once
    /// every record in them is durable in history
    /// (`max_commit_version(S) <= durable_watermark`).
    ///
    /// Trade-off: larger = rotation is rarer (fewer files, less open/seal
    /// churn) but the truncation granule is coarser — a segment is only
    /// reclaimable once its *highest* version drains, so a big segment pins
    /// more disk for longer. Smaller = finer truncation (disk released
    /// sooner) at the cost of more files and more frequent rotation. Start
    /// at 8 MiB — large enough that rotation is infrequent on typical
    /// workloads. See `docs/perf/f6-subplan.md` §4.
    ///
    /// Consumed by `SegmentSet::open` at the call-site (F6b wires
    /// `repo_instance`); `shamir-wal` itself takes the bound as a parameter
    /// to avoid a dependency on this crate.
    pub const WAL_SEGMENT_MAX_BYTES: u64 = 8 * 1024 * 1024;

    /// D2 P1e — soft backpressure threshold on the undrained version gap
    /// (`last_committed() - durable_watermark()`).
    ///
    /// After the cutover the commit ack-path writes ONLY the in-memory overlay;
    /// the value becomes durable in `history` only after the background drainer
    /// replays its WAL entry. Under sustained write pressure faster than the
    /// disk can drain, the overlay + inflight WAL tail grow unbounded. When the
    /// gap exceeds this threshold, the committer applies a soft async brake:
    /// it wakes the drainer and parks on the gate's durable-progress signal
    /// until the gap falls back below the low-watermark (`/2`, hysteresis).
    /// This is a YIELD, never a lock — committers pay latency ONLY under
    /// pressure. Higher = more RAM headroom before braking; lower = tighter
    /// overlay bound at the cost of earlier latency under bursts.
    pub const MAX_UNDRAINED_VERSIONS: u64 = 10_000;

    /// V2.3 (#402) — number of accumulated vector mutations (upserts +
    /// deletes) since the last full HNSW snapshot that triggers a background
    /// generation-flip snapshot.
    ///
    /// Between snapshots, every commit Phase 5d appends a `DeltaOp` chunk to
    /// the info store (one cheap `Store::set`). The on-restart replay walks
    /// every chunk past the manifest's `delta_applied_upto`. Once the live
    /// counter crosses this threshold, a single-flight `tokio::spawn` task
    /// dumps a fresh generation, atomically flips the manifest, and prunes the
    /// superseded gen + played delta chunks. The threshold bounds BOTH the
    /// restart-replay cost AND the orphan-chunk footprint.
    ///
    /// 10_000 default: small enough that restart-replay stays sub-second on
    /// any disk backend (one prefix scan + N `get`s), large enough that the
    /// background snapshot (a `file_dump` + chunk write) runs ~once per
    /// meaningful batch on a steady-write workload rather than continuously.
    /// `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` overrides at startup.
    pub const VECTOR_SNAPSHOT_DELTA_THRESHOLD: u64 = 10_000;

    /// V4.2 (#408) — tombstone ratio threshold above which HNSW compaction
    /// triggers. When `deleted_count / next_id >= ratio` AND `live_count >=
    /// VECTOR_COMPACTION_MIN_LIVE`, a background rebuild-aside is spawned.
    pub const VECTOR_COMPACTION_RATIO_THRESHOLD: f64 = 0.3;

    /// V4.2 (#408) — minimum live vector count to trigger compaction. Tiny
    /// indexes (< 1000 live) are not worth the rebuild cost.
    pub const VECTOR_COMPACTION_MIN_LIVE: usize = 1000;
}
