#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use shamir_collections::{TFxSet, THasher};
use shamir_db::engine::db_instance::logical_export::MaintenanceGate;
use shamir_db::ShamirDb;
use tracing::{debug, info, warn};

#[cfg(test)]
use super::handle::StartRaceHook;
use super::handle::SubHandle;
use super::ReplSourceFactory;
use crate::replication::error::ReplError;
#[cfg(test)]
use crate::replication::follower_loop::ApplyParkHook;
use crate::replication::source;

/// Supervises the lifecycle of follower loops driven by the declarative
/// subscription catalogue. See the module docs.
pub struct SubscriptionSupervisor {
    /// Local node — the follower onto which pulled events are applied, and the
    /// source of the `system/subscriptions` + `system/replication_profiles`
    /// catalogue.
    pub(crate) local: Arc<ShamirDb>,
    /// Builds the [`ReplSource`] transport for a subscription.
    pub(super) factory: ReplSourceFactory,
    /// Stable follower identity advertised in `ReplHello` (§5.2).
    pub(super) node_id: String,
    /// Active loops keyed by subscription name (lock-free, `THasher`). An
    /// `Arc` so each spawned loop can conditionally deregister its own entry
    /// on exit (round 5, A1) — `scc::HashMap` is not `Clone` for a
    /// non-`Clone` value (`JoinHandle`).
    pub(super) registry: std::sync::Arc<scc::HashMap<String, SubHandle, THasher>>,
    /// Names whose source factory has been invoked at least once during the
    /// subscription's current catalogue lifetime.
    pub(super) source_starts: scc::HashMap<String, (), THasher>,
    /// Iteration cap forwarded to each loop; `None` in prod (run forever),
    /// `Some(n)` in tests to bound the loop without relying on cancellation.
    pub(super) max_iterations: Option<usize>,
    /// Long-poll budget forwarded to each `pull` (ms).
    pub(super) poll_wait_ms: u32,
    pub(super) validate_replicated_payload: bool,
    /// Optional server-wide maintenance gate cloned into every spawned
    /// follower loop's [`FollowerLoopConfig`] (online logical-export
    /// enrolment). `None` unless installed by boot via
    /// [`Self::with_maintenance_gate`].
    pub(super) maintenance_gate: Option<MaintenanceGate>,
    // Cold catalogue lifecycle serialization; no follower write lock.
    reconcile_lock: tokio::sync::Mutex<()>,
    /// Test seam for replacing the row between lease-take and spawn.
    #[cfg(test)]
    pub(super) start_race_hook: Option<StartRaceHook>,
    #[cfg(test)]
    pub(super) apply_park: Option<Arc<ApplyParkHook>>,
    #[cfg(test)]
    pub(super) exit_hook: Option<Arc<ApplyParkHook>>,
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
        source::seed::fence::install_probe(&local, &factory);
        Self {
            local,
            factory,
            node_id: node_id.into(),
            registry: std::sync::Arc::new(scc::HashMap::with_hasher(THasher::default())),
            source_starts: scc::HashMap::with_hasher(THasher::default()),
            max_iterations: None,
            poll_wait_ms: 5000,
            validate_replicated_payload: false,
            maintenance_gate: None,
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
    pub(crate) fn with_apply_park(mut self, hook: Arc<ApplyParkHook>) -> Self {
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

    /// Enrol every loop this supervisor spawns in the server-wide
    /// maintenance gate: a raised logical-export freeze blocks each loop's
    /// apply (via `MaintenanceGate::wait_for_mutation` in
    /// `run_follower_loop`) and the loop's counted writer keeps the
    /// export's writer-drain correct. Boot owns the gate instance and
    /// installs it here once; all clones share the same admission.
    #[must_use]
    pub fn with_maintenance_gate(mut self, gate: MaintenanceGate) -> Self {
        self.maintenance_gate = Some(gate);
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
    /// `generation` is the start attempt's binding-generation snapshot — the
    /// fence for the mark below: a start whose row was concurrently replaced
    /// can never mark or quiesce the replacement (stale mark = a logged
    /// no-op inside `ShamirDb`).
    pub(in crate::sync::replication) async fn refuse_gate_init(
        &self,
        sub_name: &str,
        generation: u64,
        error: shamir_db::storage::error::DbError,
    ) -> ReplError {
        match error {
            shamir_db::storage::error::DbError::FollowerStateRefused { reason } => {
                let error = ReplError::FollowerStateRefused { reason };
                if let Err(persist) = self
                    .local
                    .mark_subscription_terminal_generation(
                        sub_name,
                        "resync_required",
                        &error.to_string(),
                        Some(generation),
                    )
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
    ///   already running under the same profile AND the same binding
    ///   generation → start its loop(s).
    /// - For each running loop whose subscription is gone, `paused`, rebound
    ///   to a different profile, or replaced under a new binding generation
    ///   → stop it (and, on rebind, the next pass restarts it under the new
    ///   profile / binding). A generation change means the binding was
    ///   replaced — an old task must never survive to mark or clear the
    ///   replacement.
    pub async fn reconcile(&self) {
        // Online-export enrolment (narrowest shared reconcile path): a pass
        // persists durable terminal subscription states (through
        // `start_subscription`) and admits loop replacements, which alter
        // exported-scope eligibility (`read_quarantine_reason`), so the pass
        // must hold the server-wide maintenance gate's counted writer before
        // taking the reconcile/control locks. Nonblocking admission: while a
        // freeze is raised, skip this tick entirely — the next tick (poke /
        // notify / retry) converges. The guard is held across the stop
        // phase's joins; a loop's exit wrapper parked on a frozen gate is
        // released by its cancel token (biased select in start.rs), so
        // joining a cancelled loop cannot deadlock against the freeze.
        let _maintenance = match &self.maintenance_gate {
            Some(gate) => match gate.enter_mutation() {
                Ok(guard) => Some(guard),
                Err(_) => {
                    debug!("supervisor: logical-export freeze raised; reconcile tick skipped");
                    return;
                }
            },
            None => None,
        };
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

        // Keep source lifecycle history only for subscriptions still present
        // in the catalogue; paused and failed subscriptions retain it.
        let current_names: TFxSet<&str> = subs.iter().map(|sub| sub.name.as_str()).collect();
        let mut removed_names = Vec::new();
        self.source_starts.iter_sync(|name, _| {
            if !current_names.contains(name.as_str()) {
                removed_names.push(name.clone());
            }
            true
        });
        for name in removed_names {
            self.source_starts.remove_sync(&name);
        }

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
}
