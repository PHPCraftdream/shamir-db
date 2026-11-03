//! Bounded volatile pending-handshake storage.

mod store;

pub use store::{PendingClaim, PendingEntry, PendingError, PendingId, PendingStore};

#[cfg(test)]
mod tests;
