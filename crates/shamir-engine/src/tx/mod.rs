pub mod commit;
pub mod commit_phases;
pub(super) mod materialize;
pub(super) mod pre_commit;
pub mod predicate_range;
pub mod recovery;
pub mod tx_outcome;

pub(crate) use commit::release_pessimistic_locks;
pub use commit::{commit_tx, wal_ops_from_tx, TxError as CommitError};
pub use recovery::{recover_inflight_v2, replay_v2_entry, replay_v2_op};
pub use tx_outcome::{BackgroundCommitHandle, MaterializationState, TxOutcome};

// Phase B — re-export the `shamir-tx` overlay/handle types through the
// engine's public surface so the `shamir-db` facade (which depends on the
// engine, not on `shamir-tx` directly) can name them in its interactive-tx
// methods. These are the SAME concrete types `shamir-server` names via its
// own `shamir-tx` dependency, so a parked `TxContext` round-trips between the
// facade and the server registry without conversion.
pub use shamir_tx::{IsolationLevel, SnapshotGuard, TxContext, TxId};

// Phase 3b — re-export the changefeed event types through the engine's
// public surface so the `shamir-db` facade can name them in its
// `subscribe_changelog` / `read_changelog_from` methods without a direct
// `shamir-tx` dependency.
pub use shamir_tx::{ChangeOp, ChangelogEvent, JournalRead, RecordChange};

#[cfg(test)]
mod tests;
