//! PR2 S5c-X — runtime clean-horizon checkpoint + WAL retention
//! enforcement (module manifest; re-exports only).
//!
//! * `retention_gate::RetentionGate` — lock-free refuse flag + counters;
//!   the commit hot path reads it with exactly one atomic load
//!   (`RepoInstance::wal_retention_refuse_writes`).
//! * `checkpoint::checkpoint_before_truncate` — journal checkpoint ahead of
//!   any WAL unlink.
//! * `retention::enforce_retention` — per-drain-pass retention
//!   measurement/action.
//!
//! Callers use the full `crate::tx::clean_horizon::{...}` paths; no
//! re-exports are kept so the surface stays greppable.

pub(crate) mod checkpoint;
pub(crate) mod recovery_floor;
pub(crate) mod repair_pending_drain;
pub(crate) mod retention;
pub(crate) mod retention_gate;

#[cfg(test)]
pub(crate) mod tests;
