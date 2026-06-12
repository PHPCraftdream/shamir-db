//! Per-transaction state bundle.
//!
//! Created at tx begin, consumed at commit (by the executor), or
//! dropped at abort. Drop = RAII rollback: all staged state is lost,
//! no storage side-effects.

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::THasher;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;

use crate::staging_store::StagingStore;
use crate::types::{IsolationLevel, TxId};
use crate::version_provider::VersionProvider;
use crate::IndexWriteOp;

/// Opt-in commit visibility / ack policy.
///
/// Default is [`Synchronous`](CommitVisibility::Synchronous) — the historical
/// behaviour: `commit_tx` returns to the caller only after EVERY phase of the
/// commit pipeline has completed (data, index, recovery markers, WAL marker
/// removal, HNSW promote). Nothing about the on-disk image or in-memory state
/// differs from the pre-async-mode code path.
///
/// [`AsyncIndex`](CommitVisibility::AsyncIndex) is the OPT-IN relaxation
/// described in the async-commit design:
///   * The Phase-4 WAL fsync (the commit point) ALWAYS happens before the
///     client ack — durability is NOT relaxed.
///   * Data is applied to MvccStore (Phase 5a) and the version is published
///     (Phase 6) BEFORE the ack — read-your-own-writes on DATA holds.
///   * Index posting application (Phase 5c), durable recovery markers
///     (Phase 6.5), WAL marker removal (Phase 7), and the HNSW promote
///     (Phase 5d, post-lock) are moved to a `tokio::task` that runs after
///     the client returns. A query served by a SECONDARY INDEX may briefly
///     miss the just-committed row; the row is immediately visible via a
///     data scan.
///   * Crash safety is preserved by the existing recovery machinery: if
///     the process dies after ack but before the background task finishes,
///     the inflight WAL marker survives and `recover_v2_inflight` replays
///     the entry on the next open — the same path that backs the
///     `MaterializationState::Deferred` contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitVisibility {
    /// Default. `commit_tx` returns only after the whole pipeline has run.
    #[default]
    Synchronous,
    /// Opt-in. `commit_tx` returns once WAL durability + data application +
    /// MVCC publish are done; index apply / markers / WAL cleanup / HNSW
    /// promote run on a background task.
    AsyncIndex,
}

/// A promise this tx makes about a unique-index posting: at stage time
/// the deterministic unique key `index_key` was free (or owned by this
/// tx's `owner`). Re-validated under `commit_lock` (closes the
/// tx-concurrent unique-violation hole) — two concurrent txs claiming
/// the same value produce the BYTE-IDENTICAL `index_key`, so a single
/// `info_store.get(index_key)` settles ownership decisively.
///
/// Layer note: `index_key` is built engine-side
/// (`build_index_key(true, name, values).to_bytes()`) and handed in as
/// raw `Bytes`; shamir-tx stays ignorant of how the key is composed.
#[derive(Debug, Clone)]
pub struct UniqueGuard {
    /// Owning table token (engine `table_token()`), used to resolve the
    /// table's `info_store` at commit time.
    pub table_token: u64,
    /// The deterministic 25-byte unique-index key this tx intends to own.
    pub index_key: Bytes,
    /// The rid claiming the value. An update re-writing its own value is
    /// not a self-conflict (`existing == owner` → OK).
    pub owner: RecordId,
}

/// Per-transaction state bundle.
///
/// Holds all mutable state accumulated during a transaction:
/// - **write_set** — per-table `StagingStore` buffers (set/remove ops).
/// - **index_write_set** — accumulated `IndexWriteOp`s across all tables.
/// - **staged_vectors** — per-table HNSW vectors awaiting commit.
/// - **interner_overlay** — new `(key_name → id)` mappings for this tx.
/// - **counter_deltas** — per-table row-count adjustments.
/// - **read_set** — SSI read tracking `(table_id, key) → version_seen`.
///
/// Drop = RAII rollback: all staged state is simply lost, no I/O.
pub struct TxContext {
    /// Unique transaction identifier.
    pub tx_id: TxId,

    /// Interned repo identifier (from the engine's interner).
    pub repo_id: u64,

    /// MVCC snapshot version — reads see only committed versions
    /// ≤ this value.
    pub snapshot_version: u64,

    /// Requested isolation level.
    pub isolation: IsolationLevel,

    /// Per-table write staging. Key = table name (interned u64).
    /// Each `StagingStore` buffers set/remove ops for that table.
    pub write_set: HashMap<u64, StagingStore, THasher>,

    /// Accumulated index write ops across all tables, with per-op table
    /// attribution. Each entry is `(table_token, op)`. Applied atomically
    /// during commit (via `apply_index_ops`).
    pub index_write_set: Vec<(u64, IndexWriteOp)>,

    /// Per-table HNSW staged vectors. Key = table token (interned table
    /// name). Each entry is a `(RecordId, embedding)` pair routed here by
    /// the executor instead of into the live HNSW graph. Promoted into the
    /// graph atomically at commit (Phase 5d); discarded by RAII drop on
    /// abort — exactly like every other tx-local field. This is the home
    /// for vector staging: nothing lives outside the `TxContext` anymore.
    pub staged_vectors: HashMap<u64, Vec<(RecordId, Vec<f32>)>, THasher>,

    /// Interner overlay: new `(key_name → id)` mappings created during
    /// this tx. Merged into base interner on commit; dropped on abort.
    pub interner_overlay: scc::HashMap<String, u64>,

    /// Next id to hand out from the overlay.  Starts at
    /// [`OVERLAY_ID_BASE`](crate::layered_interner::OVERLAY_ID_BASE)
    /// so overlay ids never clash with base ids.
    pub next_overlay_id: AtomicU64,

    /// Per-table counter delta. Applied at commit:
    /// `counter.add(delta)` for each table.
    pub counter_deltas: HashMap<u64, i64, THasher>,

    /// SSI read-set: `(table_id, key) → version_seen`. Only populated
    /// when `isolation == Serializable`. Validated at commit:
    /// `current_version(key)` must equal `version_seen`, else abort.
    ///
    /// `scc::HashMap` (not `std::HashMap`) so [`record_read`](Self::record_read)
    /// can take `&self` instead of `&mut self`. This is load-bearing for
    /// HIGH-C: the engine's tx-aware point read `TableManager::read_one_tx`
    /// holds the tx by shared reference (`Option<&TxContext>`) — the executor
    /// reborrows `&*tx` from a `&mut TxContext` — so an `&mut`-taking
    /// `record_read` could not be called from inside the read path without a
    /// signature break rippling into out-of-crate call sites. Interior
    /// mutability lets `read_one_tx` populate the read-set in place, which is
    /// what makes Serializable isolation actually detect write-skew (before
    /// this, `record_read` was wired only from unit tests, so the read-set
    /// was always empty in production and SSI silently degraded to Snapshot).
    pub read_set: scc::HashMap<(u64, Bytes), u64>,

    /// Token → original table name. Populated alongside `write_set`
    /// entries. Used at commit time to look up table names for WAL
    /// emission and interner merge (Stage 5).
    pub table_tokens: HashMap<u64, String, THasher>,

    /// Optional version provider for SSI read-set validation.
    /// When `None`, commit_tx Phase 2 falls back to a stub provider
    /// `|_, _| 0` that trivially passes — Snapshot and Serializable
    /// behave identically.
    pub version_provider: Option<std::sync::Arc<dyn VersionProvider>>,

    /// Wall-clock instant when this transaction was opened.
    /// Used for max-lifetime enforcement at commit time.
    pub started_at: std::time::Instant,

    /// Unique-index guards recorded at stage time, re-validated under
    /// `commit_lock` (closes the tx-concurrent unique-violation hole).
    /// Each entry: the deterministic unique-index key this tx intends to
    /// own, plus the owning rid (so an update re-writing its own value is
    /// not a self-conflict). Discarded by RAII on abort like all other
    /// tx-local state. Not gated in [`is_empty`](Self::is_empty): a guard
    /// only ever accompanies a staged write, so it is never the sole
    /// occupant of the tx.
    pub unique_guards: Vec<UniqueGuard>,

    /// Predicate / range read-set for SSI phantom detection (Phase C).
    /// Populated ONLY when `isolation == Serializable`, exactly like
    /// [`read_set`](Self::read_set). Interior-mutable so the engine's
    /// scan path can append through a shared `&TxContext`.
    pub predicate_set: crate::predicate_set::PredicateSet,

    /// Commit visibility / ack policy. Default `Synchronous` (no behaviour
    /// change). When set to `AsyncIndex`, the engine's `commit_tx` returns
    /// to the caller right after WAL durability + data apply + publish; the
    /// remaining materialization runs on a background `tokio::task`. See
    /// [`CommitVisibility`] for the full contract.
    pub visibility: CommitVisibility,

    /// Async-mode only book-keeping: set by the ack-path Phase-5a (data)
    /// helper if a per-table data write persistently failed. The background
    /// tail picks this up and forces a `Deferred` outcome so the inflight
    /// WAL marker is left for recovery. Never set in sync mode (the sync
    /// `materialize` path tracks `ok` on its own stack).
    pub async_prefix_failed: bool,

    /// The actor that initiated the transaction (R2).
    /// Defaults to `Actor::System`; set from the facade when a real
    /// principal is available.
    pub actor: Actor,

    /// Level-3 wound-wait abort flag. Set to `true` by an OLDER
    /// (higher-priority) tx that wounded this one during `lock_key`.
    /// Shared (`Arc`) so a tx blocked inside `lock_key` can observe a
    /// wound issued by another task. Stays `false` for Snapshot /
    /// Serializable txs (never wounded — they do not take locks).
    pub wounded: Arc<AtomicBool>,

    /// Level-3 wound wake notify. Paired with [`wounded`](Self::wounded):
    /// a wounder sets the flag AND triggers this notify so a holder parked
    /// in `lock_key` on a DIFFERENT key wakes up and observes the wound.
    /// Load-bearing for deadlock-freedom (a wound on key Y must wake a tx
    /// parked on key X). Stays unused for Snapshot / Serializable.
    pub wound_notify: Arc<tokio::sync::Notify>,

    /// Level-3 locked-key registry: every key this tx has acquired a
    /// pessimistic lock on (across all tables). Populated ONLY for
    /// `Pessimistic` txs; stays empty otherwise. Released as a batch on
    /// commit AND on abort via `MvccStore::release_locks`. Each entry key
    /// is `(table_token, key)` so release can route to the right table's
    /// `MvccStore`. Interior-mutable (`scc::HashMap`) so the read path
    /// (which holds `&TxContext`) can record locks without `&mut`.
    pub locked_keys: scc::HashMap<(u64, Bytes), ()>,
}

impl TxContext {
    pub fn new(
        tx_id: TxId,
        repo_id: u64,
        snapshot_version: u64,
        isolation: IsolationLevel,
    ) -> Self {
        Self {
            tx_id,
            repo_id,
            snapshot_version,
            isolation,
            write_set: HashMap::with_hasher(THasher::default()),
            index_write_set: Vec::new(),
            staged_vectors: HashMap::with_hasher(THasher::default()),
            interner_overlay: scc::HashMap::new(),
            next_overlay_id: AtomicU64::new(crate::layered_interner::OVERLAY_ID_BASE),
            counter_deltas: HashMap::with_hasher(THasher::default()),
            read_set: scc::HashMap::new(),
            table_tokens: HashMap::with_hasher(THasher::default()),
            version_provider: None,
            started_at: std::time::Instant::now(),
            unique_guards: Vec::new(),
            predicate_set: crate::predicate_set::PredicateSet::new(),
            visibility: CommitVisibility::default(),
            async_prefix_failed: false,
            actor: Actor::System,
            wounded: Arc::new(AtomicBool::new(false)),
            wound_notify: Arc::new(tokio::sync::Notify::new()),
            locked_keys: scc::HashMap::new(),
        }
    }

    /// Opt into async-index commit visibility (see [`CommitVisibility`]).
    /// Returns `&mut Self` for builder-style chaining.
    pub fn set_visibility(&mut self, visibility: CommitVisibility) -> &mut Self {
        self.visibility = visibility;
        self
    }

    /// Set the actor that initiated this transaction (R2).
    /// Returns `&mut Self` for builder-style chaining.
    pub fn set_actor(&mut self, actor: Actor) -> &mut Self {
        self.actor = actor;
        self
    }

    /// The actor that initiated this transaction.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Clone of this tx's wound-wait abort flag. Pass this into
    /// [`MvccStore::lock_key`](crate::mvcc_store::MvccStore::lock_key) so an
    /// older (higher-priority) tx can wound this one by setting the flag.
    pub fn wounded_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.wounded)
    }

    /// Clone of this tx's wound wake notify. Pass this into
    /// [`MvccStore::lock_key`](crate::mvcc_store::MvccStore::lock_key) so a
    /// wounder can wake this tx when it is parked waiting on a different key.
    pub fn wound_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.wound_notify)
    }

    /// True if this tx was wounded by an older tx (Level-3 wound-wait).
    /// Stays `false` for Snapshot / Serializable txs.
    pub fn is_wounded(&self) -> bool {
        self.wounded.load(Ordering::Acquire)
    }

    /// Abort if this tx was wounded. Called (a) at the top of each Level-3
    /// lock acquisition and (b) at commit entry, so a wounded tx that
    /// finished its statements still aborts instead of committing.
    ///
    /// Returns the tx's monotonic id on abort so the engine can wrap it into
    /// a clear `CommitError::Wounded { tx_version }`.
    pub fn ensure_not_wounded(&self) -> Result<(), u64> {
        if self.wounded.load(Ordering::Acquire) {
            Err(self.tx_id.0)
        } else {
            Ok(())
        }
    }

    /// Record that this tx acquired a Level-3 lock on `key` for `table_token`,
    /// so the lock is released on commit / abort. Takes `&self` via interior
    /// mutability (`locked_keys` is an `scc::HashMap`) so the engine's
    /// tx-aware read path (which holds `&TxContext`) can record locks without
    /// `&mut`. The map's dedup (same `(token, key)` inserted twice is a
    /// no-op) mirrors `lock_key`'s re-entrant idempotency.
    pub fn record_locked_key(&self, table_token: u64, key: Bytes) {
        // entry_async requires await; use the sync entry on a non-async
        // context. scc 2.x HashIndex/HashMap both expose a sync `entry` for
        // the unconditional insert path.
        use scc::hash_map::Entry;
        match self.locked_keys.entry((table_token, key)) {
            Entry::Occupied(_) => {}
            Entry::Vacant(e) => {
                e.insert_entry(());
            }
        }
    }

    /// Record a unique-index guard for commit-time re-validation.
    pub fn record_unique_guard(&mut self, g: UniqueGuard) {
        self.unique_guards.push(g);
    }

    /// How long this transaction has been open.
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Whether this transaction has exceeded the given max lifetime.
    pub fn is_expired(&self, max_lifetime: std::time::Duration) -> bool {
        self.started_at.elapsed() > max_lifetime
    }

    /// Approximate byte footprint of everything this tx has staged.
    ///
    /// Mirrors the per-field set that `wal_ops_from_tx`
    /// (`crates/shamir-engine/src/tx/commit.rs:236`) materialises into the
    /// WAL entry: per-table staging (`write_set`), accumulated index ops
    /// (`index_write_set`), and tx-buffered HNSW vectors (`staged_vectors`).
    /// Counters, interner overlay, read-set, table tokens and unique guards
    /// are bounded bookkeeping and intentionally excluded — the cap is there
    /// to protect the *payload* dimension.
    ///
    /// Note: this measures the in-memory staging footprint, not the eventual
    /// `WalEntryV2` serialized length. The cap will trip somewhat earlier
    /// than the actual on-disk WAL size — fine for a protective budget.
    ///
    /// `O(N)` over staged entries; called once per `TxExecute` from the
    /// server's interactive-tx handler. Saturating arithmetic so a degenerate
    /// caller can never wrap.
    pub fn staged_bytes(&self) -> usize {
        let mut total: usize = 0;
        for staging in self.write_set.values() {
            total = total.saturating_add(staging.staged_bytes());
        }
        for (_token, op) in &self.index_write_set {
            match op {
                crate::IndexWriteOp::SetPosting { key, value } => {
                    total = total.saturating_add(key.len()).saturating_add(value.len());
                }
                crate::IndexWriteOp::RemovePosting { key } => {
                    total = total.saturating_add(key.len());
                }
                crate::IndexWriteOp::BumpFtsStats { .. } => {} // counter-only, no payload
            }
        }
        for vecs in self.staged_vectors.values() {
            for (_rid, embedding) in vecs {
                // 16 bytes of RecordId + 4 bytes per f32 lane.
                total = total
                    .saturating_add(16)
                    .saturating_add(embedding.len().saturating_mul(4));
            }
        }
        total
    }

    /// True if the tx has no pending writes / index ops / staging at all.
    pub fn is_empty(&self) -> bool {
        self.write_set.is_empty()
            && self.index_write_set.is_empty()
            && self.staged_vectors.is_empty()
            && self.interner_overlay.is_empty()
            && self.counter_deltas.is_empty()
    }

    /// Record a counter change for a table (e.g. +N for insert_many).
    pub fn bump_counter(&mut self, table_id: u64, delta: i64) {
        *self.counter_deltas.entry(table_id).or_insert(0) += delta;
    }

    /// Record a read for SSI validation (only if Serializable).
    ///
    /// `&mut self` overload, kept for existing call sites that hold the tx
    /// mutably (tests, benches, manually-driven read tracking). Delegates to
    /// [`record_read_shared`](Self::record_read_shared); both write the same
    /// interior-mutable `read_set`.
    pub fn record_read(&mut self, table_id: u64, key: Bytes, version: u64) {
        self.record_read_shared(table_id, key, version);
    }

    /// Record a read for SSI validation (only if Serializable), taking
    /// `&self` via interior mutability (`read_set` is an `scc::HashMap`).
    ///
    /// This is the entry point the engine's tx-aware read path uses:
    /// `TableManager::read_one_tx` holds the tx by shared reference
    /// (`Option<&TxContext>`), so it cannot call the `&mut self` overload.
    /// Wiring this in is what makes Serializable isolation actually populate
    /// the read-set in production (HIGH-C) — previously `record_read` was
    /// reachable only from unit tests, so `read_set` was always empty at
    /// commit and SSI silently degraded to Snapshot isolation.
    ///
    /// No-op under Snapshot isolation.
    ///
    /// **First-read-wins**: the version recorded for a key is the one observed
    /// at the *first* read; a later re-read of the same key does NOT overwrite
    /// it. This is the load-bearing SSI semantic. Versions are monotonic, so
    /// the first read captures the lowest (earliest) version the tx ever saw —
    /// the conservative bound for conflict detection. Overwriting with a newer
    /// version (last-write-wins, the previous `HashMap::insert` behaviour, fine
    /// only while reads were recorded once from unit tests) would mask a real
    /// conflict: e.g. an update's internal old-value read runs AFTER a
    /// concurrent committer bumped the key, and last-write-wins would re-record
    /// the key at the post-commit version, defeating the abort.
    pub fn record_read_shared(&self, table_id: u64, key: Bytes, version: u64) {
        if self.isolation == IsolationLevel::Serializable {
            use scc::hash_map::Entry::{Occupied, Vacant};
            match self.read_set.entry((table_id, key)) {
                // First-read-wins: keep the earliest observed version.
                Occupied(_) => {}
                Vacant(ve) => {
                    ve.insert_entry(version);
                }
            }
        }
    }

    /// Record a predicate dependency for SSI phantom detection.
    ///
    /// No-op under Snapshot isolation — zero-overhead invariant: the
    /// isolation gate runs BEFORE any work. Takes `&self` via interior
    /// mutability so engine scan paths can append through `&TxContext`.
    pub fn record_predicate_shared(&self, dep: crate::predicate_set::PredicateDep) {
        if self.isolation == IsolationLevel::Serializable {
            self.predicate_set.push(dep);
        }
    }

    /// Validate the read-set against current committed versions.
    ///
    /// For Serializable Snapshot Isolation: every key the tx read must
    /// still be at the same version when we're about to commit. If any
    /// key has advanced, another tx wrote there → abort with the
    /// offending key.
    ///
    /// `version_provider(table_id, key) -> Option<u64>` is supplied by
    /// the caller. `None` = unknown table → conflict. `Some(0)` is the
    /// safe default for registered tables where the key has never been
    /// written (0 <= any version_seen → passes).
    pub fn validate_read_set<F>(&self, mut version_provider: F) -> Result<(), (u64, Bytes)>
    where
        F: FnMut(u64, &Bytes) -> Option<u64>,
    {
        // `scc::HashMap::scan` is a synchronous visitor `FnMut(&K, &V)` that
        // cannot early-return; capture the first conflict and report it after
        // the scan. Iteration order is unspecified (it was already with
        // `std::HashMap`), so which key surfaces on a multi-key conflict is
        // not contractual — callers test single-key scenarios.
        let mut conflict: Option<(u64, Bytes)> = None;
        self.read_set.scan(|(table_id, key), version_seen| {
            if conflict.is_some() {
                return;
            }
            match version_provider(*table_id, key) {
                None => conflict = Some((*table_id, key.clone())),
                Some(current) if current > *version_seen => {
                    conflict = Some((*table_id, key.clone()));
                }
                Some(_) => {}
            }
        });
        match conflict {
            Some(c) => Err(c),
            None => Ok(()),
        }
    }

    /// Get-or-create a StagingStore for the given table token.
    ///
    /// Also records the human-readable table name in `table_tokens`
    /// so commit-time WAL emission can look it up.
    pub fn ensure_table_staging(
        &mut self,
        token: u64,
        name: &str,
        base: std::sync::Arc<dyn shamir_storage::types::Store>,
    ) -> &mut crate::staging_store::StagingStore {
        self.table_tokens
            .entry(token)
            .or_insert_with(|| name.to_string());
        self.write_set
            .entry(token)
            .or_insert_with(|| crate::staging_store::StagingStore::new(base))
    }

    /// Stage an HNSW vector under this tx for the given table token.
    ///
    /// The pair is buffered tx-locally and applied to the live graph at
    /// commit (Phase 5d). A dropped/aborted tx discards it (RAII) — the
    /// live graph is never touched until commit.
    pub fn stage_vector(&mut self, table_token: u64, rid: RecordId, vec: Vec<f32>) {
        self.staged_vectors
            .entry(table_token)
            .or_default()
            .push((rid, vec));
    }

    /// Vectors staged under this tx for `table_token`, for in-tx search
    /// merge. `None` when the table has no staged vectors.
    pub fn staged_vectors_for(&self, table_token: u64) -> Option<&[(RecordId, Vec<f32>)]> {
        self.staged_vectors.get(&table_token).map(Vec::as_slice)
    }

    /// Attach a version provider used by commit_tx Phase 2 for SSI
    /// validation. Returns `&mut Self` for builder-style chaining.
    pub fn set_version_provider(
        &mut self,
        provider: std::sync::Arc<dyn VersionProvider>,
    ) -> &mut Self {
        self.version_provider = Some(provider);
        self
    }

    /// cancel-safe: NO — iterates `write_set` and invokes
    /// `rewrite_set_bytes` on each per-table StagingStore. Cancellation
    /// mid-iteration leaves a subset of tables remapped and the rest
    /// holding overlay ids — the tx must be aborted on error / cancel.
    ///
    /// Apply an overlay-id → base-id remap across all staged writes.
    ///
    /// Called during commit phase 1, immediately after
    /// `commit_interner_overlay`, so subsequent flush phases see
    /// stable base ids only.
    ///
    /// Errors if any staged value fails to decode/re-encode. Caller
    /// should abort the transaction on error.
    pub async fn apply_id_remap(
        &mut self,
        remap: &std::collections::HashMap<u64, u64>,
    ) -> Result<(), String> {
        if remap.is_empty() {
            return Ok(());
        }
        for staging in self.write_set.values_mut() {
            staging
                .rewrite_set_inner(|inner| {
                    crate::id_remap::remap_value(inner, remap);
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }
}
