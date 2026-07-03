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

use async_trait::async_trait;
use shamir_client::Client;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use tokio::sync::Mutex;

use super::error::ReplError;
use super::source::{hello_request, ReplSource};

/// Wire-side [`ReplSource`]: wraps a connected [`shamir_client::Client`].
///
/// The client is wrapped in a `Mutex` because `shamir_client::Client::repl`
/// takes `&self` but the underlying write path serialises requests internally
/// — we keep the mutex so the loop's backoff-sleep cannot interleave with a
/// racing re-sender in a future variant. Today the loop is single-tasked, so
/// the mutex is uncontended (lock-free fast path).
pub struct WireReplSource {
    client: Mutex<Client>,
}

impl WireReplSource {
    /// Wrap an already-connected client (must be authenticated as a
    /// `replicator`-role user or superuser).
    pub fn new(client: Client) -> Self {
        Self {
            client: Mutex::new(client),
        }
    }
}

#[async_trait]
impl ReplSource for WireReplSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        let client = self.client.lock().await;
        client
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
        };
        let client = self.client.lock().await;
        client
            .repl(req)
            .await
            .map_err(|e| ReplError::Transport(e.to_string()))
    }
}
