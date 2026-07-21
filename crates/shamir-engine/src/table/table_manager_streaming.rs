use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_query_types::batch::ResultEncoding;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::record_cow::RecordCow;
use super::table::Table;
use super::table_manager::TableManager;
use super::tx_scan_overlay::{
    merge_filtered_stream_with_tx_overlay, merge_stream_with_tx_overlay, overlay_rows_for_tx,
};
use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;

impl TableManager {
    /// Stream records in batches, returning InnerValues
    ///
    /// This is memory-efficient for large tables as it doesn't load all records at once.
    /// Returns a stream that yields batches of records.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// A stream that yields batches of (RecordId, InnerValue) tuples
    pub fn list_stream(
        &self,
        batch_size: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + '_ {
        type DynStream<'a> = std::pin::Pin<
            Box<dyn futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + Send + 'a>,
        >;
        if let Some(mvcc) = self.mvcc_store_ref() {
            let mvcc = Arc::clone(mvcc);
            let s: DynStream<'_> = Box::pin(async_stream::stream! {
                let mut raw = mvcc.current_stream(batch_size);
                while let Some(batch_result) = raw.next().await {
                    let batch_bytes = match batch_result {
                        Ok(b) => b,
                        Err(e) => {
                            yield Err(e);
                            return;
                        }
                    };
                    for decoded in Table::decode_raw_batch(batch_bytes) {
                        yield decoded;
                    }
                }
            });
            s
        } else {
            let s: DynStream<'_> = Box::pin(self.table.list_stream(batch_size));
            s
        }
    }

    /// Like [`list_stream`], but applies a bytes-level pre-filter before
    /// decoding each record to `InnerValue`.  Rows that the pre-filter
    /// definitively rejects (`Some(false)`) are skipped without a full
    /// `InnerValue` decode.  Rows where the pre-filter returns `None`
    /// (unsupported filter shape) are decoded normally — the caller's
    /// compiled filter is the authoritative decision in all cases.
    pub(crate) fn list_stream_filtered(
        &self,
        batch_size: usize,
        pre_filter: std::sync::Arc<crate::query::filter::FilterNode>,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + '_ {
        type DynStream<'a> = std::pin::Pin<
            Box<dyn futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + Send + 'a>,
        >;
        if let Some(mvcc) = self.mvcc_store_ref() {
            let mvcc = Arc::clone(mvcc);
            let s: DynStream<'_> = Box::pin(async_stream::stream! {
                let mut raw = mvcc.current_stream(batch_size);
                while let Some(batch_result) = raw.next().await {
                    let batch_bytes = match batch_result {
                        Ok(b) => b,
                        Err(e) => {
                            yield Err(e);
                            return;
                        }
                    };
                    for decoded in Table::decode_raw_batch_filtered(batch_bytes, &pre_filter) {
                        yield decoded;
                    }
                }
            });
            s
        } else {
            let pf = Arc::clone(&pre_filter);
            let s: DynStream<'_> = Box::pin(self.table.list_stream_filtered(batch_size, pf));
            s
        }
    }

    /// Stream records filtered by a compiled filter callback.
    ///
    /// Compiles the Filter AST into a callback network, then yields
    /// batches of matching records. The filter is compiled once; only
    /// matching records are yielded — non-matching records are dropped
    /// immediately without accumulation.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch from storage
    /// * `filter` - Filter AST to compile and apply
    /// * `ctx` - Filter context with interner and resolved query refs
    pub async fn filter_stream<'a>(
        &'a self,
        batch_size: usize,
        filter: &Filter,
        ctx: &'a FilterContext<'a>,
    ) -> DbResult<impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a> {
        let interner = self.interner.get().await?;
        let callback = compile_filter(filter, interner);
        let table_stream = self.list_stream(batch_size);

        Ok(async_stream::stream! {
            futures::pin_mut!(table_stream);
            while let Some(batch_result) = table_stream.next().await {
                match batch_result {
                    Err(e) => { yield Err(e); return; }
                    Ok(batch) => {
                        let filtered: Vec<_> = batch
                            .into_iter()
                            .filter(|(_, cow)| {
                                match cow {
                                    RecordCow::Borrowed(b) => {
                                        match shamir_types::record_view::RecordView::new(b) {
                                            Ok(view) => callback.matches(&view, ctx),
                                            Err(_) => false, // malformed → skip
                                        }
                                    }
                                    RecordCow::Owned(record) => callback.matches(record, ctx),
                                }
                            })
                            .collect();
                        if !filtered.is_empty() {
                            yield Ok(filtered);
                        }
                    }
                }
            }
        })
    }

    /// tx-aware streaming variant of [`list_stream`].
    ///
    /// Forwards to [`list_stream`] for the actual data, then — when `tx` is
    /// `Some` and the tx is Serializable — records each *materialised*
    /// record's key into the read-set (HIGH-C). The yielded batches are
    /// byte-for-byte the same as [`list_stream`]; recording is a pure
    /// side-effect threaded lazily through the stream, so the lazy-yield
    /// contract is preserved (a consumer that stops early only records the
    /// keys it actually pulled).
    ///
    /// Streaming-scan SSI scope: this records the keys the scan *yields* (of
    /// the RAW committed stream, before the overlay merge below). It does
    /// NOT install predicate / range locks, so phantom inserts into the
    /// scanned range by a concurrent tx are not detected — full SSI predicate
    /// locking over a stream is a known harder problem and out of scope here.
    /// Point reads ([`read_one_tx`]) and materialised scan reads are covered.
    ///
    /// FG-3: read-your-own-writes for scans. Streaming scans overlay the
    /// tx's own `write_set` on top of the committed-store stream: a record
    /// this tx staged (inserted/updated/deleted but not yet committed) IS
    /// visible to an in-tx stream — a staged insert is injected in sorted
    /// position, a staged delete is hidden, a staged update yields the
    /// STAGED (new) bytes instead of the committed (old) ones. This mirrors
    /// point reads ([`read_one_tx`]), which already did RYOW. The overlay
    /// merge (`merge_stream_with_tx_overlay`, `tx_scan_overlay.rs`) runs
    /// AFTER `record_scan_reads` below — SSI read-set recording is a
    /// separate, unchanged concern over the raw committed stream; the
    /// overlay merge is a downstream transform of what
    /// `record_scan_reads` already yielded. A tx that never wrote this
    /// table pays nothing (`overlay_rows_for_tx` returns empty, and the
    /// merge is then a zero-cost pass-through). ONLY the tx that staged a
    /// write sees it — a different, concurrent tx's stream is untouched
    /// (isolation is preserved: `tx.write_set` is per-`TxContext`, never
    /// shared).
    pub fn list_stream_tx<'a>(
        &'a self,
        tx: Option<&'a shamir_tx::TxContext>,
        batch_size: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a {
        // Phase C (Step 6): defensive coarse recording for streams
        // reached directly (bypassing read_tx). Zero-overhead: gate on
        // Serializable before any work.
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::TableScan {
                    table_token: self.table_token(),
                });
            }
        }
        let inner = self.list_stream(batch_size);
        let token = self.table_token();
        let mvcc = self.mvcc_store.clone();
        let recorded = Self::record_scan_reads(inner, tx, token, mvcc);
        // FG-3: overlay this tx's own staged writes on top of the recorded
        // committed stream (unfiltered — no Filter/FilterContext here).
        let overlay = overlay_rows_for_tx(tx, token);
        merge_stream_with_tx_overlay(recorded, overlay, batch_size)
    }

    /// tx-aware streaming variant of [`filter_stream`].
    ///
    /// Same materialised-read SSI recording as [`list_stream_tx`]: each
    /// record that survives the filter and is yielded gets recorded into the
    /// read-set (Serializable only). Same streaming-scan SSI scope note, and
    /// the same FG-3 read-your-own-writes behaviour as [`list_stream_tx`] —
    /// but here overlay-sourced rows (staged inserts / staged-overridden
    /// updates) are re-evaluated against `filter` before being yielded (the
    /// FILTERED overlay variant), so an injected/overridden staged row that
    /// does not actually match `filter` is excluded rather than blindly
    /// included.
    pub async fn filter_stream_tx<'a>(
        &'a self,
        tx: Option<&'a shamir_tx::TxContext>,
        batch_size: usize,
        filter: &Filter,
        ctx: &'a FilterContext<'a>,
    ) -> DbResult<impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a> {
        // Phase C (Step 6): predicate recording for filter streams
        // reached directly (bypassing read_tx). Zero-overhead: gate on
        // Serializable before any work.
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let token = self.table_token();
                let deps = crate::query::filter::eval::predicate_to_index_range(
                    filter,
                    self.sorted_indexes(),
                    ctx.interner,
                    token,
                );
                if deps.is_empty() {
                    t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::TableScan {
                        table_token: token,
                    });
                } else {
                    for d in deps {
                        t.record_predicate_shared(d);
                    }
                }
            }
        }
        let inner = self.filter_stream(batch_size, filter, ctx).await?;
        let token = self.table_token();
        let mvcc = self.mvcc_store.clone();
        let recorded = Self::record_scan_reads(inner, tx, token, mvcc);
        // FG-3: overlay this tx's own staged writes, filtered variant —
        // overlay-sourced rows are re-evaluated against `filter` (a staged
        // UPDATE's new bytes may match/not-match differently than the old
        // committed value did; a staged INSERT was never filtered before).
        let overlay = overlay_rows_for_tx(tx, token);
        if overlay.is_empty() {
            // Zero-cost pass-through — no compile_filter call needed.
            return Ok(Box::pin(recorded)
                as std::pin::Pin<
                    Box<dyn futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a>,
                >);
        }
        let interner = self.interner().get().await?;
        let callback = compile_filter(filter, interner);
        let keep = move |_id: &RecordId, bytes: &Bytes| match RecordView::new(bytes) {
            Ok(view) => callback.matches(&view, ctx),
            Err(_) => match InnerValue::from_bytes(bytes.clone()) {
                Ok(tree) => callback.matches(&tree, ctx),
                Err(_) => false, // malformed → exclude
            },
        };
        Ok(Box::pin(merge_filtered_stream_with_tx_overlay(
            recorded, overlay, batch_size, keep,
        ))
            as std::pin::Pin<
                Box<dyn futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a>,
            >)
    }

    /// Wrap a record stream so that, for a Serializable tx, each yielded
    /// record's key is recorded into the read-set at the version observed
    /// when it is pulled. The wrapper is transparent: it yields exactly what
    /// the inner stream yields, in the same order. For `tx == None` or a
    /// non-Serializable tx it adds no per-record work beyond a single
    /// up-front isolation check (the `version_of` lookup and recording are
    /// skipped entirely). `mvcc` is `None` → version `0` (conservative
    /// default, see [`read_one_tx`]).
    pub(super) fn record_scan_reads<'a, S>(
        inner: S,
        tx: Option<&'a shamir_tx::TxContext>,
        token: u64,
        mvcc: Option<Arc<shamir_tx::MvccStore>>,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a
    where
        S: futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a,
    {
        // Only Serializable txs track reads; everything else is a pass-through
        // so the non-SSI scan path pays nothing per record.
        let recording_tx = tx.filter(|t| t.isolation == shamir_tx::IsolationLevel::Serializable);
        async_stream::stream! {
            futures::pin_mut!(inner);
            while let Some(batch_result) = inner.next().await {
                if let (Ok(batch), Some(tx)) = (&batch_result, recording_tx) {
                    for (rid, _) in batch {
                        let key = rid.to_bytes();
                        // A3: record the version of the value ACTUALLY READ,
                        // not the cell's current version. The scan reads via
                        // `get_at(key, snapshot)` (snapshot-gated), so the
                        // returned bytes never correspond to a version newer
                        // than the tx's snapshot. Clamping the recorded
                        // version to `min(version_of(key), snapshot)` ensures
                        // a concurrent committer that pushed the cell past
                        // the snapshot is detected at `validate_read_set`
                        // (`current > version_seen`), instead of masked
                        // (`current == version_seen` when the raw cell version
                        // was recorded). `snapshot_version` is constant for
                        // the tx, so `min` is computed per-record with no
                        // extra lookup.
                        let snapshot_v = tx.snapshot_version;
                        let version = mvcc
                            .as_ref()
                            .map_or(0, |m| m.version_of(key.as_ref()).min(snapshot_v));
                        tx.record_read_shared(token, key, version);
                    }
                }
                yield batch_result;
            }
        }
    }

    /// FINAL-A helper: drain `list_stream` into a vec of `(RecordId, InnerValue)`.
    /// Used by `create_index` / `create_unique_index` to backfill from the seam
    /// rather than the raw `data_store` when an MvccStore is attached.
    /// FINAL-A helper: drain `list_stream` into a vec of `(RecordId, InnerValue)`.
    /// Used by `create_index` / `create_unique_index` to backfill from the seam
    /// rather than the raw `data_store` when an MvccStore is attached.
    ///
    /// Decodes every `Borrowed` row to an owned `InnerValue` tree — correct for
    /// index backfill which needs the full tree.
    pub(super) async fn collect_all_current_records(
        &self,
    ) -> DbResult<Vec<(RecordId, InnerValue)>> {
        let mut out = Vec::new();
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (id, cow) in batch? {
                let inner = match cow {
                    RecordCow::Borrowed(b) => InnerValue::from_bytes(b).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "Failed to deserialize record: {}",
                            e
                        ))
                    })?,
                    RecordCow::Owned(v) => v,
                };
                out.push((id, inner));
            }
        }
        Ok(out)
    }

    /// tx-aware single-record read.
    ///
    /// - `tx == None` → same as [`get`]: direct read from main data_store.
    /// - `tx == Some(tx)` and no `mvcc_store` attached → same as [`get`].
    /// - `tx == Some(tx)` and `mvcc_store` attached →
    ///   - `Pessimistic` isolation → `mvcc.get_current_bytes(rid.to_bytes())`
    ///     (the LATEST COMMITTED value), because the caller has already
    ///     acquired a Shared lock on the key (see
    ///     `acquire_pessimistic_read_lock` above) — possibly after waiting
    ///     for an Exclusive holder to release. A read taken under a held
    ///     lock MUST reflect the latest committed value at the moment the
    ///     lock was granted, not the tx's original snapshot, otherwise a
    ///     read-modify-write cycle would compute from stale data and lose
    ///     the just-released committer's update (A4 fix b).
    ///   - `Snapshot` / `Serializable` isolation →
    ///     `mvcc.get_at(rid.to_bytes(), tx.snapshot_version)`
    ///     (snapshot-gated read, unchanged):
    ///     - `Some(bytes)` → deserialize and return.
    ///     - `None` → `DbError::NotFound`.
    ///
    /// I.4 — read-your-own-writes. Before consulting the snapshot base, the
    /// tx's own staging overlay (`tx.write_set[token]`, the `StagingStore`
    /// holding this tx's un-committed set/remove ops) is checked for `key`:
    ///   - staged `Set(bytes)` → return the staged value (read-your-own-write);
    ///   - staged `Remove`     → return `NotFound` (read-your-own-delete);
    ///   - not staged          → fall through to `get_at(snapshot)` (the base).
    ///
    /// HIGH-C — SSI read tracking. When `tx` is `Some`, the key and the
    /// version observed at read time are recorded into the tx's read-set via
    /// [`record_read_shared`](shamir_tx::TxContext::record_read_shared) (a
    /// no-op under Snapshot isolation, so callers pay nothing there).
    ///
    /// §5b floor (#61): point-read API returns an OWNED `InnerValue` — the
    /// caller owns the deserialized record. Narrowing this to the lens would
    /// ripple across all callers. See `docs/dev-artifacts/perf/innervalue-floor.md`
    /// (Category 4 — owned-value boundaries).
    pub async fn read_one_tx(
        &self,
        id: RecordId,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<InnerValue> {
        if let Some(tx) = tx {
            let key = id.to_bytes();
            // Level-3: acquire a Shared lock on the key before reading.
            // No-op for Snapshot / Serializable (the helper self-gates).
            // A wound-wait abort surfaces as DbError::Conflict and the tx
            // must abort (the executor / commit path handles it). task #532:
            // the lock registry is `RecordKey`-keyed — build the key inline
            // from the rid (alloc-free) rather than round-tripping `Bytes`.
            self.acquire_pessimistic_read_lock(RecordKey::from_slice(id.as_bytes()), tx)
                .await?;
            // A3: record the version of the value ACTUALLY READ, not the
            // cell's current version. `read_one_tx` reads via
            // `get_at(key, tx.snapshot_version)` (snapshot-gated), so the
            // returned bytes never correspond to a version newer than the
            // tx's snapshot. A concurrent committer can push the cell's
            // current version past the snapshot BETWEEN this `version_of`
            // call and the `get_at` call (or the cell may already be ahead
            // before the read starts); recording that newer version would
            // mask the conflict at `validate_read_set` (which checks
            // `current > version_seen`). Clamping to
            // `min(version_of(key), snapshot_version)` guarantees the
            // recorded version never exceeds what could possibly have been
            // read, so any post-snapshot commit is correctly detected.
            // No-op for Snapshot isolation.
            let version = self.mvcc_store.as_ref().map_or(0, |mvcc| {
                mvcc.version_of(key.as_ref()).min(tx.snapshot_version)
            });
            tx.record_read_shared(self.table_token(), key.clone(), version);

            // I.4 read-your-own-writes: the tx's own staging overlay wins
            // over the snapshot base. A targeted per-key probe (alloc-free,
            // no fall-through to base): staged Set → return the staged value,
            // staged Remove → NotFound, not staged → fall through to the
            // snapshot base below. Only the table's own staging is probed
            // (guarded by the write_set lookup), so a tx that never wrote this
            // table pays nothing.
            if let Some(staging) = tx.write_set.get(&self.table_token()) {
                match staging.staged_op(key.as_ref()) {
                    Some(shamir_tx::staging_store::StagedKind::Set(v)) => {
                        return InnerValue::from_bytes(v).map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "Failed to deserialize InnerValue: {}",
                                e
                            ))
                        });
                    }
                    Some(shamir_tx::staging_store::StagedKind::Removed) => {
                        return Err(shamir_storage::error::DbError::NotFound(format!(
                            "record staged-removed in tx: {:?}",
                            id
                        )));
                    }
                    None => {}
                }
            }

            if let Some(mvcc) = self.mvcc_store.as_ref() {
                // A4 (fix b): for a Pessimistic tx reading UNDER a held lock
                // (the `acquire_pessimistic_read_lock` call above already
                // granted the Shared lock — possibly after waiting for an
                // Exclusive holder to release), the read MUST resolve to the
                // LATEST COMMITTED value, not the tx's original snapshot.
                // Otherwise a tx that began before a concurrent committer's
                // publish, then acquired the lock AFTER that publish, would
                // still see the stale snapshot value — defeating the entire
                // purpose of taking the lock (a read-modify-write cycle
                // computed from stale data → lost update). Snapshot /
                // Serializable isolation's snapshot-gated `get_at` semantics
                // are unchanged below.
                if tx.isolation == shamir_tx::IsolationLevel::Pessimistic {
                    match mvcc.get_current_bytes(key.as_ref()).await? {
                        Some(bytes) => {
                            return InnerValue::from_bytes(bytes).map_err(|e| {
                                shamir_storage::error::DbError::Codec(format!(
                                    "Failed to deserialize InnerValue: {}",
                                    e
                                ))
                            });
                        }
                        None => {
                            return Err(shamir_storage::error::DbError::NotFound(format!(
                                "record not found (latest committed) under Pessimistic lock: {:?}",
                                id
                            )));
                        }
                    }
                }
                match mvcc.get_at(key.as_ref(), tx.snapshot_version).await? {
                    Some(bytes) => {
                        return InnerValue::from_bytes(bytes).map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "Failed to deserialize InnerValue: {}",
                                e
                            ))
                        });
                    }
                    None => {
                        return Err(shamir_storage::error::DbError::NotFound(format!(
                            "record not found at snapshot {}: {:?}",
                            tx.snapshot_version, id
                        )));
                    }
                }
            }
        }
        self.get(id).await
    }

    /// tx-aware raw-bytes read (no InnerValue decode).
    ///
    /// Same lock / overlay / snapshot semantics as [`read_one_tx`] but
    /// returns the raw storage msgpack bytes without decoding to an
    /// `InnerValue` tree. Returns `None` when the record is absent
    /// (staged-removed, past the snapshot, or not in the main store) —
    /// real I/O errors propagate via `?`.
    ///
    /// Used by [`delete_tx`] to feed index planners via a zero-copy
    /// `RecordView` lens instead of a decoded `InnerValue` tree.
    pub(crate) async fn read_one_tx_bytes(
        &self,
        id: RecordId,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<Bytes>> {
        if let Some(tx) = tx {
            let key = id.to_bytes();
            // Level-3: acquire a Shared lock on the key before reading.
            // No-op for Snapshot / Serializable (the helper self-gates).
            // task #532: lock registry is `RecordKey`-keyed — build inline.
            self.acquire_pessimistic_read_lock(RecordKey::from_slice(id.as_bytes()), tx)
                .await?;
            // A3: record the version of the value ACTUALLY READ, not the
            // cell's current version. See `read_one_tx` for the full
            // rationale on the `.min(snapshot_version)` clamp. No-op for
            // Snapshot isolation.
            let version = self.mvcc_store.as_ref().map_or(0, |mvcc| {
                mvcc.version_of(key.as_ref()).min(tx.snapshot_version)
            });
            tx.record_read_shared(self.table_token(), key.clone(), version);

            // I.4 read-your-own-writes: check the tx staging overlay first.
            if let Some(staging) = tx.write_set.get(&self.table_token()) {
                match staging.staged_op(key.as_ref()) {
                    Some(shamir_tx::staging_store::StagedKind::Set(v)) => {
                        return Ok(Some(v));
                    }
                    Some(shamir_tx::staging_store::StagedKind::Removed) => {
                        return Ok(None);
                    }
                    None => {}
                }
            }

            if let Some(mvcc) = self.mvcc_store.as_ref() {
                // A4 (fix b): see `read_one_tx` — a Pessimistic tx reading
                // under a held lock resolves to the LATEST COMMITTED value,
                // not the snapshot. Snapshot / Serializable unchanged.
                if tx.isolation == shamir_tx::IsolationLevel::Pessimistic {
                    return mvcc.get_current_bytes(key.as_ref()).await;
                }
                return mvcc.get_at(key.as_ref(), tx.snapshot_version).await;
            }
        }
        // No tx, or no mvcc: read raw bytes from the data store.
        if let Some(mvcc) = self.mvcc_store.as_ref() {
            return mvcc.get_current_bytes(id.as_bytes()).await;
        }
        match self
            .table
            .data_store()
            .get(RecordKey::from_slice(id.as_bytes()))
            .await
        {
            Ok(bytes) => Ok(Some(bytes)),
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// tx-aware read-query execution (Vector I.1).
    ///
    /// The wire `execute_batch` path dispatches `BatchOp::Read` here when a
    /// `transactional` batch is in flight, so a Serializable batch's SELECT
    /// populates the tx read-set and SSI write-skew detection becomes live
    /// end-to-end.
    pub async fn read_tx(
        &self,
        query: &crate::query::read::ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<crate::query::read::QueryResult> {
        // Only a Serializable tx records reads; for everything else the
        // recording pass is pure overhead, so dispatch straight to `read`.
        let recording = tx
            .filter(|t| t.isolation == shamir_tx::IsolationLevel::Serializable)
            .is_some();
        if recording {
            // Phase C (Step 6): one-shot predicate-set recording derived
            // from query.r#where. Zero-overhead: this block only runs for
            // Serializable txs.
            let tx_ref = tx.expect("recording=true implies tx is Some");
            let token = self.table_token();
            match query.r#where.as_ref() {
                None => {
                    // No WHERE → coarse TableScan over the whole table.
                    tx_ref.record_predicate_shared(
                        shamir_tx::predicate_set::PredicateDep::TableScan { table_token: token },
                    );
                }
                Some(filter) => {
                    let deps = crate::query::filter::eval::predicate_to_index_range(
                        filter,
                        self.sorted_indexes(),
                        ctx.interner,
                        token,
                    );
                    if deps.is_empty() {
                        tx_ref.record_predicate_shared(
                            shamir_tx::predicate_set::PredicateDep::TableScan {
                                table_token: token,
                            },
                        );
                    } else {
                        for dep in deps {
                            tx_ref.record_predicate_shared(dep);
                        }
                    }
                }
            }
        }
        // Level-3: acquire a Shared lock on every record the query touches
        // BEFORE reading. Zero-overhead: only runs for Pessimistic txs. A
        // wound-wait abort surfaces as DbError::Conflict and propagates up.
        if let Some(t) = tx.filter(|t| t.isolation == shamir_tx::IsolationLevel::Pessimistic) {
            self.lock_query_reads(query, ctx, t).await?;
        }
        // Fused SSI path: read_for_tx uses tx-aware streams in the full-scan
        // fallback so SSI read-set recording is folded into the single scan
        // that emits rows (eliminates the previous double-scan).
        self.read_for_tx(query, ctx, tx).await
    }

    /// tx-aware variant of [`read_with_encoding`].
    ///
    /// Performs the same SSI predicate recording and Pessimistic lock
    /// acquisition as [`read_tx`] and then dispatches to [`read_for_tx_with_encoding`]
    /// so that the result rows honour the requested encoding.
    pub async fn read_tx_with_encoding(
        &self,
        query: &crate::query::read::ReadQuery,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
        encoding: ResultEncoding,
    ) -> DbResult<crate::query::read::QueryResult> {
        // Only a Serializable tx records reads; for everything else the
        // recording pass is pure overhead, so skip it.
        let recording = tx
            .filter(|t| t.isolation == shamir_tx::IsolationLevel::Serializable)
            .is_some();
        if recording {
            let tx_ref = tx.expect("recording=true implies tx is Some");
            let token = self.table_token();
            match query.r#where.as_ref() {
                None => {
                    tx_ref.record_predicate_shared(
                        shamir_tx::predicate_set::PredicateDep::TableScan { table_token: token },
                    );
                }
                Some(filter) => {
                    let deps = crate::query::filter::eval::predicate_to_index_range(
                        filter,
                        self.sorted_indexes(),
                        ctx.interner,
                        token,
                    );
                    if deps.is_empty() {
                        tx_ref.record_predicate_shared(
                            shamir_tx::predicate_set::PredicateDep::TableScan {
                                table_token: token,
                            },
                        );
                    } else {
                        for dep in deps {
                            tx_ref.record_predicate_shared(dep);
                        }
                    }
                }
            }
        }
        // Level-3: acquire a Shared lock on every record the query touches
        // BEFORE reading (Pessimistic isolation only).
        if let Some(t) = tx.filter(|t| t.isolation == shamir_tx::IsolationLevel::Pessimistic) {
            self.lock_query_reads(query, ctx, t).await?;
        }
        self.read_for_tx_with_encoding(query, ctx, tx, encoding)
            .await
    }

    /// Level-3: acquire a `Shared` lock on every record matching `query`'s
    /// WHERE clause (or the whole table when there is no WHERE). Pessimistic
    /// only — never called for Snapshot / Serializable.
    async fn lock_query_reads(
        &self,
        query: &crate::query::read::ReadQuery,
        ctx: &FilterContext<'_>,
        tx: &shamir_tx::TxContext,
    ) -> DbResult<()> {
        let batch_size = 1000;
        match query.r#where.as_ref() {
            Some(filter) => {
                let stream = self.filter_stream(batch_size, filter, ctx).await?;
                futures::pin_mut!(stream);
                while let Some(batch) = stream.next().await {
                    for (rid, _cow) in batch? {
                        self.acquire_pessimistic_read_lock(
                            RecordKey::from_slice(rid.as_bytes()),
                            tx,
                        )
                        .await?;
                    }
                }
            }
            None => {
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch) = stream.next().await {
                    for (rid, _cow) in batch? {
                        self.acquire_pessimistic_read_lock(
                            RecordKey::from_slice(rid.as_bytes()),
                            tx,
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }
}
