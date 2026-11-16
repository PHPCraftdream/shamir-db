//! The epoch policy a caller checks against (design 6.1). Production
//! callers pass [`EpochPolicy::compiled()`]; tests pass any values.

use super::epochs::{BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH};

/// The `current`/`base` epoch pair the startup rules compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochPolicy {
    pub current: u64,
    pub base: u64,
}

impl EpochPolicy {
    /// The constants compiled into this binary.
    pub fn compiled() -> Self {
        Self {
            current: CURRENT_FORMAT_EPOCH,
            base: BASE_FORMAT_EPOCH,
        }
    }

    /// Explicit pair (tests and future migration commands).
    pub fn new(current: u64, base: u64) -> Self {
        Self { current, base }
    }
}
