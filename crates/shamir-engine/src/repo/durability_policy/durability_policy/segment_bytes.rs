//! Effective WAL segment size for retention validation and segment rotation.
//!
//! `SHAMIR_WAL_SEGMENT_MAX_BYTES` overrides the tunable default when it parses
//! as `u64`; malformed values fall back to the default.

use shamir_tunables::instance_defaults::WAL_SEGMENT_MAX_BYTES;

/// The effective WAL segment size in bytes: the
/// `SHAMIR_WAL_SEGMENT_MAX_BYTES` env override when present and
/// well-formed, otherwise the `WAL_SEGMENT_MAX_BYTES` tunable (8 MiB).
#[must_use]
pub fn effective_wal_segment_bytes() -> u64 {
    std::env::var("SHAMIR_WAL_SEGMENT_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(WAL_SEGMENT_MAX_BYTES)
}
