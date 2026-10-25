//! Fuel-lease accounting for the WASM host (P0.1 exclusive-lease design).
//!
//! The shared counter threaded via [`FnCtx::with_fuel_budget`](super::super::runtime::context::FnCtx)
//! is the FREE-capacity pool: a Store leases its fuel out of the pool
//! BEFORE it becomes executable ([`FuelLease::take`]), and unused fuel is
//! refunded on settlement. [`FuelLease`] is the RAII reservation token: it
//! owns the reserved amount and refunds it on drop unless the lease is
//! explicitly consumed (i.e. the amount was installed into a live Store
//! whose settlement now owns the refund), so a failed `set_fuel` install
//! can never strand capacity.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Atomically take up to `max` from the free-capacity pool; returns the
/// amount actually taken (0 when the pool is empty). Never underflows.
pub(super) fn pool_take(pool: &AtomicU64, max: u64) -> u64 {
    pool.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |free| {
        (free > 0).then(|| free - free.min(max))
    })
    .map_or(0, |prev| prev.min(max))
}

/// Return previously reserved fuel to the free-capacity pool.
pub(super) fn pool_refund(pool: &AtomicU64, amount: u64) {
    pool.fetch_add(amount, Ordering::Relaxed);
}

/// RAII reservation of an exclusive fuel lease from the shared pool.
///
/// Created by [`FuelLease::take`]. The reserved amount is debited from the
/// pool at construction time and refunded exactly once, on drop, unless:
///
/// * [`FuelLease::consume`] defuses the refund — used after the amount was
///   successfully installed into a Store whose settlement now owns the
///   remainder (any failure before that point simply drops the lease).
///
/// This makes the "reserve before installing executable fuel, refund even
/// if installation fails" discipline structural: the `?`-on-`set_fuel`
/// error path drops the lease and the pool is made whole.
pub(super) struct FuelLease {
    pool: Option<Arc<AtomicU64>>,
    amount: u64,
}

impl FuelLease {
    /// Reserve up to `max` units from `pool` (0 when the pool is empty).
    pub(super) fn take(pool: &Arc<AtomicU64>, max: u64) -> Self {
        Self {
            pool: Some(pool.clone()),
            amount: pool_take(pool, max),
        }
    }

    /// The reserved amount.
    pub(super) fn amount(&self) -> u64 {
        self.amount
    }

    /// Transfer ownership of the reserved amount to the caller (the Store's
    /// settlement path) without refunding it.
    pub(super) fn consume(mut self) -> u64 {
        self.pool = None;
        self.amount
    }
}

impl Drop for FuelLease {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool_refund(&pool, self.amount);
        }
    }
}

/// Per-invocation, test-only observability of the fuel-lease protocol
/// (`cfg(test)`): one [`StoreRecord`] per entered Store, with the ACTUAL
/// `Store::get_fuel` value captured at every protocol step — after each
/// `set_fuel` install (reservation vs installed), after each suspension
/// revoke (must be 0), and after each settlement (must be 0) — plus the
/// per-Store segment consumption `baseline − current` at each suspend and
/// settle. Carried parent→child through the `FnCtx`/`HostState` plumbing
/// exactly like the fuel pool itself, so parallel tests never share
/// counters.
///
/// Two repair properties (both RV-WASM-HOST-6F709AC2 test-oracle work):
///
/// * **Chronological event tracking.** Every install and suspension gets a
///   monotonic event sequence number assigned under the SAME observer
///   Mutex discipline; the "last X" getters scan for the MAX sequence —
///   never "the last-created Store's most recent record" (the old
///   reverse-creation-order scan reported stale events whenever an
///   earlier-created Store acted again later, e.g. a parent reacquiring
///   after its child installed).
/// * **Calibration independence.** Each Store's segment consumption is
///   computed from the observer's OWN per-Store baseline (the ACTUAL
///   installed fuel observed at the last `record_install`, reset to 0 at
///   every suspend/settle) — never from the production `fuel_baseline` —
///   so a missed production reset cannot corrupt the recorded demand. A
///   production `baseline` argument that disagrees with the observer's own
///   baseline is recorded as an inconsistency (checked arithmetic; the
///   inconsistency is never saturated away) and surfaced by
///   [`FuelObserver::check_settled`].
///
/// A `std::sync::Mutex` guards the (tiny, low-frequency, test-only)
/// record vector; production paths never touch it. The `stores` Mutex is
/// always acquired BEFORE the `inconsistencies` Mutex (never the reverse),
/// so there is no lock-order inversion.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FuelObserver {
    /// Total fuel seeded into a lazily-created pool (full-u64 audit; a
    /// single seeding per invocation, so no summation overflow).
    seeded: AtomicU64,
    /// Number of pool seedings observed.
    seeded_count: AtomicU64,
    next_store_id: AtomicU64,
    /// Monotonic event sequence for chronological "last X" getters.
    next_event: AtomicU64,
    stores: std::sync::Mutex<Vec<StoreRecord>>,
    /// Recorded calibration/protocol inconsistencies (empty in healthy
    /// runs; surfaced by `check_settled`). Guarded by its own Mutex,
    /// always acquired AFTER `stores`.
    inconsistencies: std::sync::Mutex<Vec<String>>,
    /// Store fuel observed at the `db_get` import boundary (child Store).
    gate_child_fuel: AtomicU64,
    /// Number of gate-boundary fuel observations.
    gate_observed: AtomicU64,
}

/// One install of executable fuel into a Store: the RESERVATION taken from
/// the pool vs the fuel ACTUALLY observed in the Store right after the
/// `set_fuel`, and the Store's own remaining cap at that moment.
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct InstallRecord {
    /// Chronological event sequence (max wins in the "last" getters).
    seq: u64,
    reserved: u64,
    actual: u64,
    own_cap: u64,
}

/// One suspension (parent revoke before a nested call), with its
/// chronological event sequence. The production `baseline` argument is
/// compared against the observer's own baseline in `record_suspend` and
/// is not stored (a mismatch is recorded as an inconsistency instead).
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct SuspendRecord {
    seq: u64,
    before: u64,
    after: u64,
}

/// One entered Store's complete protocol trace.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct StoreRecord {
    /// The Store identity allocated at entry (`HostState::store_id`).
    id: u64,
    installs: Vec<InstallRecord>,
    /// One record per suspension (revoke before a nested call).
    suspends: Vec<SuspendRecord>,
    /// Set once settlement ran for this Store.
    settles: u64,
    /// (baseline, fuel_before, fuel_after_set0) of the settlement.
    settle: Option<(u64, u64, u64)>,
    /// Sum of this Store's segment consumptions (baseline − current at
    /// each suspend and at settle). Small deltas — no full-u64 overflow.
    burned: u64,
    /// The most recently observed fuel value in this Store (after the last
    /// install, revoke or settle).
    last_fuel: u64,
    /// The observer's OWN calibration baseline for this Store: the actual
    /// installed fuel observed at the last install, reset to 0 at every
    /// suspend/settle. Segment deltas are computed from THIS value — not
    /// from the production `fuel_baseline` — so calibration is independent
    /// of production baseline bookkeeping.
    obs_baseline: u64,
}

#[cfg(test)]
impl FuelObserver {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Allocate the identity of a Store about to be entered.
    pub(crate) fn next_store_id(&self) -> u64 {
        self.next_store_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Test-only RAII probe for the reservation token, defined here
    /// (rather than re-exporting the `pub(super)` type): exercises
    /// take/drop and take/consume against a scratch pool and returns
    /// `(after_take, after_drop, after_consume, consumed)` — pool levels
    /// plus the consumed amount. Proves: drop refunds the whole
    /// reservation; `consume` takes the token BY VALUE (its defused Drop
    /// runs inside `consume`), so the debit stays and no refund fires.
    #[cfg(test)]
    pub(crate) fn lease_raii_probe(initial: u64, take: u64) -> (u64, u64, u64, u64) {
        let pool = Arc::new(AtomicU64::new(initial));
        let level = |p: &Arc<AtomicU64>| p.load(Ordering::SeqCst);
        let lease = FuelLease::take(&pool, take);
        let amount = lease.amount();
        let after_take = level(&pool);
        drop(lease);
        let after_drop = level(&pool);
        // `consume` moves the token: the (defused) Drop runs inside it, so
        // `after_consume` is already the post-drop level — no further drop
        // is possible or needed.
        let consumed = FuelLease::take(&pool, take).consume();
        let after_consume = level(&pool);
        debug_assert_eq!(amount, take.min(initial));
        (after_take, after_drop, after_consume, consumed)
    }

    pub(crate) fn record_seed(&self, amount: u64) {
        self.seeded.store(amount, Ordering::SeqCst);
        self.seeded_count.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn record_install(&self, store_id: u64, reserved: u64, actual: u64, own_cap: u64) {
        let seq = self.next_event.fetch_add(1, Ordering::SeqCst);
        let mut stores = self.stores.lock().expect("observer mutex");
        let rec = Self::record_of(&mut stores, store_id);
        rec.installs.push(InstallRecord {
            seq,
            reserved,
            actual,
            own_cap,
        });
        rec.last_fuel = actual;
        // Calibration independence: remember OUR OWN last actually-installed
        // baseline for this Store (never the production `fuel_baseline`).
        rec.obs_baseline = actual;
    }

    /// Record one suspension. `baseline` is the production `fuel_baseline`
    /// (compared against — but never used as — the observer's own
    /// baseline); the segment delta is computed from the OBSERVER's own
    /// baseline with checked arithmetic: `before > baseline` or a
    /// production/observer baseline mismatch is recorded as an
    /// inconsistency and surfaced by `check_settled` (never saturated
    /// away), isolating a missed production baseline reset.
    pub(crate) fn record_suspend(&self, store_id: u64, baseline: u64, before: u64, after: u64) {
        let seq = self.next_event.fetch_add(1, Ordering::SeqCst);
        let mut stores = self.stores.lock().expect("observer mutex");
        let rec = Self::record_of(&mut stores, store_id);
        if baseline != rec.obs_baseline {
            self.note_inconsistency(format!(
                "store #{store_id} suspend: production fuel_baseline {baseline} != \
                 observer baseline {} (missed production reset?)",
                rec.obs_baseline
            ));
        }
        let delta = match rec.obs_baseline.checked_sub(before) {
            Some(d) => d,
            None => {
                self.note_inconsistency(format!(
                    "store #{store_id} suspend: observed fuel {before} exceeds the \
                     observer's own baseline {}",
                    rec.obs_baseline
                ));
                0
            }
        };
        match rec.burned.checked_add(delta) {
            Some(sum) => rec.burned = sum,
            None => self.note_inconsistency(format!(
                "store #{store_id} suspend: burned counter overflow (delta {delta})"
            )),
        }
        rec.suspends.push(SuspendRecord { seq, before, after });
        rec.last_fuel = after;
        // The observer's baseline resets with the segment (mirrors the
        // production protocol: the Store now holds 0 executable fuel).
        rec.obs_baseline = 0;
    }

    /// Record one settlement; same independent-calibration rules as
    /// [`FuelObserver::record_suspend`].
    pub(crate) fn record_settle(&self, store_id: u64, baseline: u64, before: u64, after: u64) {
        let mut stores = self.stores.lock().expect("observer mutex");
        let rec = Self::record_of(&mut stores, store_id);
        if baseline != rec.obs_baseline {
            self.note_inconsistency(format!(
                "store #{store_id} settle: production fuel_baseline {baseline} != \
                 observer baseline {} (missed production reset?)",
                rec.obs_baseline
            ));
        }
        let delta = match rec.obs_baseline.checked_sub(before) {
            Some(d) => d,
            None => {
                self.note_inconsistency(format!(
                    "store #{store_id} settle: observed fuel {before} exceeds the \
                     observer's own baseline {}",
                    rec.obs_baseline
                ));
                0
            }
        };
        match rec.burned.checked_add(delta) {
            Some(sum) => rec.burned = sum,
            None => self.note_inconsistency(format!(
                "store #{store_id} settle: burned counter overflow (delta {delta})"
            )),
        }
        rec.settles += 1;
        rec.settle = Some((baseline, before, after));
        rec.last_fuel = after;
        rec.obs_baseline = 0;
    }

    /// Record a calibration inconsistency (called while the `stores` guard
    /// is held — the `inconsistencies` Mutex is always acquired second).
    fn note_inconsistency(&self, msg: String) {
        self.inconsistencies
            .lock()
            .expect("observer mutex")
            .push(msg);
    }

    pub(crate) fn record_gate_fuel(&self, store_fuel: u64) {
        self.gate_child_fuel.store(store_fuel, Ordering::SeqCst);
        self.gate_observed.fetch_add(1, Ordering::SeqCst);
    }

    fn record_of(stores: &mut Vec<StoreRecord>, store_id: u64) -> &mut StoreRecord {
        if let Some(i) = stores.iter().position(|r| r.id == store_id) {
            return &mut stores[i];
        }
        stores.push(StoreRecord {
            id: store_id,
            ..StoreRecord::default()
        });
        let i = stores.len() - 1;
        &mut stores[i]
    }

    pub(crate) fn seeded(&self) -> u64 {
        self.seeded.load(Ordering::SeqCst)
    }

    pub(crate) fn seeded_count(&self) -> u64 {
        self.seeded_count.load(Ordering::SeqCst)
    }

    pub(crate) fn gate_child_fuel(&self) -> u64 {
        self.gate_child_fuel.load(Ordering::SeqCst)
    }

    pub(crate) fn gate_observed(&self) -> u64 {
        self.gate_observed.load(Ordering::SeqCst)
    }

    /// Total number of `set_fuel` installs across all Stores.
    pub(crate) fn install_count(&self) -> u64 {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .map(|r| r.installs.len() as u64)
            .sum()
    }

    /// The CHRONOLOGICALLY most recent install's RESERVED amount (max event
    /// sequence across all Stores — the last-created Store's most recent
    /// install is NOT necessarily the latest event).
    pub(crate) fn last_install(&self) -> u64 {
        self.last_install_field(|i| i.reserved)
    }

    /// The CHRONOLOGICALLY most recent install's ACTUALLY-installed fuel
    /// (must equal the reservation — a mismatch is a clamp/cast bug in the
    /// install path).
    pub(crate) fn last_install_actual(&self) -> u64 {
        self.last_install_field(|i| i.actual)
    }

    fn last_install_field(&self, field: impl Fn(&InstallRecord) -> u64) -> u64 {
        let stores = self.stores.lock().expect("observer mutex");
        let mut best: Option<(u64, u64)> = None; // (seq, value)
        for rec in stores.iter() {
            for inst in rec.installs.iter() {
                if best.is_none_or(|(s, _)| inst.seq > s) {
                    best = Some((inst.seq, field(inst)));
                }
            }
        }
        best.map_or(0, |(_, v)| v)
    }

    /// Number of parent suspensions.
    pub(crate) fn suspends(&self) -> u64 {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .map(|r| r.suspends.len() as u64)
            .sum()
    }

    /// The CHRONOLOGICALLY most recent suspension's refunded remainder (the
    /// Store's fuel observed immediately BEFORE its revoke) — max event
    /// sequence, never reverse-creation order.
    pub(crate) fn suspended_fuel_last(&self) -> u64 {
        let stores = self.stores.lock().expect("observer mutex");
        let mut best: Option<(u64, u64)> = None; // (seq, before)
        for rec in stores.iter() {
            for s in rec.suspends.iter() {
                if best.is_none_or(|(sq, _)| s.seq > sq) {
                    best = Some((s.seq, s.before));
                }
            }
        }
        best.map_or(0, |(_, v)| v)
    }

    /// This Store's independently recorded total segment consumption
    /// (sum of the observer-baseline deltas at each suspend and settle).
    /// Lets tests read the EXACT recorded demand per Store instead of
    /// deriving it from pool arithmetic.
    pub(crate) fn store_burned(&self, store_id: u64) -> Option<u64> {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .find(|r| r.id == store_id)
            .map(|r| r.burned)
    }

    /// This Store's per-install own-cap observations (entry + reacquires,
    /// in install order).
    pub(crate) fn store_install_own_caps(&self, store_id: u64) -> Vec<u64> {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .find(|r| r.id == store_id)
            .map(|r| r.installs.iter().map(|i| i.own_cap).collect())
            .unwrap_or_default()
    }

    /// This Store's per-install reserved amounts (entry + reacquires, in
    /// install order).
    pub(crate) fn store_install_reserves(&self, store_id: u64) -> Vec<u64> {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .find(|r| r.id == store_id)
            .map(|r| r.installs.iter().map(|i| i.reserved).collect())
            .unwrap_or_default()
    }

    /// This Store's per-suspend `fuel_before_revoke` observations (in
    /// chronological order).
    pub(crate) fn store_suspend_befores(&self, store_id: u64) -> Vec<u64> {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .find(|r| r.id == store_id)
            .map(|r| r.suspends.iter().map(|s| s.before).collect())
            .unwrap_or_default()
    }

    /// Number of Stores that have settled.
    pub(crate) fn settles(&self) -> u64 {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .filter(|r| r.settles > 0)
            .count() as u64
    }

    /// Number of entered Stores whose most recently observed fuel is still
    /// nonzero (Stores currently holding executable fuel).
    pub(crate) fn stores_with_fuel(&self) -> u64 {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .filter(|r| r.last_fuel > 0)
            .count() as u64
    }

    /// Exact fuel actually burned, summed from INDEPENDENT per-Store
    /// segment deltas (baseline − current at each suspend and settle) —
    /// never from pool-side reservation arithmetic, so it cannot be
    /// fooled by a pool-mutation that still records true Store fuel.
    pub(crate) fn burned(&self) -> u64 {
        self.stores
            .lock()
            .expect("observer mutex")
            .iter()
            .map(|r| r.burned)
            .sum()
    }

    /// Structural settlement invariants, per Store — counts alone do not
    /// prove "each Store settled exactly once" or "no fuel left behind":
    /// * no recorded calibration inconsistency (production baseline vs
    ///   observer baseline mismatch, or an inverted segment delta);
    /// * every entered Store settled, exactly once;
    /// * every suspension observed fuel EXACTLY 0 after its revoke;
    /// * every settlement observed fuel EXACTLY 0 after its clear;
    /// * every install's reservation equalled the actually-installed fuel;
    /// * each reacquire's recorded own_cap equals the previous own_cap minus
    ///   the ACTUAL segment delta (own_cap_left reset/refill detector).
    pub(crate) fn check_settled(&self) -> Result<(), String> {
        let inconsistencies = self.inconsistencies.lock().expect("observer mutex");
        if let Some(first) = inconsistencies.first() {
            return Err(first.clone());
        }
        drop(inconsistencies);
        let stores = self.stores.lock().expect("observer mutex");
        for rec in stores.iter() {
            if rec.settles != 1 {
                return Err(format!(
                    "store #{} settled {} times (must be exactly 1)",
                    rec.id, rec.settles
                ));
            }
            for (i, s) in rec.suspends.iter().enumerate() {
                if s.after != 0 {
                    return Err(format!(
                        "store #{} suspend {i} left {} fuel installed (must revoke to 0)",
                        rec.id, s.after
                    ));
                }
            }
            match rec.settle {
                Some((_, _, 0)) => {}
                Some((_, before, after)) => {
                    return Err(format!(
                        "store #{} settled with before={before}, after={after} (must clear to 0)",
                        rec.id
                    ))
                }
                None => return Err(format!("store #{} never settled", rec.id)),
            }
            for inst in rec.installs.iter() {
                if inst.reserved != inst.actual {
                    return Err(format!(
                        "store #{} install reserved={} but actual={} (clamp/cast bug)",
                        rec.id, inst.reserved, inst.actual
                    ));
                }
                if inst.actual > inst.own_cap {
                    return Err(format!(
                        "store #{} installed {} beyond its own per-Store ceiling {}",
                        rec.id, inst.actual, inst.own_cap
                    ));
                }
            }
            // OWN-cap evolution (structural): install k+1's recorded
            // own_cap must EQUAL install k's own_cap minus the ACTUAL
            // segment delta (install k's reserve − the fuel observed
            // before the k-th revoke), in checked arithmetic — so a
            // production `own_cap_left` reset/refill (or a skipped
            // decrement) fails settlement invariants even when the
            // reacquire's `min(parent_unused)` bound masks the mutation
            // behaviorally. (Event order in the lease protocol is
            // install k → suspend k → install k+1; a trailing settle has
            // no following install and is not part of this check.)
            for k in 0..rec.suspends.len() {
                let Some(prev) = rec.installs.get(k) else {
                    return Err(format!(
                        "store #{} suspend {k} recorded without a preceding install",
                        rec.id
                    ));
                };
                let Some(next) = rec.installs.get(k + 1) else {
                    break;
                };
                let spent = match prev.reserved.checked_sub(rec.suspends[k].before) {
                    Some(d) => d,
                    None => {
                        return Err(format!(
                            "store #{} suspend {k}: observed fuel {} exceeds its grant {}",
                            rec.id, rec.suspends[k].before, prev.reserved
                        ));
                    }
                };
                let expected = match prev.own_cap.checked_sub(spent) {
                    Some(c) => c,
                    None => {
                        return Err(format!(
                            "store #{}: own_cap underflow after segment {k}",
                            rec.id
                        ));
                    }
                };
                if next.own_cap != expected {
                    return Err(format!(
                        "store #{}: own_cap {} after segment {k} != expected {expected} \
                         (own_cap_left reset/refill?)",
                        rec.id, next.own_cap
                    ));
                }
            }
        }
        Ok(())
    }

    /// Test-only chronological-getter probe (RV-WASM-HOST-6F709AC2
    /// OBSERVER REPAIR — instrumentation card, no separate production
    /// mutation): scripts the exact event shape of a nested chain —
    /// parent entry install 10, parent suspend, child install 6, child
    /// suspend, parent REACQUIRE install 4, parent suspend again — so the
    /// chronologically LAST install (4) and LAST suspend (before = 2)
    /// both belong to the FIRST-created Store, while a later-created
    /// Store recorded earlier events. Returns
    /// `(last_install, last_install_actual, suspended_fuel_last)`.
    ///
    /// The pre-repair getters (reverse-Store-creation order) would return
    /// the child's `(6, 6, 1)`; the repaired max-sequence getters return
    /// `(4, 4, 2)`. All events are recorded under the SAME existing
    /// observer Mutex (no separate tracking locks, no global state).
    #[cfg(test)]
    pub(crate) fn chronological_probe() -> (u64, u64, u64) {
        let obs = Self::new();
        // Store #0 (parent, created first).
        obs.record_install(0, 10, 10, 100);
        obs.record_suspend(0, 10, 3, 0);
        // Store #1 (child, created and recorded BETWEEN the parent's events).
        obs.record_install(1, 6, 6, 100);
        obs.record_suspend(1, 6, 1, 0);
        // Parent reacquires 4 and suspends again — chronologically last.
        obs.record_install(0, 4, 4, 97);
        obs.record_suspend(0, 4, 2, 0);
        (
            obs.last_install(),
            obs.last_install_actual(),
            obs.suspended_fuel_last(),
        )
    }
}
