//! Experimental migration cutover (#1219).
//!
//! `Draining -> CutoverReady` requires zero residual lag; rejection is a
//! retryable `DbError::Conflict`. Late entries are replayed page-by-page in
//! LSN order, to an empty read, before destination flush and shadow purge.
//! `Committed` means raw data replay is complete, NOT admin finalisation.
//! Re-entering final drain in Committed is a no-op (and persists the receipt).
//!
//! State uses the existing bincode MigrationState layout, unchanged, under
//! `__migration_state_<id>` in the source info store. Finalisation progress is
//! a separate single-byte `__migration_finalise_<id>` record: absent/0 = seed
//! pending, 1 = seed durable/index population pending, 2 = both complete.
//! Old experimental states without progress therefore start at 0. Admin retry
//! discovers receipts in configured source tables when the active map is empty.
//! Completed receipts are retained for idempotent cleanup/retry, not purged.
//!
//! Under destination write/DDL exclusion, seed skips rows already current in
//! MVCC (including writes preceding a lost stage marker). Incomplete index
//! population recreates derived backends before replay: posting sets overwrite,
//! BM25 counts start empty rather than accumulating duplicate inserts. Rebuild
//! on an existing FTS backend is additive, so retry uses fresh constructors,
//! not restore/rebuild. Replacements stay Building until full replay succeeds;
//! the reader drain guard is dropped before cleanup and response construction.
//! Each marker follows its collaborator's flush. Completed stages are skipped;
//! active-map removal occurs only after stage 2. Backend state on full reopen
//! remains the table-open restore path's responsibility.
//!
//! Storage durability is only as strong as Store::flush. The currently exposed
//! in_memory destination cannot survive process loss; retry recovery requires
//! the source/destination stores and table configuration to remain available.
//! This does not supply source-write interception, snapshot isolation, routing
//! swap, or persistent receipt discovery outside configured tables. Destination
//! data must not be externally mutated before finalisation completes. Lock
//! order in admin retry is coordinator finalisation -> source write/DDL barrier
//! (released after final drain) -> destination write/DDL barrier -> index reader
//! drain gate. No source and destination write locks are held together.

pub mod coordinator;
pub mod shadow_key;
pub mod shadow_log;

pub use coordinator::{DrainCatchUpResult, MigrationCoordinator, MigrationPhase, MigrationState};
pub use shadow_log::{MigrationShadowLog, ShadowEntry, ShadowOp};

#[cfg(test)]
mod tests;
