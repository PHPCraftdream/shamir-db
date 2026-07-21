//! R1-c — `ReplSource`: transport-agnostic abstraction over the leader side
//! of the replication protocol (REPLICATION §5.1/§5.2/§5.3).
//!
//! The trait decouples the follower pull-loop
//! ([`run_follower_loop`](super::follower_loop::run_follower_loop)) from the
//! concrete transport. Two implementations ship:
//!   * an **in-process** `ReplSource` used by the engine tests and the
//!     `@server` integration tests — it wraps the leader's
//!     `Arc<ShamirDb>` + `ShamirDbHandler` and calls `handle_repl` directly,
//!     short-circuiting the network;
//!   * a **wire** `ReplSource` ([`WireReplSource`](super::wire_source::WireReplSource))
//!     that wraps a connected `shamir_client::Client` and drives the real
//!     TLS+SCRAM path.
//!
//! Both speak the same `ReplRequest`/`ReplResponse` wire types, so the loop
//! logic is identical. The trait surface mirrors exactly the two ops the
//! follower needs: `hello` (learn the leader epoch + advertised repos) and
//! `pull` (fetch a batch of events).

use async_trait::async_trait;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse, CURRENT_REPL_PROTO_VER};

use super::error::ReplError;

/// Transport-agnostic source of replication data from a leader.
///
/// Every reply carries `leader_epoch` (REPLICATION §5.2); the loop fences on
/// epoch regression using [`ReplResponse::leader_epoch`](#method.leader_epoch)
/// extracted from each variant. Implementations need NOT perform fencing
/// themselves — that is the loop's job — they only need to surface the
/// epoch the leader returned.
#[async_trait]
pub trait ReplSource: Send + Sync {
    /// Send a [`ReplRequest::Hello`] advertising the follower `node_id` and
    /// return the leader's [`ReplResponse::Hello`] (epoch + advertised
    /// repos). Errors are transient unless they carry
    /// [`ReplError::StaleLeaderEpoch`].
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError>;

    /// Send a [`ReplRequest::Pull`] for one `(db, repo)` starting at
    /// `from_version` (inclusive), capped at `limit` events, with an
    /// optional long-poll `wait_ms` budget (REPLICATION §5.1). Returns the
    /// leader's [`ReplResponse::Pull`] (epoch + msgpack events + optional
    /// `gap_at`). Errors are transient unless they carry
    /// [`ReplError::StaleLeaderEpoch`].
    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError>;
}

/// Helper: extract `leader_epoch` from any [`ReplResponse`] variant.
///
/// Every reply variant carries the epoch (§5.2); this is used by the loop's
/// fencing check before it looks at the payload.
pub(crate) fn leader_epoch_of(resp: &ReplResponse) -> u64 {
    match resp {
        ReplResponse::Hello { leader_epoch, .. }
        | ReplResponse::Pull { leader_epoch, .. }
        | ReplResponse::Error { leader_epoch, .. } => *leader_epoch,
    }
}

/// Helper: build the [`ReplRequest::Hello`] for a follower.
///
/// Kept here so both the in-process and wire implementations agree on the
/// protocol version they advertise.
pub(crate) fn hello_request(node_id: &str) -> ReplRequest {
    ReplRequest::Hello {
        proto_ver: CURRENT_REPL_PROTO_VER,
        node_id: node_id.to_string(),
    }
}
