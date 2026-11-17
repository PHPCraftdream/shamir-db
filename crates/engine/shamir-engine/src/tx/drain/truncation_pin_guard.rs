use std::sync::Arc;

use super::truncation_pin::TruncationPin;

/// Keeps a floor live until drop, including cancellation and panic unwind.
#[must_use = "dropping the guard releases retained WAL history"]
pub(crate) struct TruncationPinGuard {
    pub(super) registry: Arc<TruncationPin>,
    pub(super) id: u64,
}

impl Drop for TruncationPinGuard {
    fn drop(&mut self) {
        self.registry.release_pin(self.id);
    }
}
