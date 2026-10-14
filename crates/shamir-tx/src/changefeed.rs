//! Changefeed (Phase 3b) — hybrid live-push + durable journal.
//!
//! From the commit point a committed transaction's write footprint is
//! projected ONCE into a [`ChangelogEvent`] and fanned out down two
//! independent, non-blocking tracks:
//!
//! 1. **Live push** — a per-repo [`tokio::sync::broadcast`] channel. Each
//!    subscriber gets the event the instant it is published. A slow
//!    subscriber that falls behind the channel's bounded ring receives a
//!    `RecvError::Lagged(n)` and SKIPS those events — that is acceptable
//!    because it can re-read the missed window from the durable journal
//!    (track 2). Emission is `Sender::send`, which never waits for
//!    receivers; with zero subscribers it returns `Err` which the
//!    commit-path ignores.
//!
//! 2. **Durable journal** — a per-repo append log keyed by
//!    `commit_version` (big-endian 8 bytes, so lexicographic store order
//!    equals numeric order). The commit-path hands the event to a bounded
//!    [`tokio::sync::mpsc`] channel via `try_send` (NEVER blocks); a
//!    background task batches pending events and writes them durably to
//!    the changelog store. Engine pulls confirm a writer barrier and range-read
//!    only the settled version prefix.
//!
//! NEITHER track blocks the hot commit-path. The projection is done once
//! and reused for both tracks; a commit with an empty footprint emits
//! nothing.
//!
//! ## Durability trade-off (journal)
//!
//! Local commits and replicated applies enqueue (or mark a gap) immediately
//! before version publication, without an await or fallible step between them.
//! Engine journal pulls capture the contiguous completion watermark W (every
//! version through W is settled), then confirm the preceding channel prefix
//! with a writer barrier before reading events at or below the confirmed bound.
//! Confirmation has a bounded budget; timeout returns an older safe prefix.
//! A version still committing holds back the prefix rather than being skipped.
//!
//! Persistence is asynchronous: crash may lose the unflushed tail; cold start
//! rebuilds derivable events from surviving WAL. Missing/legacy entries,
//! closure, write failures or corruption mark gaps; channel overflow instead
//! defers the event to a repair-pending set (see `repair_pending`) for
//! re-enqueue by the engine drainer, and only declares a gap when the writer
//! is closed. With follower
//! provenance, unknown in-flight range makes unresolved (J, H] conservatively
//! gapped; H is the recovered gate floor. Failed shutdown alone adds no blanket
//! gap: WAL rebuilds through H. Gap metadata persists asynchronously; failed
//! marker writes stay dirty for retry. The journal is not the source of truth.
//!
//! Live broadcast occurs after publication in commit-completion order, not
//! necessarily version order. A consumer that discards versions at or below
//! its last delivered version can skip a late lower-version event. Use bounded
//! engine journal pulls for ordered history, and handle reported gaps.

pub mod cold;

mod repair_pending;

pub use repair_pending::RepairPending;
use repair_pending::RepairPending as RepairPendingInner;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::access::Actor;
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};
use tokio::time::{sleep_until, timeout_at, Instant};

/// One record-level change inside a committed transaction.
///
/// `key` is the raw storage key the tx staged (for data tables this is the
/// 16-byte `RecordId`). `value` carries the FULL new record bytes for a
/// [`ChangeOp::Put`] — the same bytes the WAL entry serialises, taken from
/// the tx's staging snapshot at projection time — and is `None` for a
/// [`ChangeOp::Delete`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordChange {
    /// Human-readable table name (resolved from the tx's `table_tokens`).
    pub table: String,
    /// Raw storage key (16-byte `RecordId` for data tables).
    #[serde(with = "serde_bytes_compat")]
    pub key: Bytes,
    /// The mutation kind.
    pub op: ChangeOp,
    /// New record bytes for `Put`; `None` for `Delete`.
    #[serde(with = "serde_opt_bytes_compat")]
    pub value: Option<Bytes>,
}

/// The mutation kind of a [`RecordChange`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeOp {
    /// Insert or update — `RecordChange::value` carries the new bytes.
    Put,
    /// Delete — `RecordChange::value` is `None`.
    Delete,
}

/// One committed transaction, projected for the changefeed.
///
/// Journal keys order events by `commit_version` (monotonic per repo), but
/// enqueue and live-broadcast order need not match version order. The engine
/// enqueues immediately BEFORE version publication and broadcasts AFTER it.
/// Engine journal pulls confirm and bound the settled prefix before exposing
/// events; live subscribers observe published versions in completion order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogEvent {
    /// Repository name this commit belongs to.
    pub repo: String,
    /// MVCC commit version assigned to this tx (monotonic per repo).
    pub commit_version: u64,
    /// The committing transaction's id.
    pub tx_id: u64,
    /// The actor that initiated the transaction.
    pub actor: Actor,
    /// Wall-clock nanoseconds at projection time (best-effort, for
    /// observability — NOT used for ordering; `commit_version` is).
    pub timestamp_ns: u64,
    /// Per-record changes carried by this commit.
    pub changes: Vec<RecordChange>,
}

/// `bytes::Bytes` <-> serde adapter (serialises as a byte sequence).
mod serde_bytes_compat {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(b)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let v = Vec::<u8>::deserialize(d)?;
        Ok(Bytes::from(v))
    }
}

/// `Option<bytes::Bytes>` <-> serde adapter.
mod serde_opt_bytes_compat {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Option<Bytes>, s: S) -> Result<S::Ok, S::Error> {
        match b {
            Some(b) => s.serialize_some(b.as_ref()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Bytes>, D::Error> {
        let v = Option::<Vec<u8>>::deserialize(d)?;
        Ok(v.map(Bytes::from))
    }
}

/// Capacity of the per-repo live broadcast ring. A subscriber lagging past
/// this many events sees `RecvError::Lagged` and catches up via the journal.
pub const BROADCAST_CAPACITY: usize = 1024;

/// Bounded depth of the journal-writer channel. `try_send` past this drops
/// the event with a warning (see [`RepoChangefeed::emit`]).
pub const JOURNAL_CHANNEL_CAPACITY: usize = 4096;

/// How many pending events the background writer drains per flush batch.
const WRITER_BATCH: usize = 256;

/// Trait the journal writer needs from a per-repo durable store. Mirrors
/// the subset of `shamir_storage::types::Store` the changelog uses, kept
/// as a tiny local trait so this module stays storage-agnostic and easily
/// testable (an in-memory fake implements it in unit tests).
#[async_trait::async_trait]
pub trait ChangelogStore: Send + Sync {
    /// Persist a journal pair or the dedicated [`gap_marker_key`] metadata.
    /// Clean-horizon puts must flush prior events BEFORE set, then flush the
    /// metadata AFTER set. Success certifies both boundaries, not just set.
    /// Marker success must include the backend's explicit durability boundary.
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String>;
    /// Range-read serialized events with keys in
    /// `[from_key, ..)` ascending, up to `limit` entries, excluding metadata.
    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String>;
}

/// Barriers share the bounded channel's 4096 slots with events.
#[derive(Debug)]
enum JournalMsg {
    Event(Arc<ChangelogEvent>),
    Barrier(u64),
    CleanHorizon {
        through: u64,
        horizon: Arc<AtomicU64>,
        ack: oneshot::Sender<Result<(), String>>,
    },
    RecoveryGap {
        interval: (u64, u64),
        ack: oneshot::Sender<Result<(), String>>,
    },
}

/// Per-repo changefeed: live broadcast + durable journal writer.
///
/// Constructed lazily by the engine's `RepoInstance` on first use (so a
/// repo that never subscribes still gets a journal — the feed is "always
/// on" by design, but the projection only happens when there are real
/// changes to emit).
pub struct RepoChangefeed {
    /// Live-push fan-out. `Arc<ChangelogEvent>` so a single projection is
    /// shared by every subscriber without per-subscriber clones.
    live: broadcast::Sender<Arc<ChangelogEvent>>,
    /// Sender side of the journal-writer channel. `try_send` from the
    /// commit-path; the background task owns the receiver.
    journal_tx: mpsc::Sender<JournalMsg>,
    /// Wakes the writer for shutdown-drain.
    notify: Arc<Notify>,
    /// Set to stop the writer after it drains the channel.
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    /// Count of events deferred because the journal channel was full
    /// (recorded in the repair-pending set, NOT dropped). Observability only.
    journal_dropped: AtomicU64,
    /// CF-2 heartbeat/durability-watermark: the highest `commit_version`
    /// successfully persisted by the background writer (updated via
    /// `fetch_max` after every `persist_one`). A reader can compare this
    /// against the highest emitted version to detect a stalled/dead writer.
    /// `0` means "nothing persisted yet".
    last_persisted_version: Arc<AtomicU64>,
    /// Shared conservative hole interval and writer wakeup.
    gap: Arc<GapMarker>,
    confirmed_through: Arc<AtomicU64>,
    barrier_notify: Arc<Notify>,
    writer_done: watch::Receiver<bool>,
    clean_horizon: Arc<AtomicU64>,
    confirm_timeouts: AtomicU64,
    /// Channel-full deferrals awaiting re-enqueue by the engine drainer.
    repair_pending: Arc<RepairPending>,
}

/// Result of a [`RepoChangefeed::read_from`] call.
///
/// `gap_at` is `Some(max(from_version, first_missing))` while the cursor is
/// at or before the invalidation horizon. The consumer must snapshot resync
/// rather than trusting the journal for an unbroken history.
pub struct JournalRead {
    /// Events read from the journal.
    pub events: Vec<ChangelogEvent>,
    /// `Some(v)` if a gap is known at version `v >= from_version`.
    pub gap_at: Option<u64>,
}

impl std::fmt::Debug for RepoChangefeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoChangefeed")
            .field("subscribers", &self.live.receiver_count())
            .field(
                "journal_dropped",
                &self.journal_dropped.load(Ordering::Relaxed),
            )
            .field(
                "last_persisted_version",
                &self.last_persisted_version.load(Ordering::Relaxed),
            )
            .field("gap_interval", &self.gap.current.load_full())
            .finish()
    }
}

impl RepoChangefeed {
    /// Build a changefeed bound to `store` and spawn its background writer.
    ///
    /// The writer holds only a `Weak` to nothing — it owns the receiver and
    /// the `Arc<dyn ChangelogStore>`; it exits when the `journal_tx` sender
    /// is dropped (channel closed) or `shutdown` is set + `notify` fired.
    pub fn new(store: Arc<dyn ChangelogStore>) -> Arc<Self> {
        Self::new_with_gap(store, 0)
    }

    /// Seed only a loaded or flushed point marker; `0` means absent.
    /// Existing `new` callers must only use it for stores without a prior marker.
    /// Exactly one live feed may own a given store's marker writer at a time;
    /// stop/drain the old writer before re-opening the same store.
    pub fn new_with_gap(store: Arc<dyn ChangelogStore>, restored_gap: u64) -> Arc<Self> {
        Self::new_with_gap_interval(
            store,
            (restored_gap != 0).then_some((restored_gap, restored_gap)),
        )
    }

    /// Seed ONLY an interval loaded from durable storage or successfully flushed
    /// before construction. Exactly one live feed owns the store's marker writer.
    pub fn new_with_gap_interval(
        store: Arc<dyn ChangelogStore>,
        restored_gap: Option<(u64, u64)>,
    ) -> Arc<Self> {
        assert!(restored_gap.is_none_or(|(first, through)| first != 0 && through >= first));
        let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (journal_tx, journal_rx) = mpsc::channel(JOURNAL_CHANNEL_CAPACITY);
        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let last_persisted_version = Arc::new(AtomicU64::new(0));
        let confirmed_through = Arc::new(AtomicU64::new(0));
        let barrier_notify = Arc::new(Notify::new());
        let gap = Arc::new(GapMarker {
            current: ArcSwap::from_pointee(restored_gap),
            persisted: ArcSwap::from_pointee(restored_gap),
            notify: Arc::clone(&notify),
        });

        let (done, writer_done) = watch::channel(false);
        let me = Arc::new(Self {
            live,
            journal_tx,
            notify: Arc::clone(&notify),
            shutdown: Arc::clone(&shutdown),
            journal_dropped: AtomicU64::new(0),
            last_persisted_version: Arc::clone(&last_persisted_version),
            gap: Arc::clone(&gap),
            confirmed_through: Arc::clone(&confirmed_through),
            barrier_notify: Arc::clone(&barrier_notify),
            writer_done,
            clean_horizon: Arc::new(AtomicU64::new(0)),
            confirm_timeouts: AtomicU64::new(0),
            repair_pending: Arc::new(RepairPendingInner::new()),
        });

        // fire-and-forget: detached by design; owns no feed and drains on shutdown.
        tokio::spawn(async move {
            journal_writer_loop(
                journal_rx,
                store,
                notify,
                shutdown,
                last_persisted_version,
                gap,
                confirmed_through,
                barrier_notify,
                Arc::clone(&me.repair_pending),
            )
            .await;
            // No waiter is required; channel closure also reports writer failure.
            let _ = done.send(true);
        });
        me
    }

    /// Subscribe to the live feed. Returns a fresh receiver; events emitted
    /// after this call are delivered. To not miss the window between "what
    /// is already in the journal" and "what arrives live", a caller should
    /// subscribe FIRST, then `read_from` the journal, then drain the live
    /// receiver — the journal/live overlap is de-duplicated by
    /// `commit_version`.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<ChangelogEvent>> {
        self.live.subscribe()
    }

    /// Emit one projected event down BOTH tracks. NEVER blocks and NEVER
    /// errors out to the caller.
    ///
    /// - Live: `broadcast::Sender::send` returns immediately; `Err` (no
    ///   subscribers) is ignored.
    /// - Journal: `mpsc::Sender::try_send`; on `Full` the event is deferred
    ///   to the repair-pending set with a warning and a counter bump (the
    ///   engine drainer re-enqueues it later; the WAL entry stays retained).
    ///
    /// The single `Arc<ChangelogEvent>` is shared by both tracks.
    pub fn emit(&self, event: ChangelogEvent) {
        let event = Arc::new(event);

        self.journal_enqueue(&event);
        self.broadcast(event);
    }

    /// L10(a) journal-safe: emit ONLY to the durable journal, skipping the
    /// live broadcast. Used when there are no live subscribers — saves the
    /// broadcast overhead while preserving the journal invariant that
    /// `changes_since` / late subscribers always see every committed event.
    pub fn emit_journal_only(&self, event: ChangelogEvent) {
        let event = Arc::new(event);
        self.journal_enqueue(&event);
    }

    /// Enqueue without awaiting. Full defers to the repair-pending set;
    /// Closed marks a gap.
    pub fn journal_enqueue(&self, event: &Arc<ChangelogEvent>) {
        self.journal_enqueue_impl(event);
    }

    /// Broadcast only, without enqueueing or waiting for subscribers.
    pub fn broadcast(&self, event: Arc<ChangelogEvent>) {
        let _ = self.live.send(event);
    }

    /// Confirm the channel prefix preceding a barrier for completion watermark `w`.
    /// Callers must enqueue every published event (or mark its gap) before `w`
    /// advances; a persistence maximum or an event counter is not a safe bound.
    /// The single budget covers channel admission and confirmation together.
    /// cancel-safe: yes — an unsent barrier may be discarded; a sent barrier
    /// only monotonically confirms a prefix, and notification waits mutate no data.
    pub async fn confirm_through(&self, w: u64, budget: Duration) -> u64 {
        if self.confirmed_through.load(Ordering::Acquire) >= w {
            return w;
        }
        let deadline = Instant::now() + budget;
        match timeout_at(deadline, self.journal_tx.send(JournalMsg::Barrier(w))).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return self.confirmed_through.load(Ordering::Acquire),
            Err(_) => return self.confirm_timeout(w),
        }
        loop {
            let notified = self.barrier_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.confirmed_through.load(Ordering::Acquire) >= w {
                return w;
            }
            tokio::select! {
                biased;
                () = sleep_until(deadline) => return self.confirm_timeout(w),
                () = &mut notified => {}
            }
        }
    }

    fn confirm_timeout(&self, w: u64) -> u64 {
        self.confirm_timeouts.fetch_add(1, Ordering::Relaxed);
        let bound = self.confirmed_through.load(Ordering::Acquire);
        log::warn!("changefeed journal confirmation timed out at {w}; safe bound {bound}");
        bound
    }

    /// Total number of journal events deferred due to channel overflow
    /// (recorded in the repair-pending set, not dropped).
    pub fn journal_dropped(&self) -> u64 {
        self.journal_dropped.load(Ordering::Relaxed)
    }

    /// CF-2: the highest `commit_version` the background writer has durably
    /// persisted. Returns `0` if nothing has been persisted yet.
    ///
    /// A consumer can compare this against the highest emitted version to
    /// detect a stalled or dead writer (if `last_persisted_version` stops
    /// advancing while `emit` keeps being called, the writer has crashed).
    pub fn last_persisted_version(&self) -> u64 {
        self.last_persisted_version.load(Ordering::Acquire)
    }

    /// Record a known point hole, monotonically widening the interval.
    /// Version zero (a read starting before the first commit) maps to one.
    /// Safe on non-runtime threads: only RCU/CAS and a writer notification.
    /// RAM visibility is immediate; durability follows asynchronously. Failed
    /// marker writes retain dirty state and retry on writer activity, explicit
    /// flush hints, repeated marks, and shutdown (no busy retry loop).
    pub fn mark_gap(&self, version: u64) {
        self.gap.mark_gap(version);
    }

    /// Durably certify recovery invalidation through the sole marker writer.
    /// Admission awaits capacity in the existing bounded queue; commits still
    /// only try_send. Success includes marker put + flush and all runtime marks
    /// observed during that put. Failures never publish a speculative repair.
    /// Cancellation before admission does nothing; after admission the writer
    /// completes independently, so cancellation is not a durability acknowledgement.
    pub async fn record_recovery_gap(
        &self,
        first_missing: u64,
        affected_through: u64,
    ) -> DbResult<()> {
        if first_missing == 0 || affected_through < first_missing {
            return Err(DbError::Storage("invalid recovery gap interval".into()));
        }
        let (ack, result) = oneshot::channel();
        self.journal_tx
            .send(JournalMsg::RecoveryGap {
                interval: (first_missing, affected_through),
                ack,
            })
            .await
            .map_err(|_| DbError::Storage("changefeed writer closed".into()))?;
        result
            .await
            .map_err(|_| DbError::Storage("changefeed repair acknowledgement lost".into()))?
            .map_err(DbError::Storage)
    }

    /// Construct from strictly loaded durable metadata. One writer per store.
    pub fn new_with_metadata(
        store: Arc<dyn ChangelogStore>,
        restored_gap: Option<(u64, u64)>,
        clean_horizon: u64,
    ) -> Arc<Self> {
        let feed = Self::new_with_gap_interval(store, restored_gap);
        feed.clean_horizon.store(clean_horizon, Ordering::Release);
        feed
    }

    /// A checkpoint covers the sampled settled prefix. Any unresolved version
    /// in that prefix must already have a durable journal event or persisted
    /// gap before J advances.
    /// cancel-safe: NO — after admission the writer owns the request. A closed
    /// acknowledgement suppresses a not-yet-started J put, but an in-flight
    /// backend write may finish; cancellation never acknowledges durability.
    pub async fn record_clean_horizon(&self, through: u64) -> DbResult<()> {
        let (ack, result) = oneshot::channel();
        self.journal_tx
            .send(JournalMsg::CleanHorizon {
                through,
                horizon: Arc::clone(&self.clean_horizon),
                ack,
            })
            .await
            .map_err(|_| DbError::Storage("changefeed writer closed".into()))?;
        result
            .await
            .map_err(|_| DbError::Storage("clean horizon acknowledgement lost".into()))?
            .map_err(DbError::Storage)
    }

    /// CF-1: the lowest known journal hole; zero means none.
    ///
    /// A non-zero value means the durable journal has a gap: at least one
    /// event at this version was never written. A consumer reading the journal
    /// from a version at or before this value cannot trust it for an unbroken
    /// history and should perform a full snapshot resync.
    pub fn first_gap_version(&self) -> u64 {
        (*self.gap.current.load_full()).map_or(0, |(first, _)| first)
    }

    /// Stop the writer without recording J, then await its final drain.
    /// Admission must already be stopped. Suitable for hard-reopen fixtures:
    /// unlike a clean checkpoint, this never certifies the stopped prefix.
    /// cancel-safe: yes — cancellation drops only this waiter; shutdown remains
    /// requested and a later waiter can observe the same termination signal.
    pub async fn shutdown_and_wait(&self) -> DbResult<()> {
        self.shutdown();
        let mut done = self.writer_done.clone();
        done.wait_for(|finished| *finished)
            .await
            .map_err(|_| DbError::Storage("changefeed writer termination lost".into()))?;
        Ok(())
    }

    /// Read up to `limit` journal events with `commit_version >=
    /// from_version`, ascending. Resumable pull: a consumer that processed
    /// through version V calls `read_from(V + 1, n)` to continue.
    ///
    /// Returns a [`JournalRead`] that carries the events AND a `gap_at` field.
    /// When `gap_at` is `Some(v)` a known-dropped version `v >= from_version`
    /// exists — the journal is not contiguous from `from_version` and the
    /// consumer must perform a full snapshot resync instead of trusting it for
    /// an unbroken history.
    pub async fn read_from(
        &self,
        store: &Arc<dyn ChangelogStore>,
        from_version: u64,
        limit: usize,
    ) -> JournalRead {
        self.read_from_bounded(store, from_version, limit, u64::MAX)
            .await
    }

    /// Read the ascending prefix at or below `bound`, obtained by confirming
    /// the caller's completion watermark first. Gap inspection follows that barrier.
    pub async fn read_from_bounded(
        &self,
        store: &Arc<dyn ChangelogStore>,
        from_version: u64,
        limit: usize,
        bound: u64,
    ) -> JournalRead {
        let from_key = Bytes::copy_from_slice(&from_version.to_be_bytes());
        let raw = match store.range_from(from_key, limit).await {
            Ok(v) => v,
            Err(e) => {
                log::warn!("changefeed read_from(store) failed at {from_version}: {e}");
                return JournalRead {
                    events: Vec::new(),
                    gap_at: Some(from_version),
                };
            }
        };
        let mut events = Vec::with_capacity(raw.len());
        let mut corrupt_gap = None;
        for bytes in raw {
            match rmp_serde::from_slice::<ChangelogEvent>(&bytes) {
                Ok(ev) => {
                    if ev.commit_version > bound {
                        break;
                    }
                    events.push(ev);
                }
                Err(e) => {
                    self.mark_gap(from_version);
                    corrupt_gap = Some(from_version.max(1));
                    log::warn!("changefeed read_from: corrupt journal entry skipped: {e}");
                }
            }
        }
        // CF-1: signal if any known-dropped version lies at/after from_version.
        // Conservative over-signal is acceptable; silent omission is not.
        let interval = *self.gap.current.load_full();
        let gap_at = match interval {
            Some((first, through)) if from_version <= through => Some(from_version.max(first)),
            _ => corrupt_gap,
        };
        JournalRead { events, gap_at }
    }
}

/// Versioned clean-stop / cold-certification metadata, never a journal event.
pub fn clean_horizon_key() -> Bytes {
    Bytes::from_static(b"\0journal-clean-v1")
}

/// Format: ASCII JCLN, format version 1, BE u64 horizon (13 bytes).
pub fn encode_clean_horizon(through: u64) -> Bytes {
    let mut bytes = Vec::with_capacity(13);
    bytes.extend_from_slice(b"JCLN\x01");
    bytes.extend_from_slice(&through.to_be_bytes());
    Bytes::from(bytes)
}

pub fn decode_clean_horizon(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() != 13 || &bytes[..5] != b"JCLN\x01" {
        return Err("invalid clean horizon format/version/length".into());
    }
    Ok(u64::from_be_bytes(
        bytes[5..].try_into().map_err(|_| "invalid clean horizon")?,
    ))
}

/// Dedicated metadata key, sorting before every eight-byte journal key.
/// This key is never an event and must be excluded from journal ranges.
pub fn gap_marker_key() -> Bytes {
    Bytes::from_static(b"\0")
}

/// Encode a valid conservative interval. Point markers retain the legacy BE u64
/// encoding; ranges append a BE u64 upper endpoint (16 bytes total).
pub fn encode_gap_marker((first, through): (u64, u64)) -> Bytes {
    assert!(first != 0 && through >= first);
    if first == through {
        return version_key(first);
    }
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&first.to_be_bytes());
    bytes.extend_from_slice(&through.to_be_bytes());
    Bytes::from(bytes)
}

/// Decode legacy point metadata or the extended interval, rejecting corruption.
pub fn decode_gap_marker(bytes: &[u8]) -> Result<(u64, u64), String> {
    if bytes.len() != 8 && bytes.len() != 16 {
        return Err("invalid gap marker length".into());
    }
    let first = u64::from_be_bytes(bytes[..8].try_into().map_err(|_| "invalid first")?);
    let through = if bytes.len() == 8 {
        first
    } else {
        u64::from_be_bytes(bytes[8..].try_into().map_err(|_| "invalid through")?)
    };
    if first == 0 || through < first {
        return Err("invalid gap marker interval".into());
    }
    Ok((first, through))
}

fn merge_gap(current: Option<(u64, u64)>, (first, through): (u64, u64)) -> (u64, u64) {
    current.map_or((first, through), |(old_first, old_through)| {
        (old_first.min(first), old_through.max(through))
    })
}

/// Shared atomic snapshots; only the journal writer performs marker I/O.
struct GapMarker {
    // Publish the pair with one CAS/RCU snapshot: independent endpoint atomics
    // could expose a torn interval. Allocation/CAS occurs only on gap events,
    // never healthy commits, and readers take no Mutex/RwLock.
    current: ArcSwap<Option<(u64, u64)>>,
    persisted: ArcSwap<Option<(u64, u64)>>,
    notify: Arc<Notify>,
}

impl GapMarker {
    fn merge(&self, interval: (u64, u64)) {
        self.current
            .rcu(|current| Some(merge_gap(**current, interval)));
    }

    fn mark_gap(&self, version: u64) {
        let version = version.max(1);
        self.merge((version, version));
        // Also wake for unchanged marks: a prior marker put may have failed.
        self.notify.notify_one();
    }

    async fn persist(&self, store: &Arc<dyn ChangelogStore>) {
        if let Err(e) = self.persist_current(store).await {
            log::warn!("changefeed gap marker put failed: {e}");
        }
    }

    async fn persist_current(&self, store: &Arc<dyn ChangelogStore>) -> Result<(), String> {
        loop {
            let current = *self.current.load_full();
            if current == *self.persisted.load_full() {
                return Ok(());
            }
            let Some(interval) = current else {
                return Ok(());
            };
            store
                .put(gap_marker_key(), encode_gap_marker(interval))
                .await?;
            self.persisted.store(Arc::new(Some(interval)));
            // Finish marks arriving during put before sleeping or acknowledging.
            // The sole writer never completes an older put after a newer one.
        }
    }

    async fn persist_recovery(
        &self,
        store: &Arc<dyn ChangelogStore>,
        interval: (u64, u64),
    ) -> Result<(), String> {
        let merged = merge_gap(*self.current.load_full(), interval);
        // Do NOT put the proposed repair into RAM until put's flush succeeds.
        // On error the store may have staged bytes, but no speculative repair
        // can be retried by the runtime writer or exposed to readers.
        store
            .put(gap_marker_key(), encode_gap_marker(merged))
            .await?;
        self.persisted.store(Arc::new(Some(merged)));
        self.merge(merged);
        self.persist_current(store).await
    }
}

/// Big-endian 8-byte key for a commit version — lexicographic order on the
/// store matches numeric order.
pub fn version_key(commit_version: u64) -> Bytes {
    Bytes::copy_from_slice(&commit_version.to_be_bytes())
}

/// Project a committed [`crate::TxContext`] into a [`ChangelogEvent`].
///
/// Must be called on the commit-path BEFORE `write_set` is drained
/// (Phase 5a `collect_data_batches` consumes it), so the staged values are
/// still present. Reads each per-table `StagingStore` via `iter_ops`
/// (borrowed, no intermediate `Vec<KvOp>`) and resolves the human-readable
/// table name from `tx.table_tokens`.
///
/// Returns `None` when the tx staged no data writes — an empty footprint
/// emits nothing (index-only / counter-only commits do not produce a
/// record-level changefeed event in this MVP; see module docs).
///
/// `repo` is supplied by the engine (the `TxContext` only carries the
/// interned `repo_id`). `timestamp_ns` is captured here, best-effort.
pub fn project_event(
    tx: &crate::TxContext,
    repo: &str,
    commit_version: u64,
) -> Option<ChangelogEvent> {
    let total_ops: usize = tx.write_set.values().map(|s| s.len()).sum();
    let mut changes: Vec<RecordChange> = Vec::with_capacity(total_ops);
    for (token, staging) in &tx.write_set {
        let table = tx
            .table_tokens
            .get(token)
            .cloned()
            .unwrap_or_else(|| format!("token:{token}"));
        for kv in staging.iter_ops() {
            match kv {
                shamir_storage::types::KvOp::Set(key, value) => changes.push(RecordChange {
                    table: table.clone(),
                    // Boundary: KvOp keys are `RecordKey`; the changelog event
                    // key is `Bytes` (byte-identical conversion).
                    key: key.into(),
                    op: ChangeOp::Put,
                    value: Some(value),
                }),
                shamir_storage::types::KvOp::Remove(key) => changes.push(RecordChange {
                    table: table.clone(),
                    key: key.into(),
                    op: ChangeOp::Delete,
                    value: None,
                }),
            }
        }
    }

    if changes.is_empty() {
        return None;
    }

    let timestamp_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    Some(ChangelogEvent {
        repo: repo.to_string(),
        commit_version,
        // EXTERNAL contract: implicit batch txs (non-tx writes routed through
        // the tx pipeline, F4b-1) report tx_id == 0 — "0 = non-tx write" per
        // docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md. The INTERNAL tx.tx_id stays real.
        tx_id: if tx.implicit { 0 } else { tx.tx_id.0 },
        actor: tx.actor.clone(),
        timestamp_ns,
        changes,
    })
}

/// Assemble a [`ChangelogEvent`] from pre-built record changes for a
/// **non-transactional** write batch.
///
/// The transactional [`project_event`] reads its changes out of a
/// committed [`crate::TxContext`]'s staging snapshot. Non-tx writes
/// (`execute_insert` / `execute_update` / `execute_set` / `execute_delete`)
/// never build a `TxContext`; they apply mutations directly to the table
/// and already hold the `(key, value)` pairs they wrote. This constructor
/// lets that path emit an identically-shaped event without a tx.
///
/// `commit_version` MUST be allocated from the SAME per-repo
/// [`crate::RepoTxGate`] the commit pipeline uses, so non-tx and tx events
/// share one monotonic version sequence per repo. `tx_id` is `0` — a non-tx
/// write has no transaction id (the field is retained for shape parity).
///
/// Returns `None` for an empty `changes` vector — an empty footprint emits
/// nothing, matching [`project_event`].
pub fn nontx_event(
    repo: &str,
    commit_version: u64,
    actor: Actor,
    changes: Vec<RecordChange>,
) -> Option<ChangelogEvent> {
    if changes.is_empty() {
        return None;
    }
    let timestamp_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some(ChangelogEvent {
        repo: repo.to_string(),
        commit_version,
        tx_id: 0,
        actor,
        timestamp_ns,
        changes,
    })
}

/// Serialise an event to msgpack for the journal store.
fn serialize_event(ev: &ChangelogEvent) -> Result<Bytes, String> {
    rmp_serde::to_vec(ev)
        .map(Bytes::from)
        .map_err(|e| format!("changefeed serialize: {e}"))
}

/// Background journal writer. Drains the channel in batches, persists each
/// event to the store keyed by its `commit_version` (BE bytes), and exits
/// on shutdown after a final drain.
///
/// After every successful `persist_one` it advances `last_persisted_version`
/// monotonically via `fetch_max` (CF-2 heartbeat/durability-watermark).
///
/// Crash-window: an event acked into the channel but not yet persisted when
/// the process dies is lost — by design (see module docs).
#[allow(clippy::too_many_arguments)]
async fn journal_writer_loop(
    mut rx: mpsc::Receiver<JournalMsg>,
    store: Arc<dyn ChangelogStore>,
    notify: Arc<Notify>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    last_persisted_version: Arc<AtomicU64>,
    gap: Arc<GapMarker>,
    confirmed_through: Arc<AtomicU64>,
    barrier_notify: Arc<Notify>,
    repair_pending: Arc<RepairPendingInner>,
) {
    loop {
        gap.persist(&store).await;
        // Re-check shutdown at the top so a flag set between iterations is
        // honoured deterministically (closes the lost-wakeup window of a
        // freshly-armed `notified()` missing an already-consumed permit —
        // §B15e). On shutdown, drain whatever is buffered and exit.
        if shutdown.load(Ordering::SeqCst) {
            drain_and_persist(
                &mut rx,
                &store,
                &last_persisted_version,
                &gap,
                &confirmed_through,
                &barrier_notify,
                &repair_pending,
            )
            .await;
            break;
        }

        // Block until at least one event is ready, a shutdown/flush hint
        // fires, or the channel closes (all senders dropped).
        let first = tokio::select! {
            biased;
            ev = rx.recv() => ev,
            () = notify.notified() => {
                // A hint — drain whatever is buffered, then decide on exit.
                drain_and_persist(
                    &mut rx,
                    &store,
                    &last_persisted_version,
                    &gap,
                    &confirmed_through,
                    &barrier_notify,
                    &repair_pending,
                )
                .await;
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
        };

        let Some(first) = first else {
            // Channel closed: senders gone. Final drain (nothing left, but
            // defensive) and exit.
            drain_and_persist(
                &mut rx,
                &store,
                &last_persisted_version,
                &gap,
                &confirmed_through,
                &barrier_notify,
                &repair_pending,
            )
            .await;
            break;
        };

        process_journal_msg(
            &store,
            first,
            &last_persisted_version,
            &gap,
            &confirmed_through,
            &barrier_notify,
            &repair_pending,
        )
        .await;
        // Opportunistically batch any already-queued events.
        let mut batched = 0usize;
        while batched < WRITER_BATCH {
            match rx.try_recv() {
                Ok(ev) => {
                    process_journal_msg(
                        &store,
                        ev,
                        &last_persisted_version,
                        &gap,
                        &confirmed_through,
                        &barrier_notify,
                        &repair_pending,
                    )
                    .await;
                    batched += 1;
                }
                Err(_) => break,
            }
        }

        if shutdown.load(Ordering::SeqCst) {
            drain_and_persist(
                &mut rx,
                &store,
                &last_persisted_version,
                &gap,
                &confirmed_through,
                &barrier_notify,
                &repair_pending,
            )
            .await;
            break;
        }
    }
}

/// Drain every currently-queued event and persist it.
async fn drain_and_persist(
    rx: &mut mpsc::Receiver<JournalMsg>,
    store: &Arc<dyn ChangelogStore>,
    last_persisted_version: &Arc<AtomicU64>,
    gap: &GapMarker,
    confirmed_through: &AtomicU64,
    barrier_notify: &Notify,
    repair_pending: &RepairPendingInner,
) {
    while let Ok(ev) = rx.try_recv() {
        process_journal_msg(
            store,
            ev,
            last_persisted_version,
            gap,
            confirmed_through,
            barrier_notify,
            repair_pending,
        )
        .await;
    }
    gap.persist(store).await;
}

async fn process_journal_msg(
    store: &Arc<dyn ChangelogStore>,
    msg: JournalMsg,
    last_persisted_version: &Arc<AtomicU64>,
    gap: &GapMarker,
    confirmed_through: &AtomicU64,
    barrier_notify: &Notify,
    repair_pending: &RepairPendingInner,
) {
    match msg {
        JournalMsg::Event(ev) => persist_one(store, &ev, last_persisted_version, gap).await,
        JournalMsg::CleanHorizon {
            through,
            horizon,
            ack,
        } => {
            // FIFO processing drains all preceding events. Failed events have
            // marked gaps; those gaps must cross durability before J can advance.
            let result = async {
                gap.persist_current(store).await?;
                if ack.is_closed() {
                    return Err("clean horizon caller cancelled".into());
                }
                let through = through.max(horizon.load(Ordering::Acquire));
                store
                    .put(clean_horizon_key(), encode_clean_horizon(through))
                    .await?;
                horizon.store(through, Ordering::Release);
                Ok(())
            }
            .await;
            // Cancellation means the caller no longer wants the acknowledgement.
            let _ = ack.send(result);
        }
        JournalMsg::RecoveryGap { interval, ack } => {
            let result = gap.persist_recovery(store, interval).await;
            // A cancelled caller no longer wants the ack; the repair still finishes.
            let _ = ack.send(result);
        }
        JournalMsg::Barrier(w) => {
            let w = repair_pending.clamp_barrier(w);
            confirmed_through.fetch_max(w, Ordering::Release);
            barrier_notify.notify_waiters();
        }
    }
}

/// Persist a single event. Failures are logged, never propagated — the
/// journal is best-effort and must not stall the writer loop.
///
/// On success, advances `last_persisted_version` monotonically (CF-2
/// heartbeat/durability-watermark).
async fn persist_one(
    store: &Arc<dyn ChangelogStore>,
    ev: &ChangelogEvent,
    last_persisted_version: &Arc<AtomicU64>,
    gap: &GapMarker,
) {
    let value = match serialize_event(ev) {
        Ok(v) => v,
        Err(e) => {
            gap.mark_gap(ev.commit_version);
            log::warn!("changefeed writer: {e}; dropping journal entry");
            return;
        }
    };
    let key = version_key(ev.commit_version);
    if let Err(e) = store.put(key, value).await {
        gap.mark_gap(ev.commit_version);
        log::warn!(
            "changefeed writer: journal put failed for repo {} commit_version {}: {e}",
            ev.repo,
            ev.commit_version
        );
    } else {
        // CF-2: advance the watermark monotonically.
        last_persisted_version.fetch_max(ev.commit_version, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
