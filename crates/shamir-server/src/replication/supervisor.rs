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
//! # Destination ownership and terminal state
//!
//! Catalogue lifecycle operations serialize under an async cold-path mutex.
//! Each registered destination has one owner, including while its stopped task
//! is being joined. No concurrent-map guard crosses an await. Terminal apply
//! failures persist `reopen_required`; admission refuses a still-dead gate.
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

use std::sync::atomic::Ordering;
use std::sync::Arc;

use shamir_collections::{THasher, TMap};
use shamir_db::engine::meta::repl::bookmark_v3::{self, FollowerBookmarkState};
use shamir_db::query::admin::ReplStream;
use shamir_db::query::read::{QueryResult, ReadQuery};
use shamir_db::types::value::QueryValue;
use shamir_db::{SeedRefusal, ShamirDb};
use shamir_query_types::admin::{ReplDirection, ReplScope};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::error::ReplError;
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
    targets: Vec<(String, String)>,
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
    validate_replicated_payload: bool,
    // Cold catalogue lifecycle serialization; no follower write lock.
    reconcile_lock: tokio::sync::Mutex<()>,
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
        super::source::seed::fence::install_probe(&local, &factory);
        Self {
            local,
            factory,
            node_id: node_id.into(),
            registry: scc::HashMap::with_hasher(THasher::default()),
            max_iterations: None,
            poll_wait_ms: 5000,
            validate_replicated_payload: false,
            reconcile_lock: tokio::sync::Mutex::new(()),
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

    #[must_use]
    pub fn with_validate_replicated_payload(mut self, validate: bool) -> Self {
        self.validate_replicated_payload = validate;
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

    /// Map a gate-initialization failure by KIND: a typed follower-state
    /// refusal stays machine-readable and terminal — the subscription is
    /// marked `resync_required` and the typed error is returned; every
    /// other error keeps the generic retryable `Start` mapping and marks
    /// nothing. Extracted (round 4) so tests can drive every kind
    /// directly: the supervisor cannot reach a FAILING `tx_gate()` through
    /// this fixture — the gate OnceCell is initialized during open
    /// recovery (tx/recovery.rs:373 → repo_instance.rs:860) and open
    /// itself refuses legacy/malformed bytes
    /// (durability_policy/repo_impl.rs:64-65), so the supervisor always
    /// sees the cached gate.
    pub(super) async fn refuse_gate_init(
        &self,
        sub_name: &str,
        error: shamir_db::storage::error::DbError,
    ) -> ReplError {
        match error {
            shamir_db::storage::error::DbError::FollowerStateRefused { reason } => {
                let error = ReplError::FollowerStateRefused { reason };
                if let Err(persist) = self
                    .local
                    .mark_subscription_terminal(sub_name, "resync_required", &error.to_string())
                    .await
                {
                    return ReplError::Start(persist.to_string());
                }
                error
            }
            error => ReplError::Start(error.to_string()),
        }
    }

    /// Converge running loops to the declarative catalogue state. Idempotent.
    ///
    /// - For each `active` subscription with a resolvable profile and NOT
    ///   already running under the same profile → start its loop(s).
    /// - For each running loop whose subscription is gone, `paused`, or
    ///   rebound to a different profile → stop it (and, on rebind, the next
    ///   pass restarts it under the new profile).
    pub async fn reconcile(&self) {
        let _lifecycle = self.reconcile_lock.lock().await;
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
                for join in handle.joins {
                    let _ = join.await;
                }
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
        let _lifecycle = self.reconcile_lock.lock().await;
        let mut names: Vec<String> = Vec::new();
        self.registry.iter_sync(|name, _| {
            names.push(name.clone());
            true
        });
        for name in names {
            if let Some((_, handle)) = self.registry.remove_sync(&name) {
                handle.stop();
                for join in handle.joins {
                    let _ = join.await;
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_subscription(&self, name: &str) {
        let (_, handle) = self
            .registry
            .remove_sync(name)
            .expect("subscription must be registered");
        for join in handle.joins {
            join.await.expect("subscription task panicked");
        }
    }

    /// Resolve `sub`'s profile → `pull` streams, then spawn one follower loop
    /// per `pull` stream targeting `(scope.db, scope.repo)`.
    async fn start_subscription(&self, sub: &Subscription) -> Result<(), ReplError> {
        let streams = self
            .resolve_profile_streams(&sub.profile)
            .await
            .map_err(ReplError::Start)?;
        let pull_targets: Vec<(String, String)> = streams
            .iter()
            .filter(|st| matches!(st.direction, ReplDirection::Pull))
            .filter_map(|st| pull_target(&st.scope))
            .collect();

        if pull_targets.is_empty() {
            return Err(ReplError::Start(format!(
                "profile '{}' has no pull streams with a (db, repo) scope",
                sub.profile
            )));
        }

        for (index, (db, repo)) in pull_targets.iter().enumerate() {
            super::source::seed::admission::admit(&self.local, &sub.name, &streams, db, repo)
                .await?;
            let mut owner = None;
            self.registry.iter_sync(|name, handle| {
                if handle
                    .targets
                    .iter()
                    .any(|(target_db, target_repo)| target_db == db && target_repo == repo)
                {
                    owner = Some(name.clone());
                    return false;
                }
                true
            });
            if pull_targets[..index].contains(&(db.clone(), repo.clone())) {
                owner = Some(sub.name.clone());
            }
            if let Some(owner) = owner {
                let error = ReplError::DuplicateOwner {
                    db: db.clone(),
                    repo: repo.clone(),
                    owner,
                };
                self.local
                    .mark_subscription_terminal(&sub.name, "duplicate_owner", &error.to_string())
                    .await
                    .map_err(|error| ReplError::Start(error.to_string()))?;
                return Err(error);
            }
            if let Some(destination) = self.local.get_db(db).and_then(|db| db.get_repo(repo)) {
                // R7-2(i) — UP-FRONT capability refusal. The destination repo
                // handle (RepoInstance) does NOT expose BoxRepo's
                // `supports_atomic_batch` publicly, but its info store does:
                // `Store::supports_atomic_batch` is `true` only on raw
                // backends (raw fjall / raw in-memory) and keeps the default
                // `false` on wrapper stores (MemBufferStore/CachedStore) —
                // exactly the stacks whose receipt admission later fails with
                // `DbError::AtomicBatchUnsupported`. Refuse BEFORE spawning so
                // a seeded follower with a MemBuffer-wrapped destination can
                // never enter the warn+backoff+retry-forever loop. RESUME goes
                // through this same `start_subscription` path (reconcile →
                // start), so it is covered too.
                let info = destination
                    .tx_info_store()
                    .await
                    .map_err(|error| ReplError::Start(error.to_string()))?;
                if !info.supports_atomic_batch() {
                    let error = ReplError::AtomicBatchUnsupported {
                        db: db.clone(),
                        repo: repo.clone(),
                    };
                    self.local
                        .mark_subscription_terminal(
                            &sub.name,
                            "resync_required",
                            &error.to_string(),
                        )
                        .await
                        .map_err(|error| ReplError::Start(error.to_string()))?;
                    return Err(error);
                }
                // Gate-init failures map by KIND, not by string — see
                // `refuse_gate_init` below (typed refusal → terminal
                // `resync_required`; anything else → retryable `Start`).
                let gate = match destination.tx_gate().await {
                    Ok(gate) => gate,
                    Err(error) => return Err(self.refuse_gate_init(&sub.name, error).await),
                };
                if gate.requires_reopen() {
                    let error = ReplError::ReopenRequired {
                        db: db.clone(),
                        repo: repo.clone(),
                        detail: "Resume refused until destination is reopened".into(),
                    };
                    self.local
                        .mark_subscription_terminal(
                            &sub.name,
                            "reopen_required",
                            &error.to_string(),
                        )
                        .await
                        .map_err(|error| ReplError::Start(error.to_string()))?;
                    return Err(error);
                }
                // PR6b-3: refuse resume of a repo whose durable follower
                // state this version does not support (legacy bookmark,
                // malformed record, floor-without-bookmark evidence). A
                // refused repo can never be resumed — the operator reseeds.
                match bookmark_v3::classify_follower_bookmark(&info).await {
                    Err(shamir_db::storage::error::DbError::FollowerStateRefused { reason }) => {
                        let error = ReplError::FollowerStateRefused { reason };
                        self.local
                            .mark_subscription_terminal(
                                &sub.name,
                                "resync_required",
                                &error.to_string(),
                            )
                            .await
                            .map_err(|error| ReplError::Start(error.to_string()))?;
                        return Err(error);
                    }
                    Err(e) => return Err(ReplError::Start(e.to_string())),
                    Ok(state) => {
                        // R2-W3: admission-time refusal of Resume for a
                        // corrupt or retired durable receipt pointer — the v3
                        // record decodes fine, so this is NOT the typed
                        // refusal above, but a resume would only re-trip at
                        // the pipeline with zero progress. Refuse BEFORE any
                        // loop is spawned.
                        if let FollowerBookmarkState::Native(v3) = &state {
                            let resolves = bookmark_v3::receipt_pointer_resolves(&info, v3)
                                .await
                                .map_err(|error| ReplError::Start(error.to_string()))?;
                            if !resolves {
                                let error = ReplError::ReopenRequired {
                                    db: db.clone(),
                                    repo: repo.clone(),
                                    detail:
                                        "durable follower record references a corrupt or retired receipt pointer; reopen required"
                                            .into(),
                                };
                                self.local
                                    .mark_subscription_terminal(
                                        &sub.name,
                                        "reopen_required",
                                        &error.to_string(),
                                    )
                                    .await
                                    .map_err(|error| ReplError::Start(error.to_string()))?;
                                return Err(error);
                            }
                        }
                    }
                }
            }
        }
        let source = (self.factory)(sub);
        let cancel = CancellationToken::new();
        let mut joins = Vec::with_capacity(pull_targets.len());

        for (db, repo) in pull_targets.iter().cloned() {
            let mut cfg = FollowerLoopConfig::new(self.node_id.clone(), db, repo)
                .with_poll_wait_ms(self.poll_wait_ms)
                .with_validate_replicated_payload(self.validate_replicated_payload)
                // PR6b-3 stall visibility: let the loop persist a
                // status_message on this row after repeated no-progress
                // failures (the loop swallows transient errors internally,
                // so the supervisor can never observe them via the loop's
                // return value).
                .with_subscription(sub.name.clone());
            if let Some(n) = self.max_iterations {
                cfg = cfg.with_max_iterations(n);
            }
            let local = self.local.clone();
            let src = source.clone();
            let tok = cancel.clone();
            let sub_name = sub.name.clone();
            // §5.6 — each loop is its own task; never blocks the server.
            let join = tokio::spawn(async move {
                match run_follower_loop(local.clone(), src, cfg, tok).await {
                    Ok(()) => {}
                    Err(e @ ReplError::JournalGap { gap_at, .. })
                    | Err(
                        e @ ReplError::ApplyRejected {
                            leader_version: gap_at,
                            ..
                        },
                    ) => {
                        let from_version = if let ReplError::JournalGap { from_version, .. } = &e {
                            *from_version
                        } else {
                            gap_at
                        };
                        if let Err(mark_err) = local
                            .mark_subscription_resync_required(&sub_name, gap_at, from_version)
                            .await
                        {
                            warn!(
                                subscription = %sub_name,
                                error = %mark_err,
                                "supervisor: failed to persist resync_required after terminal replication rejection"
                            );
                        }
                        warn!(
                            subscription = %sub_name,
                            error = %e,
                            "follower loop terminated; subscription marked resync_required"
                        );
                    }
                    Err(
                        e @ (ReplError::IdentityMismatch { .. }
                        | ReplError::Seed(
                            SeedRefusal::WrongIdentity
                            | SeedRefusal::WrongHistoryEpoch
                            | SeedRefusal::MissingIdentity
                            | SeedRefusal::WrongSchema,
                        )),
                    ) => {
                        // S5c-X PR8 chunk 4 — SERVER FENCES: an identity/epoch
                        // fence trip means the leader is not the incarnation
                        // this subscription recorded. QUARANTINE the follower
                        // (state = "resync_required") so the split is visible
                        // and the loop stays stopped, exactly like the
                        // journal-gap path. The fence fires BEFORE decode/
                        // apply, so nothing was applied and the gap metadata
                        // is 0/0 (no version was reached).
                        let skip_quarantine = cfg!(test)
                            && super::identity_fence::checks::SKIP_FENCE_QUARANTINE_FOR_TEST
                                .load(Ordering::Relaxed);
                        if skip_quarantine {
                            warn!(
                                subscription = %sub_name,
                                error = %e,
                                "identity fence tripped; quarantine disabled by test seam"
                            );
                            // Negative control must exercise the UNPROTECTED
                            // path — the pre-quarantine production behaviour,
                            // i.e. the generic terminal mapping.
                            super::source::seed::terminal::persist_terminal(&local, &sub_name, &e)
                                .await;
                        } else {
                            // Same catalogue state the gap path persists via
                            // `mark_subscription_resync_required`
                            // (state = "resync_required"), but the message
                            // keeps the TYPED fence reason plus the recorded
                            // and observed identities instead of gap text —
                            // there is no gap; nothing was applied.
                            if let Err(mark_err) = local
                                .mark_subscription_terminal(
                                    &sub_name,
                                    "resync_required",
                                    &e.to_string(),
                                )
                                .await
                            {
                                warn!(
                                    subscription = %sub_name,
                                    error = %mark_err,
                                    "supervisor: failed to persist resync_required after identity fence trip"
                                );
                            }
                            metrics::counter!("shamir_repl_resync_required_total").increment(1);
                            warn!(
                                subscription = %sub_name,
                                error = %e,
                                "identity fence tripped; subscription marked resync_required"
                            );
                        }
                    }
                    Err(e @ ReplError::FollowerStateRefused { .. }) => {
                        // S5c-X PR6b-3 — TERMINAL: the follower repo's durable
                        // state is unsupported (no migration exists). Same
                        // quarantine family as the identity fence, WITHOUT
                        // the identity-fence test seam: always quarantined.
                        if let Err(mark_err) = local
                            .mark_subscription_terminal(
                                &sub_name,
                                "resync_required",
                                &e.to_string(),
                            )
                            .await
                        {
                            warn!(
                                subscription = %sub_name,
                                error = %mark_err,
                                "supervisor: failed to persist resync_required after follower-state refusal"
                            );
                        }
                        metrics::counter!("shamir_repl_resync_required_total").increment(1);
                        warn!(
                            subscription = %sub_name,
                            error = %e,
                            "follower state refused; subscription marked resync_required (reseed required)"
                        );
                    }
                    Err(e @ ReplError::AtomicBatchUnsupported { .. }) => {
                        // S5c-X PR6b-3 R7-2(ii) — TERMINAL: the destination
                        // backend cannot host the receipt pipeline (no atomic
                        // batches). Same quarantine family as the
                        // follower-state refusal: persist the typed terminal
                        // status ONCE with the remedy in the message; the loop
                        // never retries it.
                        if let Err(mark_err) = local
                            .mark_subscription_terminal(
                                &sub_name,
                                "resync_required",
                                &e.to_string(),
                            )
                            .await
                        {
                            warn!(
                                subscription = %sub_name,
                                error = %mark_err,
                                "supervisor: failed to persist resync_required after atomic-batch refusal"
                            );
                        }
                        metrics::counter!("shamir_repl_resync_required_total").increment(1);
                        warn!(
                            subscription = %sub_name,
                            error = %e,
                            "atomic batches unsupported on destination; subscription marked resync_required (re-create the repo raw)"
                        );
                    }
                    Err(e @ ReplError::ReopenRequired { .. }) => {
                        if let Err(error) = local
                            .mark_subscription_terminal(
                                &sub_name,
                                "reopen_required",
                                &e.to_string(),
                            )
                            .await
                        {
                            warn!(
                                subscription = %sub_name,
                                error = %error,
                                "failed to persist reopen_required"
                            );
                        }
                    }
                    Err(e) => {
                        super::source::seed::terminal::persist_terminal(&local, &sub_name, &e)
                            .await;
                        warn!(subscription = %sub_name, error = %e, "follower loop terminated");
                    }
                }
            });
            joins.push(join);
        }

        let handle = SubHandle {
            cancel,
            joins,
            profile: sub.profile.clone(),
            targets: pull_targets,
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
    let refs: TMap<String, QueryResult> = TMap::default();
    // S5c-X F1 (Round 6): the FilterContext is built engine-side; no
    // `&Interner` crosses the boundary.
    let ctx = table.info_store_read_view().filter_context(&refs).await?;
    let query = ReadQuery::new(table_name);
    let result = table.read(&query, &ctx).await?;
    Ok(result
        .records
        .into_iter()
        .map(|r| r.as_value().into_owned())
        .collect())
}
