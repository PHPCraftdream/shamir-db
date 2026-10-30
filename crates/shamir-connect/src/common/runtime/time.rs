//! Unix nanosecond timestamps.
//!
//! Per spec §15.5 [NORMATIVE]: all wire-level and persisted timestamps use
//! `u64` unix nanos with `_ns` suffix. NTP-disciplined clock required.

use crate::common::error::EnvironmentError;
use std::time::{SystemTime, UNIX_EPOCH};

/// Unix timestamp in nanoseconds.
///
/// Newtype to prevent accidental mixing with other `u64` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixNanos(pub u64);

impl UnixNanos {
    /// Legacy wrapper; prefer [`UnixNanos::try_now`].
    ///
    /// # Panics
    /// Panics when the clock is invalid or out of range.
    pub fn now() -> Self {
        Self::try_now().expect("system clock unavailable")
    }

    /// Read and validate the system clock.
    pub fn try_now() -> Result<Self, EnvironmentError> {
        Self::try_from_system_time(SystemTime::now())
    }

    /// Convert a system time without truncation.
    pub fn try_from_system_time(time: SystemTime) -> Result<Self, EnvironmentError> {
        let duration = time
            .duration_since(UNIX_EPOCH)
            .map_err(|_| EnvironmentError::ClockBeforeEpoch)?;
        let nanos =
            u64::try_from(duration.as_nanos()).map_err(|_| EnvironmentError::ClockOverflow)?;
        Ok(Self(nanos))
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
