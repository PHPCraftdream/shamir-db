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

pub mod error;
pub mod follower_loop;
pub mod in_process;
pub mod source;
pub mod supervisor;
pub mod wire_source;

pub use error::ReplError;
pub use follower_loop::{run_follower_loop, FollowerLoopConfig};
pub use source::ReplSource;
pub use supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};
pub use wire_source::WireReplSource;

#[cfg(test)]
mod tests;
