//! R1-c — wire [`ReplSource`] backed by a connected
//! [`shamir_client::Client`].
//!
//! This is the production path: the follower opens a TLS+SCRAM session to the
//! leader under a `replicator`-role account (REPLICATION §5.4) and the loop
//! calls into this source, which forwards each [`ReplRequest`] through the
//! real wire stack (`shamir_client::Client::repl`).
//!
//! Errors from the client are mapped to [`ReplError::Transport`]; the loop's
//! backoff machinery handles retries. A regressed epoch is detected by the
//! loop (not here) via the `leader_epoch` carried on every reply variant.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_client::Client;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse, REPL_CAP_IDENTITY_FENCE};

use super::source::{hello_request, ReplSource};
use crate::replication::error::ReplError;

/// [`ReplSource`] backed by one authenticated `Client`. The client internally
/// serializes individual writes; no source-level lock is held during a long
/// poll, so the pull targets of one subscription share this source and their
/// long polls overlap.
pub struct WireReplSource {
    client: Arc<Client>,
}

impl WireReplSource {
    /// Wrap an already-connected client (must be authenticated as a
    /// `replicator`-role user or superuser).
    pub fn new(client: Client) -> Self {
        Self {
            client: Arc::new(client),
        }
    }
}

#[async_trait]
impl ReplSource for WireReplSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.client
            .repl(hello_request(node_id))
            .await
            .map_err(|e| ReplError::Transport(e.to_string()))
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let req = ReplRequest::Pull {
            db: db.to_string(),
            repo: repo.to_string(),
            from_version,
            limit,
            wait_ms,
            // Mirror `hello_request`: the production follower runs the
            // identity fence, so every request carries the capability
            // (a `completed_reseed` leader re-checks it on every pull).
            capabilities: Some(vec![REPL_CAP_IDENTITY_FENCE.to_string()]),
        };
        self.client
            .repl(req)
            .await
            .map_err(|e| ReplError::Transport(e.to_string()))
    }
}
