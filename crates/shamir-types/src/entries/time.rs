//! Wall-clock time helpers shared across the workspace.
//!
//! Every call site that needs "now" as a plain `u64` unix timestamp used to
//! hand-roll `SystemTime::now().duration_since(UNIX_EPOCH)` and either
//! `.unwrap()` (panics if the system clock is set before 1970) or its own
//! ad-hoc `.unwrap_or(0)` fallback. These two functions are the single safe
//! form, reused everywhere instead of re-deriving it per call site.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock milliseconds since the UNIX epoch; `0` if the clock is set
/// before 1970 (never panics).
#[inline]
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Wall-clock nanoseconds since the UNIX epoch; `0` if the clock is set
/// before 1970 (never panics).
#[inline]
pub fn unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
