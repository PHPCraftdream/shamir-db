//! FG-5b — server-side result cursor registry.
//!
//! Parks a bookmark-shaped, `Send + 'static` cursor state server-side
//! between `FetchNext` round-trips, keyed by an opaque `cursor_id` (see
//! `shamir_query_types::wire::CursorId`) and bound to the authenticated
//! session. Mirrors `crate::tx_registry` (`InteractiveTx` / `TxRegistry` /
//! `spawn_reaper_task`) almost exactly — see that module's doc comment for
//! the shared design rationale (dashmap over the server layer, the one
//! across-`.await` `tokio::sync::Mutex` per handle).
//!
//! # Design (see `docs/dev-artifacts/prompts/post-alpha/03-fg5b-engine-session-cursor.md`)
//!
//! A cursor does NOT hold a live `futures::Stream` across async calls — that
//! would fight `TableManager`'s `'a`-borrowed stream lifetimes for no
//! benefit. Instead cursor state is just a **bookmark**: the caller's
//! original `ReadQuery`, a resume bookmark (either a `Gt`/`Lt` boundary-
//! filter seek key when the query has an ORDER BY, or a row-count offset
//! otherwise — see `db_handler::cursor_handlers`'s module doc for why a
//! bare `Pagination::After` does not work here), whether a next page is
//! known to exist, and a pinned `shamir_tx::SnapshotGuard` (+ the version it
//! pinned) so every `FetchNext` reads a stable, snapshot-consistent view
//! for the cursor's whole lifetime — even though the stored query's own
//! `temporal` field says `Latest` (the only temporal mode a cursor
//! supports; see `shamir_query_types::batch::BatchError::CursorTemporalNotSupported`).
//!
//! **Difference from `TxRegistry`:** a session may open MANY cursors (up to
//! `CursorLimitsCap::max_cursors_per_session`), not just one. Tracking a
//! live per-session count needs O(1) cardinality, not `Vec<u64>::len()`
//! (banned per CLAUDE.md's O(x→0) pillar) — so `by_session` maps
//! `[u8; 32] → Arc<AtomicUsize>`, a live open-count per session that
//! `register` increments (rejecting over-cap) and `remove` decrements.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use shamir_query_types::read::ReadQuery;
use shamir_tx::SnapshotGuard;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use shamir_collections::THasher;

/// Default idle TTL for an open cursor before the reaper evicts it.
///
/// Deliberately longer than the interactive-tx idle TTL
/// (`crate::tx_registry::DEFAULT_INTERACTIVE_TX_IDLE_TTL`, 30 s): a cursor
/// is read-only and its `FetchNext` cadence is paced by how fast the CLIENT
/// consumes pages (e.g. streaming rows into a slow downstream sink), not by
/// a single round-trip like an interactive tx's next `TxExecute`. 60 s gives
/// a consuming client a generous window between pages before the server
/// reclaims the pinned MVCC snapshot.
pub const DEFAULT_CURSOR_IDLE_TTL: Duration = Duration::from_secs(60);

/// Default sweep cadence for the cursor reaper task. Comfortably below the
/// idle TTL so a cursor that idles past its TTL is reaped within a few
/// seconds of becoming reapable. Mirrors
/// `crate::tx_registry::DEFAULT_REAPER_INTERVAL`.
pub const DEFAULT_CURSOR_REAPER_INTERVAL: Duration = Duration::from_secs(5);

/// Handle for the periodic cursor reaper task. Stop signal is the shared
/// root `shutdown_token` (mirrors `crate::tx_registry::ReaperTask`).
pub struct CursorReaperTask {
    pub handle: JoinHandle<()>,
}

/// Errors surfaced when driving a cursor handle through the registry.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CursorRegistryError {
    /// The session already has `limit` cursors open.
    #[error("cursor limit exceeded (max: {limit})")]
    CursorLimitExceeded { limit: u32 },
    /// No open cursor for this id (never opened, explicitly canceled, or
    /// reaped for idle timeout — see `CursorExpired` for the latter when
    /// the registry can distinguish it).
    #[error("cursor not found")]
    CursorNotFound,
    /// The cursor id WAS issued and has since been reaped for sitting idle
    /// past its TTL — distinguishable from `CursorNotFound` (a "you waited
    /// too long" signal vs. "that id was never valid") via a short-lived
    /// tombstone (see [`CursorRegistry`] doc comment on `reaped_tombstones`).
    #[error("cursor expired (idle-timeout eviction)")]
    CursorExpired,
    /// The cursor exists but belongs to a different session (cross-session
    /// theft attempt — even the same user on another connection is
    /// rejected). Mirrors `TxRegistryError::TxOwnershipMismatch`.
    #[error("cursor does not belong to this session")]
    CursorOwnershipMismatch,
}

/// Mutable per-cursor pagination state, guarded by the cursor's
/// `tokio::sync::Mutex` (a `FetchNext` mutates it across the async engine
/// read call — the same across-`.await` lock pattern `InteractiveTx.ctx`
/// uses for `TxExecute`).
pub struct CursorState {
    /// The caller's original query this cursor scans. `db_handler::
    /// cursor_handlers` clones it per `FetchNext` call and overwrites the
    /// clone's `pagination`/`where`/`temporal` fields to express the
    /// current bookmark — this stored copy's `from`/`select`/`where`/
    /// `order_by` are the caller's original request and never mutated.
    pub query: ReadQuery,
    /// Current keyset seek key — the last row's (single-column) ORDER BY
    /// value, used to build a `field > seek_key` (or `<` for DESC) boundary
    /// filter on the next `FetchNext`. Only set when the caller's query
    /// carries an `order_by`; `None` when there is no ORDER BY (see
    /// `offset` below) OR before the first `FetchNext` (the initial page
    /// has nothing to seek past yet).
    pub seek_key: Option<shamir_types::types::value::QueryValue>,
    /// Row-count bookmark used when the caller's query has NO `order_by`.
    /// Without a caller-specified total order there is no field to build a
    /// `Gt`/`Lt` boundary filter on, so `FetchNext` instead resumes via
    /// `Pagination::LimitOffset { offset, limit }` against the pinned
    /// snapshot's stable (but engine-internal, insertion-order) full scan.
    pub offset: u64,
    /// Whether the cursor has already reported `has_more == false`.  Once
    /// set, a further `FetchNext` returns `CursorNotFound`/closes the
    /// cursor rather than re-running an exhausted scan.
    pub exhausted: bool,
}

/// A live server-side result cursor parked between `FetchNext` round-trips.
pub struct Cursor {
    /// Mutable pagination bookmark. `tokio::sync::Mutex` because
    /// `FetchNext` holds the guard across the async `TableManager::read`
    /// call (the sanctioned across-await lock, mirroring `InteractiveTx::ctx`).
    state: tokio::sync::Mutex<CursorState>,
    /// Pins the MVCC snapshot this cursor reads at for its whole lifetime.
    /// Held only for its `Drop`; released when the owning `Arc` drops
    /// (cancel/reap).
    _snapshot: SnapshotGuard,
    /// The version `_snapshot` pinned — every `FetchNext` reads
    /// `Temporal::AsOf { at: At::Version(pinned_version) }` at this value,
    /// giving snapshot-consistent pagination across the cursor's lifetime
    /// even though the caller's query said `Temporal::Latest`.
    pinned_version: u64,
    /// Default page size (from `CreateCursor`); a `FetchNext` may request a
    /// different `page_size` per call, so this is only the fallback.
    default_page_size: u32,
    /// Owning session id — the theft guard.
    owner_sid: [u8; 32],
    /// Database the cursor is pinned to.
    db: String,
    /// Repo the cursor is pinned to.
    repo: String,
    /// Construction baseline for idle/deadline nanos (mirrors `InteractiveTx`).
    created_at: Instant,
    /// Nanos since `created_at` of the last activity bump.
    last_activity_nanos: AtomicU64,
}

impl Cursor {
    /// Build a parked cursor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        query: ReadQuery,
        snapshot: SnapshotGuard,
        pinned_version: u64,
        default_page_size: u32,
        owner_sid: [u8; 32],
        db: String,
        repo: String,
    ) -> Self {
        Self {
            state: tokio::sync::Mutex::new(CursorState {
                query,
                seek_key: None,
                offset: 0,
                exhausted: false,
            }),
            _snapshot: snapshot,
            pinned_version,
            default_page_size,
            owner_sid,
            db,
            repo,
            created_at: Instant::now(),
            last_activity_nanos: AtomicU64::new(0),
        }
    }

    /// The mutable pagination bookmark — lock it to run a `FetchNext`.
    pub fn state(&self) -> &tokio::sync::Mutex<CursorState> {
        &self.state
    }

    /// The MVCC version every `FetchNext` reads at.
    pub fn pinned_version(&self) -> u64 {
        self.pinned_version
    }

    /// Default page size from `CreateCursor`.
    pub fn default_page_size(&self) -> u32 {
        self.default_page_size
    }

    /// Owning session id.
    pub fn owner_sid(&self) -> &[u8; 32] {
        &self.owner_sid
    }

    /// Database the cursor is pinned to.
    pub fn db(&self) -> &str {
        &self.db
    }

    /// Repo the cursor is pinned to.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Mark activity (call on each successful `FetchNext`) to defer the
    /// idle timeout. Single atomic store — no lock taken.
    pub fn bump_activity(&self) {
        let elapsed = self.created_at.elapsed().as_nanos() as u64;
        self.last_activity_nanos.store(elapsed, Ordering::Release);
    }

    /// Whether this cursor has been idle longer than `idle_ttl` as of `now`.
    /// Unlike `InteractiveTx::is_expired` a cursor has no separate absolute
    /// deadline — its whole lifetime budget IS the idle TTL (a cursor being
    /// actively paged through, however long that takes, is legitimate use;
    /// only genuine abandonment should reap it).
    pub fn is_expired(&self, now: Instant, idle_ttl: Duration) -> bool {
        let elapsed_now = now.saturating_duration_since(self.created_at).as_nanos() as u64;
        let last = self.last_activity_nanos.load(Ordering::Acquire);
        elapsed_now.saturating_sub(last) >= idle_ttl.as_nanos() as u64
    }
}

/// The server-resident table of open result cursors.
///
/// `open` maps `cursor_id → Cursor`; `by_session` tracks a LIVE per-session
/// open-cursor COUNT (`Arc<AtomicUsize>`), not a `Vec<u64>` — an O(1)
/// cardinality check on the hot `register`/`remove` path, per CLAUDE.md's
/// O(x→0) pillar (a `.len()` on a per-session `Vec` would be banned only if
/// scc were in play here, but the intent — never materialise-then-count —
/// applies equally to `dashmap`).
///
/// `reaped_tombstones` is a short-lived marker set: `remove_for_idle_reap`
/// inserts the id here (with a bounded self-expiry sweep alongside the
/// regular cursor sweep) so a `FetchNext` against a JUST-reaped id can
/// report `CursorExpired` instead of the less-specific `CursorNotFound` —
/// distinguishing "you waited too long" from "that id was never valid".
/// Explicit `CancelCursor` does NOT tombstone (canceling is a deliberate,
/// successful close — not an error condition a later fetch should complain
/// about differently).
#[derive(Default)]
pub struct CursorRegistry {
    open: DashMap<u64, Arc<Cursor>>,
    by_session: DashMap<[u8; 32], Arc<AtomicUsize>>,
    reaped_tombstones: DashMap<u64, Instant, THasher>,
}

/// How long a reaped-for-idle cursor id stays tombstoned (so a `FetchNext`
/// racing the reaper still gets `CursorExpired` rather than
/// `CursorNotFound`). Generous relative to the reaper's own sweep interval
/// so a client's in-flight `FetchNext` at the moment of eviction reliably
/// observes the more specific error.
const TOMBSTONE_TTL: Duration = Duration::from_secs(300);

impl CursorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly-opened cursor under `cursor_id`, enforcing the
    /// per-session cap.
    ///
    /// On success increments the session's live open-count. On rejection
    /// the just-built `cursor` (and the `SnapshotGuard`/table handle it
    /// holds) is dropped — RAII release of the just-opened, unused
    /// resource — mirroring `TxRegistry::register`'s `TxAlreadyOpen` path.
    pub fn register(
        &self,
        cursor_id: u64,
        owner_sid: [u8; 32],
        cursor: Cursor,
        max_per_session: u32,
    ) -> Result<Arc<Cursor>, CursorRegistryError> {
        let slot = self
            .by_session
            .entry(owner_sid)
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        let counter = Arc::clone(slot.value());
        drop(slot);

        // CAS loop: only admit if the count is still below the cap at the
        // moment of the bump (no TOCTOU window between a `load` and a
        // separate `fetch_add`).
        loop {
            let cur = counter.load(Ordering::Acquire);
            if cur >= max_per_session as usize {
                // `cursor` (the just-built Cursor + SnapshotGuard) drops
                // here — RAII release, same pattern as TxAlreadyOpen.
                return Err(CursorRegistryError::CursorLimitExceeded {
                    limit: max_per_session,
                });
            }
            if counter
                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }

        let arc = Arc::new(cursor);
        self.open.insert(cursor_id, Arc::clone(&arc));
        // Clear any stale tombstone for a reused id (ids are u64-minted
        // monotonically by the caller in production; tests may reuse small
        // ids, so keep this registry-side invariant tight regardless).
        self.reaped_tombstones.remove(&cursor_id);
        Ok(arc)
    }

    /// Look up a cursor, verifying it belongs to `sid`.
    ///
    /// Returns [`CursorRegistryError::CursorExpired`] when `cursor_id` was
    /// reaped for idle-timeout within the tombstone window,
    /// [`CursorRegistryError::CursorNotFound`] when it was never issued (or
    /// its tombstone has aged out), and
    /// [`CursorRegistryError::CursorOwnershipMismatch`] on a cross-session
    /// lookup.
    pub fn get_owned(
        &self,
        cursor_id: u64,
        sid: &[u8; 32],
    ) -> Result<Arc<Cursor>, CursorRegistryError> {
        let arc = match self.open.get(&cursor_id) {
            Some(r) => Arc::clone(r.value()),
            None => {
                return Err(if self.reaped_tombstones.contains_key(&cursor_id) {
                    CursorRegistryError::CursorExpired
                } else {
                    CursorRegistryError::CursorNotFound
                });
            }
        };
        if &arc.owner_sid != sid {
            return Err(CursorRegistryError::CursorOwnershipMismatch);
        }
        Ok(arc)
    }

    /// Remove a cursor (explicit `CancelCursor`, or the scan-exhausted
    /// auto-close on the last `FetchNext` page). Frees the session's slot.
    /// Does NOT tombstone — a deliberate close is not an error condition a
    /// later fetch should be told "expired" about.
    pub fn remove(&self, cursor_id: u64) -> Option<Arc<Cursor>> {
        let (_, arc) = self.open.remove(&cursor_id)?;
        self.free_session_slot(&arc.owner_sid);
        Some(arc)
    }

    /// Remove a cursor because the reaper found it idle-expired, leaving a
    /// tombstone so a racing `FetchNext` gets `CursorExpired` instead of
    /// `CursorNotFound`.
    pub fn remove_for_idle_reap(&self, cursor_id: u64) -> Option<Arc<Cursor>> {
        let (_, arc) = self.open.remove(&cursor_id)?;
        self.free_session_slot(&arc.owner_sid);
        self.reaped_tombstones.insert(cursor_id, Instant::now());
        Some(arc)
    }

    fn free_session_slot(&self, owner_sid: &[u8; 32]) {
        if let Some(counter) = self.by_session.get(owner_sid) {
            counter.value().fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Cursor ids idle past `idle_ttl` as of `now`. The background sweep
    /// removes each via [`Self::remove_for_idle_reap`].
    pub fn expired_ids(&self, now: Instant, idle_ttl: Duration) -> Vec<u64> {
        self.open
            .iter()
            .filter(|e| e.value().is_expired(now, idle_ttl))
            .map(|e| *e.key())
            .collect()
    }

    /// Drop tombstones older than [`TOMBSTONE_TTL`] — called by the reaper
    /// sweep alongside `expired_ids`/`remove_for_idle_reap` so the
    /// tombstone set does not grow unbounded.
    pub fn sweep_tombstones(&self, now: Instant) {
        self.reaped_tombstones
            .retain(|_, inserted_at| now.saturating_duration_since(*inserted_at) < TOMBSTONE_TTL);
    }

    /// Number of open cursors.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    /// Whether no cursors are open.
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Live open-cursor count for a session (0 if the session has never
    /// opened one). O(1) — reads the `AtomicUsize` mirror, never counts.
    pub fn open_count_for_session(&self, sid: &[u8; 32]) -> usize {
        self.by_session
            .get(sid)
            .map(|c| c.value().load(Ordering::Acquire))
            .unwrap_or(0)
    }
}

/// Spawn the periodic cursor reaper.
///
/// Mirrors `crate::tx_registry::spawn_reaper_task`: loops every
/// `reap_interval`, calling [`CursorRegistry::expired_ids`] with
/// `idle_ttl`, then [`CursorRegistry::remove_for_idle_reap`] on each
/// (dropping the `Arc<Cursor>` releases the `SnapshotGuard`, unpinning the
/// MVCC GC floor). Also sweeps aged-out tombstones each tick.
pub fn spawn_reaper_task(
    registry: Arc<CursorRegistry>,
    idle_ttl: Duration,
    reap_interval: Duration,
    shutdown: CancellationToken,
) -> CursorReaperTask {
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(reap_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Drop the immediate first tick — pointless to scan an empty
        // registry the moment we boot.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    tracing::debug!("cursor_reaper: shutdown cancelled");
                    break;
                }
                _ = interval.tick() => {
                    let now = Instant::now();
                    registry.sweep_tombstones(now);
                    let expired = registry.expired_ids(now, idle_ttl);
                    if expired.is_empty() {
                        continue;
                    }
                    let reaped = expired.len();
                    for id in expired {
                        let _ = registry.remove_for_idle_reap(id);
                    }
                    tracing::info!(reaped, "cursor_reaper: evicted idle-timeout cursors");
                }
            }
        }
    });
    CursorReaperTask { handle }
}
