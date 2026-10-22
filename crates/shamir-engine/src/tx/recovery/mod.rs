// `tx::recovery` — V2 WAL recovery + cold-start reconciliation (stage E1
// regroup). Declarations and re-exports only. The former `tx/recovery.rs`
// body lives in `v2.rs`.
pub(crate) mod clean_horizon;
pub(crate) mod cold_reconcile;
mod v2;

#[cfg(test)]
pub(crate) use v2::FAIL_HISTORY_SEED_TX_ID;
pub use v2::{recover_inflight_v2, replay_v2_entry, replay_v2_op};
