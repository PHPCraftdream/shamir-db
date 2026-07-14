//! 386-b — [`SubscriptionSupervisor`]: bind the declarative replication
//! catalogue (386-a) to the follower engine (R1-c).
//!
//! An **active** subscription row in `system/subscriptions` (persisted by
//! `admin_replication.rs`) must translate into a running
//! [`run_follower_loop`](super::follower_loop::run_follower_loop) pulling from
//! the subscription's `upstream` and applying onto the local `(db, repo)`
//! resolved from the bound profile's `pull` streams. `pause` / `resume` /
//! `drop` (via `alter_subscription` / `drop_subscription`, 386-a) flip the
//! row's `state`; the supervisor's [`reconcile`](Self::reconcile) converges
//! the running loops to that declarative state.
//!
//! # Registry (lock-free, §ideology)
//!
//! Active loops live in a [`scc::HashMap`] keyed by subscription name, hashed
//! with the workspace [`THasher`]. Each entry owns a
//! [`CancellationToken`] (stop signal) + the spawned [`JoinHandle`]s (one per
//! `pull` stream) + the profile name it was started against (so a profile
//! rebind can be detected and the loop restarted).
//!
//! # Reactivity — reconcile + `notify_changed`, NOT changefeed-watch (yet)
//!
//! For R1 the supervisor is **reconcile-driven**: [`reconcile`](Self::reconcile)
//! reads the catalogue and converges. It is idempotent, so it is safe to call
//! at boot and again after any admin batch that may have touched the
//! catalogue via [`notify_changed`](Self::notify_changed). A fully
//! event-driven watch on the `system` repo changefeed (REPLICATION §5.6) is
//! the intended end-state but is deferred:
//!
//! TODO(386-c): subscribe to the `system` repo changelog (table
//! `subscriptions`) and call `reconcile()` on each event, replacing the
//! explicit `notify_changed()` pokes with an event-driven watch.
//!
//! # §5.6 — non-blocking
//!
//! Every follower loop runs on its own [`tokio::spawn`] task; the supervisor
//! holds no follower locks and never blocks the server. `reconcile` only
//! reads the catalogue and starts/stops tasks.

use std::sync::Arc;

use shamir_collections::{THasher, TMap};
use shamir_db::query::admin::ReplStream;
use shamir_db::query::filter::FilterContext;
use shamir_db::query::read::{QueryResult, ReadQuery};
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_types::admin::{ReplDirection, ReplScope};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::follower_loop::{run_follower_loop, FollowerLoopConfig};
use super::source::ReplSource;

/// A subscription resolved from `system/subscriptions` (386-a schema).
///
/// Passed to the [`ReplSourceFactory`] so it can build the transport
/// (`WireReplSource` in prod, `InProcessReplSource` in tests) with the
/// subscription's `upstream` endpoint and credentials.
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Subscription name (registry key).
    pub name: String,
    /// Upstream leader endpoint string (e.g. `tcp://leader:9000`). For R1 the
    /// factory owns credential resolution (see the module TODO on creds).
    pub upstream: String,
    /// Publication name advertised by the upstream.
    pub publication: String,
    /// Bound replication-profile name (resolved to `pull` streams).
    pub profile: String,
    /// `active` → loop should run; `paused` (or anything else) → stopped.
    pub state: String,
}

impl Subscription {
    /// `true` when the row's `state` field is exactly `active`.
    fn is_active(&self) -> bool {
        self.state == "active"
    }
}

/// Factory that builds a [`ReplSource`] for a subscription.
///
/// This is the seam that makes the supervisor unit-testable: production wires
/// a `WireReplSource` (a TLS+SCRAM `shamir_client::Client` connected to
/// `sub.upstream` under a `replicator` account), tests wire an
/// `InProcessReplSource` over a leader `Arc<ShamirDb>`. The supervisor never
/// looks at credentials — the factory owns them.
///
/// TODO(386-c): upstream credentials. Today the factory carries whatever creds
/// it needs (prod: a config/env `replicator` account keyed by `upstream`).
/// A dedicated per-subscription credential store on the catalogue row is
/// future work — `Subscription.upstream` is the only endpoint hint available.
pub type ReplSourceFactory =
    Arc<dyn Fn(&Subscription) -> Arc<dyn ReplSource> + Send + Sync + 'static>;

/// A running subscription: its stop signal, spawned loop tasks, and the
/// profile it was started against (to detect a rebind on reconcile).
struct SubHandle {
    cancel: CancellationToken,
    joins: Vec<JoinHandle<()>>,
    profile: String,
}

impl SubHandle {
    /// Cancel the loops and detach the join handles (best-effort stop; the
    /// tasks observe the token and exit on their next iteration).
    fn stop(&self) {
        self.cancel.cancel();
        for j in &self.joins {
            j.abort();
        }
    }
}

/// Supervises the lifecycle of follower loops driven by the declarative
/// subscription catalogue. See the module docs.
pub struct SubscriptionSupervisor {
    /// Local node — the follower onto which pulled events are applied, and the
    /// source of the `system/subscriptions` + `system/replication_profiles`
    /// catalogue.
    local: Arc<ShamirDb>,
    /// Builds the [`ReplSource`] transport for a subscription.
    factory: ReplSourceFactory,
    /// Stable follower identity advertised in `ReplHello` (§5.2).
    node_id: String,
    /// Active loops keyed by subscription name (lock-free, `THasher`).
    registry: scc::HashMap<String, SubHandle, THasher>,
    /// Iteration cap forwarded to each loop; `None` in prod (run forever),
    /// `Some(n)` in tests to bound the loop without relying on cancellation.
    max_iterations: Option<usize>,
    /// Long-poll budget forwarded to each `pull` (ms).
    poll_wait_ms: u32,
}

impl SubscriptionSupervisor {
    /// Build a supervisor for `local` with the given source `factory` and
    /// follower `node_id`. Loops run forever (until reconciled away or the
    /// supervisor is dropped) with the default long-poll budget.
    pub fn new(
        local: Arc<ShamirDb>,
        factory: ReplSourceFactory,
        node_id: impl Into<String>,
    ) -> Self {
        Self {
            local,
            factory,
            node_id: node_id.into(),
            registry: scc::HashMap::with_hasher(THasher::default()),
            max_iterations: None,
            poll_wait_ms: 5000,
        }
    }

    /// Cap each loop at `n` iterations (tests only — avoids relying on
    /// cancellation timing; the loop exits cleanly after `n` pulls).
    #[must_use]
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = Some(n);
        self
    }

    /// Override the per-pull long-poll budget (ms).
    #[must_use]
    pub fn with_poll_wait_ms(mut self, ms: u32) -> Self {
        self.poll_wait_ms = ms;
        self
    }

    /// Number of currently-running subscription loops (test/telemetry).
    ///
    /// `scc::HashMap::len()` is O(N); this is an off-hot-path introspection
    /// helper, not a reconcile-loop primitive.
    #[allow(clippy::disallowed_methods)] // O(N) ack: test/telemetry, not hot path
    pub fn active_count(&self) -> usize {
        self.registry.len()
    }

    /// `true` if a loop is currently registered for `name`.
    pub fn is_running(&self, name: &str) -> bool {
        self.registry.contains_sync(name)
    }

    /// Poke the supervisor after an admin batch that may have changed the
    /// catalogue. Equivalent to [`reconcile`](Self::reconcile); a distinct
    /// name documents the call-site intent (event → converge).
    ///
    /// TODO(386-c): replaced by an event-driven `system` changefeed watch.
    pub async fn notify_changed(&self) {
        self.reconcile().await;
    }

    /// Converge running loops to the declarative catalogue state. Idempotent.
    ///
    /// - For each `active` subscription with a resolvable profile and NOT
    ///   already running under the same profile → start its loop(s).
    /// - For each running loop whose subscription is gone, `paused`, or
    ///   rebound to a different profile → stop it (and, on rebind, the next
    ///   pass restarts it under the new profile).
    pub async fn reconcile(&self) {
        let subs = match self.read_subscriptions().await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "supervisor: failed to read subscriptions catalogue");
                return;
            }
        };

        // 1. Stop loops that should no longer run (removed / paused / rebound).
        let mut to_stop: Vec<String> = Vec::new();
        self.registry.iter_sync(|name, handle| {
            let keep = subs
                .iter()
                .any(|s| &s.name == name && s.is_active() && s.profile == handle.profile);
            if !keep {
                to_stop.push(name.clone());
            }
            true
        });
        for name in to_stop {
            if let Some((_, handle)) = self.registry.remove_sync(&name) {
                handle.stop();
                info!(subscription = %name, "supervisor: stopped follower loop");
            }
        }

        // 2. Start loops for active subscriptions not yet running.
        for sub in &subs {
            if !sub.is_active() || self.registry.contains_sync(&sub.name) {
                continue;
            }
            if let Err(e) = self.start_subscription(sub).await {
                warn!(subscription = %sub.name, error = %e, "supervisor: start failed");
            }
        }
    }

    /// Stop every running loop and clear the registry (shutdown).
    pub async fn stop_all(&self) {
        let mut names: Vec<String> = Vec::new();
        self.registry.iter_sync(|name, _| {
            names.push(name.clone());
            true
        });
        for name in names {
            if let Some((_, handle)) = self.registry.remove_sync(&name) {
                handle.stop();
            }
        }
    }

    /// Resolve `sub`'s profile → `pull` streams, then spawn one follower loop
    /// per `pull` stream targeting `(scope.db, scope.repo)`.
    async fn start_subscription(&self, sub: &Subscription) -> Result<(), String> {
        let streams = self.resolve_profile_streams(&sub.profile).await?;
        let pull_targets: Vec<(String, String)> = streams
            .iter()
            .filter(|st| matches!(st.direction, ReplDirection::Pull))
            .filter_map(|st| pull_target(&st.scope))
            .collect();

        if pull_targets.is_empty() {
            return Err(format!(
                "profile '{}' has no pull streams with a (db, repo) scope",
                sub.profile
            ));
        }

        let source = (self.factory)(sub);
        let cancel = CancellationToken::new();
        let mut joins = Vec::with_capacity(pull_targets.len());

        for (db, repo) in pull_targets {
            let mut cfg = FollowerLoopConfig::new(self.node_id.clone(), db, repo)
                .with_poll_wait_ms(self.poll_wait_ms);
            if let Some(n) = self.max_iterations {
                cfg = cfg.with_max_iterations(n);
            }
            let local = self.local.clone();
            let src = source.clone();
            let tok = cancel.clone();
            let sub_name = sub.name.clone();
            // §5.6 — each loop is its own task; never blocks the server.
            let join = tokio::spawn(async move {
                if let Err(e) = run_follower_loop(local, src, cfg, tok).await {
                    warn!(subscription = %sub_name, error = %e, "follower loop terminated");
                }
            });
            joins.push(join);
        }

        let handle = SubHandle {
            cancel,
            joins,
            profile: sub.profile.clone(),
        };
        // A racing reconcile may have inserted already; if so, stop the loops
        // we just spawned rather than leak them.
        if let Err((_, handle)) = self.registry.insert_sync(sub.name.clone(), handle) {
            handle.stop();
        } else {
            info!(subscription = %sub.name, profile = %sub.profile, "supervisor: started follower loop");
        }
        Ok(())
    }

    /// Resolve a profile name → its `streams` by reading
    /// `system/replication_profiles` (386-a schema: `{name, streams}`).
    async fn resolve_profile_streams(&self, profile: &str) -> Result<Vec<ReplStream>, String> {
        let table = self
            .local
            .system_store()
            .replication_profiles_table()
            .await
            .map_err(|e| e.to_string())?;
        let records = read_all(&table, "replication_profiles")
            .await
            .map_err(|e| e.to_string())?;
        let row = records
            .into_iter()
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(profile))
            .ok_or_else(|| format!("replication profile '{profile}' not found"))?;
        let streams_val = row
            .get("streams")
            .cloned()
            .ok_or_else(|| format!("profile '{profile}' has no streams field"))?;
        // `streams` was persisted via `to_qv(&Vec<ReplStream>)` (a msgpack
        // round-trip); reverse it the same way.
        decode_streams(&streams_val)
    }

    /// Read `system/subscriptions` into typed [`Subscription`]s.
    async fn read_subscriptions(&self) -> Result<Vec<Subscription>, String> {
        let table = self
            .local
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| e.to_string())?;
        let records = read_all(&table, "subscriptions")
            .await
            .map_err(|e| e.to_string())?;
        Ok(records
            .into_iter()
            .filter_map(|r| {
                Some(Subscription {
                    name: r.get("name")?.as_str()?.to_string(),
                    upstream: r
                        .get("upstream")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    publication: r
                        .get("publication")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    profile: r
                        .get("profile")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    state: r
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            })
            .collect())
    }
}

/// Extract a `(db, repo)` pull target from a scope. A pull loop needs a
/// concrete repo (the follower applies onto `(db, repo)`), so a bare-database
/// scope (`repo == None`) is skipped for R1 — expanding it to every repo in
/// the db is future work.
fn pull_target(scope: &ReplScope) -> Option<(String, String)> {
    scope
        .repo
        .as_ref()
        .map(|repo| (scope.db.clone(), repo.clone()))
}

/// Reverse `to_qv(&Vec<ReplStream>)`: re-encode the `QueryValue` to msgpack
/// and deserialise the typed streams back out.
fn decode_streams(v: &QueryValue) -> Result<Vec<ReplStream>, String> {
    let bytes = rmp_serde::to_vec_named(v).map_err(|e| format!("encode streams: {e}"))?;
    rmp_serde::from_slice::<Vec<ReplStream>>(&bytes).map_err(|e| format!("decode streams: {e}"))
}

/// Read every record of a system-store table as owned `QueryValue`s.
///
/// Mirrors `admin_replication.rs::read_all` — the system catalogue read path.
async fn read_all(
    table: &shamir_db::engine::table::TableManager,
    table_name: &str,
) -> shamir_db::DbResult<Vec<QueryValue>> {
    let interner = table.interner().get().await?;
    let refs: TMap<String, QueryResult> = TMap::default();
    let ctx = FilterContext::new(interner, &refs);
    let query = ReadQuery::new(table_name);
    let result = table.read(&query, &ctx).await?;
    Ok(result
        .records
        .into_iter()
        .map(|r| r.as_value().into_owned())
        .collect())
}
