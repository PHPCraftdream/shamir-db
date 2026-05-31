//! Phase B — interactive (multi-call) transaction registry.
//!
//! Parks a live [`shamir_tx::TxContext`] + its [`shamir_tx::SnapshotGuard`]
//! server-side between client round-trips, keyed by an opaque `tx_handle`
//! (the engine-minted `TxId`) and bound to the authenticated session. See
//! `docs/roadmap/PHASE_B_INTERACTIVE_TX.md` §4.
//!
//! **Concurrency.** The server layer already builds on `dashmap` /
//! `parking_lot` (unlike the engine, whose hot paths mandate `scc`); the
//! registry follows suit with [`dashmap::DashMap`]. The per-handle
//! [`tokio::sync::Mutex`] is the one across-`.await` lock — a `TxExecute`
//! mutates the parked `TxContext` across the async plan run — and its
//! contention is bounded to a single client serially driving its own handle
//! (one tx per session, enforced here).
//!
//! **Ownership across COMMIT.** `TxContext` is not `Clone`. To hand it to the
//! Phase-A commit pipeline, COMMIT/ROLLBACK [`Option::take`]s it out of the
//! shared `Arc` through the per-handle mutex, leaving `None`; any later call
//! on a taken (closed) handle observes `None` and is rejected. The owning
//! `Arc<InteractiveTx>` — and thus the `SnapshotGuard` it holds — is kept
//! alive by the caller until commit returns, so the MVCC snapshot stays
//! pinned through commit (SSI validation + history reads need it), then drops.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use shamir_tx::{SnapshotGuard, TxContext};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Phase B Stage 6 — default idle TTL for an open interactive tx.
/// Matches `TRANSACTIONS.md` (30 s) and `PHASE_B_INTERACTIVE_TX.md` §6.4.
///
/// The absolute lifetime cap is a separate constant
/// (`INTERACTIVE_TX_MAX_LIFETIME` in `db_handler.rs`, 5 min, mirroring
/// `shamir_engine::DEFAULT_MAX_TX_LIFETIME`). Both are checked by
/// [`InteractiveTx::is_expired`] and so by [`TxRegistry::expired_handles`].
pub const DEFAULT_INTERACTIVE_TX_IDLE_TTL: Duration = Duration::from_secs(30);

/// Phase B Stage 6 — default sweep cadence for the reaper task.
/// Comfortably below the 30 s idle TTL so a tx that idles past its TTL is
/// reaped within a few seconds of becoming reapable.
pub const DEFAULT_REAPER_INTERVAL: Duration = Duration::from_secs(5);

/// Phase B Stage 6 — handle for the periodic interactive-tx reaper task.
/// Same shape as the server's `MetaSnapshotTask`: a `JoinHandle<()>` plus
/// a `Notify` stop signal so shutdown wakes the task immediately rather
/// than waiting one full sweep interval.
pub struct ReaperTask {
    pub handle: JoinHandle<()>,
    pub stop: Arc<Notify>,
}

/// Errors surfaced when driving a handle through the registry.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TxRegistryError {
    /// The session already owns an open interactive tx (one-tx-per-session).
    #[error("session already has an open transaction")]
    TxAlreadyOpen,
    /// No open tx for this handle (never opened, or already committed /
    /// rolled back / reaped).
    #[error("transaction handle not found")]
    TxNotFound,
    /// The handle exists but belongs to a different session — a
    /// cross-session-theft attempt (even the same user on another
    /// connection is rejected).
    #[error("transaction handle does not belong to this session")]
    TxOwnershipMismatch,
}

/// A live interactive transaction parked between client round-trips.
pub struct InteractiveTx {
    /// The live overlay, wrapped so COMMIT/ROLLBACK can `take()` ownership.
    /// `tokio::sync::Mutex` because `TxExecute` holds the guard across the
    /// async plan run (the sanctioned across-await lock).
    ctx: tokio::sync::Mutex<Option<TxContext>>,
    /// Pins the MVCC snapshot for GC. Held only for its `Drop`; released when
    /// the owning `Arc` drops (after commit/rollback/timeout removal).
    _snapshot: SnapshotGuard,
    /// Owning session id — the abort-on-disconnect key and theft guard.
    owner_sid: [u8; 32],
    /// Owning user id (informational / defence-in-depth).
    owner_user_id: [u8; 16],
    /// Database the handle is pinned to — every `TxExecute` must match.
    db: String,
    /// Repo the handle is pinned to — the engine tx commits against one repo.
    repo: String,
    /// Idle-timeout bookkeeping; bumped on each `TxExecute`. `parking_lot`
    /// (server layer) — a brief lock, never held across `.await`.
    last_activity: parking_lot::Mutex<Instant>,
    /// Absolute deadline = created_at + max-lifetime. Bounds how long any one
    /// interactive tx can pin GC, even if the client keeps it busy.
    deadline: Instant,
}

impl InteractiveTx {
    /// Build a parked interactive tx. `max_lifetime` sets the absolute
    /// deadline (mirror Phase A's `DEFAULT_MAX_TX_LIFETIME`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: TxContext,
        snapshot: SnapshotGuard,
        owner_sid: [u8; 32],
        owner_user_id: [u8; 16],
        db: String,
        repo: String,
        max_lifetime: Duration,
    ) -> Self {
        let now = Instant::now();
        Self {
            ctx: tokio::sync::Mutex::new(Some(ctx)),
            _snapshot: snapshot,
            owner_sid,
            owner_user_id,
            db,
            repo,
            last_activity: parking_lot::Mutex::new(now),
            deadline: now + max_lifetime,
        }
    }

    /// The parked overlay. Lock it to run a `TxExecute` (`Some` → run; `None`
    /// → the handle was already committed/rolled back) or to `take()` the
    /// `TxContext` for COMMIT/ROLLBACK.
    pub fn ctx(&self) -> &tokio::sync::Mutex<Option<TxContext>> {
        &self.ctx
    }

    /// Owning session id — compare against the caller's `&Session` before
    /// touching the tx (theft guard).
    pub fn owner_sid(&self) -> &[u8; 32] {
        &self.owner_sid
    }

    /// Owning user id.
    pub fn owner_user_id(&self) -> &[u8; 16] {
        &self.owner_user_id
    }

    /// Database the handle is pinned to.
    pub fn db(&self) -> &str {
        &self.db
    }

    /// Repo the handle is pinned to (every `TxExecute` must target it).
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Mark activity (call on each successful `TxExecute`) to defer the idle
    /// timeout.
    pub fn bump_activity(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Whether this tx is past its absolute deadline OR has been idle longer
    /// than `idle_ttl` as of `now`. The sweep reaps any tx that returns true.
    pub fn is_expired(&self, now: Instant, idle_ttl: Duration) -> bool {
        if now >= self.deadline {
            return true;
        }
        let last = *self.last_activity.lock();
        now.saturating_duration_since(last) >= idle_ttl
    }
}

/// The server-resident table of open interactive transactions.
///
/// `open` maps `tx_handle → InteractiveTx`; `by_session` enforces the
/// one-tx-per-session invariant (`session_id → tx_handle`). Both are
/// `dashmap` (lock-free-enough for the server layer, no `RwLock` poisoning
/// surfaced to callers).
#[derive(Default)]
pub struct TxRegistry {
    open: DashMap<u64, Arc<InteractiveTx>>,
    by_session: DashMap<[u8; 32], u64>,
}

impl TxRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly-opened interactive tx under `handle`.
    ///
    /// Enforces one-tx-per-session: if the owning session already has an open
    /// tx, returns [`TxRegistryError::TxAlreadyOpen`] and the passed `it` is
    /// dropped (RAII rollback of the just-opened, unused tx). Returns the
    /// shared `Arc` on success.
    pub fn register(
        &self,
        handle: u64,
        it: InteractiveTx,
    ) -> Result<Arc<InteractiveTx>, TxRegistryError> {
        use dashmap::mapref::entry::Entry;

        let owner_sid = it.owner_sid;
        match self.by_session.entry(owner_sid) {
            // Session already drives a tx — reject (the new `it` drops here).
            Entry::Occupied(_) => Err(TxRegistryError::TxAlreadyOpen),
            Entry::Vacant(slot) => {
                let arc = Arc::new(it);
                self.open.insert(handle, Arc::clone(&arc));
                slot.insert(handle);
                Ok(arc)
            }
        }
    }

    /// Look up a handle, verifying it belongs to `sid`. Clones the `Arc` out
    /// (the `DashMap` ref is dropped before returning — never held across the
    /// caller's subsequent `.await`).
    pub fn get_owned(
        &self,
        handle: u64,
        sid: &[u8; 32],
    ) -> Result<Arc<InteractiveTx>, TxRegistryError> {
        let arc = match self.open.get(&handle) {
            Some(r) => Arc::clone(r.value()),
            None => return Err(TxRegistryError::TxNotFound),
        };
        if &arc.owner_sid != sid {
            return Err(TxRegistryError::TxOwnershipMismatch);
        }
        Ok(arc)
    }

    /// Remove a handle (COMMIT / ROLLBACK / reap). Also frees the session
    /// slot so the session can open a new tx. Returns the `Arc` so the caller
    /// can `take()` the `TxContext` for commit and keep the `SnapshotGuard`
    /// alive until commit returns.
    pub fn remove(&self, handle: u64) -> Option<Arc<InteractiveTx>> {
        let (_, arc) = self.open.remove(&handle)?;
        // Free the one-tx-per-session slot (only if it still points at us — a
        // racing re-register for the same session can't happen while we held
        // the entry, but guard against a stale pointer regardless).
        self.by_session
            .remove_if(&arc.owner_sid, |_, h| *h == handle);
        Some(arc)
    }

    /// Handles whose tx is past its absolute deadline or idle past `idle_ttl`
    /// as of `now`. The background sweep removes each (drop = RAII abort).
    pub fn expired_handles(&self, now: Instant, idle_ttl: Duration) -> Vec<u64> {
        self.open
            .iter()
            .filter(|e| e.value().is_expired(now, idle_ttl))
            .map(|e| *e.key())
            .collect()
    }

    /// Number of open interactive transactions.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    /// Whether no interactive transactions are open.
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// Spawn the periodic interactive-tx reaper.
///
/// The task loops every `reap_interval`, calling
/// [`TxRegistry::expired_handles`] with `idle_ttl`, then [`TxRegistry::remove`]
/// on each. Removing the [`Arc<InteractiveTx>`] drops it; if no commit /
/// rollback path took the inner `TxContext` first, drop = RAII rollback per
/// the `TxContext` doc-comment (no storage I/O). The `SnapshotGuard` held
/// inside drops alongside, releasing MVCC GC's `min_alive` hold.
///
/// **Abort-on-disconnect (Stage 7 limitation).** `SessionStore`
/// (`shamir_connect::server::session`) has no observer hook on idle
/// eviction — there is no callback surface to inject "abort tx for this
/// session" when a TCP connection drops. The idle TTL checked by this
/// reaper is therefore the authoritative cleanup for txs orphaned by a
/// dropped connection (the §6.3 backstop). A future `SessionStore`
/// eviction-hook API could tighten this, but nothing is durable until
/// commit, so the wait is free.
pub fn spawn_reaper_task(
    registry: Arc<TxRegistry>,
    idle_ttl: Duration,
    reap_interval: Duration,
) -> ReaperTask {
    let stop = Arc::new(Notify::new());
    let stop_inner = stop.clone();
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(reap_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Drop the immediate first tick — pointless to scan an empty registry
        // the moment we boot.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = stop_inner.notified() => {
                    tracing::debug!("interactive_tx_reaper: shutdown notified");
                    break;
                }
                _ = interval.tick() => {
                    let now = Instant::now();
                    let expired = registry.expired_handles(now, idle_ttl);
                    if expired.is_empty() {
                        continue;
                    }
                    let reaped = expired.len();
                    for h in expired {
                        // remove() returns the Arc<InteractiveTx>; drop = RAII
                        // rollback if the ctx was never committed/rolled back.
                        let _ = registry.remove(h);
                    }
                    tracing::info!(reaped, "interactive_tx_reaper: aborted past-deadline transactions");
                }
            }
        }
    });
    ReaperTask { handle, stop }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID_A: [u8; 32] = [0xAA; 32];
    const SID_B: [u8; 32] = [0xBB; 32];

    /// Build a real `(handle, InteractiveTx)` from a standalone gate — the
    /// registry stores genuine `TxContext` + `SnapshotGuard` values.
    async fn make_tx(sid: [u8; 32], max_lifetime: Duration, seed: u64) -> (u64, InteractiveTx) {
        // `seed` is the gate's first `fresh_tx_id()` — pass distinct seeds
        // when a test needs distinct handles (each call builds a fresh gate).
        let gate = shamir_tx::RepoTxGate::new(0, seed);
        let guard = gate.open_snapshot().await;
        let tx_id = gate.fresh_tx_id();
        let tx = shamir_tx::TxContext::new(
            tx_id,
            0,
            guard.version(),
            shamir_tx::IsolationLevel::Snapshot,
        );
        let it = InteractiveTx::new(
            tx,
            guard,
            sid,
            [0u8; 16],
            "db".to_string(),
            "repo".to_string(),
            max_lifetime,
        );
        (tx_id.0, it)
    }

    #[tokio::test]
    async fn register_then_get_owned_roundtrip() {
        let reg = TxRegistry::new();
        let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
        reg.register(handle, it).unwrap();

        let got = reg.get_owned(handle, &SID_A).unwrap();
        assert_eq!(got.db(), "db");
        assert_eq!(got.repo(), "repo");
        assert_eq!(got.owner_user_id(), &[0u8; 16]);
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn one_tx_per_session_rejected() {
        let reg = TxRegistry::new();
        let (h1, it1) = make_tx(SID_A, Duration::from_secs(300), 1).await;
        let (h2, it2) = make_tx(SID_A, Duration::from_secs(300), 2).await;
        reg.register(h1, it1).unwrap();

        assert!(
            matches!(reg.register(h2, it2), Err(TxRegistryError::TxAlreadyOpen)),
            "second BEGIN on a session with an open tx must be rejected"
        );
        // The rejected tx left no trace.
        assert_eq!(reg.len(), 1);
        assert!(reg.get_owned(h2, &SID_A).is_err());
    }

    #[tokio::test]
    async fn get_owned_foreign_session_rejected() {
        let reg = TxRegistry::new();
        let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
        reg.register(handle, it).unwrap();

        // Same handle, different session id → theft guard fires.
        assert!(matches!(
            reg.get_owned(handle, &SID_B),
            Err(TxRegistryError::TxOwnershipMismatch)
        ));
    }

    #[tokio::test]
    async fn get_owned_unknown_handle() {
        let reg = TxRegistry::new();
        assert!(matches!(
            reg.get_owned(999, &SID_A),
            Err(TxRegistryError::TxNotFound)
        ));
    }

    #[tokio::test]
    async fn remove_frees_session_slot() {
        let reg = TxRegistry::new();
        let (h1, it1) = make_tx(SID_A, Duration::from_secs(300), 1).await;
        reg.register(h1, it1).unwrap();

        let removed = reg.remove(h1).expect("handle present");
        assert_eq!(removed.owner_sid(), &SID_A);
        assert!(reg.is_empty());
        assert!(matches!(
            reg.get_owned(h1, &SID_A),
            Err(TxRegistryError::TxNotFound)
        ));

        // Session slot is freed → the session can open a new tx.
        let (h2, it2) = make_tx(SID_A, Duration::from_secs(300), 2).await;
        reg.register(h2, it2).unwrap();
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn take_ctx_closes_handle_on_commit() {
        let reg = TxRegistry::new();
        let (handle, it) = make_tx(SID_A, Duration::from_secs(300), 1).await;
        let arc = reg.register(handle, it).unwrap();

        // COMMIT/ROLLBACK semantics: take the TxContext out of the Arc.
        let taken = arc.ctx().lock().await.take();
        assert!(taken.is_some(), "first take yields the live TxContext");

        // A later call on the (now closed) handle sees None.
        let again = arc.ctx().lock().await.take();
        assert!(again.is_none(), "second take on a closed handle is None");
    }

    #[tokio::test]
    async fn expired_by_absolute_deadline() {
        let reg = TxRegistry::new();
        // Zero lifetime → deadline == creation instant; monotonic time has
        // advanced by the assert, so `now >= deadline`.
        let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
        reg.register(handle, it).unwrap();

        let expired = reg.expired_handles(Instant::now(), Duration::from_secs(3600));
        assert_eq!(
            expired,
            vec![handle],
            "zero-lifetime tx is past its deadline"
        );
    }

    #[tokio::test]
    async fn reaper_contract_past_deadline_tx_is_removed() {
        let reg = TxRegistry::new();
        // Zero lifetime -> deadline == creation instant; the registry's
        // is_expired check fires immediately. Same trick as
        // `expired_by_absolute_deadline`.
        let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
        reg.register(handle, it).unwrap();

        // The contract the reaper task runs each tick:
        let expired = reg.expired_handles(Instant::now(), Duration::from_secs(60));
        assert_eq!(expired, vec![handle], "past-deadline tx is listed by sweep");
        for h in expired {
            let arc = reg.remove(h);
            assert!(arc.is_some(), "remove yields the parked tx for RAII drop");
        }
        assert!(reg.is_empty(), "registry empty after sweep");
        assert!(
            matches!(
                reg.get_owned(handle, &SID_A),
                Err(TxRegistryError::TxNotFound)
            ),
            "lookup after reap returns TxNotFound"
        );
    }

    #[tokio::test]
    async fn reaper_task_reaps_past_deadline_tx() {
        let reg = Arc::new(TxRegistry::new());
        let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
        reg.register(handle, it).unwrap();
        assert_eq!(reg.len(), 1);

        // Tight sweep, generous idle TTL -> only the absolute-deadline branch fires.
        let reaper = spawn_reaper_task(
            Arc::clone(&reg),
            Duration::from_secs(60),
            Duration::from_millis(50),
        );
        // The first tick is dropped (set in spawn_reaper_task), so the first
        // real sweep fires ~50ms later. Sleep generously to avoid CI flake.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(reg.is_empty(), "reaper task drained the past-deadline tx");
        assert!(matches!(
            reg.get_owned(handle, &SID_A),
            Err(TxRegistryError::TxNotFound)
        ));

        // Clean drain -- mirror ServerHandle::shutdown so the test never leaks the task.
        reaper.stop.notify_waiters();
        let _ = reaper.handle.await;
    }

    #[tokio::test]
    async fn expired_by_idle_ttl() {
        let reg = TxRegistry::new();
        // Long absolute deadline, but a zero idle TTL → always idle-expired.
        let (handle, it) = make_tx(SID_A, Duration::from_secs(3600), 1).await;
        reg.register(handle, it).unwrap();

        let expired = reg.expired_handles(Instant::now(), Duration::ZERO);
        assert_eq!(expired, vec![handle], "zero idle-ttl reaps any inactive tx");

        // With a generous idle TTL and far deadline, nothing is expired.
        assert!(reg
            .expired_handles(Instant::now(), Duration::from_secs(3600))
            .is_empty());
    }

    /// Sweep workflow: `expired_handles` yields the reaped set; `remove`
    /// drops the `InteractiveTx` (RAII = no storage side effect, design
    /// §6.4) and frees the one-tx-per-session slot.
    #[tokio::test]
    async fn sweep_reaps_expired_handle_and_frees_session_slot() {
        let reg = TxRegistry::new();
        // absolute=0 → already expired
        let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
        reg.register(handle, it).unwrap();

        // Sweep step 1: collect expired handles.
        let expired = reg.expired_handles(Instant::now(), Duration::from_secs(3600));
        assert_eq!(
            expired,
            vec![handle],
            "sweep must surface the past-deadline handle"
        );

        // Sweep step 2: drop each — RAII rollback, no storage I/O.
        for h in expired {
            let arc = reg.remove(h).expect("sweep removes the entry");
            // Closing the handle on the sweep path mirrors
            // commit/rollback semantics.
            let _ = arc.ctx().lock().await.take();
        }

        assert!(reg.is_empty(), "sweep drained the open map");
        assert!(
            matches!(
                reg.get_owned(handle, &SID_A),
                Err(TxRegistryError::TxNotFound)
            ),
            "reaped handle is no longer addressable"
        );

        // Session slot is freed → the session can open a NEW tx (would have
        // hit TxAlreadyOpen if `remove` had skipped by_session cleanup).
        let (h2, it2) = make_tx(SID_A, Duration::from_secs(300), 2).await;
        reg.register(h2, it2)
            .expect("session slot freed after sweep");
        assert_eq!(reg.len(), 1);
    }

    /// `bump_activity` defers idle-deadline reaping but does NOT extend the
    /// absolute deadline (the hard upper bound on how long a tx can pin GC).
    #[tokio::test]
    async fn bump_activity_defers_idle_reap() {
        let reg = TxRegistry::new();
        // Far absolute deadline.
        let (handle, it) = make_tx(SID_A, Duration::from_secs(3600), 1).await;
        let arc = reg.register(handle, it).unwrap();

        // Before bump: zero idle-ttl reaps any inactive tx.
        let expired_pre = reg.expired_handles(Instant::now(), Duration::ZERO);
        assert_eq!(
            expired_pre,
            vec![handle],
            "sanity: idle-reap fires at ZERO ttl"
        );

        // Bump activity — the idle clock restarts.
        arc.bump_activity();

        // With a generous idle ttl (1 hour), the just-bumped tx is NOT
        // idle-expired.
        assert!(
            reg.expired_handles(Instant::now(), Duration::from_secs(3600))
                .is_empty(),
            "bump_activity defers the idle-reap when the absolute deadline is far"
        );
    }

    /// Even after `bump_activity`, an absolute-deadline-past tx is reaped.
    /// The absolute deadline is the hard upper bound (design doc §6.4).
    #[tokio::test]
    async fn absolute_deadline_overrides_bump_activity() {
        let reg = TxRegistry::new();
        // absolute=0 → already past
        let (handle, it) = make_tx(SID_A, Duration::ZERO, 1).await;
        let arc = reg.register(handle, it).unwrap();

        // Bump activity — but the absolute deadline is the hard cap.
        arc.bump_activity();

        // Even with a huge idle ttl, the past-absolute-deadline tx is
        // expired.
        let expired = reg.expired_handles(Instant::now(), Duration::from_secs(3600));
        assert_eq!(
            expired,
            vec![handle],
            "absolute deadline overrides bump_activity — it is the hard \
             upper bound on tx lifetime"
        );
    }
}
