use super::plan::OnUpdateRef;
use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
use crate::repo::build_reverse_fk_entries;
use crate::table::TableManager;

use super::helpers::{
    cow_to_bytes, filter_matches, qv_scalar_to_inner, record_field_matches_any_by_id,
    record_field_matches_by_id, scalar_ref_to_qv,
};

// ── Discovery ────────────────────────────────────────────────────────────────

/// Discover all child tables in the repo that have a FK pointing at
/// `parent_table` with `on_update != NoAction`, already narrowed to the
/// references relevant to THIS update's `set_fields`.
///
/// (#1181): reads through the repo's cached reverse-FK map
/// (`RepoInstance::fk_reverse_cache`) instead of re-running the O(tables)
/// `repo.list_table_names()` + `collect_fk_refs()` scan on every UPDATE —
/// mirrors `fk_actions::discover_action_refs` / `fk_restrict::discover_restrict_refs`,
/// sharing the SAME cache (and the same underlying scan): a cache miss (never
/// built, or invalidated by a schema mutation since the last build) pays that
/// scan exactly once and seeds every parent table's entry in the repo, not
/// just this one. The cache carries every `on_update` action; this filters
/// for what the UPDATE path needs.
///
/// The set-fields membership check (`set_fields.contains_key`) is applied as
/// the FIRST predicate in the filter chain, before the `on_update` check and
/// before allocating an owned [`OnUpdateRef`] (three `String` clones) — for
/// the common no-op update (touching no FK-referenced field), every cached
/// entry is rejected by this cheap local-map lookup alone.
pub(crate) async fn discover_on_update_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    set_fields: &shamir_collections::TFxMap<String, QueryValue>,
) -> Result<Vec<OnUpdateRef>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_on_update: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_on_update".to_string()),
        })?;

    let repo_name = parent_table_ref.repo.clone();
    let all_for_parent = repo
        .fk_reverse_cache()
        .get_or_build_snapshot(|| {
            // F-36: `Fn` (re-callable) build closure so a
            // generation-mismatched scan can retry under the same
            // single-flight lock. Clone `repo_name` per call.
            let repo_name = repo_name.clone();
            async move { build_reverse_fk_entries(resolver, &repo_name).await }
        })
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_on_update: reverse-FK scan failed: {e}"),
            code: Some("fk_on_update".to_string()),
        })?
        .by_parent(&parent_table_ref.table);

    Ok(all_for_parent
        .iter()
        .filter(|e| set_fields.contains_key(&e.parent_ref_field))
        .filter(|e| e.on_update != FkAction::NoAction)
        .map(|e| OnUpdateRef {
            child_table: e.child_table.clone(),
            child_field: e.child_field.clone(),
            parent_ref_field: e.parent_ref_field.clone(),
            action: e.on_update,
        })
        .collect())
}

// ── Scan helpers (mirrors fk_restrict.rs / fk_actions.rs) ────────────────────

/// Scan the parent table for rows matching `where_clause` and extract the
/// DISTINCT values of the given `ref_fields`.
///
/// `None` for `where_clause` means match-all (mirrors `execute_update_tx`'s
/// treatment of an absent WHERE).
///
/// Returns a map: `field_name -> TFxSet<QueryValue>` — deduped, so that many
/// matched parent rows sharing the same ref_field value collapse to one
/// downstream probe instead of one per row (previously this pushed every
/// matched row's value into a `Vec`, including duplicates, despite the doc
/// claiming "distinct values").
pub(super) async fn collect_parent_values(
    table: &TableManager,
    where_clause: Option<&Filter>,
    ctx: &FilterContext<'_>,
    ref_fields: &[&str],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<
    shamir_collections::TFxMap<String, shamir_collections::TFxSet<QueryValue>>,
> {
    let interner = table.interner().get().await?;
    let callback = where_clause
        .map(|f| compile_filter(f, interner))
        .unwrap_or(FilterNode::True);
    let batch_size = 1000;

    let mut result: shamir_collections::TFxMap<String, shamir_collections::TFxSet<QueryValue>> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        result.insert(field.to_string(), shamir_collections::TFxSet::default());
    }

    // F-28 Step 2: resolve each ref_field's id through the tx-layered
    // interner (base, then this tx's `interner_overlay`) once, up front —
    // a ref_field touched for the first time earlier in this SAME tx only
    // exists in the overlay until commit; a base-only lookup would silently
    // drop it from every matched row's extracted values.
    let mut field_ids: shamir_collections::TFxMap<&str, u64> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        if let Some(id) = resolve_field_id_layered(interner, tx, field).await {
            field_ids.insert(field, id);
        }
    }

    // F-28 Step 2: tx-aware scan — overlays this tx's own staged writes so an
    // old parent value changed/removed EARLIER in the same tx is correctly
    // hidden/reflected (read-your-own-writes), closing D1 for this probe.
    let stream = table.list_stream_tx(Some(tx), batch_size);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_id, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;

            if !filter_matches(&bytes, &callback, ctx) {
                continue;
            }

            for &field in ref_fields {
                if let Some(&field_id) = field_ids.get(field) {
                    let key = shamir_types::core::interner::InternerKey::new(field_id);
                    let path = std::slice::from_ref(&key);
                    // Decode-corrupt row: fail closed instead of silently
                    // dropping this row's value from the RESTRICT/CASCADE/
                    // SET NULL candidate set (decode-error twin of the
                    // F-65/#891 read-error fix — see
                    // `extract_ref_field_value`'s doc).
                    if let Some(qv) = extract_ref_field_value(&bytes, path)? {
                        result.get_mut(field).unwrap().insert(qv);
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Decode a row's bytes (`RecordView`, falling back to `InnerValue` for
/// non-map records — same two-step decode `record_field_matches_by_id`
/// uses) and extract the scalar at `path` as an owned `QueryValue`.
///
/// Returns `Ok(None)` when the record decodes fine but the field is absent
/// at `path` — a legitimate "this row doesn't have that field" outcome, not
/// corruption. Returns `Err(DbError::Codec)` only when BOTH decode attempts
/// fail on the raw bytes: a genuinely corrupt parent row must abort the
/// scan, not be silently dropped from the candidate value set — the
/// decode-error twin of the F-65/#891 read-error fail-closed fix (see
/// `fk_indexed_action_read_error_tests.rs`).
fn extract_ref_field_value(
    bytes: &Bytes,
    path: &[shamir_types::core::interner::InternerKey],
) -> shamir_storage::error::DbResult<Option<QueryValue>> {
    if let Ok(view) = RecordView::new(bytes) {
        view.get_with_err(path[0].clone())
            .map_err(|error| shamir_storage::error::DbError::Codec(error.to_string()))?;
        return Ok(view.scalar_at(path).map(scalar_ref_to_qv));
    }
    match InnerValue::from_bytes(bytes.clone()) {
        Ok(tree) => Ok(tree.scalar_at(path).map(scalar_ref_to_qv)),
        Err(e) => Err(shamir_storage::error::DbError::Codec(format!(
            "fk_on_update: parent row decode failed (RecordView and InnerValue \
             both failed): {e}"
        ))),
    }
}

/// Check whether any row in `child_table` has `field` equal to ANY member of
/// `values` — a single pass over the child table testing the whole (deduped)
/// old-value set for one FK field.
///
/// Replaces the previous per-value `child_has_reference`, which was called
/// once PER (old-value, child_field) pair from the RESTRICT branch above
/// and, whenever the field lacked a supporting index, ran a full
/// `list_stream_tx` scan of the child table EVERY time — O(distinct values ×
/// child-table size) instead of O(child-table size). The per-value INDEXED
/// lookup path is unchanged in shape (one `lookup_by_index` call per value —
/// already O(log n) each, not the fan-out this fixes); only the no-index
/// fallback collapses to exactly one scan, and each scanned row is decoded
/// exactly once and tested against every value instead of being
/// independently re-scanned-and-decoded once per value.
///
/// Mirrors `fk_restrict::any_child_references`: index-fast / scan-fallback,
/// PLUS (F-28 Step 2) a staged-overlay probe on the index fast-path's empty
/// result, since a staged-but-not-yet-committed insert/update is never
/// reflected in the index (indexing happens at commit).
pub(super) async fn any_child_references(
    table: &TableManager,
    field: &str,
    values: &[&QueryValue],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<bool> {
    // F-40b (RI barrier): record this child-table scan REGARDLESS of
    // isolation — mirrors `fk_restrict.rs::any_child_references`'s recording
    // at function entry, before the index fast-path, so an EXPLICIT
    // `Snapshot`-isolation parent UPDATE (which never populates
    // `predicate_set`) still gets a commit-time re-check (`pre_commit.rs`
    // Phase 2-bis) against concurrent committers that touched this child
    // table. Fires even when a supporting index short-circuits the scan
    // below.
    tx.record_ri_barrier(table.table_token());

    let interner = table.interner().get().await?;

    // F-28 Step 2: resolve the field id through the tx-layered interner
    // (base, then this tx's `interner_overlay`) — a field first interned
    // EARLIER IN THE SAME tx only exists in the overlay until commit; a
    // base-only lookup would silently miss it. Mirrors
    // `fk_restrict::any_child_references`.
    let field_id = resolve_field_id_layered(interner, tx, field).await;

    // Fast path: one `lookup_by_index` call per value (only meaningful for a
    // base id). Falls through to the single full-scan fallback below the
    // moment ANY value can't be resolved via the index — matching the old
    // per-value function's per-call behavior exactly.
    if let Some(base_field_id) = interner.get_ind(field) {
        let field_path = [base_field_id.id()];
        if let Some(idx_name) = table.find_single_field_index(&field_path) {
            let mut index_conclusive = true;
            for &value in values {
                let Some(inner_value) = qv_scalar_to_inner(value) else {
                    index_conclusive = false;
                    break;
                };
                // P0-3a (#1011): `None` = DROP in its drain→sweep window — the
                // index is momentarily unusable, so fall through to the full
                // scan below instead of treating it as "no match" (which would
                // wrongly let a FK-referencing child row pass the check).
                match table
                    .index_manager_ref()
                    .lookup_by_index(idx_name, std::slice::from_ref(&inner_value))
                    .await?
                {
                    Some(ids) => {
                        if !ids.is_empty() {
                            return Ok(true);
                        }
                        // Index says no match — authoritative for the COMMITTED
                        // store, but cannot rule out a same-tx staged insert/update.
                        if let Some(field_id) = field_id {
                            let key = shamir_types::core::interner::InternerKey::new(field_id);
                            if staged_field_matches(tx, table.table_token(), &key, value) {
                                return Ok(true);
                            }
                        }
                    }
                    None => {
                        index_conclusive = false;
                        break;
                    }
                }
            }
            if index_conclusive {
                return Ok(false);
            }
            // else: fall through to the full scan for correctness (it
            // harmlessly re-checks any value already ruled out via index).
        }
    }

    // Fallback: ONE full scan with field match against the WHOLE `values`
    // set — each row is decoded exactly once and tested against every
    // value, instead of once per value. Tx-aware: overlays this tx's own
    // staged writes (read-your-own-writes, closing D1 for this probe).
    //
    // F-28 Step 5 (memo §2.4): construct `list_stream_tx` UNCONDITIONALLY —
    // it records the Serializable `TableScan` predicate synchronously at
    // construction time (before this line returns), regardless of whether
    // `field_id` resolved. See the identical fix + rationale in
    // `fk_restrict.rs::any_child_references`.
    let batch_size = 1000;
    let stream = table.list_stream_tx(Some(tx), batch_size);
    let Some(field_id) = field_id else {
        return Ok(false);
    };
    let key = shamir_types::core::interner::InternerKey::new(field_id);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;
            if record_field_matches_any_by_id(bytes.as_ref(), &key, values) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// F-28 Step 2: resolve `field` to its interner id, checking `base` first
/// and then this tx's `interner_overlay`. See the identical helper + doc
/// comment in `fk_actions.rs` / `fk_restrict.rs` for the full rationale.
pub(super) async fn resolve_field_id_layered(
    base: &shamir_types::core::interner::Interner,
    tx: &shamir_tx::TxContext,
    field: &str,
) -> Option<u64> {
    let layered = shamir_tx::LayeredInterner::Layered {
        base,
        overlay: &tx.interner_overlay,
        next_overlay_id: &tx.next_overlay_id,
    };
    layered.get_id(field).await
}

/// F-28 Step 2: does this tx's own staged (uncommitted) write_set for
/// `target_token` contain a row where `field_id == value`? Mirrors
/// `fk_restrict::staged_field_matches` / `ValidatorDb::staged_field_matches`.
fn staged_field_matches(
    tx: &shamir_tx::TxContext,
    target_token: u64,
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let Some(staging) = tx.write_set.get(&target_token) else {
        return false;
    };
    // The field index narrows staged candidates; exact recheck preserves semantics.
    staging
        .probe_field(field_id, value)
        .iter()
        .any(|(_, bytes)| record_field_matches_by_id(bytes.as_ref(), field_id, value))
}
