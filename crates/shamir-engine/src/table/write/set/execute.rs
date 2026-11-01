use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::function::builtin_scalars;
use crate::query::write::{InsertedRecord, SetOp, WriteResult};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{
    merge_storage_bytes, query_value_to_inner_with, query_value_to_storage_bytes,
    record_view_to_query_value,
};
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

use crate::validator::WriteOp;
use shamir_types::access::Actor;

use crate::table::table_manager::TableManager;
use crate::table::write::write_helpers::{
    apply_transforms, intern_via_layered, make_layered_interner, resolve_computed_record,
    validator_failure_to_db_error,
};
impl TableManager {
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
        actor: &Actor,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields in the value first (fail-closed).
        let mut resolved_value = resolve_computed_record(&op.value, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;

        // Derive the scalar key fields from `op.key` and run the existing-record
        // lookup BEFORE applying transforms. `key_fields` depends only on
        // `op.key` (never on the transformed value), so the lookup can run
        // earlier without missing information. The MERGE-vs-INSERT decision it
        // returns then drives `is_insert` for `apply_transforms` below — this is
        // load-bearing because `AutoNowAdd` (created_at) and `ComputedDefault`
        // are INSERT-only: running them with is_insert=true on a MERGE would
        // stamp a fresh created_at into the merge overlay whenever the caller
        // omits it, silently overwriting the existing row's real creation time.
        let key_fields: Vec<(Vec<u64>, InnerValue)> = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);
            match &op.key {
                Value::Map(map) => {
                    let mut fields = Vec::with_capacity(map.len());
                    for (k, v) in map {
                        // RU8: propagate the typed quota error.
                        let key_id = layered
                            .try_touch_sync(k.as_str())
                            .map_err(shamir_storage::error::DbError::from)?;
                        let inner_v = query_value_to_inner_with(v, &intern_fn)
                            .map_err(shamir_storage::error::DbError::from)?;
                        fields.push((vec![key_id], inner_v));
                    }
                    fields
                }
                _ => {
                    return Err(shamir_storage::error::DbError::Validation(
                        "SET key must be a map object".to_string(),
                    ))
                }
            }
        };

        let found = self
            .lookup_existing_for_set(&key_fields, batch_size)
            .await?;

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
                found.is_none(),
            );
        }

        // Intern the (now correctly transformed) value's field names through
        // the tx overlay, then release the immutable borrow on `tx` before the
        // mutable staging calls. Two artefacts come out of this block:
        //   * `set_map` — the interned-key InnerValue overlay that
        //     `merge_storage_bytes` patches onto the old bytes (the same shape
        //     W3c's update path builds).
        //   * `new_bytes_fresh` — INSERT-branch bytes: the resolved value
        //     encoded directly via the tree-free storage encoder (same call
        //     `execute_insert_tx` makes).
        let (set_map, new_bytes_fresh, new_inner) = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);

            let new_inner = query_value_to_inner_with(&resolved_value, &intern_fn)
                .map_err(shamir_storage::error::DbError::from)?;
            let set_map = match &new_inner {
                InnerValue::Map(m) => m.clone(),
                _ => {
                    return Err(shamir_storage::error::DbError::Validation(
                        "SET value must be a map object".to_string(),
                    ))
                }
            };

            let new_bytes_fresh = query_value_to_storage_bytes(&resolved_value, &intern_fn)
                .map_err(shamir_storage::error::DbError::from)?;

            (set_map, new_bytes_fresh, new_inner)
        };
        // #1205: `new_inner` (built above, `layered`'s borrow of `tx` now
        // ended) is byte-identical to what `new_bytes_fresh` encodes AND is
        // the source `set_map` was cloned from — capturing its ids ONCE
        // here covers both the MERGE branch (`set_map` patched onto old
        // bytes below) and the INSERT branch (`new_bytes_fresh` staged
        // as-is), no extra decode needed.
        shamir_tx::collect_referenced_ids(&new_inner, &mut tx.referenced_interner_ids);

        // Check whether any Upsert validators are registered BEFORE the
        // merge/insert branch, mirroring `execute_update_tx`'s
        // `has_update_validators`. Both the MERGE and INSERT branches call
        // `run_validators_qv(WriteOp::Upsert, ...)` — gating on this avoids
        // the per-call de-intern when no Upsert validator is bound (audit
        // `2026-07-06-perf-hot-paths.md` §1.3).
        //
        // Note: this checks `WriteOp::Upsert` (NOT `WriteOp::Update`) because
        // both branches below invoke `run_validators_qv` with `WriteOp::Upsert`;
        // a `WriteOp::Update`-bound validator does NOT fire on the upsert path.
        let has_upsert_validators = {
            let bindings = self.validator_bindings();
            bindings.iter().any(|b| b.ops.contains(&WriteOp::Upsert))
        };

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

            // Track the validator-built `old_qv` so the result-record
            // construction below can REUSE it instead of de-interning
            // `old_bytes` a second time. Only populated when validators ran
            // (which implies `has_upsert_validators && changed`).
            let mut reuse_old_qv: Option<QueryValue> = None;

            if changed {
                if has_upsert_validators {
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
                        actor,
                        Some(tx),
                        crate::validator::ValidatorFkCtx {
                            resolver,
                            // SET (upsert) handles one record per call — no
                            // batch-scoped FK membership cache to share.
                            fk_cache: None,
                        },
                    )
                    .await
                    .map_err(validator_failure_to_db_error)?;

                    // Make the validator-built old_qv available for result
                    // reuse (avoid re-de-interning the same bytes below).
                    reuse_old_qv = Some(old_qv);
                }

                // Stage the merged bytes + index ops through the zero-copy
                // lens path (no InnerValue tree decode, no value.to_bytes()).
                // #1205: `new_bytes`'s new content (`set_map`) had its ids
                // captured above, before this branch.
                self.update_tx_bytes(id, &old_bytes, new_bytes, &mut *tx, true)
                    .await?;
            }

            // Result: old record (base-interned keys — always safe to
            // decode) overlaid with the string-keyed resolved SET fields.
            // Decoding new_bytes via the base interner would fail when op.value
            // introduces brand-new field names interned only into the tx overlay.
            //
            // REUSE the validator-built `old_qv` when validators ran (avoid a
            // second `RecordView::new` + de-intern of the SAME `old_bytes`).
            // Only de-intern here when the validator path was skipped (no
            // Upsert validators bound, or the row did not change).
            let base_qv = match reuse_old_qv.take() {
                Some(qv) => qv,
                None => {
                    let old_view = RecordView::new(&old_bytes).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "execute_set_tx: RecordView for result: {e}"
                        ))
                    })?;
                    record_view_to_query_value(&old_view, interner)?
                }
            };
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
                actor,
                Some(tx),
                crate::validator::ValidatorFkCtx {
                    resolver,
                    // SET (upsert) handles one record per call — no batch-scoped
                    // FK membership cache to share.
                    fk_cache: None,
                },
            )
            .await
            .map_err(validator_failure_to_db_error)?;

            // Stage the pre-encoded bytes through the lens-driven batch insert
            // (single-element slice). No InnerValue record tree is built.
            let ids = self
                .insert_tx_many_bytes(std::slice::from_ref(&new_bytes_fresh), tx)
                .await?;
            // #1205: `new_bytes_fresh`'s ids were captured above (same
            // `new_inner` tree `set_map` was cloned from).
            self.mark_staged_ids_captured(tx);
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
pub(in crate::table::write) fn build_insert_result_records(
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
pub(in crate::table::write) fn project_query_value(
    value: QueryValue,
    fields: Option<&[String]>,
) -> QueryValue {
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
