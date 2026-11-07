//! R1-c — follower replication pull-loop (REPLICATION §4/§5.2/§5.3/§5.6).
//!
//! The follower engine lives here: a transport-agnostic [`ReplSource`]
//! trait, the [`run_follower_loop`] engine that drives it, and a wire
//! implementation over [`shamir_client::Client`]. See the sub-modules for
//! the per-piece rationale.
//!
//! # Module layout
//!
//! - [`error`] — the [`ReplError`] enum (terminal `StaleLeaderEpoch` vs
//!   transient everything-else).
//! - [`source`] — the [`ReplSource`] trait + epoch helpers.
//! - [`follower_loop`] — the [`run_follower_loop`] engine.
//! - [`wire_source`] — the wire `ReplSource` over `shamir_client::Client`.
//! - [`in_process`] — an in-process `ReplSource` over a leader
//!   `Arc<ShamirDb>` + `ShamirDbHandler`, used by tests.
//! - [`identity_fence`] — the S5c-X PR7 universal leader-identity fence
//!   (record the leader `(lineage, history_epoch)` at first contact, verify
//!   on every Hello/Pull).

pub mod error;
pub mod follower_loop;
pub(crate) mod identity_fence;
pub mod supervisor;
mod transport;

// Module-name re-exports so `crate::replication::{in_process, source,
// wire_source, prod_factory}` keep resolving for callers inside and
// outside this module.
pub use transport::{in_process, prod_factory, source, wire_source};

pub use error::ReplError;
pub use follower_loop::{run_follower_loop, FollowerLoopConfig};
pub use prod_factory::build_prod_factory;
// Public so the seed-recovery follower harness (external test crate) can
// construct it; the transport module itself stays private.
pub use source::ReplSource;
pub use supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};
pub use transport::replication_config::{ReplicationConfig, UpstreamPins};
pub use transport::replicator_password::ReplicatorPassword;
pub use wire_source::WireReplSource;

#[cfg(test)]
mod tests;
