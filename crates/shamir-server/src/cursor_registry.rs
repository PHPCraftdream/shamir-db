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

/// CR-A4 (#764): which coordinate system a cursor's bookmark resumes from,
/// decided ONCE at `create_cursor` time (see [`Cursor::new`]) from whether
/// the caller's query has a simple single-column ORDER BY — never
/// re-derived per `FetchNext` call. Pinning the mode up front closes a
/// latent hazard where a later page could otherwise flip coordinate
/// systems mid-scroll (e.g. if a projection quirk made one page's
/// `seek_key` extraction fail, silently falling back to the row-count
/// `offset` bookmark for that page only) — a flip like that could
/// duplicate or skip rows, since the two bookmark kinds are not
/// interchangeable positions in the same scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaginationMode {
    /// Boundary-filter seek on the single ORDER BY column (`seek_key` +
    /// `tie_skip`), CR-A4's inclusive-boundary + skip-past-ties scheme.
    Keyset,
    /// Plain row-count `offset`, used when the query has no ORDER BY (or
    /// not a simple single-column one) — no field to build a boundary
    /// filter on.
    Offset,
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
    /// CR-A4 (#764): the pagination coordinate system this cursor was
    /// pinned to at `create_cursor` time — see [`PaginationMode`]. Decided
    /// once, never re-derived per `FetchNext`.
    pub mode: PaginationMode,
    /// Current keyset seek key — the last row's (single-column) ORDER BY
    /// value, used to build an INCLUSIVE `field >= seek_key` (or `<=` for
    /// DESC) boundary filter on the next `FetchNext` (CR-A4: was `>`/`<`,
    /// which silently dropped tied rows straddling a page boundary — see
    /// `db_handler::cursor_handlers::boundary_filter`). Only set when
    /// `mode == PaginationMode::Keyset`; `None` before the first
    /// `FetchNext` (the initial page has nothing to seek past yet).
    pub seek_key: Option<shamir_types::types::value::QueryValue>,
    /// CR-A4 (#764): how many rows EXACTLY equal to `seek_key` have
    /// already been returned to the client (across all prior pages, for
    /// the CURRENT run of ties ending at `seek_key`). Since the cursor's
    /// read path (`TableManager::read_as_of`) never attaches a record's
    /// `_id` to a projected row (confirmed: `_id` is only ever attached on
    /// the WRITE-result path and the Latest-temporal sorted-index seek
    /// path — `read_as_of` -> `apply_select_value_bytes` discards the
    /// `RecordId` outright — see this task's brief,
    /// `docs/dev-artifacts/prompts/post-alpha/11-cr-a4-keyset-tie-breaker.md`),
    /// a real `RecordId`-based "skip past last_id" is not available on
    /// this path. `tie_skip` is the substitute: since `list_stream`'s
    /// enumeration order is stable and `apply_order_by_qv`'s sort is
    /// stable (`Vec::sort_by`, a documented std guarantee) across two
    /// `read_as_of` calls at the SAME pinned version with no concurrent
    /// write, the Nth row (by return order) among rows tied at
    /// `seek_key` is deterministic — so "skip the first `tie_skip` rows
    /// equal to `seek_key` in the next fetch" reproduces exactly the same
    /// effect "skip past last_id" would if `_id` were available. `0` when
    /// there is no active tie run to skip past (i.e. `seek_key` is set but
    /// no page has ended mid-tie yet — the boundary row is included
    /// zero times, so nothing needs skipping).
    pub tie_skip: u64,
    /// Row-count bookmark used when `mode == PaginationMode::Offset`.
    /// Without a caller-specified total order there is no field to build a
    /// `Gte`/`Lte` boundary filter on, so `FetchNext` instead resumes via
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
    /// Build a parked cursor. `mode` is decided ONCE by the caller (see
    /// `db_handler::cursor_handlers::create_cursor`, CR-A4 #764) from
    /// whether `query` has a simple single-column ORDER BY, and never
    /// re-derived afterward.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        query: ReadQuery,
        mode: PaginationMode,
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
                mode,
                seek_key: None,
                tie_skip: 0,
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
    open: DashMap<u64, Arc<Cursor>, THasher>,
    by_session: DashMap<[u8; 32], Arc<AtomicUsize>, THasher>,
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

    /// Decrement the session's live open-cursor count and, if it just
    /// reached zero, remove the `by_session` entry entirely — otherwise a
    /// long-lived server leaks one `[u8; 32] -> Arc<AtomicUsize>` entry per
    /// connection that ever opened a cursor (CR-B8 / #774, R-3).
    ///
    /// The decrement and the "now zero, so remove" check are combined into
    /// ONE atomic step via `DashMap::remove_if`, whose predicate runs while
    /// `dashmap` holds the shard's write lock for `owner_sid` (verified
    /// against `dashmap` 6.1.0's `_remove_if`, which calls
    /// `_yield_write_shard` before invoking the predicate and only releases
    /// it after — see `dashmap::HashMap::_entry`, used by `register`'s
    /// `.entry(owner_sid)`, which takes the SAME shard write lock). So this
    /// decrement-then-maybe-remove is atomic relative to a concurrent
    /// `register()`: either `register`'s `entry()` call happens strictly
    /// before this `remove_if` (it sees the pre-decrement count, adds to
    /// the same `Arc`, and this `remove_if`'s predicate then observes a
    /// NON-zero result and does not remove), or strictly after (this
    /// `remove_if` already removed the entry, `register`'s `entry()` finds
    /// nothing and creates a fresh `AtomicUsize(0)` via `or_insert_with`,
    /// correctly starting over for a session that just hit zero). Either
    /// interleaving preserves correct cap accounting — a session can never
    /// exceed `max_per_session` split across two independently-created
    /// counters.
    fn free_session_slot(&self, owner_sid: &[u8; 32]) {
        self.by_session.remove_if(owner_sid, |_, counter| {
            // Pre-decrement value == 1 means the NEW value is 0.
            counter.fetch_sub(1, Ordering::AcqRel) == 1
        });
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

    /// Number of `by_session` entries currently tracked (test/diagnostic
    /// probe only — asserts CR-B8's leak fix: a session's entry must be
    /// removed once its live count reaches zero, not merely zeroed out).
    /// NOT a hot-path method: `DashMap::len()` is fine here (unlike `scc`'s
    /// banned O(N) `len()`, `dashmap`'s isn't on `clippy.toml`'s
    /// `disallowed-methods` list), but production code should never need
    /// this cardinality — only `open_count_for_session`'s O(1) per-session
    /// lookup.
    #[cfg(test)]
    pub(crate) fn by_session_len(&self) -> usize {
        self.by_session.len()
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
