//! Write operation execution on TableManager.
//!
//! Implements execute_insert_tx, execute_update_tx, execute_delete_tx,
//! execute_set_tx for TableManager. The legacy non-transactional
//! `execute_insert` / `execute_update` / `execute_delete` / `execute_set`
//! were removed (W3a / W3d-2): every non-tx mutation now routes through an
//! implicit Snapshot batch-tx via `run_implicit_batch_tx` → the `_tx` variants.

use std::borrow::Cow;
use std::cell::RefCell;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use captrack::tvec;

use bytes::Bytes;
use futures::StreamExt;
use fxhash::FxHashMap;

use crate::function::builtin_scalars;
use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{
    DeleteOp, InsertOp, InsertedRecord, SetOp, UpdateOp, UpdateReturnMode, WriteResult,
};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{
    merge_storage_bytes, query_value_to_inner_with, query_value_to_storage_bytes,
    query_value_to_storage_bytes_into, record_view_to_query_value, validate_keys_resolve_interner,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

use crate::validator::WriteOp;
use shamir_types::access::Actor;

use super::record_cow::RecordCow;
use super::table_manager::TableManager;
use super::write_helpers::{
    apply_defaults, apply_transforms, intern_via_layered, make_layered_interner,
    resolve_computed_record, validator_failure_to_db_error,
};

impl TableManager {
    /// tx-aware variant of INSERT.
    ///
    /// Stages each insert through [`insert_tx`](Self::insert_tx) — no
    /// physical writes until `commit_tx` Phase 5. Returns the same
    /// `WriteResult` shape (records with `_id`, affected count).
    /// Interner / counter persistence is **skipped** — commit_tx
    /// handles that uniformly for all staged mutations.
    pub async fn execute_insert_tx(
        &self,
        op: &InsertOp,
        tx: &mut shamir_tx::TxContext,
        return_result: bool,
        resolver: Option<&dyn crate::query::TableResolver>,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields first (fail-closed), then
        // intern field names.
        //
        // C5: on the IMPLICIT / auto-commit path (`run_implicit_batch_tx` →
        // `tx.implicit == true`) we intern field names **directly into base**
        // instead of into the tx overlay. The implicit tx always commits
        // (Snapshot, last-writer-wins, never rolls back), so there is no
        // rollback-isolation to preserve — and base-interning makes the
        // commit-time `commit_interner_overlay` remap EMPTY, which
        // short-circuits the deep `rewrite_set_bytes` walk over every staged
        // record (the redundant second full pass this cycle removes). The
        // newly-interned `(name, id)` pairs are still collected here and
        // threaded into `tx.interner_deltas` below so they reach the
        // `WalEntryV2.interner_delta` — recovery replays them via
        // `touch_with_id` exactly as for the overlay path (inv. #2). This
        // mirrors the original non-tx `execute_insert`, which also interns
        // straight into base via `touch_ind`.
        //
        // On the INTERACTIVE path (`tx.implicit == false`) we keep the tx
        // overlay: multi-statement txs must stay rollback-isolated, so new
        // field names live in the overlay until commit (inv. #1).
        //
        // Per-batch intern cache (C1): field names repeat across every row in
        // the batch (e.g. all 100 rows have "email", "city", "score"). Without
        // the cache, the per-field intern does a DashMap lookup per field per
        // row — O(N×k) sharded-map operations. With the cache we pay one
        // DashMap lookup per unique field name per batch and amortise the rest
        // to an FxHashMap lookup — 3-5× cheaper for typical short batches with
        // uniform schema.
        let intern_to_base = tx.implicit;
        let mut resolved_values: Vec<Cow<'_, QueryValue>> =
            tvec!("engine/write_exec/resolved_values", op.values.len());
        // Phase ②.4c — literal-default rules for this table, fetched ONCE per
        // batch. Empty for the common case (no `default` declared) → the
        // `apply_defaults` call below is skipped entirely (fast-skip; the
        // hot path pays nothing). When non-empty, each ABSENT field is
        // stamped with its default AFTER `resolve_computed_record` and BEFORE
        // encode — so both the stored bytes and the validator see the
        // default value. Explicit values (including explicit `Null`) are
        // never overwritten (replay-safe invariant, DDL-EVOLUTION-PLAN §②.4a).
        let defaults = self.schema_defaults();
        // ③.2b — declarative transform rules for this table, fetched ONCE per
        // batch.  Empty until #281/#282 land the schema-surface fields
        // (`auto_now` / `auto_now_add` / expression-default) → wiring is
        // inert for now, which is the expected and correct state.
        let transforms = self.schema_transforms();
        // now_ns: admission-time wall-clock nanoseconds, computed ONCE per
        // batch so every record in the batch shares the same timestamp
        // (deterministic within a batch, not per-record).  Off-replay by
        // construction — transforms run at admission, not WAL-replay.
        let now_ns: u64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        // Collected on the base-intern path only: `(field_name, base_id)` for
        // every name genuinely NEW to the base interner this insert. Drained
        // into `tx.interner_deltas` after the immutable-borrow block ends.
        let new_base_keys: RefCell<Vec<(String, u64)>> =
            RefCell::new(tvec!("engine/write_exec/new_base_keys", 0));
        // W2d-cutover: build storage Bytes directly via the byte-identical
        // encoder `query_value_to_storage_bytes` (no InnerValue tree).
        let staged: Vec<bytes::Bytes> = {
            let layered = make_layered_interner(interner, tx);
            let base_intern_fn = intern_via_layered(&layered);
            // RefCell lets the closure capture and mutate the cache while
            // satisfying the `Fn` (not `FnMut`) bound on
            // `query_value_to_storage_bytes`.
            let cache: RefCell<FxHashMap<String, InternerKey>> = RefCell::new(FxHashMap::default());
            let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
                {
                    let c = cache.borrow();
                    if let Some(ik) = c.get(key) {
                        return Ok(ik.clone());
                    }
                }
                let ik = if intern_to_base {
                    // Intern straight into base. `touch_ind` returns whether the
                    // key was newly created so we can record the delta for WAL /
                    // recovery — the overlay path's `commit_interner_overlay`
                    // builds the identical `(name, id)` delta from `is_new`.
                    let ti = interner.touch_ind(key).map_err(|e| {
                        shamir_types::codecs::CodecError::Decode(format!(
                            "Failed to intern key '{}': {}",
                            key, e
                        ))
                    })?;
                    if ti.is_new() {
                        new_base_keys
                            .borrow_mut()
                            .push((key.to_string(), ti.key().id()));
                    }
                    ti.into_key()
                } else {
                    base_intern_fn(key)?
                };
                cache.borrow_mut().insert(key.to_string(), ik.clone());
                Ok(ik)
            };
            let mut out = tvec!("engine/write_exec/staged_bytes", op.values.len());
            let mut scratch = tvec!("engine/write_exec/encode_scratch", 0);
            for value in &op.values {
                let mut resolved = resolve_computed_record(value, interner)
                    .map_err(shamir_storage::error::DbError::Codec)?;
                // Phase ②.4c — stamp literal defaults onto absent fields
                // BEFORE encode so both stored bytes and validators see them.
                // Fast-skip when the table declares no defaults (common case).
                if !defaults.is_empty() {
                    apply_defaults(resolved.to_mut(), &defaults);
                }
                // ③.2b/③.2d — apply declarative transforms (computed-default +
                // server-side timestamps) AFTER literal defaults and BEFORE
                // encode so both stored bytes and CHECK-validators see the
                // transformed values.  Fast-skip when empty (hot path).
                // is_insert=true: AutoNowAdd/ComputedDefault run here;
                // AutoNow also runs (it always runs).
                if !transforms.is_empty() {
                    apply_transforms(
                        resolved.to_mut(),
                        &transforms,
                        builtin_scalars(),
                        now_ns,
                        true,
                    );
                }
                let bytes = query_value_to_storage_bytes_into(&resolved, &intern_fn, &mut scratch)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                out.push(bytes);
                resolved_values.push(resolved);
            }
            out
        };

        // C5: thread the base-interned delta into the tx so pre-commit emits it
        // in the `WalEntryV2.interner_delta`. The overlay stays empty on this
        // path, so `pre_commit` Phase 1 skips the overlay merge entirely and
        // the staging-bytes remap is a no-op (no overlay ids to rewrite).
        //
        // Stage I: the interner is per-REPO (one id-namespace across tables),
        // so the delta is a single flat `Vec` — no per-table key.
        let new_base_keys = new_base_keys.into_inner();
        if !new_base_keys.is_empty() {
            tx.interner_deltas.extend(new_base_keys);
        }

        // S3: run validators on each record before staging.
        // tx path: resolve keys through the tx overlay so brand-new field
        // names (staged above into the layered interner, not yet in base)
        // resolve at validation time.
        // TODO actor threading — use Actor::System for now.
        //
        // W1: feed the resolved `QueryValue` directly (the resolved record
        // already carries the tx-overlay keys as plain strings, so no
        // de-intern is needed). Identity vs. the legacy
        // `run_validators_tx` path is proven by
        // `validator::tests::query_value_conv_tests`.
        for qv in &resolved_values {
            self.run_validators_qv(
                WriteOp::Insert,
                Some(qv),
                None,
                &Actor::System,
                Some(tx),
                resolver,
            )
            .await
            .map_err(validator_failure_to_db_error)?;
        }

        // W2d-cutover: lens-driven batched tx insert — the staged bytes
        // feed `insert_tx_many_bytes`, which builds `RecordView`s over the
        // bytes and drives every index/unique/vector planner through the
        // zero-copy lens. No InnerValue tree is built on the insert path.
        let values_ids: Vec<RecordId> = self.insert_tx_many_bytes(&staged, tx).await?;

        // S-write id-keyed branch: accept pre-encoded id-msgpack records
        // from `op.records_idmsgpack`. For each record:
        //   1. Structural validation — `RecordView::new` (must be a msgpack map).
        //   2. Security spine — `validate_keys_resolve_interner` confirms every
        //      interned key id resolves in the server interner. REJECT the whole
        //      op on first unresolved id — never write bytes with stale/forged keys.
        //   3. Per-write interning is SKIPPED (the client already interned all
        //      keys; `tx.interner_deltas` for this branch is empty).
        //   4. Validators — if any Insert validators are bound, decode each row
        //      via `record_view_to_query_value` and run `run_validators_qv`.
        //      If no validators are bound, skip the decode entirely (lens-only).
        //   5. Feed the validated bytes verbatim to `insert_tx_many_bytes` —
        //      same lens-driven path as the `values` branch above.
        //
        // When BOTH `values` and `records_idmsgpack` are non-empty, the
        // `values` records are inserted first and their IDs appear first in
        // the combined result.
        let idmsgpack_ids: Vec<RecordId> = if op.records_idmsgpack.is_empty() {
            tvec!("engine/write_exec/idmsgpack_ids_empty", 0)
        } else {
            // Collect validated bytes — validate-before-collect so we never
            // stage any bytes whose keys don't resolve (security invariant).
            let mut idmsgpack_staged: Vec<Bytes> = tvec!(
                "engine/write_exec/idmsgpack_staged",
                op.records_idmsgpack.len()
            );
            // Check if any Insert validators are bound; if not, skip the
            // per-row decode. One snapshot covers the whole batch.
            let has_validators = {
                let bindings = self.validator_bindings();
                bindings.iter().any(|b| b.ops.contains(&WriteOp::Insert))
            };
            // Phase ②.4c — DEFAULT stamp on the id-keyed branch.
            //
            // MVP choice: (ii) — the id-keyed (client-pre-interned) path
            // assumes a COMPLETE record and does NOT stamp literal defaults
            // here. Rationale: the bytes are pre-interned msgpack from the
            // client; stamping a default would require decode → stamp →
            // re-encode (re-interning brand-new field names through the tx
            // overlay), which breaks the lens-only verbatim fast path that
            // makes id-keyed worth using. The common case (no defaults on
            // the table) is a verbatim copy regardless — `has_defaults` is
            // computed purely for documentation/clarity and does not gate a
            // decode here. If a future use case needs defaults on id-keyed,
            // gate a decode-stamp-reencode behind `has_defaults` (variant (i)).
            let _has_defaults = !defaults.is_empty();
            for buf in &op.records_idmsgpack {
                // 1. Structural validation — reject non-map bytes.
                let view = RecordView::new(buf.as_ref()).map_err(|e| {
                    shamir_storage::error::DbError::Validation(format!(
                        "execute_insert_tx (id-keyed): malformed record bytes: {e}"
                    ))
                })?;
                // 2. Security spine — reject if any key id is unresolved.
                validate_keys_resolve_interner(&view, interner).map_err(|e| {
                    shamir_storage::error::DbError::Validation(format!(
                        "execute_insert_tx (id-keyed): unresolved interner key — {e}"
                    ))
                })?;
                // 3. Validators — only decode when validators are actually bound.
                if has_validators {
                    let qv = record_view_to_query_value(&view, interner)?;
                    self.run_validators_qv(
                        WriteOp::Insert,
                        Some(&qv),
                        None,
                        &Actor::System,
                        Some(tx),
                        resolver,
                    )
                    .await
                    .map_err(validator_failure_to_db_error)?;
                }
                idmsgpack_staged.push(Bytes::copy_from_slice(buf.as_ref()));
            }
            self.insert_tx_many_bytes(&idmsgpack_staged, tx).await?
        };

        // Merge IDs: values first, then id-keyed (deterministic ordering).
        let mut all_ids = values_ids;
        all_ids.extend_from_slice(&idmsgpack_ids);

        // Skip result-map assembly for fire-and-forget inserts
        // (return_result=false) — avoids per-row QueryValue map build
        // and clone on the hot batch-insert path.
        let records = if return_result {
            let mut records =
                build_insert_result_records(&resolved_values, &all_ids[..resolved_values.len()]);
            // Build RETURNING rows for id-keyed records: decode each
            // record via `record_view_to_query_value` to get name-keyed
            // `QueryValue`, then wrap in the same `InsertedRecord::Direct`
            // the values branch produces.  Order: values-first, then
            // id-keyed — matching `all_ids`.
            if !op.records_idmsgpack.is_empty() {
                let id_offset = resolved_values.len();
                for (i, buf) in op.records_idmsgpack.iter().enumerate() {
                    let view = RecordView::new(buf.as_ref()).map_err(|e| {
                        shamir_storage::error::DbError::Validation(format!(
                            "execute_insert_tx (id-keyed return_result): malformed record bytes: {e}"
                        ))
                    })?;
                    let qv = record_view_to_query_value(&view, interner)?;
                    records.push(InsertedRecord {
                        id: Some(all_ids[id_offset + i]),
                        fields: qv,
                    });
                }
            }
            // Optional RETURNING projection (InsertSelect.fields). When set,
            // restrict every returned row to the named fields. This is a
            // post-build pass — the rows above were already materialised by
            // `build_insert_result_records` / the id-keyed branch, so the
            // projection just walks each `fields: QueryValue::Map` and drops
            // unrequested keys. Cheap (one map rebuild per row) and only
            // runs when the caller actually asks for it.
            if let Some(fields) = op.select.as_ref().and_then(|s| s.fields.as_deref()) {
                for rec in records.iter_mut() {
                    rec.fields = project_query_value(rec.fields.clone(), Some(fields));
                }
            }
            records
        } else {
            tvec!("engine/write_exec/insert_records_empty", 0)
        };

        let affected = all_ids.len() as u64;
        Ok(WriteResult {
            affected,
            records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware variant of UPDATE.
    ///
    /// Filters records by `where_clause`, merges `set` fields, then
    /// stages each changed record via [`update_tx`](Self::update_tx).
    /// WAL is NOT opened here — `commit_tx` Phase 4 emits one V2
    /// entry covering the whole tx. Returns the same `WriteResult`
    /// shape. Interner / counter persistence is **skipped** —
    /// commit_tx handles that uniformly.
    pub async fn execute_update_tx(
        &self,
        op: &UpdateOp,
        ctx: &FilterContext<'_>,
        tx: &mut shamir_tx::TxContext,
        resolver: Option<&dyn crate::query::TableResolver>,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields, then apply server-side
        // transforms (③.2d — AutoNow stamps updated_at on every write),
        // then intern field names through the tx overlay.
        let mut resolved_set = resolve_computed_record(&op.set, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        // ③.2d UPDATE-path: apply transforms with is_insert=false.
        // Only AutoNow runs here (injects updated_at into the set-map);
        // AutoNowAdd/ComputedDefault are gated on is_insert and are skipped.
        // Fast-skip when the table declares no transforms (common hot path).
        let transforms = self.schema_transforms();
        if !transforms.is_empty() {
            let now_ns: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            apply_transforms(
                resolved_set.to_mut(),
                &transforms,
                builtin_scalars(),
                now_ns,
                false,
            );
        }
        let set_inner = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);
            query_value_to_inner_with(&resolved_set, &intern_fn)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
        };
        let set_map = match &set_inner {
            InnerValue::Map(m) => m,
            _ => {
                return Err(shamir_storage::error::DbError::Validation(
                    "UPDATE set must produce a Map".to_string(),
                ))
            }
        };

        // Collect matched records as raw storage bytes — no InnerValue tree
        // decode on the hot scan path. The index-via-index arm returns
        // pre-decoded InnerValue trees; serialize them once to bytes so the
        // downstream merge/change-detect/index-plan pipeline is uniform.
        let matched: Vec<(RecordId, Bytes)> = if let Some(ref filter) = op.where_clause {
            if let Some(via_index) = self.lookup_records_via_index(filter, ctx).await? {
                via_index
                    .into_iter()
                    .map(|(id, iv)| {
                        let b = iv.to_bytes().map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "execute_update_tx: index-path serialize: {e}"
                            ))
                        })?;
                        Ok((id, b))
                    })
                    .collect::<DbResult<Vec<_>>>()?
            } else {
                let callback = compile_filter(filter, interner);
                let mut result = tvec!("engine/write_exec/update_scan_result", 0);
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, cow) in batch {
                        match cow {
                            RecordCow::Borrowed(bytes) => {
                                // Zero-copy lens for the filter; keep raw bytes.
                                match RecordView::new(&bytes) {
                                    Ok(view) => {
                                        if callback.matches(&view, ctx) {
                                            result.push((id, bytes));
                                        }
                                    }
                                    Err(_) => {
                                        // Non-map: decode to InnerValue for filter.
                                        if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                                            if callback.matches(&tree, ctx) {
                                                result.push((id, bytes));
                                            }
                                        }
                                    }
                                }
                            }
                            RecordCow::Owned(tree) => {
                                if callback.matches(&tree, ctx) {
                                    let bytes = tree.to_bytes().map_err(|e| {
                                        shamir_storage::error::DbError::Codec(format!(
                                            "execute_update_tx: owned serialize: {e}"
                                        ))
                                    })?;
                                    result.push((id, bytes));
                                }
                            }
                        }
                    }
                }
                result
            }
        } else {
            let mut result = tvec!("engine/write_exec/update_full_scan_result", 0);
            let stream = self.list_stream(batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                for (id, cow) in batch_result? {
                    match cow {
                        RecordCow::Borrowed(bytes) => result.push((id, bytes)),
                        RecordCow::Owned(tree) => {
                            let bytes = tree.to_bytes().map_err(|e| {
                                shamir_storage::error::DbError::Codec(format!(
                                    "execute_update_tx: owned serialize: {e}"
                                ))
                            })?;
                            result.push((id, bytes));
                        }
                    }
                }
            }
            result
        };

        let mut affected: u64 = 0;
        let mut result_records: Vec<InsertedRecord> =
            tvec!("engine/write_exec/update_result_records", matched.len());
        let return_mode = op
            .select
            .as_ref()
            .map(|s| s.return_mode)
            .unwrap_or(UpdateReturnMode::Changed);
        let wants_records = op.select.is_some();

        for (id, old_bytes) in &matched {
            // Byte-level merge: patch old_bytes with the interned set overlay.
            // Produces bytes identical to merge_inner_maps(old_tree, set_map).to_bytes().
            let new_bytes = merge_storage_bytes(old_bytes, set_map)?;

            // Change detection: a no-op set yields identical bytes (the
            // merge copies old value spans verbatim when no key is patched),
            // so byte equality is safe for this "patched from old" shape.
            let changed = new_bytes.as_ref() != old_bytes.as_ref();

            if changed {
                // S3: run validators before staging (tx overlay-aware).
                //
                // Build old/new QueryValues WITHOUT decoding an InnerValue tree.
                // - old_qv: de-intern the committed bytes via RecordView (base
                //   interner — all keys are committed).
                // - new_qv: overlay the string-keyed resolved_set on top of old_qv
                //   (no overlay-minted interned ids to resolve — the overlay keys
                //   are already strings in resolved_set). This matches the W3a
                //   result-QueryValue pattern and avoids the "Interned key not found"
                //   failure that would occur if we de-interned new_bytes through
                //   the base-only reverse snapshot (new_bytes may contain overlay-
                //   minted key ids not yet in base).
                let old_view = RecordView::new(old_bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "execute_update_tx: RecordView for validator old: {e}"
                    ))
                })?;
                let old_qv = record_view_to_query_value(&old_view, interner)?;
                let new_qv = {
                    let mut m = match old_qv.clone() {
                        Value::Map(m) => m,
                        _ => shamir_types::types::common::new_map(),
                    };
                    if let Value::Map(overlay) = resolved_set.as_ref() {
                        for (k, v) in overlay {
                            m.insert(k.clone(), v.clone());
                        }
                    }
                    QueryValue::Map(m)
                };
                self.run_validators_qv(
                    WriteOp::Update,
                    Some(&new_qv),
                    Some(&old_qv),
                    &Actor::System,
                    Some(tx),
                    resolver,
                )
                .await
                .map_err(validator_failure_to_db_error)?;

                self.update_tx_bytes(*id, old_bytes, new_bytes.clone(), &mut *tx)
                    .await?;
                affected += 1;
            }

            if wants_records {
                let should_include = match return_mode {
                    UpdateReturnMode::All => true,
                    UpdateReturnMode::Changed => changed,
                    UpdateReturnMode::Unchanged => !changed,
                };
                if should_include {
                    // Build the result QueryValue from the old record (base-interned
                    // keys — always safe) overlaid with the resolved SET fields
                    // (string-keyed QueryValue — no overlay ids). Decoding
                    // new_bytes via the base interner would fail when op.set
                    // introduces brand-new field names that were only interned
                    // into the tx overlay and are not yet in base.
                    let old_view = RecordView::new(old_bytes).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "execute_update_tx: RecordView for result: {e}"
                        ))
                    })?;
                    let base_qv = record_view_to_query_value(&old_view, interner)?;
                    let mut m = match base_qv {
                        Value::Map(m) => m,
                        _ => shamir_types::types::common::new_map(),
                    };
                    if let Value::Map(overlay) = resolved_set.as_ref() {
                        for (k, v) in overlay {
                            m.insert(k.clone(), v.clone());
                        }
                    }
                    // Optional field projection (UpdateSelect.fields). When
                    // present, restrict the returned row to the named fields
                    // — symmetric with INSERT/DELETE RETURNING projections.
                    let projection = op.select.as_ref().and_then(|s| s.fields.as_deref());
                    let fields = project_query_value(QueryValue::Map(m), projection);
                    result_records.push(InsertedRecord { id: None, fields });
                }
            }
        }

        Ok(WriteResult {
            affected,
            records: result_records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware variant of DELETE.
    ///
    /// Filters records by `where_clause`, stages each removal via
    /// [`delete_tx`](Self::delete_tx). WAL is NOT opened here —
    /// `commit_tx` Phase 4 emits one V2 entry covering the whole tx.
    pub async fn execute_delete_tx(
        &self,
        op: &DeleteOp,
        ctx: &FilterContext<'_>,
        tx: &mut shamir_tx::TxContext,
        resolver: Option<&dyn crate::query::TableResolver>,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Check whether any Delete validators are registered BEFORE the scan
        // so we know whether to carry bytes alongside ids.
        let has_delete_validators = {
            let bindings = self.validator_bindings();
            bindings.iter().any(|b| b.ops.contains(&WriteOp::Delete))
        };
        // RETURNING: when the caller opted in via `op.select`, the matched
        // records' raw bytes must be retained alongside the id so we can
        // de-intern them into a `QueryValue` map for the result. Combined
        // with `has_delete_validators` — either reason forces bytes-on.
        let wants_records = op.select.is_some();
        let keep_bytes = has_delete_validators || wants_records;

        // `to_delete`: (RecordId, Option<Bytes>).
        //
        // - Full-scan arm: the filter runs over a `RecordView` lens (no
        //   `InnerValue` tree decode). Bytes are kept alongside the id so the
        //   validator pass below can build a `RecordView` without a second
        //   store read, and so RETURNING can de-intern the row. When neither
        //   validators nor RETURNING are active, bytes are dropped (skip the
        //   per-row clone). `RecordCow::Owned` (aggregate / GROUP-BY path)
        //   has the tree already; we serialize it once if needed.
        // - Index arm: the planner already resolved the id set; bytes are
        //   serialised from the already-decoded `InnerValue` only when
        //   validators or RETURNING are active, otherwise the record is
        //   skipped.
        let to_delete: Vec<(RecordId, Option<Bytes>)> =
            if let Some(via_index) = self.lookup_records_via_index(&op.where_clause, ctx).await? {
                via_index
                    .into_iter()
                    .map(|(id, iv)| {
                        let bytes = if keep_bytes { iv.to_bytes().ok() } else { None };
                        (id, bytes)
                    })
                    .collect()
            } else {
                let callback = compile_filter(&op.where_clause, interner);
                let mut result: Vec<(RecordId, Option<Bytes>)> =
                    tvec!("engine/write_exec/delete_scan_result", 0);
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, cow) in batch {
                        match cow {
                            RecordCow::Borrowed(bytes) => {
                                // Zero-copy: try a lens over the raw bytes.
                                // Non-map records (bare scalars from legacy
                                // tests) fail RecordView::new — fall back to a
                                // full InnerValue decode for the filter match.
                                match RecordView::new(&bytes) {
                                    Ok(view) => {
                                        if callback.matches(&view, ctx) {
                                            result.push((id, Some(bytes)));
                                        }
                                    }
                                    Err(_) => {
                                        // Non-map: decode to InnerValue for filter.
                                        if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                                            if callback.matches(&tree, ctx) {
                                                result.push((id, Some(bytes)));
                                            }
                                        }
                                        // truly malformed → skip
                                    }
                                }
                            }
                            RecordCow::Owned(tree) => {
                                // Aggregate / GROUP-BY path — tree already in
                                // memory. Filter via the tree (implements
                                // RecordRef), serialise only when needed.
                                if callback.matches(&tree, ctx) {
                                    let bytes = if keep_bytes {
                                        tree.to_bytes().ok()
                                    } else {
                                        None
                                    };
                                    result.push((id, bytes));
                                }
                            }
                        }
                    }
                }
                result
            };

        // S3: run validators on each record before deleting (tx).
        // Feeds a `RecordView` lens over the bytes collected above — no
        // second store read, no `InnerValue` tree decode.
        if has_delete_validators && !to_delete.is_empty() {
            for (_, maybe_bytes) in &to_delete {
                if let Some(bytes) = maybe_bytes {
                    let view = RecordView::new(bytes).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "execute_delete_tx: malformed record bytes for validator: {}",
                            e
                        ))
                    })?;
                    // TODO actor threading — use Actor::System for now.
                    self.run_validators_view(
                        WriteOp::Delete,
                        None,
                        Some(&view),
                        &Actor::System,
                        tx,
                        resolver,
                    )
                    .await
                    .map_err(validator_failure_to_db_error)?;
                }
            }
        }

        // Stage the deletions and collect ids of actually-removed rows for
        // RETURNING. Only rows that `delete_tx` reports as removed (the id
        // existed in this tx's view) contribute to `affected` and to the
        // returned records.
        let mut affected: u64 = 0;
        let mut removed_with_bytes: Vec<Bytes> = tvec!("engine/write_exec/removed_with_bytes", 0);
        for (id, maybe_bytes) in to_delete {
            if self.delete_tx(id, Some(&mut *tx)).await? {
                affected += 1;
                if wants_records {
                    if let Some(b) = maybe_bytes {
                        removed_with_bytes.push(b);
                    }
                }
            }
        }

        // Build RETURNING rows. Each removed record's bytes are de-interned
        // via the base interner (always safe — these are committed rows) and
        // wrapped as `InsertedRecord { id: None, fields }` so the wire shape
        // matches UPDATE-RETURNING. When `op.select.fields` is set, each row
        // is restricted to the named fields.
        let mut result_records: Vec<InsertedRecord> =
            tvec!("engine/write_exec/delete_result_records", 0);
        if wants_records {
            let projection = op.select.as_ref().and_then(|s| s.fields.as_deref());
            result_records.reserve(removed_with_bytes.len());
            for bytes in &removed_with_bytes {
                let view = RecordView::new(bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "execute_delete_tx: RecordView for returning: {e}"
                    ))
                })?;
                let qv = record_view_to_query_value(&view, interner)?;
                let projected = project_query_value(qv, projection);
                result_records.push(InsertedRecord {
                    id: None,
                    fields: projected,
                });
            }
        }

        Ok(WriteResult {
            affected,
            records: result_records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware SET (upsert).
    ///
    /// Locates an existing record by key fields, then either merges-updates
    /// (existing) or inserts (new record). The MERGE branch is tree-free
    /// (W3d, the SET counterpart of W3c's `execute_update_tx` cutover):
    ///
    /// - the existing record's raw storage bytes are read once via the
    ///   tx-aware byte read (`read_one_tx_bytes`);
    /// - the merge runs through `merge_storage_bytes(old_bytes, set_map)` —
    ///   byte-identical to `merge_inner_maps(existing, set_map).to_bytes()`;
    /// - change-detection is a byte compare (`new_bytes == old_bytes`);
    /// - index ops are planned through `update_tx_bytes` (zero-copy
    ///   `RecordView` lenses for old/new — no `InnerValue` tree decode);
    /// - validators run through `run_validators_qv` with `old_qv` de-interned
    ///   from the old bytes via `RecordView`, and `new_qv = old_qv + resolved
    ///   value overlay` (string-keyed — sidesteps de-interning overlay-minted
    ///   key ids, the W3c keystone pattern);
    /// - result is `record_view_to_query_value(old_view) + resolved value
    ///   overlay` wrapped as `InsertedRecord::Direct` (same keystone).
    ///
    /// The INSERT branch encodes the value via `query_value_to_storage_bytes`
    /// and stages it through `insert_tx_many_bytes` — the same tree-free
    /// machinery `execute_insert_tx` uses (no `InnerValue` record tree).
    pub async fn execute_set_tx(
        &self,
        op: &SetOp,
        tx: &mut shamir_tx::TxContext,
        resolver: Option<&dyn crate::query::TableResolver>,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields in the value first (fail-closed).
        let mut resolved_value = resolve_computed_record(&op.value, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;

        // ③.2d UPSERT-path: apply transforms with is_insert=true.
        // `op.value` is a FULL record (insert-semantics): AutoNow + AutoNowAdd
        // both apply.  For the MERGE branch this means AutoNowAdd may write
        // `created_at` into the set-map, which then merges over the old record.
        // TODO(③.2d): UPSERT MERGE created_at: if the existing record already
        // has created_at and the caller did not supply one, the absence-guard in
        // AutoNowAdd will stamp it into the set-map, overwriting the old value.
        // A correct fix requires knowing at transform-time whether we are in the
        // INSERT or MERGE branch — which we can't know until after the key lookup.
        // For now we use is_insert=true (full-record semantics per brief) and
        // accept that UPSERT MERGE may inadvertently overwrite created_at when
        // the caller omits it. Track and fix in a follow-up if needed.
        let transforms = self.schema_transforms();
        if !transforms.is_empty() {
            let now_ns: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            apply_transforms(
                resolved_value.to_mut(),
                &transforms,
                builtin_scalars(),
                now_ns,
                true,
            );
        }

        // Intern field names through the tx overlay, then release the
        // immutable borrow on `tx` before the mutable staging calls. Two
        // artefacts come out of this block:
        //   * `key_fields` — TINY scalar InnerValue keys for the lookup
        //     (`lookup_existing_for_set` takes `(Vec<u64>, InnerValue)`; these
        //      are scalar key values, NOT the record tree — out of scope).
        //   * `set_map` — the interned-key InnerValue overlay that
        //     `merge_storage_bytes` patches onto the old bytes (the same shape
        //     W3c's update path builds).
        let (key_fields, set_map, new_bytes_fresh) = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);

            let key_fields: Vec<(Vec<u64>, InnerValue)> = match &op.key {
                Value::Map(map) => {
                    let mut fields = tvec!("engine/write_exec/set_key_fields", map.len());
                    for (k, v) in map {
                        let key_id = layered.touch_sync(k.as_str());
                        let inner_v = query_value_to_inner_with(v, &intern_fn)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        fields.push((vec![key_id], inner_v));
                    }
                    fields
                }
                _ => {
                    return Err(shamir_storage::error::DbError::Validation(
                        "SET key must be a map object".to_string(),
                    ))
                }
            };

            let new_inner = query_value_to_inner_with(&resolved_value, &intern_fn)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
            let set_map = match &new_inner {
                InnerValue::Map(m) => m.clone(),
                _ => {
                    return Err(shamir_storage::error::DbError::Validation(
                        "SET value must be a map object".to_string(),
                    ))
                }
            };

            // INSERT-branch bytes: encode the resolved value directly via the
            // tree-free storage encoder (same call `execute_insert_tx` makes).
            let new_bytes_fresh = query_value_to_storage_bytes(&resolved_value, &intern_fn)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;

            (key_fields, set_map, new_bytes_fresh)
        };

        let found = self
            .lookup_existing_for_set(&key_fields, batch_size)
            .await?;

        let result_record = if let Some((id, _existing)) = found {
            // ----- MERGE branch (tree-free, mirrors W3c execute_update_tx) -----
            //
            // Read the existing record's raw storage bytes through the tx-aware
            // byte read. For the common implicit-tx path this returns the
            // committed bytes `lookup_existing_for_set` matched; for an
            // interactive tx it also reflects prior tx-staged writes to the
            // same record (read-your-writes — strictly safer than the old
            // committed-only tree lookup).
            let old_bytes = self
                .read_one_tx_bytes(id, Some(&*tx))
                .await?
                .ok_or_else(|| {
                    shamir_storage::error::DbError::NotFound(format!(
                        "execute_set_tx: existing record vanished before merge: {id:?}"
                    ))
                })?;

            // Byte-level merge: patch old_bytes with the interned set overlay.
            // Produces bytes identical to merge_inner_maps(existing, set_map).to_bytes().
            let new_bytes = merge_storage_bytes(&old_bytes, &set_map)?;

            // Change detection: a no-op set yields identical bytes (the merge
            // copies old value spans verbatim when no key is patched), so byte
            // equality is safe for this "patched from old" shape.
            let changed = new_bytes.as_ref() != old_bytes.as_ref();

            if changed {
                // S3: run validators before staging (the W3c keystone pattern).
                //
                // Build old/new QueryValues WITHOUT decoding an InnerValue tree:
                // - old_qv: de-intern the committed bytes via RecordView (base
                //   interner — all keys are committed).
                // - new_qv: overlay the string-keyed resolved value on top of
                //   old_qv (no overlay-minted interned ids to resolve — the
                //   overlay keys are already strings in resolved_value).
                let old_view = RecordView::new(&old_bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "execute_set_tx: RecordView for validator old: {e}"
                    ))
                })?;
                let old_qv = record_view_to_query_value(&old_view, interner)?;
                let new_qv = {
                    let mut m = match old_qv.clone() {
                        Value::Map(m) => m,
                        _ => shamir_types::types::common::new_map(),
                    };
                    if let Value::Map(overlay) = resolved_value.as_ref() {
                        for (k, v) in overlay {
                            m.insert(k.clone(), v.clone());
                        }
                    }
                    Value::Map(m)
                };
                self.run_validators_qv(
                    WriteOp::Upsert,
                    Some(&new_qv),
                    Some(&old_qv),
                    &Actor::System,
                    Some(tx),
                    resolver,
                )
                .await
                .map_err(validator_failure_to_db_error)?;

                // Stage the merged bytes + index ops through the zero-copy
                // lens path (no InnerValue tree decode, no value.to_bytes()).
                self.update_tx_bytes(id, &old_bytes, new_bytes, &mut *tx)
                    .await?;
            }

            // Result: old record (base-interned keys — always safe to
            // decode) overlaid with the string-keyed resolved SET fields.
            // Decoding new_bytes via the base interner would fail when op.value
            // introduces brand-new field names interned only into the tx overlay.
            let old_view = RecordView::new(&old_bytes).map_err(|e| {
                shamir_storage::error::DbError::Codec(format!(
                    "execute_set_tx: RecordView for result: {e}"
                ))
            })?;
            let base_qv = record_view_to_query_value(&old_view, interner)?;
            let mut m = match base_qv {
                Value::Map(m) => m,
                _ => shamir_types::types::common::new_map(),
            };
            if let Value::Map(overlay) = resolved_value.as_ref() {
                for (k, v) in overlay {
                    m.insert(k.clone(), v.clone());
                }
            }
            m.insert("_created".to_string(), QueryValue::Bool(false));
            InsertedRecord {
                id: None,
                fields: QueryValue::Map(m),
            }
        } else {
            // ----- INSERT branch (tree-free, mirrors execute_insert_tx) -----
            //
            // Validators via the QueryValue entry (no InnerValue round-trip);
            // the resolved value IS the new record.
            self.run_validators_qv(
                WriteOp::Upsert,
                Some(resolved_value.as_ref()),
                None,
                &Actor::System,
                Some(tx),
                resolver,
            )
            .await
            .map_err(validator_failure_to_db_error)?;

            // Stage the pre-encoded bytes through the lens-driven batch insert
            // (single-element slice). No InnerValue record tree is built.
            let ids = self
                .insert_tx_many_bytes(std::slice::from_ref(&new_bytes_fresh), tx)
                .await?;
            let id = ids.into_iter().next().ok_or_else(|| {
                shamir_storage::error::DbError::Internal(
                    "execute_set_tx: insert_tx_many_bytes returned no id".to_string(),
                )
            })?;
            // Result from the original resolved value (string-keyed — no
            // overlay-id reverse lookup). For map values, overlay _created
            // directly; for non-map, wrap in {_value, _created}.
            let mut m = match resolved_value.as_ref() {
                Value::Map(fields) => fields.clone(),
                other => {
                    let mut wrap = shamir_types::types::common::new_map();
                    wrap.insert("_value".to_string(), other.clone());
                    wrap
                }
            };
            m.insert("_created".to_string(), QueryValue::Bool(true));
            InsertedRecord {
                id: Some(id),
                fields: QueryValue::Map(m),
            }
        };

        Ok(WriteResult {
            affected: 1,
            records: vec![result_record],
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}

/// Build the `Vec<InsertedRecord>` result for an INSERT response.
///
/// Returns `Direct` variants — no per-row map allocation needed.
/// The serialiser emits the same msgpack map shape as the old legacy path.
fn build_insert_result_records(
    resolved_values: &[std::borrow::Cow<'_, QueryValue>],
    ids: &[RecordId],
) -> Vec<InsertedRecord> {
    resolved_values
        .iter()
        .zip(ids.iter())
        .map(|(value, id)| InsertedRecord {
            id: Some(*id),
            fields: (**value).clone(),
        })
        .collect()
}

/// Apply an optional field projection to a `QueryValue::Map`.
///
/// When `fields` is `None` or the value is not a map, `value` is returned
/// unchanged. When `fields` is `Some(names)`, a new map is built containing
/// only the entries whose key is in `names`, preserving the projection's
/// order (callers typically ask for a stable, predictable wire shape). Keys
/// absent from the input map are silently dropped — RETURNING a missing
/// field is not an error, it is just absent from the row.
///
/// Used by INSERT / DELETE RETURNING to honour `InsertSelect.fields` /
/// `DeleteSelect.fields`.
fn project_query_value(value: QueryValue, fields: Option<&[String]>) -> QueryValue {
    let Some(names) = fields else {
        return value;
    };
    let Value::Map(src) = value else {
        return value;
    };
    let mut out: shamir_types::types::common::TMap<String, Value<String>> =
        shamir_types::types::common::new_map();
    for name in names {
        if let Some(v) = src.get(name) {
            out.insert(name.clone(), v.clone());
        }
    }
    QueryValue::Map(out)
}
