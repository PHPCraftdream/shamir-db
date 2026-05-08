//! Unix nanosecond timestamps.
//!
//! Per spec §15.5 [NORMATIVE]: all wire-level and persisted timestamps use
//! `u64` unix nanos with `_ns` suffix. NTP-disciplined clock required.

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix timestamp in nanoseconds.
///
/// Newtype to prevent accidental mixing with other `u64` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixNanos(pub u64);

impl UnixNanos {
    /// Current wall-clock time as unix nanos.
    ///
    /// Panics if the system clock is before 1970 (extremely unlikely).
    pub fn now() -> Self {
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch");
        Self(dur.as_nanos() as u64)
    }

    /// Unwrap to raw `u64`.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Big-endian byte representation (used in `auth_message`, `identity_input`).
    pub const fn to_be_bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }

    /// Add a duration in nanoseconds, saturating on overflow.
    #[must_use]
    pub fn saturating_add(self, ns: u64) -> Self {
        Self(self.0.saturating_add(ns))
    }
}

impl From<u64> for UnixNanos {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<UnixNanos> for u64 {
    fn from(v: UnixNanos) -> u64 {
        v.0
    }
}

/// Common nanosecond constants.
pub mod ns {
    /// One second.
    pub const SECOND: u64 = 1_000_000_000;
    /// One minute.
    pub const MINUTE: u64 = 60 * SECOND;
    /// One hour.
    pub const HOUR: u64 = 60 * MINUTE;
    /// One day.
    pub const DAY: u64 = 24 * HOUR;
}
