//! Phase C1 — tx-scoped read-only database handle for relational validators.
//!
//! [`ValidatorDb`] is the narrow read-only interface that a
//! [`RecordValidator`](super::RecordValidator) uses for cross-table
//! (`foreign_key`, Phase C2) and self-table (`unique`, Phase C3) checks.
//!
//! # No re-entrancy deadlock
//!
//! The write path calls validators **before** staging any data or acquiring
//! any write locks (see `write_exec.rs`: `run_validators_qv` at line ~170,
//! `insert_tx_many_bytes` — the staging + exclusive-lock step — at line ~179).
//! Therefore when a validator reads through `ValidatorDb`, the current
//! transaction holds **no** locks on the keys being inspected, and the read
//! proceeds entirely on the committed snapshot (`tx.snapshot_version`)
//! without entering the commit pipeline or re-acquiring `commit_lock`.
//!
//! ## Pessimistic isolation caveat
//!
//! Under `Pessimistic` isolation, `read_one_tx` acquires a `Shared` lock via
//! `acquire_pessimistic_read_lock`. This is safe for **cross-table** FK reads
//! (different table → independent lock space). For **self-table** unique
//! reads, `exists_in_self` deliberately reads committed state through
//! `lookup_by_index` / `list_stream` (which do **not** call
//! `acquire_pessimistic_read_lock`), avoiding a self-deadlock on keys the tx
//! might later lock. Validators run pre-staging, so even under Pessimistic
//! the tx does not yet hold exclusive locks at validator time — but the
//! committed-only read path keeps the invariant structural rather than
//! temporal.
//!
//! # Read-your-own-writes (batch-unique)
//!
//! `exists_in_self` additionally probes the tx's own staging overlay
//! (`tx.write_set`) so that a batch-insert of duplicate unique values within
//! the same transaction is caught at validation time, not deferred to
//! commit-time `UniqueGuard` re-validation.

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::TableRef;
use crate::query::TableResolver;
use crate::table::TableManager;

// ── QueryValue → InnerValue scalar conversion ───────────────────────────────

/// Convert a scalar `QueryValue` to `InnerValue` for index lookups.
///
/// Returns `None` for containers (`Map`/`List`/`Set`) — those cannot be
/// single-field index keys.  This mirrors `filter_value_to_inner` but works
/// on `QueryValue` directly (the form validators receive).
fn qv_scalar_to_inner(qv: &QueryValue) -> Option<InnerValue> {
    match qv {
        QueryValue::Null => Some(InnerValue::Null),
        QueryValue::Bool(b) => Some(InnerValue::Bool(*b)),
        QueryValue::Int(i) => Some(InnerValue::Int(*i)),
        QueryValue::F64(f) => Some(InnerValue::F64(*f)),
        QueryValue::Str(s) => Some(InnerValue::Str(s.clone())),
        QueryValue::Bin(b) => Some(InnerValue::Bin(b.clone())),
        QueryValue::Dec(d) => Some(InnerValue::Dec(*d)),
        QueryValue::Big(b) => Some(InnerValue::Big(b.clone())),
        _ => None,
    }
}

/// Check whether a decoded record's `field` matches `value` by name.
///
/// Uses `RecordView` scalar probing (zero-copy) when possible; falls back
/// to `InnerValue` field traversal for non-map records.
fn record_field_matches(
    record_bytes: &[u8],
    field: &str,
    value: &QueryValue,
    interner: &shamir_types::core::interner::Interner,
) -> bool {
    if let Some(field_id) = interner.get_ind(field) {
        return record_field_matches_by_id(record_bytes, &field_id, value);
    }
    false
}

/// Check whether a decoded record's field (by pre-resolved [`InternerKey`])
/// matches `value`.
///
/// Factored out of [`record_field_matches`] so the staged-probe path can
/// resolve the field id through the tx-layered interner and reuse the same
/// matching logic.
fn record_field_matches_by_id(
    record_bytes: &[u8],
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let path = std::slice::from_ref(field_id);
    // Try zero-copy RecordView lens first.
    if let Ok(view) = RecordView::new(record_bytes) {
        if let Some(actual) = view.scalar_at(path) {
            return scalar_ref_matches_query_value(&actual, value);
        }
    }
    // Fallback: decode InnerValue tree (non-map records).
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(record_bytes)) {
        if let Some(actual) = tree.scalar_at(path) {
            return scalar_ref_matches_query_value(&actual, value);
        }
    }
    false
}

/// Compare a `ScalarRef` against a `QueryValue` for equality.
fn scalar_ref_matches_query_value(
    actual: &shamir_types::record_view::ScalarRef<'_>,
    value: &QueryValue,
) -> bool {
    use shamir_types::record_view::ScalarRef;
    match (actual, value) {
        (ScalarRef::Null, QueryValue::Null) => true,
        (ScalarRef::Bool(a), QueryValue::Bool(b)) => a == b,
        (ScalarRef::Int(a), QueryValue::Int(b)) => a == b,
        (ScalarRef::F64(a), QueryValue::F64(b)) => a == b,
        (ScalarRef::Str(a), QueryValue::Str(b)) => *a == b.as_str(),
        (ScalarRef::Bin(a), QueryValue::Bin(b)) => *a == b.as_slice(),
        _ => false,
    }
}

// ── ValidatorDb ─────────────────────────────────────────────────────────────

/// Tx-scoped read-only database handle for relational validators.
///
/// Carries everything a `foreign_key` (C2) or `unique` (C3) validator needs
/// to inspect database state **on the current transaction's snapshot**
/// without re-entering the write/commit pipeline:
///
/// - `tx` — the active `TxContext`; reads are capped at `tx.snapshot_version`.
/// - `self_table` — the `TableManager` of the table being written (for
///   self-table unique checks).
/// - `resolver` — optional `TableResolver` for cross-table FK lookups.
///   `None` when no resolver is wired (unit tests, standalone table without
///   a repo context).
///
/// **All reads go through the existing read path** (`read_one_tx`,
/// `lookup_by_index`, `list_stream`, `mvcc.get_at`) — never through
/// `DbGateway` (which is autocommit-per-op and would deadlock the batch
/// planner).
pub struct ValidatorDb<'a> {
    /// The active transaction — reads see `snapshot_version`.
    pub tx: &'a shamir_tx::TxContext,
    /// The table being written (self-table for unique checks).
    pub self_table: &'a TableManager,
    /// Cross-table resolver for FK semi-joins. `None` in contexts without a
    /// repo-level resolver (tests, standalone tables).
    pub resolver: Option<&'a dyn TableResolver>,
}

impl<'a> ValidatorDb<'a> {
    /// Construct a validator DB handle.
    ///
    /// `resolver` may be `None` — cross-table `exists_in` will then return
    /// `Ok(false)` (fail-open for FK when no resolver is available, matching
    /// the Phase B scalar-bridge "skip silently when unavailable" precedent).
    pub fn new(
        tx: &'a shamir_tx::TxContext,
        self_table: &'a TableManager,
        resolver: Option<&'a dyn TableResolver>,
    ) -> Self {
        Self {
            tx,
            self_table,
            resolver,
        }
    }

    // ── FK semi-join (cross-table) ──────────────────────────────────────

    /// Check whether a record exists in **another** table where `field`
    /// equals `value`, visible at this tx's snapshot version.
    ///
    /// This is the `foreign_key` primitive (Phase C2): the validator confirms
    /// that a referenced value exists in the parent table.
    ///
    /// # Read path
    ///
    /// 1. Resolve `table` via the [`TableResolver`] → target `TableManager`.
    /// 2. If an index covers `field`, use `lookup_by_index` (O(log n)) and
    ///    confirm the posting set is non-empty.
    /// 3. Otherwise, scan via `list_stream` and match by field name.
    /// 4. FG-3: additionally probe THIS tx's own `write_set` entry for the
    ///    resolved target table (a staged insert into the parent table,
    ///    earlier in the SAME tx, not yet committed — e.g. insert-parent then
    ///    insert-child-referencing-it in one batch). Mirrors
    ///    `exists_in_self`'s staged-probe step, generalised to a table other
    ///    than `self_table`.
    ///
    /// All reads are on `tx.snapshot_version` — **no write locks acquired**,
    /// **no commit pipeline re-entry**, **no `DbGateway` (autocommit)**.
    ///
    /// Returns `Ok(false)` when no resolver is attached (fail-open).
    pub async fn exists_in(
        &self,
        table: &TableRef,
        field: &str,
        value: &QueryValue,
    ) -> DbResult<bool> {
        let Some(resolver) = self.resolver else {
            // No cross-table resolver — fail-open (skip the FK check).
            // Matches Phase B scalar-bridge precedent: unavailable capability
            // → silent skip, never panic.
            return Ok(false);
        };
        let target = resolver.resolve(table).await?;
        self.exists_in_table(&target, field, value).await
    }

    /// Internal: probe a resolved target table for `field == value`.
    async fn exists_in_table(
        &self,
        target: &TableManager,
        field: &str,
        value: &QueryValue,
    ) -> DbResult<bool> {
        let interner = target.interner().get().await?;

        // Fast path: single-field index lookup.
        if let Some(inner_value) = qv_scalar_to_inner(value) {
            if let Some(field_id) = interner.get_ind(field) {
                let field_path = [field_id.id()];
                if let Some(idx_name) = target.find_single_field_index(&field_path) {
                    let ids = target
                        .index_manager_ref()
                        .lookup_by_index(idx_name, std::slice::from_ref(&inner_value))
                        .await?;
                    if !ids.is_empty() {
                        return Ok(true);
                    }
                    // Index says no match — authoritative for the COMMITTED
                    // store, so skip the full-scan fallback below entirely
                    // (it would re-scan the same store the index already
                    // covers and can only agree). The index alone cannot
                    // rule out a same-tx FK match though: a staged insert
                    // into the target table (this tx, not yet committed) is
                    // never in the index (indexing happens at commit), so go
                    // straight to the staged-overlay probe.
                    return Ok(self.staged_field_matches(target.table_token(), &field_id, value));
                }
            }
        }

        // Fallback: full scan with field match (committed store only).
        let batch_size = 1000;
        let stream = target.list_stream(batch_size);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (_, cow) in batch {
                let bytes: Bytes = match cow {
                    crate::table::record_cow::RecordCow::Borrowed(b) => b,
                    crate::table::record_cow::RecordCow::Owned(tree) => {
                        // Aggregate path: serialize once for uniform matching.
                        tree.to_bytes().map_err(|e| {
                            DbError::Codec(format!(
                                "ValidatorDb::exists_in_table owned serialize: {e}"
                            ))
                        })?
                    }
                };
                if record_field_matches(bytes.as_ref(), field, value, interner) {
                    return Ok(true);
                }
            }
        }

        // FG-3: read-your-own-writes for cross-table FK checks — probe this
        // tx's own staged writes to the TARGET table (a parent row inserted
        // earlier in the SAME tx, not yet committed, never reachable via the
        // index or the committed-store scan above). Resolved through the
        // TARGET table's token, not `self_table`'s — this is the key
        // difference from `exists_in_self`'s self-table staged probe.
        if let Some(field_id) = interner.get_ind(field) {
            if self.staged_field_matches(target.table_token(), &field_id, value) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// FG-3 helper: does this tx's own staged (uncommitted) write_set for
    /// `target_token` contain a row where `field_id == value`? Shared by
    /// both `exists_in_table` exit paths (index-conclusive and full-scan
    /// fallback) so the staged-overlay probe isn't duplicated.
    fn staged_field_matches(
        &self,
        target_token: u64,
        field_id: &shamir_types::core::interner::InternerKey,
        value: &QueryValue,
    ) -> bool {
        let Some(staging) = self.tx.write_set.get(&target_token) else {
            return false;
        };
        staging.snapshot_ops().into_iter().any(|op| match op {
            shamir_storage::types::KvOp::Set(_, ref bytes) => {
                record_field_matches_by_id(bytes.as_ref(), field_id, value)
            }
            shamir_storage::types::KvOp::Remove(_) => false,
        })
    }

    // ── Unique probe (self-table) ───────────────────────────────────────

    /// Check whether a record exists in the **same** table where `field`
    /// equals `value`, visible at the snapshot **plus** this tx's own
    /// staged writes.
    ///
    /// This is the `unique` primitive (Phase C3). The `exclude_rid` parameter
    /// allows an UPDATE to exclude the record being modified (so changing
    /// a non-unique field does not self-conflict).
    ///
    /// # Read path (committed state)
    ///
    /// Reads **committed** state via `lookup_by_index` (if an index covers
    /// `field`) or `list_stream` (fallback scan). These paths do **not**
    /// call `acquire_pessimistic_read_lock`, so there is no self-deadlock
    /// risk even under Pessimistic isolation.
    ///
    /// # Read-your-own-writes (staged overlay)
    ///
    /// Additionally probes `tx.write_set` for this table — staged inserts in
    /// the same tx that match `field == value`. This catches batch-insert
    /// duplicates within one tx (which `lookup_by_index` cannot see because
    /// postings are applied only at commit).
    ///
    /// Returns `Ok(true)` if any committed record OR staged write matches
    /// (and is not excluded by `exclude_rid`).
    pub async fn exists_in_self(
        &self,
        field: &str,
        value: &QueryValue,
        exclude_rid: Option<&RecordId>,
    ) -> DbResult<bool> {
        let table = self.self_table;
        let interner = table.interner().get().await?;

        // --- 1. Committed state via index (fast path) ---
        if let Some(inner_value) = qv_scalar_to_inner(value) {
            if let Some(field_id) = interner.get_ind(field) {
                let field_path = [field_id.id()];
                if let Some(idx_name) = table.find_single_field_index(&field_path) {
                    let ids = table
                        .index_manager_ref()
                        .lookup_by_index(idx_name, std::slice::from_ref(&inner_value))
                        .await?;
                    // Audit 1.5/3.2: `ids` is now `Arc<[RecordId]>` (sorted slice); iterate.
                    for id in ids.iter() {
                        if Some(id) != exclude_rid {
                            return Ok(true);
                        }
                    }
                    // All matches excluded → fall through to scan (rare).
                }
            }
        }

        // --- 2. Committed state via scan (fallback) ---
        let batch_size = 1000;
        let stream = table.list_stream(batch_size);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (id, cow) in batch {
                if Some(&id) == exclude_rid {
                    continue;
                }
                let bytes: Bytes = match cow {
                    crate::table::record_cow::RecordCow::Borrowed(b) => b,
                    crate::table::record_cow::RecordCow::Owned(tree) => {
                        tree.to_bytes().map_err(|e| {
                            DbError::Codec(format!(
                                "ValidatorDb::exists_in_self owned serialize: {e}"
                            ))
                        })?
                    }
                };
                if record_field_matches(bytes.as_ref(), field, value, interner) {
                    return Ok(true);
                }
            }
        }

        // --- 3. Staged writes in this tx (read-your-own-writes) ---
        //
        // Staged record bytes encode field names with the TX-OVERLAY interner
        // (a brand-new field name staged in the current tx is layered, not yet
        // in base). We resolve `field` through the tx-layered interner so
        // overlay-minted ids are found.
        let table_token = table.table_token();
        if let Some(staging) = self.tx.write_set.get(&table_token) {
            // Resolve field through the layered interner: base first, then
            // tx overlay. This finds both committed field names AND
            // overlay-minted names from earlier statements in this tx.
            let layered = shamir_tx::LayeredInterner::Layered {
                base: interner,
                overlay: &self.tx.interner_overlay,
                next_overlay_id: &self.tx.next_overlay_id,
            };
            // get_id is async but only the Layered::overlay branch issues
            // a single scc read — cheap and non-blocking.
            if let Some(field_id_raw) = layered.get_id(field).await {
                let field_key = shamir_types::core::interner::InternerKey::new(field_id_raw);
                for op in staging.snapshot_ops() {
                    if let shamir_storage::types::KvOp::Set(ref key, ref bytes) = op {
                        // Exclude the record being updated (by key, which
                        // encodes the RecordId).
                        if let Some(rid) = exclude_rid {
                            if key.as_ref() == rid.as_bytes() {
                                continue;
                            }
                        }
                        if record_field_matches_by_id(bytes.as_ref(), &field_key, value) {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }
}
