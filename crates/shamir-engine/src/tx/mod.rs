// Stage E1 regroup: the direct entries of `tx/` are the five group modules
// below plus `follower_durability` (inside `replication`), `tests/` and this
// manifest. Every historical `crate::tx::<module>` path keeps resolving
// through the re-exports below, with the same visibility audience.
pub mod commit;
pub(crate) mod drain;
pub(crate) mod preparation;
pub mod recovery;
pub(crate) mod replication;

pub(crate) use commit::acknowledgement as ack;
pub(crate) use recovery::{clean_horizon, cold_reconcile};
pub use replication::{apply_replicated, replicated_event};
// P1d-2a: additive repo-level drainer (generalized recovery loop). Defined
// and unit-tested, but NOT wired into the commit path — the cutover that
// makes it the sole history writer is P1d-2b. `dead_code` until then.
#[cfg(test)]
pub(super) use commit::pipeline::phase::publish_journaled;
pub(super) use commit::pipeline::phase::{finalize, materialize};
#[allow(dead_code)]
pub use drain::drainer;
pub(super) use preparation::pre_commit;
pub use preparation::predicate_range;
pub(crate) use replication::follower_durability;

pub use apply_replicated::{apply_replicated_batch, ApplyOutcome, ApplyPolicy};
pub(crate) use commit::pipeline::driver::commit_driver::CommitDriver;
pub(crate) use commit::pipeline::driver::commit_tail_barrier::CommitTailBarrier;
pub use commit::pipeline::driver::tx_outcome;
#[allow(unused_imports)] // Compatibility paths for the moved driver modules.
pub(crate) use commit::pipeline::driver::{commit_driver, commit_tail_barrier};
pub use commit::pipeline::phase::commit_phases;
pub(crate) use commit::release_pessimistic_locks;
pub use commit::{commit_tx, wal_ops_from_tx, TxError as CommitError};
pub use recovery::{recover_inflight_v2, replay_v2_entry, replay_v2_op};
pub use replicated_event::ReplicatedEvent;
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
pub(crate) mod tests;
