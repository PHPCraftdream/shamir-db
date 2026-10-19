//! Round-6 item 3 — RAII startup lease for `SubscriptionSupervisor::
//! start_subscription`. A subscription lease is issued early in every start
//! (under the replication control lock), but the start can still fail or be
//! CANCELLED afterwards (profile-resolution error, coverage mismatch,
//! admission error, or the caller simply dropping the in-flight
//! `start_subscription` future). Without RAII, every such exit strands a
//! VALID lease with no follower behind it, and every later
//! `take_subscription_lease` is refused forever (W1's valid-lease semantics)
//! — the subscription can never start again until an unrelated lifecycle
//! mutation invalidates the lease.
//!
//! The guard is created at issuance and DEFUSED exactly where the spawned
//! loops take ownership of the lease (their exit housekeeping — round-6
//! item 4's last-exit teardown — owns cleanup from there on). On any other
//! exit path the guard is dropped and cleans the lease up.

use std::sync::Arc;

use shamir_db::repl_binding::SubscriptionLease;
use shamir_db::ShamirDb;

/// RAII guard over a just-issued subscription startup lease. See the module
/// docs. NOT `Clone` — exactly one guard owns the startup phase.
pub(crate) struct StartupLeaseGuard {
    local: Arc<ShamirDb>,
    name: String,
    lease: Arc<SubscriptionLease>,
    defused: bool,
}

impl StartupLeaseGuard {
    /// Take ownership of `lease` (freshly issued for `(name, generation)`).
    /// From this point every drop of the guard — i.e. any early exit or
    /// cancellation of the start — invalidates the lease.
    pub(crate) fn new(
        local: Arc<ShamirDb>,
        name: impl Into<String>,
        lease: Arc<SubscriptionLease>,
    ) -> Self {
        Self {
            local,
            name: name.into(),
            lease,
            defused: false,
        }
    }

    /// The guarded lease (cloned into loop configs while the guard is alive).
    pub(crate) fn lease(&self) -> Arc<SubscriptionLease> {
        Arc::clone(&self.lease)
    }

    /// Hand ownership over to the spawned follower loops (their exit
    /// housekeeping invalidates/deregisters). A defused guard's `Drop` is a
    /// no-op.
    pub(crate) fn defuse(mut self) {
        self.defused = true;
    }
}

impl Drop for StartupLeaseGuard {
    fn drop(&mut self) {
        if self.defused {
            return;
        }
        // WHY a spawned task: Drop runs synchronously on whatever thread
        // drops the guard (including mid-poll when a caller drops an
        // in-flight `start_subscription` future) and has no executor context
        // to `.await` on — async cleanup (control lock + apply-gate drain)
        // cannot run inside Drop. A detached task is correct here: the
        // supervisor/task graph no longer references the lease, so the
        // cleanup is fire-and-forget, and every test observing it waits via
        // a bounded yield/notify budget.
        let (local, name, lease) = (
            Arc::clone(&self.local),
            self.name.clone(),
            Arc::clone(&self.lease),
        );
        tokio::spawn(async move {
            // Lock order: control lock → apply_gate write (the documented
            // mutator order). The write-gate drain is defensive: the start
            // never spawned a loop, but the lease was registered and may in
            // principle have been observed by a concurrent reader; drain any
            // in-flight apply before invalidating.
            let control = local.lock_replication_control().await;
            let gate = lease.apply_gate.write().await;
            // Removes only when ptr-current (a newer lease is not ours to
            // touch) — same semantics as the loop-exit teardown.
            local.invalidate_subscription_lease(&name, &lease, &control);
            drop(gate);
            drop(control);
        });
    }
}
