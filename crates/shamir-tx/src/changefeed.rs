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
//!    the changelog store. [`RepoChangefeed::read_from`] range-reads the
//!    journal from a given version (resumable pull). A late subscriber
//!    catches up via `read_from`, then switches to the live broadcast.
//!
//! NEITHER track blocks the hot commit-path. The projection is done once
//! and reused for both tracks; a commit with an empty footprint emits
//! nothing.
//!
//! ## Durability trade-off (journal)
//!
//! The journal write is asynchronous: `try_send` enqueues the event and
//! returns immediately, and the background writer persists it shortly
//! after. There is therefore a small window after a commit acks during
//! which a process crash loses the not-yet-flushed journal tail. This is
//! by design — the journal must never sit on the commit's critical path.
//! The committed DATA itself is durable independently (Phase 4 WAL); the
//! journal is an *observability/replication* feed, not a source of truth.
//! On channel overflow (a sustained burst faster than the writer drains)
//! the event is dropped with a `log::warn!` rather than growing memory
//! unboundedly — see [`RepoChangefeed::emit`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_types::access::Actor;
use tokio::sync::{broadcast, mpsc, Notify};

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
/// Ordered by `commit_version` (monotonic per repo). Emitted from the
/// commit-path AFTER `gate.publish_committed`, so by the time a subscriber
/// (or journal reader) observes it the version is already visible to
/// readers.
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
    /// Durably persist one `(version_be_key, serialized_event)` pair.
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String>;
    /// Range-read serialized events with keys in
    /// `[from_key, ..)` ascending, up to `limit` entries.
    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String>;
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
    journal_tx: mpsc::Sender<Arc<ChangelogEvent>>,
    /// Wakes the writer for shutdown-drain.
    notify: Arc<Notify>,
    /// Set to stop the writer after it drains the channel.
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    /// Count of events dropped because the journal channel was full.
    /// Observability only.
    journal_dropped: AtomicU64,
    /// CF-2 heartbeat/durability-watermark: the highest `commit_version`
    /// successfully persisted by the background writer (updated via
    /// `fetch_max` after every `persist_one`). A reader can compare this
    /// against the highest emitted version to detect a stalled/dead writer.
    /// `0` means "nothing persisted yet".
    last_persisted_version: Arc<AtomicU64>,
    /// CF-1 gap marker: the lowest `commit_version` that was dropped due to
    /// journal-channel overflow, or `0` if no drops have occurred. Once set
    /// it is only ever lowered (CAS min-loop), so it faithfully marks the
    /// earliest known hole in the durable journal.
    first_gap_version: AtomicU64,
}

/// Result of a [`RepoChangefeed::read_from`] call.
///
/// `gap_at` is `Some(v)` when a known-dropped version `v` lies at or after
/// `from_version` — the journal is NOT contiguous from that point and the
/// consumer should perform a full snapshot resync rather than trusting the
/// journal for an unbroken history.
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
            .field(
                "first_gap_version",
                &self.first_gap_version.load(Ordering::Relaxed),
            )
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
        let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (journal_tx, journal_rx) = mpsc::channel(JOURNAL_CHANNEL_CAPACITY);
        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let last_persisted_version = Arc::new(AtomicU64::new(0));

        let me = Arc::new(Self {
            live,
            journal_tx,
            notify: Arc::clone(&notify),
            shutdown: Arc::clone(&shutdown),
            journal_dropped: AtomicU64::new(0),
            last_persisted_version: Arc::clone(&last_persisted_version),
            first_gap_version: AtomicU64::new(0),
        });

        tokio::spawn(journal_writer_loop(
            journal_rx,
            store,
            notify,
            shutdown,
            last_persisted_version,
        ));
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

    /// Number of live subscribers right now. Used by the commit-path to
    /// decide whether the live broadcast is worth attempting (the journal
    /// is fed unconditionally).
    pub fn subscriber_count(&self) -> usize {
        self.live.receiver_count()
    }

    /// Emit one projected event down BOTH tracks. NEVER blocks and NEVER
    /// errors out to the caller.
    ///
    /// - Live: `broadcast::Sender::send` returns immediately; `Err` (no
    ///   subscribers) is ignored.
    /// - Journal: `mpsc::Sender::try_send`; on `Full` the event is dropped
    ///   with a warning and a counter bump (bounded memory beats unbounded
    ///   growth — the data is durable via the WAL regardless).
    ///
    /// The single `Arc<ChangelogEvent>` is shared by both tracks.
    pub fn emit(&self, event: ChangelogEvent) {
        let event = Arc::new(event);

        // Track 1: live push (does not wait for receivers).
        let _ = self.live.send(Arc::clone(&event));

        // Track 2: durable journal (does not block the commit-path).
        if let Err(e) = self.journal_tx.try_send(event) {
            match e {
                mpsc::error::TrySendError::Full(ev) => {
                    let n = self.journal_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                    // CF-1: record the lowest dropped version so a resuming
                    // reader can detect the gap (min-CAS loop).
                    let v = ev.commit_version;
                    let mut cur = self.first_gap_version.load(Ordering::Relaxed);
                    loop {
                        if cur != 0 && cur <= v {
                            break;
                        }
                        match self.first_gap_version.compare_exchange_weak(
                            cur,
                            v,
                            Ordering::Release,
                            Ordering::Relaxed,
                        ) {
                            Ok(_) => break,
                            Err(actual) => cur = actual,
                        }
                    }
                    log::warn!(
                        "changefeed journal channel full; dropped event for repo {} \
                         commit_version {} (total dropped: {n})",
                        ev.repo,
                        ev.commit_version
                    );
                }
                mpsc::error::TrySendError::Closed(_) => {
                    // Writer task gone (shutdown). Nothing durable to do.
                }
            }
        }
    }

    /// Total number of journal events dropped due to channel overflow.
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

    /// CF-1: the lowest `commit_version` that was dropped due to
    /// journal-channel overflow. Returns `0` if no events have been dropped.
    ///
    /// A non-zero value means the durable journal has a gap: at least one
    /// event at this version was never written. A consumer reading the journal
    /// from a version at or before this value cannot trust it for an unbroken
    /// history and should perform a full snapshot resync.
    pub fn first_gap_version(&self) -> u64 {
        self.first_gap_version.load(Ordering::Acquire)
    }

    /// Wake the writer and ask it to drain + stop. Used at shutdown so the
    /// in-flight journal tail is flushed.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// Wake the writer to flush promptly (used by tests / explicit flush).
    pub fn flush_hint(&self) {
        self.notify.notify_one();
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
        let from_key = Bytes::copy_from_slice(&from_version.to_be_bytes());
        let raw = match store.range_from(from_key, limit).await {
            Ok(v) => v,
            Err(e) => {
                log::warn!("changefeed read_from(store) failed at {from_version}: {e}");
                return JournalRead {
                    events: Vec::new(),
                    gap_at: None,
                };
            }
        };
        let mut events = Vec::with_capacity(raw.len());
        for bytes in raw {
            match rmp_serde::from_slice::<ChangelogEvent>(&bytes) {
                Ok(ev) => events.push(ev),
                Err(e) => log::warn!("changefeed read_from: corrupt journal entry skipped: {e}"),
            }
        }
        // CF-1: signal if any known-dropped version lies at/after from_version.
        // Conservative over-signal is acceptable; silent omission is not.
        let g = self.first_gap_version.load(Ordering::Acquire);
        let gap_at = if g != 0 && g >= from_version {
            Some(g)
        } else {
            None
        };
        JournalRead { events, gap_at }
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
/// still present. Reads each per-table `StagingStore` via `snapshot_ops`
/// (the same alloc the WAL already pays) and resolves the human-readable
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
        for kv in staging.snapshot_ops() {
            match kv {
                shamir_storage::types::KvOp::Set(key, value) => changes.push(RecordChange {
                    table: table.clone(),
                    key,
                    op: ChangeOp::Put,
                    value: Some(value),
                }),
                shamir_storage::types::KvOp::Remove(key) => changes.push(RecordChange {
                    table: table.clone(),
                    key,
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
        tx_id: tx.tx_id.0,
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
async fn journal_writer_loop(
    mut rx: mpsc::Receiver<Arc<ChangelogEvent>>,
    store: Arc<dyn ChangelogStore>,
    notify: Arc<Notify>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    last_persisted_version: Arc<AtomicU64>,
) {
    loop {
        // Re-check shutdown at the top so a flag set between iterations is
        // honoured deterministically (closes the lost-wakeup window of a
        // freshly-armed `notified()` missing an already-consumed permit —
        // §B15e). On shutdown, drain whatever is buffered and exit.
        if shutdown.load(Ordering::SeqCst) {
            drain_and_persist(&mut rx, &store, &last_persisted_version).await;
            break;
        }

        // Block until at least one event is ready, a shutdown/flush hint
        // fires, or the channel closes (all senders dropped).
        let first = tokio::select! {
            biased;
            ev = rx.recv() => ev,
            () = notify.notified() => {
                // A hint — drain whatever is buffered, then decide on exit.
                drain_and_persist(&mut rx, &store, &last_persisted_version).await;
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
        };

        let Some(first) = first else {
            // Channel closed: senders gone. Final drain (nothing left, but
            // defensive) and exit.
            drain_and_persist(&mut rx, &store, &last_persisted_version).await;
            break;
        };

        persist_one(&store, &first, &last_persisted_version).await;
        // Opportunistically batch any already-queued events.
        let mut batched = 0usize;
        while batched < WRITER_BATCH {
            match rx.try_recv() {
                Ok(ev) => {
                    persist_one(&store, &ev, &last_persisted_version).await;
                    batched += 1;
                }
                Err(_) => break,
            }
        }

        if shutdown.load(Ordering::SeqCst) {
            drain_and_persist(&mut rx, &store, &last_persisted_version).await;
            break;
        }
    }
}

/// Drain every currently-queued event and persist it.
async fn drain_and_persist(
    rx: &mut mpsc::Receiver<Arc<ChangelogEvent>>,
    store: &Arc<dyn ChangelogStore>,
    last_persisted_version: &Arc<AtomicU64>,
) {
    while let Ok(ev) = rx.try_recv() {
        persist_one(store, &ev, last_persisted_version).await;
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
) {
    let value = match serialize_event(ev) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("changefeed writer: {e}; dropping journal entry");
            return;
        }
    };
    let key = version_key(ev.commit_version);
    if let Err(e) = store.put(key, value).await {
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
