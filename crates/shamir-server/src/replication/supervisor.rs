//! 386-b — [`SubscriptionSupervisor`]: bind the declarative replication
//! catalogue (386-a) to the follower engine (R1-c).
//!
//! An **active** subscription row in `system/subscriptions` (persisted by
//! `admin_replication.rs`) must translate into a running
//! [`run_follower_loop`](super::follower_loop::run_follower_loop) pulling from
//! the subscription's `upstream` and applying onto the local `(db, repo)`
//! destinations recorded in the row's DURABLE binding (S5c-X PR8b-2) — the
//! bound profile's `pull` streams are only checked for coverage, never used
//! as the target source. `pause` / `resume` /
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
//! bound destination) + the profile name and binding generation it was
//! started against (so a profile rebind or a replaced binding can be
//! detected and the loop restarted).
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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_collections::{THasher, TMap};
use shamir_db::query::admin::ReplStream;
use shamir_db::query::read::{QueryResult, ReadQuery};
use shamir_db::repl_binding::SubscriptionLease;
use shamir_db::types::value::QueryValue;
use shamir_db::{SeedRefusal, ShamirDb, SubscriptionBinding};
use shamir_query_types::admin::ReplDirection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::error::ReplError;
use super::follower_loop::{run_follower_loop, FollowerLoopConfig};
use super::source::seed::startup_lease::StartupLeaseGuard;
use super::source::seed::terminal;
use super::source::ReplSource;

/// Test seam awaited between admission and spawn to replace the row in that window.
#[cfg(test)]
type StartRaceHook = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

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
    /// S5c-X PR8b-2: the row's durable binding generation. The DURABLE
    /// authority for loop identity on reconcile — a loop started under an
    /// older generation must never survive to mark or clear a replacement
    /// binding.
    pub binding_generation: u64,
    /// S5c-X PR8b-2: the row's durable destination set — the ENFORCED write
    /// boundary for follower targets and the coverage authority for
    /// quarantine. The replication profile is NOT the authority.
    pub binding_destinations: Vec<(String, String)>,
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
/// profile + binding generation it was started against (to detect a rebind
/// or a replaced binding on reconcile).
///
/// `lease` is the [`SubscriptionLease`] every loop of this handle runs
/// under; `dones[i]` is loop i's done flag (Release/Acquire), set by the
/// loop's own wrapper task AFTER `run_follower_loop` returned. A handle is
/// STALE when its lease went invalid OR all its loops finished.
/// (round 5, A1). NOT `Clone` — `JoinHandle` is not; the spawned loops get
/// an `Arc` of the shared registry instead.
struct SubHandle {
    cancel: CancellationToken,
    joins: Vec<JoinHandle<()>>,
    /// Per-loop done flags, parallel to `joins` (round 5, A1). One flag per
    /// spawned loop — a shared flag would wrongly mark the whole handle done
    /// when only one of several target loops finished.
    dones: Vec<Arc<AtomicBool>>,
    /// The lease all loops of this handle run under (round 5, A1). Arc
    /// identity of the lease identifies THIS handle: a newer start installs
    /// a handle carrying a different lease.
    lease: Arc<SubscriptionLease>,
    profile: String,
    generation: u64,
    targets: Vec<(String, String)>,
}

impl SubHandle {
    /// Round-5 A1: `true` when the handle is stale — its lease went invalid
    /// (superseded by a newer lease / invalidated by the loop's own exit) or
    /// every loop's wrapper task has set its done flag (the loops returned).
    /// A stale handle's rows must be re-started through the normal
    /// start-barrier path instead of being kept as running.
    fn stale(&self) -> bool {
        !self.lease.valid.load(Ordering::Acquire)
            || self.dones.iter().all(|d| d.load(Ordering::Acquire))
    }

    /// Cancel the loops and detach the join handles. Cancellation is GRACEFUL
    /// (token only, no `abort()`): each loop observes the token at its next
    /// await and returns, so the wrapper's exit housekeeping (done flag,
    /// lease invalidation, self-deregistration) always completes before the
    /// caller's `join.await` returns.
    fn stop(&self) {
        self.cancel.cancel();
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
    /// Active loops keyed by subscription name (lock-free, `THasher`). An
    /// `Arc` so each spawned loop can conditionally deregister its own entry
    /// on exit (round 5, A1) — `scc::HashMap` is not `Clone` for a
    /// non-`Clone` value (`JoinHandle`).
    registry: std::sync::Arc<scc::HashMap<String, SubHandle, THasher>>,
    /// Iteration cap forwarded to each loop; `None` in prod (run forever),
    /// `Some(n)` in tests to bound the loop without relying on cancellation.
    max_iterations: Option<usize>,
    /// Long-poll budget forwarded to each `pull` (ms).
    poll_wait_ms: u32,
    validate_replicated_payload: bool,
    // Cold catalogue lifecycle serialization; no follower write lock.
    reconcile_lock: tokio::sync::Mutex<()>,
    /// Test seam for replacing the row between lease-take and spawn.
    #[cfg(test)]
    start_race_hook: Option<StartRaceHook>,
    #[cfg(test)]
    apply_park: Option<Arc<super::follower_loop::ApplyParkHook>>,
    #[cfg(test)]
    exit_hook: Option<Arc<super::follower_loop::ApplyParkHook>>,
    #[cfg(test)]
    pub(crate) teardown_count: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) teardown_done: Arc<tokio::sync::Notify>,
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
            registry: std::sync::Arc::new(scc::HashMap::with_hasher(THasher::default())),
            max_iterations: None,
            poll_wait_ms: 5000,
            validate_replicated_payload: false,
            reconcile_lock: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            start_race_hook: None,
            #[cfg(test)]
            apply_park: None,
            #[cfg(test)]
            exit_hook: None,
            #[cfg(test)]
            teardown_count: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            teardown_done: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Install a hook awaited after admission and before spawn (tests
    /// only): the T5 seam that replaces the row between lease-take and
    /// spawn, exposing the startup interior window.
    #[cfg(test)]
    pub fn with_start_race_hook(mut self, hook: StartRaceHook) -> Self {
        self.start_race_hook = Some(hook);
        self
    }

    /// Test hook: park selected targets during apply and signal exits.
    #[cfg(test)]
    pub(crate) fn with_apply_park(
        mut self,
        hook: Arc<super::follower_loop::ApplyParkHook>,
    ) -> Self {
        self.apply_park = Some(Arc::clone(&hook));
        self.exit_hook = Some(hook);
        self
    }

    #[cfg(test)]
    pub(crate) fn teardown_count(&self) -> usize {
        self.teardown_count.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn teardown_signal(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.teardown_done)
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

    /// Live-loop count for test assertions.
    #[allow(clippy::disallowed_methods)] // O(N) ack: test/telemetry, not hot path
    pub fn active_count(&self) -> usize {
        self.registry.len()
    }

    /// `true` if a loop is currently registered for `name`.
    pub fn is_running(&self, name: &str) -> bool {
        self.registry.contains_sync(name)
    }

    /// Poke after admin changes; equivalent to `reconcile`.
    /// TODO(386-c): replace with a system changefeed watch.
    pub async fn notify_changed(&self) {
        self.reconcile().await;
    }

    /// Converge running loops to the declarative catalogue state. Idempotent.
    ///
    /// - For each `active` subscription with a resolvable profile and NOT
    ///   already running under the same profile AND the same binding
    ///   generation → start its loop(s).
    /// - For each running loop whose subscription is gone, `paused`, rebound
    ///   to a different profile, or replaced under a new binding generation
    ///   → stop it (and, on rebind, the next pass restarts it under the new
    ///   profile / binding). A generation change means the binding was
    ///   replaced — an old task must never survive to mark or clear the
    ///   replacement.
    pub async fn reconcile(&self) {
        let _lifecycle = self.reconcile_lock.lock().await;
        // Lock only the catalogue read; release before task stop/start callbacks.
        let control = self.local.lock_replication_control().await;
        let subs = match self.read_subscriptions().await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "supervisor: failed to read subscriptions catalogue");
                return;
            }
        };
        drop(control);

        // 1. Stop loops that should no longer run (removed / paused /
        // rebound / replaced binding) or whose handle went STALE (round 5,
        // A1): a stale handle's loop(s) have already returned or lost their
        // lease, so the row must be re-started through the normal
        // start-barrier path by this pass's start phase.
        let mut to_stop: Vec<(String, &'static str)> = Vec::new();
        self.registry.iter_sync(|name, handle| {
            let qualifies = subs.iter().any(|s| {
                &s.name == name
                    && s.is_active()
                    && s.profile == handle.profile
                    && s.binding_generation == handle.generation
            });
            if !qualifies {
                to_stop.push((name.clone(), "row no longer qualifies"));
            } else if handle.stale() {
                let reason = if !handle.lease.valid.load(Ordering::Acquire) {
                    "lease superseded"
                } else {
                    "task finished"
                };
                debug!(
                    subscription = %name,
                    reason,
                    "supervisor: stale follower handle; deregistering for restart"
                );
                to_stop.push((name.clone(), reason));
            }
            true
        });
        for (name, _reason) in to_stop {
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
            match self.start_subscription(sub).await {
                // Stale snapshot: expected under races — a concurrent admin
                // write changed the row between the catalogue read and the
                // start. The pass that observes the new row owns any
                // restart; no terminal marking.
                Ok(false) => {
                    debug!(
                        subscription = %sub.name,
                        "supervisor: stale snapshot; start dropped"
                    );
                }
                Ok(true) => {}
                Err(e) => {
                    warn!(subscription = %sub.name, error = %e, "supervisor: start failed");
                }
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
        // Joins when the entry is still registered; a loop that already
        // exited self-deregistered (round 5, A1), so absence means it is
        // already gone.
        let Some((_, handle)) = self.registry.remove_sync(name) else {
            return;
        };
        for join in handle.joins {
            join.await.expect("subscription task panicked");
        }
    }

    /// Start the follower loops for `sub`: one per destination recorded in
    /// the row's DURABLE binding (the enforced write boundary), after
    /// requiring the resolved profile's `pull` streams to cover every bound
    /// destination (keeps the admission/hello machinery honest).
    ///
    /// Returns `Ok(true)` when the loops were started and `Ok(false)` when
    /// the caller's snapshot turned out stale: nothing is marked and nothing
    /// is spawned. The reconcile pass that observes the new row owns any
    /// restart.
    pub(crate) async fn start_subscription(&self, sub: &Subscription) -> Result<bool, ReplError> {
        // Re-read under the control lock; a stale snapshot must not start or
        // mark anything. The new reconcile pass owns any restart.
        // Take the generation lease under the control lock; the start barrier
        // follows after the lock is released.
        let lease_guard = {
            let _control = self.local.lock_replication_control().await;
            let stale = match self.read_subscription_row(&sub.name).await {
                Ok(Some(row)) => {
                    let binding = SubscriptionBinding::from_row(&row)
                        .map_err(|e| ReplError::Start(e.to_string()))?;
                    row.get("state").and_then(|v| v.as_str()) != Some("active")
                        || binding.generation() != sub.binding_generation
                        || binding.destinations().to_vec() != sub.binding_destinations
                }
                Ok(None) => true,
                Err(e) => return Err(ReplError::Start(e)),
            };
            if stale {
                warn!(
                    subscription = %sub.name,
                    "supervisor: subscription row changed since the reconcile snapshot; start dropped"
                );
                return Ok(false);
            }
            // Round-5 A2 wiring: `None` = REFUSED — a valid lease for this
            // name exists somewhere (another supervisor's live follower).
            // Never kill it: defer; this supervisor's reconcile retries
            // later, and the refusal keeps exactly one live follower.
            let Some(lease) = self
                .local
                .take_subscription_lease(&sub.name, sub.binding_generation, &_control)
                .await
            else {
                debug!(
                    subscription = %sub.name,
                    "a valid lease for this subscription already exists; start deferred"
                );
                return Ok(false);
            };
            // Every start failure/cancellation invalidates the acquired lease.
            let lease_guard = StartupLeaseGuard::new(
                Arc::clone(&self.local),
                sub.name.clone(),
                Arc::clone(&lease),
            );
            // Round-4 MECHANISM 1: the start-time durability barrier — the
            // same catalogue flush CREATE/RESUME use. Only a successful
            // flush may start a follower; on failure the row stays
            // live-active and a later reconcile retries the barrier.
            if let Err(e) = self.local.flush_seed_catalogue().await {
                debug!(
                    subscription = %sub.name,
                    error = %e,
                    "supervisor: start barrier flush failed; start deferred (reconcile retries)"
                );
                // The unused lease must not linger valid (round-5 A2), or
                // the retry is refused forever by W1's valid-lease
                // semantics — the guard's Drop (on this return) cleans it.
                return Ok(false);
            }
            lease_guard
        };
        let lease = lease_guard.lease();
        // Interior window 1: the lease may have been invalidated between the
        // lease-take inside the block above and here (a concurrent lifecycle
        // mutation ran its quiesce before we re-check). Never start on an
        // invalid lease.
        if !lease.valid.load(Ordering::Acquire) {
            return Ok(false);
        }
        let streams = self
            .resolve_profile_streams(&sub.profile)
            .await
            .map_err(ReplError::Start)?;
        // The binding is the enforced boundary — targets come from the row,
        // NEVER from the profile. The profile is only checked for coverage.
        let profile_pulls: Vec<(String, String)> = streams
            .iter()
            .filter(|st| matches!(st.direction, ReplDirection::Pull))
            .filter_map(|st| {
                st.scope
                    .repo
                    .as_ref()
                    .map(|repo| (st.scope.db.clone(), repo.clone()))
            })
            .collect();
        for (db, repo) in &sub.binding_destinations {
            if !profile_pulls
                .iter()
                .any(|(p_db, p_repo)| p_db == db && p_repo == repo)
            {
                return Err(ReplError::Start(format!(
                    "profile '{}' does not cover bound destination '{}/{}'",
                    sub.profile, db, repo
                )));
            }
        }
        let pull_targets = sub.binding_destinations.clone();
        for (index, (db, repo)) in pull_targets.iter().enumerate() {
            // Interior-window check: an invalidated lease marks nothing in
            // this iteration and never admits a destination.
            if !lease.valid.load(Ordering::Acquire) {
                return Ok(false);
            }
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
                // Generation-aware: this runs synchronously inside a start
                // whose snapshot was just re-validated against the current
                // row under the control lock, so the generation passed here
                // is the row's CURRENT generation.
                self.local
                    .mark_subscription_terminal_generation(
                        &sub.name,
                        "duplicate_owner",
                        &error.to_string(),
                        Some(sub.binding_generation),
                    )
                    .await
                    .map_err(|error| ReplError::Start(error.to_string()))?;
                return Err(error);
            }
            if let Some(destination) = self.local.get_db(db).and_then(|db| db.get_repo(repo)) {
                let gate = destination
                    .tx_gate()
                    .await
                    .map_err(|error| ReplError::Start(error.to_string()))?;
                if gate.requires_reopen() {
                    let error = ReplError::ReopenRequired {
                        db: db.clone(),
                        repo: repo.clone(),
                        detail: "Resume refused until destination is reopened".into(),
                    };
                    // Generation-aware: this runs synchronously inside a
                    // start whose snapshot was just re-validated against the
                    // current row under the control lock, so the generation
                    // passed here is the row's CURRENT generation.
                    self.local
                        .mark_subscription_terminal_generation(
                            &sub.name,
                            "reopen_required",
                            &error.to_string(),
                            Some(sub.binding_generation),
                        )
                        .await
                        .map_err(|error| ReplError::Start(error.to_string()))?;
                    return Err(error);
                }
            }
        }
        let source = (self.factory)(sub);
        // T5 seam: awaited after admission and before spawn — the startup
        // interior window (round 4).
        #[cfg(test)]
        if let Some(hook) = &self.start_race_hook {
            hook().await;
        }
        // Interior window 2 (T5): re-check after the hook / between
        // admission and spawn; a superseded lease never spawns loops.
        if !lease.valid.load(Ordering::Acquire) {
            return Ok(false);
        }
        let cancel = CancellationToken::new();
        let mut joins = Vec::with_capacity(pull_targets.len());
        let mut dones: Vec<Arc<AtomicBool>> = Vec::with_capacity(pull_targets.len());
        // One live-count reference per target; only the final exit may
        // invalidate the shared lease after every apply guard is gone.
        let live_targets = Arc::new(AtomicUsize::new(pull_targets.len()));
        // Last exit tears down the shared lease.
        lease_guard.defuse();

        for (db, repo) in pull_targets.iter().cloned() {
            let mut cfg = FollowerLoopConfig::new(self.node_id.clone(), db, repo)
                .with_poll_wait_ms(self.poll_wait_ms)
                .with_validate_replicated_payload(self.validate_replicated_payload);
            if let Some(n) = self.max_iterations {
                cfg = cfg.with_max_iterations(n);
            }
            // Bind the loop to this subscription's lease.
            cfg.subscription = sub.name.clone();
            cfg.lease = Some(lease.clone());
            #[cfg(test)]
            {
                cfg.apply_park = self.apply_park.clone();
                cfg.exit_hook = self.exit_hook.clone();
            }

            let local = self.local.clone();
            let src = source.clone();
            let tok = cancel.clone();
            let sub_name = sub.name.clone();
            // Generation fences stale terminal callbacks from replacements.
            let generation = sub.binding_generation;
            // Per-loop done state shared with the handle and exit path.
            let done_flag = Arc::new(AtomicBool::new(false));
            let done = Arc::clone(&done_flag);
            let lease_for_exit = Arc::clone(&lease);
            let live = Arc::clone(&live_targets);
            #[cfg(test)]
            let teardown_count = Arc::clone(&self.teardown_count);
            #[cfg(test)]
            let teardown_done = Arc::clone(&self.teardown_done);
            #[cfg(test)]
            let exit_signal = cfg.exit_hook.clone();
            #[cfg(test)]
            let signal_other_exit = exit_signal
                .as_ref()
                .is_some_and(|hook| hook.repo.as_deref() != Some(cfg.repo.as_str()));

            let registry = self.registry.clone();
            // §5.6 — each loop is its own task; never blocks the server.
            let join = tokio::spawn(async move {
                // Retry terminal marking after the loop returns.
                let tok_exit = tok.clone();
                let loop_result = run_follower_loop(local.clone(), src, cfg, tok).await;
                #[cfg(test)]
                if signal_other_exit {
                    if let Some(hook) = &exit_signal {
                        hook.exited.notify_one();
                    }
                }
                match loop_result {
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
                        let (local_m, name_m) = (Arc::clone(&local), sub_name.clone());
                        terminal::mark_with_retry(
                            &local,
                            &sub_name,
                            "resync_required",
                            &tok_exit,
                            generation,
                            || {
                                let l = Arc::clone(&local_m);
                                let n = name_m.clone();
                                async move {
                                    l.mark_subscription_resync_required_generation(
                                        &n,
                                        gap_at,
                                        from_version,
                                        generation,
                                    )
                                    .await
                                }
                            },
                        )
                        .await;
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
                            // Negative control: the UNPROTECTED pre-quarantine
                            // path (generic terminal mapping), still retried.
                            terminal::persist_terminal_with_retry(
                                &local, &sub_name, &e, generation, &tok_exit,
                            )
                            .await;
                        } else {
                            // Same catalogue state the gap path persists via
                            // `mark_subscription_resync_required_generation`
                            // (state = "resync_required"), but the message
                            // keeps the TYPED fence reason plus the recorded
                            // and observed identities instead of gap text —
                            // there is no gap; nothing was applied.
                            let (local_m, name_m, msg_m) =
                                (Arc::clone(&local), sub_name.clone(), e.to_string());
                            terminal::mark_with_retry(
                                &local,
                                &sub_name,
                                "resync_required (identity fence)",
                                &tok_exit,
                                generation,
                                || {
                                    let l = Arc::clone(&local_m);
                                    let n = name_m.clone();
                                    let msg = msg_m.clone();
                                    async move {
                                        l.mark_subscription_terminal_generation(
                                            &n,
                                            "resync_required",
                                            &msg,
                                            Some(generation),
                                        )
                                        .await
                                    }
                                },
                            )
                            .await;
                            metrics::counter!("shamir_repl_resync_required_total").increment(1);
                            warn!(
                                subscription = %sub_name,
                                error = %e,
                                "identity fence tripped; subscription marked resync_required"
                            );
                        }
                    }
                    Err(e @ ReplError::ReopenRequired { .. }) => {
                        // Generation-aware: a replaced binding outlives the
                        // stale task's reopen mark. Round-5 B3: bounded,
                        // cancellation-aware retry.
                        let (local_m, name_m, msg_m) =
                            (Arc::clone(&local), sub_name.clone(), e.to_string());
                        terminal::mark_with_retry(
                            &local,
                            &sub_name,
                            "reopen_required",
                            &tok_exit,
                            generation,
                            || {
                                let l = Arc::clone(&local_m);
                                let n = name_m.clone();
                                let msg = msg_m.clone();
                                async move {
                                    l.mark_subscription_terminal_generation(
                                        &n,
                                        "reopen_required",
                                        &msg,
                                        Some(generation),
                                    )
                                    .await
                                }
                            },
                        )
                        .await;
                    }
                    Err(e) => {
                        terminal::persist_terminal_with_retry(
                            &local, &sub_name, &e, generation, &tok_exit,
                        )
                        .await;
                        warn!(subscription = %sub_name, error = %e, "follower loop terminated");
                    }
                }
                // ---- exit housekeeping: shared by every loop return path.
                // The apply guard is dropped before this point; decrement the
                // shared count, and let only the last loop invalidate/remove.
                done.store(true, Ordering::Release);
                if live.fetch_sub(1, Ordering::AcqRel) == 1 {
                    let control = local.lock_replication_control().await;
                    local.invalidate_subscription_lease(&sub_name, &lease_for_exit, &control);
                    drop(control);
                    let lease_check = Arc::clone(&lease_for_exit);
                    let _ =
                        registry.remove_if_sync(&sub_name, |h| Arc::ptr_eq(&h.lease, &lease_check));
                    #[cfg(test)]
                    {
                        teardown_count.fetch_add(1, Ordering::AcqRel);
                        teardown_done.notify_one();
                    }
                }
            });
            joins.push(join);
            dones.push(done_flag);
        }

        let handle = SubHandle {
            cancel,
            joins,
            dones,
            lease,
            profile: sub.profile.clone(),
            generation: sub.binding_generation,
            targets: pull_targets,
        };
        // A racing reconcile may have inserted already; if so, stop the loops
        // we just spawned rather than leak them.
        if let Err((_, handle)) = self.registry.insert_sync(sub.name.clone(), handle) {
            handle.stop();
        } else {
            info!(subscription = %sub.name, profile = %sub.profile, "supervisor: started follower loop");
        }
        Ok(true)
    }

    /// Read the CURRENT catalogue row for one subscription by name,
    /// mirroring [`Self::read_subscriptions`]' public read path.
    async fn read_subscription_row(&self, name: &str) -> Result<Option<QueryValue>, String> {
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
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name)))
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
        records
            .into_iter()
            .map(|r| {
                // S5c-X PR8b-2: the binding is strictly validated; a row
                // that fails is a typed catalogue error naming the
                // subscription. Reconcile logs a warning and leaves running
                // loops alone — fail-safe, never fail-open.
                let binding = SubscriptionBinding::from_row(&r).map_err(|e| e.to_string())?;
                let (binding_generation, binding_destinations) =
                    (binding.generation(), binding.destinations().to_vec());
                Ok(Subscription {
                    name: r
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "subscription row is missing its 'name' string".to_string())?
                        .to_string(),
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
                    binding_generation,
                    binding_destinations,
                })
            })
            .collect()
    }
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
/// `pub(crate)` (round-6 item 2): `seed/terminal.rs`'s generation-currency
/// check reuses this reader instead of duplicating it.
pub(crate) async fn read_all(
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
