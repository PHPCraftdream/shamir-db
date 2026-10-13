//! Journal-event header carried by WAL entry format version 3 (S5c-X I1).
//!
//! Version 3 entries embed a [`WalEventHeader`] holding everything the
//! journal event needs that the data ops alone do not carry: the repo
//! name, the committing actor, the PUBLIC transaction id (0 for implicit
//! batch writes), the projection timestamp, the human-readable names of
//! every touched table, and the (rare) record changes whose staged key is
//! NOT a 16-byte `RecordId` (the WAL's `Put`/`Delete` data ops cover only
//! `RecordId`-keyed writes; the projection visits every `Set`/`Remove`).
//!
//! Entries of versions 1 and 2 decode with `event: None` — "unknown",
//! no journal event can be derived from them.

pub mod header;

#[cfg(test)]
mod tests;

pub use header::{WalEventHeader, WalExtraChange};
