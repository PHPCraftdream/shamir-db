//! Centralized tunable constants for ShamirDB.
//!
//! One home for build-time tunables, organized by their owner level in the
//! (future) Instance→Repo→Table→Store config cascade. Today these are plain
//! `const`s (change = edit here + rebuild + benchmark via /opti); a later
//! phase promotes selected knobs to a runtime cascade where these become
//! the defaults.

/// Knobs whose natural owner is the storage backend / store level.
pub mod store_defaults {
    /// Batch size for foreground / read-path full scans and index backfill
    /// (latency-sensitive; larger batch = fewer round-trips).
    pub const FULL_SCAN_BATCH: usize = 1000;
    /// Batch size for background maintenance scans — gc / vacuum / purge /
    /// migration drain / metadata prefix scans (smaller batch keeps memory
    /// and CPU spikes modest, avoids starving foreground work).
    pub const MAINT_SCAN_BATCH: usize = 256;
}

/// Knobs whose natural owner is the instance / deployment level.
pub mod instance_defaults {
    /// Initial capacity for transport frame / scratch byte buffers.
    pub const IO_FRAME_BUFFER_CAP: usize = 4096;
}
