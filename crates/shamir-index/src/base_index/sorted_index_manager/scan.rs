use bytes::Bytes;
use futures::StreamExt;

use super::covering::{
    decode_record_id_suffix, extract_and_encode, key_value_slice, skip_boundary_row,
};
use crate::base_index::sorted_index_definition::SORTED_TAG;
use shamir_storage::error::{DbError, DbResult};
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;

impl super::inner::SortedIndexManager {
    /// Range lookup: return all record IDs whose indexed value is in
    /// `[start, end]` (both inclusive), sorted ascending and duplicate-free.
    /// `start` / `end` are the already-encoded value bytes (call sites use
    /// `sort_codec::encode_*` to produce them).
    ///
    /// Builds the lower / upper bounds in the physical-key space and
    /// delegates to `Store::iter_range_stream` — on B-tree-backed
    /// stores (sled, redb, fjall, persy, canopy) this seeks straight
    /// to `lower` and stops at `upper`, doing zero wasted work
    /// outside the range. In-memory / cached fall back to
    /// `iter_range_stream`'s default filter wrapper, still correct.
    ///
    /// F-12 (#1079): returns `Vec<RecordId>`, not `BTreeSet<RecordId>` — the
    /// hash family already dropped per-element `BTreeSet::insert` (node
    /// allocation + rebalance on every element) in favor of a flat `Vec`
    /// (see `IndexManager::lookup_by_index`'s doc comment); every caller of
    /// this method immediately re-collected the `BTreeSet` into a `Vec`
    /// anyway (see the removed `read_index_scan.rs` collect), so the
    /// `BTreeSet` was pure overhead on both ends.
    pub async fn lookup_range(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<Vec<RecordId>> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        // `None` means a DROP INDEX is currently in its raise→sweep window: the
        // caller MUST NOT read the physical store and MUST fall back (a full
        // table scan). See `crate::reader_drain_gate`'s module doc for the
        // placement invariant: this guard is always the INNERMOST
        // synchronization primitive on the read path.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);

        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        // `Vec::push` into one contiguous buffer, then a single
        // `sort_unstable` + `dedup` reproduces the exact sorted,
        // duplicate-free output a `BTreeSet` would have (the scan is only
        // record-id-ordered WITHIN one indexed value's postings, not across
        // the whole range, so the sort is still required).
        let mut out: Vec<RecordId> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                if let Some(id) = decode_record_id_suffix(k.as_ref()) {
                    out.push(id);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    /// Range lookup with physical values: identical to [`lookup_range`] but
    /// returns `(RecordId, Bytes)` pairs (preserving scan / value order in a
    /// `Vec`, NOT de-duplicating into a `BTreeSet`).  The `Bytes` is the
    /// raw physical_value stored in the index entry — for covering indexes
    /// that is the versioned projection envelope written by
    /// `build_covering_projection`; for non-covering indexes it is empty.
    ///
    /// Used by the index-only read path (slice A3).
    pub async fn lookup_range_with_values(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<Vec<(RecordId, Bytes)>> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);

        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut out: Vec<(RecordId, Bytes)> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                if let Some(id) = decode_record_id_suffix(k.as_ref()) {
                    out.push((id, v));
                }
            }
        }
        Ok(out)
    }

    /// Min lookup — the first record under the sorted prefix.
    /// `iter_range_stream` with batch_size=1 reads exactly the first
    /// entry on B-tree backends; in-memory falls back to its default.
    pub async fn lookup_min(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            if let Some((k, _)) = batch?.into_iter().next() {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// Max lookup — the last record under the sorted prefix.
    /// Uses `iter_range_stream_reverse` so disk backends seek
    /// straight to the upper bound and walk one entry backwards.
    pub async fn lookup_max(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream = self
            .info_store
            .iter_range_stream_reverse(Some(lower), Some(upper), 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            if let Some((k, _)) = batch?.into_iter().next() {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// Last K record ids under the sorted prefix, in value-DESC order.
    /// Mirror of `lookup_first_k` using `iter_range_stream_reverse`.
    pub async fn lookup_last_k(&self, name_interned: u64, k: usize) -> DbResult<Vec<RecordId>> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream =
            self.info_store
                .iter_range_stream_reverse(Some(lower), Some(upper), k.min(256));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// Keyset-seek helper: the first `k` record ids in the half-plane
    /// beyond `seek_encoded`, returned in value order, EXCLUDING every
    /// entry whose indexed value equals the seek value (the already-seen
    /// page boundary).
    ///
    /// `forward == true`  → ASC : walk `[seek, +∞)` low→high, skip `== seek`.
    /// `forward == false` → DESC: walk `(-∞, seek]` high→low, skip `== seek`.
    ///
    /// This is the ordered early-stop replacement for
    /// `lookup_range` + full fetch + full sort + truncate on the keyset
    /// path: the physical key is `[tag|name|encoded_value|record_id]`, so
    /// the backend stream is already value-ordered — we compare the
    /// encoded-value slice of each key against `seek_encoded` to drop the
    /// boundary rows and stop the walk the instant `k` survivors are
    /// collected. Per-page cost is O(k + |rows == seek|), not
    /// O(remaining table).
    pub async fn lookup_range_first_k(
        &self,
        name_interned: u64,
        seek_encoded: &[u8],
        k: usize,
        forward: bool,
    ) -> DbResult<Vec<RecordId>> {
        // First page only; the continuation cursor is discarded. Callers
        // that must survive stale index entries (dead postings whose record
        // body is gone) use `lookup_range_first_k_page` to resume the walk.
        // No tie-breaker (`after_id = None`) → today's skip-all-ties behavior.
        let (ids, _cursor) = self
            .lookup_range_first_k_page(name_interned, seek_encoded, None, None, k, forward)
            .await?;
        Ok(ids)
    }

    /// Continuation-aware sibling of [`lookup_range_first_k`]: collects the
    /// next `k` **physical** index entries in value order and also returns a
    /// resume cursor so the caller can drive a live-row-aware page loop.
    ///
    /// Returns `(ids, cursor)`:
    /// * `ids` — up to `k` record ids in ORDER BY direction, skipping every
    ///   entry whose indexed value equals `seek_encoded` (the page boundary).
    /// * `cursor` — `Some(last_physical_key)` when the walk stopped because it
    ///   reached `k` survivors (there may be MORE range beyond); `None` when
    ///   the underlying stream was exhausted (the range is truly done — a
    ///   genuine last page, never resume past this).
    ///
    /// `after_key`:
    /// * `None` → first page: walk the half-plane bounded by `seek_encoded`.
    /// * `Some(prev_cursor)` → resume STRICTLY after (ASC) / before (DESC) the
    ///   physical key returned as the previous page's cursor, so already-seen
    ///   entries are never re-emitted.
    ///
    /// `after_id` (task #537 — the record-id tie-breaker):
    /// * `None` → the boundary rows (value == `seek_encoded`) are ALL skipped —
    ///   today's exact, backward-compatible behavior. Correct only for the one
    ///   row that established the boundary; every OTHER row tied on the same
    ///   ORDER BY value is silently dropped (the pre-existing #537 limitation
    ///   an old client — one that doesn't echo a tie-breaker — still gets).
    /// * `Some(id)` → the physical `(value, record_id)` order is used to skip
    ///   ONLY the rows at the boundary value that are not-strictly-past `id`:
    ///   for ASC, value == seek AND record_id <= id; for DESC, value == seek
    ///   AND record_id >= id. Rows tied on the seek value but strictly past the
    ///   client's last-seen id are RETURNED, so no tied row is ever lost.
    ///
    /// Note: the caller (`TableManager::read_keyset_seek`) passes the SAME
    /// `after_id` on every internal-loop call this function drives across a
    /// single client request (e.g. when a stale posting forces another
    /// `after_key`-cursored page). This is safe and necessary: `after_id`
    /// only ever affects rows whose encoded value still equals
    /// `seek_encoded` — once `after_key` has moved the walk past the tied
    /// value, `key_value_slice(kb) == seek_encoded` is false and the
    /// boundary check never fires, so `after_id` is a no-op for those rows.
    /// Dropping `after_id` to `None` on continuation calls (an earlier,
    /// buggy version of the caller) made every remaining boundary-value row
    /// unconditionally skipped, silently losing tied rows sitting behind a
    /// stale posting on the SAME request — an adversarial review caught
    /// this empirically before it shipped.
    ///
    /// This keeps the index layer purely about ordered physical traversal:
    /// liveness (fetching record bodies, dropping dead ids) lives entirely in
    /// the engine's read path, which loops here until it has `limit` LIVE rows
    /// or this method reports the range exhausted (`cursor == None`).
    #[allow(clippy::too_many_arguments)] // ordered-traversal cursor + tie-breaker params
    pub async fn lookup_range_first_k_page(
        &self,
        name_interned: u64,
        seek_encoded: &[u8],
        after_key: Option<&[u8]>,
        after_id: Option<&RecordId>,
        k: usize,
        forward: bool,
    ) -> DbResult<(Vec<RecordId>, Option<Bytes>)> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        if k == 0 {
            return Ok((Vec::new(), None));
        }
        let prefix = self.entry_prefix(name_interned);
        let value_start = prefix.len();

        // Bounds. On a continuation page the moving bound is `after_key`
        // itself (inclusive in the store API); we drop the entry equal to it
        // below so the resume is STRICTLY past the previous cursor.
        let (lower, upper) = if forward {
            match after_key {
                // `after_key` is a full physical key → use it verbatim as the
                // (inclusive) lower bound; the open upper is the same
                // `prefix||0xFF*64` as the first page. We do NOT re-wrap
                // through `range_bounds`, which carries seek-value semantics.
                Some(ak) => {
                    let (_, up) = self.range_bounds(&prefix, None, None);
                    (Bytes::copy_from_slice(ak), up)
                }
                None => self.range_bounds(&prefix, Some(seek_encoded), None),
            }
        } else {
            match after_key {
                // Resume high→low: `after_key` becomes the inclusive upper
                // bound; the lower stays the prefix (start of this index).
                Some(ak) => (prefix.clone(), Bytes::copy_from_slice(ak)),
                None => self.range_bounds(&prefix, None, Some(seek_encoded)),
            }
        };

        let mut out = Vec::with_capacity(k);
        let mut cursor: Option<Bytes> = None;
        let mut exhausted = true;
        let batch = k.min(MAINT_SCAN_BATCH);

        if forward {
            let stream = self
                .info_store
                .iter_range_stream(Some(lower), Some(upper), batch);
            futures::pin_mut!(stream);
            'outer: while let Some(b) = stream.next().await {
                for (key, _) in b? {
                    let kb = key.as_ref();
                    // Resume strictly-after: skip the entry equal to the
                    // previous cursor (inclusive lower bound).
                    if let Some(ak) = after_key {
                        if kb == ak {
                            continue;
                        }
                    }
                    // Boundary-value skip (task #537): with a tie-breaker,
                    // skip only the tied rows not-strictly-past `after_id`
                    // (ASC: record_id <= after_id); without one, skip all.
                    if key_value_slice(kb, value_start) == seek_encoded
                        && skip_boundary_row(kb, after_id, true)
                    {
                        continue;
                    }
                    if let Some(id) = decode_record_id_suffix(kb) {
                        out.push(id);
                        if out.len() == k {
                            cursor = Some(Bytes::copy_from_slice(kb));
                            exhausted = false;
                            break 'outer;
                        }
                    }
                }
            }
        } else {
            let stream = self
                .info_store
                .iter_range_stream_reverse(Some(lower), Some(upper), batch);
            futures::pin_mut!(stream);
            'outer: while let Some(b) = stream.next().await {
                for (key, _) in b? {
                    let kb = key.as_ref();
                    if let Some(ak) = after_key {
                        if kb == ak {
                            continue;
                        }
                    }
                    // Boundary-value skip (task #537), DESC direction: skip
                    // only tied rows not-strictly-before `after_id`
                    // (record_id >= after_id); without one, skip all.
                    if key_value_slice(kb, value_start) == seek_encoded
                        && skip_boundary_row(kb, after_id, false)
                    {
                        continue;
                    }
                    if let Some(id) = decode_record_id_suffix(kb) {
                        out.push(id);
                        if out.len() == k {
                            cursor = Some(Bytes::copy_from_slice(kb));
                            exhausted = false;
                            break 'outer;
                        }
                    }
                }
            }
        }

        // If the stream ran dry before we hit `k`, the range is exhausted and
        // there is nothing to resume from → cursor stays `None`.
        if exhausted {
            cursor = None;
        }
        Ok((out, cursor))
    }

    /// First K record ids under the sorted prefix, in value-asc order.
    pub async fn lookup_first_k(&self, name_interned: u64, k: usize) -> DbResult<Vec<RecordId>> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream =
            self.info_store
                .iter_range_stream(Some(lower), Some(upper), k.min(MAINT_SCAN_BATCH));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// tx-aware variant of [`lookup_range`].
    ///
    /// Phase C (Step 5): records an `IndexRange` predicate dependency
    /// for Serializable txs BEFORE forwarding to the non-tx method.
    /// Zero-overhead: Snapshot / non-tx callers skip the recording
    /// block entirely (single tag-compare on `Option<&TxContext>`).
    pub async fn lookup_range_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_range(name_interned, start_encoded, end_encoded)
            .await
    }

    /// tx-aware variant of [`lookup_min`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency (the entire sorted index) for Serializable txs.
    pub async fn lookup_min_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_min(name_interned).await
    }

    /// tx-aware variant of [`lookup_max`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs.
    pub async fn lookup_max_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_max(name_interned).await
    }

    /// tx-aware variant of [`lookup_last_k`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs. The interval does not depend on
    /// `k` — every entry the scan could reach is in the full-prefix range.
    pub async fn lookup_last_k_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        k: usize,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_last_k(name_interned, k).await
    }

    /// tx-aware variant of [`lookup_first_k`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs.
    pub async fn lookup_first_k_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        k: usize,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_first_k(name_interned, k).await
    }

    /// Build inclusive (lower, upper) physical-key bounds for one
    /// sorted-index range query.
    ///
    /// - `start_encoded = None` → lower = `prefix` itself (start of
    ///   the index's keyspace).
    /// - `end_encoded = None` → upper = `prefix || [0xFF; 64]`,
    ///   strictly greater than any real entry in this prefix and
    ///   strictly less than the start of the next prefix
    ///   (`name_interned + 1`), so it correctly bounds "everything in
    ///   this index" without leaking into neighbours.
    /// - Otherwise the bounds are `prefix || encoded[ || 0xFF×16]`.
    pub(super) fn range_bounds(
        &self,
        prefix: &Bytes,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> (Bytes, Bytes) {
        let lower = match start_encoded {
            Some(enc) => {
                let mut k = Vec::with_capacity(prefix.len() + enc.len());
                k.extend_from_slice(prefix);
                k.extend_from_slice(enc);
                Bytes::from(k)
            }
            None => prefix.clone(),
        };
        let upper = match end_encoded {
            Some(enc) => {
                let mut k = Vec::with_capacity(prefix.len() + enc.len() + 16);
                k.extend_from_slice(prefix);
                k.extend_from_slice(enc);
                // Cover all record_id tiebreakers at the upper value.
                k.extend_from_slice(&[0xFFu8; 16]);
                Bytes::from(k)
            }
            None => {
                let mut k = Vec::with_capacity(prefix.len() + 64);
                k.extend_from_slice(prefix);
                k.extend_from_slice(&[0xFFu8; 64]);
                Bytes::from(k)
            }
        };
        (lower, upper)
    }

    // ----- internals --------------------------------------------------------

    /// Count of entries currently in the sorted index — used by the
    /// doctor's verify pass. O(K) where K is the entry count.
    pub async fn entry_count(&self, name_interned: u64) -> DbResult<u64> {
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        // `None` means a DROP INDEX is currently in its raise→sweep window:
        // the caller must report an error (fall back to full scan or similar).
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

        let prefix = self.entry_prefix(name_interned);
        let mut count: u64 = 0;
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        Ok(count)
    }

    /// True iff `record` carries a value at `field_path` that the
    /// sort codec can encode (i.e. an entry for this record *should*
    /// exist in a sorted index keyed on this path).
    pub fn has_indexable_value(record: &(impl RecordRef + ?Sized), field_path: &[u64]) -> bool {
        matches!(extract_and_encode(record, field_path), Ok(Some(_)))
    }

    /// Prefix common to every entry of one sorted index.
    pub(super) fn entry_prefix(&self, name_interned: u64) -> Bytes {
        let mut buf = Vec::with_capacity(9);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        Bytes::from(buf)
    }

    /// Full entry key for one (value, record_id) pair.
    pub(super) fn build_entry_key(
        &self,
        name_interned: u64,
        encoded_value: &[u8],
        record_id: &RecordId,
    ) -> Bytes {
        let mut buf = Vec::with_capacity(1 + 8 + encoded_value.len() + 16);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        buf.extend_from_slice(encoded_value);
        buf.extend_from_slice(&record_id.to_bytes());
        Bytes::from(buf)
    }
}
