//! 386-c — production [`ReplSourceFactory`]: builds a [`WireReplSource`] for
//! each subscription by opening a real TLS+SCRAM [`shamir_client::Client`]
//! session to the subscription's `upstream` under a `replicator` account.
//!
//! # Credentials (386-c minimal)
//!
//! All outbound connections authenticate with a single shared `replicator`
//! account taken from the `[replication]` config section
//! ([`ReplicationConfig::replicator_user`] / `replicator_password`). This is
//! the minimal working path.
//!
//! TODO(386-c): a per-subscription credential store keyed by the
//! subscription's `upstream` — the current shape only supports one leader
//! identity for every subscription. `Subscription.upstream` is the only
//! endpoint hint available on the catalogue row today.
//!
//! # Connection is lazy and non-blocking (§5.6)
//!
//! The factory is a synchronous `Fn(&Subscription) -> Arc<dyn ReplSource>`,
//! but establishing a TLS+SCRAM session is async. We therefore return a
//! [`LazyWireSource`] that connects on first use, inside the follower loop's
//! own task — the factory itself never blocks the server, and a leader that
//! is momentarily unreachable at reconcile time does not stall boot (the
//! connect error surfaces as a transient [`ReplError::Transport`] the loop
//! retries with backoff).

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use shamir_client::{Client, ConnectOptions};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Mutex;
use tracing::warn;
use zeroize::Zeroizing;

use super::error::ReplError;
use super::source::ReplSource;
use super::supervisor::{ReplSourceFactory, Subscription};
use super::wire_source::WireReplSource;

/// Replicator credentials + TLS SNI resolved once from `[replication]` config
/// and shared (cheaply cloned) into every source the factory builds.
#[derive(Clone)]
struct ReplicatorCreds {
    user: String,
    password: Arc<str>,
    server_name: String,
}

/// Build a production [`ReplSourceFactory`] from the resolved replicator
/// credentials. Returns `None` when no `replicator_user` / `replicator_password`
/// is configured — with no credentials there is nothing to connect *as*, so
/// the supervisor is constructed without a factory (any `active` subscription
/// then logs and is skipped by reconcile until creds are configured).
pub fn build_prod_factory(
    user: Option<String>,
    password: Option<String>,
    server_name: String,
) -> Option<ReplSourceFactory> {
    let (user, password) = match (user, password) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => (u, p),
        _ => return None,
    };
    let creds = ReplicatorCreds {
        user,
        password: Arc::from(password.as_str()),
        server_name,
    };
    let factory: ReplSourceFactory = Arc::new(move |sub: &Subscription| {
        Arc::new(LazyWireSource::new(sub.upstream.clone(), creds.clone())) as Arc<dyn ReplSource>
    });
    Some(factory)
}

/// A [`ReplSource`] that connects to its upstream on first use and caches the
/// connected [`WireReplSource`] for subsequent calls.
///
/// Connection state lives behind a `tokio::sync::Mutex` (sanctioned async
/// guard — held only across the connect `.await`, uncontended in the
/// single-tasked loop). Once connected, every op delegates to the inner
/// [`WireReplSource`].
struct LazyWireSource {
    upstream: String,
    creds: ReplicatorCreds,
    inner: Mutex<Option<Arc<WireReplSource>>>,
}

impl LazyWireSource {
    fn new(upstream: String, creds: ReplicatorCreds) -> Self {
        Self {
            upstream,
            creds,
            inner: Mutex::new(None),
        }
    }

    /// Return the connected inner source, establishing the session on first
    /// call. Connect failures are surfaced as transient
    /// [`ReplError::Transport`] so the loop retries with backoff.
    async fn connected(&self) -> Result<Arc<WireReplSource>, ReplError> {
        let mut guard = self.inner.lock().await;
        if let Some(src) = guard.as_ref() {
            return Ok(src.clone());
        }
        let addr = parse_upstream_addr(&self.upstream)
            .map_err(|e| ReplError::Transport(format!("upstream '{}': {e}", self.upstream)))?;
        let opts = ConnectOptions {
            addr,
            server_name: self.creds.server_name.clone(),
            username: self.creds.user.clone(),
            password: Zeroizing::new(self.creds.password.as_bytes().to_vec()),
            // Trust-on-first-use: the follower has no pre-pinned leader key in
            // 386-c. Persisting a leader pin is future work (#388).
            accept_new_host: true,
            trusted_pin: None,
        };
        let client = Client::connect(opts).await.map_err(|e| {
            warn!(upstream = %self.upstream, error = %e, "supervisor: upstream connect failed");
            ReplError::Transport(e.to_string())
        })?;
        let src = Arc::new(WireReplSource::new(client));
        *guard = Some(src.clone());
        Ok(src)
    }
}

#[async_trait]
impl ReplSource for LazyWireSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.connected().await?.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        self.connected()
            .await?
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

/// A [`ReplSource`] that can never connect — used as the boot-time stub when
/// no replicator credentials are configured (`replication = None`).
///
/// Every op returns a transient [`ReplError::Transport`]; the follower loop
/// treats it as a retryable failure and backs off. In practice reconcile only
/// starts loops for `active` subscriptions, so with an empty catalogue this
/// source is never driven — it exists so the supervisor can boot without
/// credentials.
pub struct NoopReplSource;

#[async_trait]
impl ReplSource for NoopReplSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport(
            "replication upstream unavailable: no replicator credentials configured".into(),
        ))
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport(
            "replication upstream unavailable: no replicator credentials configured".into(),
        ))
    }
}

/// Parse a subscription `upstream` string into a [`SocketAddr`].
///
/// Accepts a bare `host:port` or a `tcp://host:port` URL form. Only literal
/// socket addresses are resolved (no DNS) — matching the loopback e2e path;
/// hostname resolution is future work.
fn parse_upstream_addr(upstream: &str) -> Result<SocketAddr, String> {
    let hostport = upstream
        .strip_prefix("tcp://")
        .or_else(|| upstream.strip_prefix("tls://"))
        .unwrap_or(upstream);
    hostport
        .parse::<SocketAddr>()
        .map_err(|e| format!("not a socket address: {e}"))
}
