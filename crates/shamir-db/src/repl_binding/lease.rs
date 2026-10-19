//! Per-subscription generation lease with a quiesce barrier (S5c-X PR8b-2
//! round 4, mechanism 2; round 5 A2/A3). Closes both stale-apply windows:
//! the interior of supervisor startup after `replication_control_lock` is
//! released, and the gap between a follower loop's row check and applying a
//! Pull response.

use crate::shamir_db::shamir_db::ReplicationControlGuard;
use crate::ShamirDb;

/// Per-subscription generation lease (round 4 mechanism 2). `valid` is the
/// fast liveness flag (Acquire/Release); `apply_gate` is the quiesce
/// barrier: followers hold a READ guard across each engine apply; every
/// lifecycle mutation takes the WRITE gate under the replication control
/// lock BEFORE mutating the row, draining any in-flight apply. Lock order:
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
    /// Issue a fresh lease for `(name, generation)` — round 5 A2 semantics:
    /// **refuse while a valid lease exists; invalidate + quiesce an invalid
    /// displaced lease.**
    ///
    /// WHY: every legitimate re-issue path invalidates its own lease first —
    /// a start-barrier failure invalidates its just-taken unused lease; a
    /// finished follower loop invalidates its own still-current lease; and a
    /// loop that exits because its lease was invalidated needs nothing. So a
    /// VALID existing lease always means a LIVE follower: refusing keeps
    /// exactly one live follower per subscription name and makes a second
    /// supervisor defer instead of flapping-kill the live one. An INVALID
    /// displaced lease may still have an in-flight apply from its dead
    /// generation, so it is quiesced (apply-gate write drain) before the new
    /// lease is installed — exactly the mutator protocol.
    ///
    /// `None` = refused (a valid lease exists — the caller must defer).
    /// The `_control` parameter proves the caller holds the replication
    /// control lock via [`ShamirDb::lock_replication_control`].
    /// `pub` (not `pub(crate)`): the supervisor lives in `shamir-server`
    /// and hands the lease to its follower loop.
    pub async fn take_subscription_lease(
        &self,
        name: &str,
        generation: u64,
        _control: &ReplicationControlGuard<'_>,
    ) -> Option<std::sync::Arc<SubscriptionLease>> {
        // Clone the Arc out of the DashMap ref; drop the ref before the
        // `.await` — a DashMap guard must never be held across an await.
        if let Some(existing) = self.leases.get(name).map(|e| e.value().clone()) {
            if existing.valid.load(std::sync::atomic::Ordering::Acquire) {
                // A live follower holds this lease — refuse; never touch it.
                return None;
            }
            // Already false (Release here is idempotent); drain the
            // in-flight apply of the dead generation before replacing.
            existing
                .valid
                .store(false, std::sync::atomic::Ordering::Release);
            let q = existing.apply_gate.write().await;
            drop(q);
        }
        let lease = std::sync::Arc::new(SubscriptionLease {
            generation,
            valid: std::sync::atomic::AtomicBool::new(true),
            apply_gate: tokio::sync::RwLock::new(()),
        });
        self.leases.insert(name.to_string(), lease.clone());
        Some(lease)
    }

    /// The subscription's current lease, if any (plain read; for tests and
    /// monitoring — e.g. a supervisor probing whether a live follower holds
    /// a lease before attempting to take its own).
    pub fn subscription_lease(&self, name: &str) -> Option<std::sync::Arc<SubscriptionLease>> {
        self.leases.get(name).map(|e| e.value().clone())
    }

    /// Invalidate the lease `lease` and REMOVE it from the registry, but
    /// ONLY if it is still the current entry (`Arc::ptr_eq`). For the
    /// start-barrier-failure caller that check always passes (freshly
    /// inserted); for the finished-loop caller it passes unless a new lease
    /// was issued meanwhile (a replaced lease is not ours to remove).
    ///
    /// Deliberately NO `apply_gate` write here: the caller is the lease's
    /// own finished follower loop or the failed start barrier, so no reader
    /// can be in flight; any remaining reader re-checks the `valid` flag
    /// under its read guard. Runs under the control lock via
    /// [`ShamirDb::lock_replication_control`].
    ///
    /// `pub` (not `pub(crate)`): W2's supervisor lives in `shamir-server`
    /// and calls this from its start-barrier failure path and its loop-exit
    /// wrapper; until W2 lands, shamir-db-side tests exercise it.
    pub fn invalidate_subscription_lease(
        &self,
        name: &str,
        lease: &SubscriptionLease,
        _control: &ReplicationControlGuard<'_>,
    ) {
        let remove = self
            .leases
            .get(name)
            // Arc identity via raw-pointer equality (same allocation).
            .map(|e| std::ptr::eq(std::sync::Arc::as_ptr(e.value()), lease))
            .unwrap_or(false);
        if remove {
            lease
                .valid
                .store(false, std::sync::atomic::Ordering::Release);
            self.leases.remove(name);
        }
    }

    /// Invalidate the subscription's current lease and drain an in-flight
    /// apply (blocks new ones). MUST run under the control lock and BEFORE
    /// the row mutation; no-op when no lease exists (never-started rows
    /// have none). The `_control` parameter proves the caller holds the
    /// replication control lock.
    pub(crate) async fn quiesce_subscription_lease_locked(
        &self,
        name: &str,
        _control: &ReplicationControlGuard<'_>,
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
