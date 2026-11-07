//! Deriving the journal event from its WAL entry (S5c-X, invariant I1).
//!
//! `event = event_from_wal_entry(entry)` is the ONE derivation used by the
//! live commit path; cold recovery (PR4) reuses the same function. The
//! entry's format-3 header (`shamir_wal::WalEventHeader`) carries the
//! fields the data ops cannot supply (repo, actor, public tx id,
//! timestamp, table names, non-`RecordId` changes).

pub mod derive;
pub mod header_from_tx;

#[cfg(test)]
mod tests;

pub use derive::event_from_wal_entry;
pub use header_from_tx::event_header_from_tx;
