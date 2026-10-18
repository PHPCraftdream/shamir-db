//! Per-subscription generation lease with a quiesce barrier (S5c-X PR8b-2
//! round 4, mechanism 2). Closes both stale-apply windows: the interior of
//! supervisor startup after `replication_control_lock` is released, and the
//! gap between a follower loop's row check and applying a Pull response.

use crate::ShamirDb;

/// Per-subscription generation lease (round 4 mechanism 2). `valid` is the
/// fast liveness flag (Acquire/Release); `apply_gate` is the quiesce
/// barrier: followers hold a READ guard across each engine apply; every
/// lifecycle mutation takes the WRITE gate under `replication_control_lock`
/// BEFORE mutating the row, draining any in-flight apply. Lock order:
/// control lock → `apply_gate.write()`. The follower never takes the
/// control lock while holding the read guard; its own terminal marking
/// runs in the supervisor wrapper after the loop returned and the guard is
/// dropped.
pub struct SubscriptionLease {
    /// Binding generation this lease was issued for.
    pub generation: u64,
    /// Liveness flag: cleared (Release) before any lifecycle mutation;
    /// followers re-check (Acquire) under their read guard before applying.
    pub valid: std::sync::atomic::AtomicBool,
    /// Quiesce barrier — see the struct docs for the lock order.
    pub apply_gate: tokio::sync::RwLock<()>,
}

impl ShamirDb {
    /// Issue a fresh lease for `(name, generation)`, replacing any previous
    /// lease (previous holders saw their own `Arc` invalidated by the
    /// quiesce that preceded the row change). The `_control` parameter
    /// proves the caller holds `replication_control_lock` — the guard
    /// parameter enforces the caller-holds-lock contract.
    /// `pub` (not `pub(crate)`): the supervisor lives in `shamir-server`
    /// and hands the lease to its follower loop.
    pub fn take_subscription_lease(
        &self,
        name: &str,
        generation: u64,
        _control: &tokio::sync::MutexGuard<'_, ()>,
    ) -> std::sync::Arc<SubscriptionLease> {
        let lease = std::sync::Arc::new(SubscriptionLease {
            generation,
            valid: std::sync::atomic::AtomicBool::new(true),
            apply_gate: tokio::sync::RwLock::new(()),
        });
        self.leases.insert(name.to_string(), lease.clone());
        lease
    }

    /// Invalidate the subscription's current lease and drain an in-flight
    /// apply (blocks new ones). MUST run under the control lock and BEFORE
    /// the row mutation; no-op when no lease exists (never-started rows
    /// have none). The `_control` parameter proves the caller holds
    /// `replication_control_lock`.
    pub(crate) async fn quiesce_subscription_lease_locked(
        &self,
        name: &str,
        _control: &tokio::sync::MutexGuard<'_, ()>,
    ) {
        // Clone the Arc out of the DashMap ref and DROP THE REF before the
        // `.await` — a DashMap guard must never be held across an await.
        if let Some(lease) = self.leases.get(name).map(|e| e.value().clone()) {
            lease
                .valid
                .store(false, std::sync::atomic::Ordering::Release);
            let _q = lease.apply_gate.write().await;
            drop(_q);
        }
    }
}
