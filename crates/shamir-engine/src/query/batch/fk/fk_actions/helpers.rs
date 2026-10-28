use bytes::Bytes;
use futures::StreamExt;
use std::collections::BTreeSet;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::record_view::{scalar_ref_numeric_eq, RecordRef, RecordView, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
use crate::repo::{build_reverse_fk_entries, ReverseFkEntry};
use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

/// Check whether a field is nullable per the table's schema validators.
pub(super) async fn field_is_nullable(table: &TableManager, field: &str) -> bool {
    let bindings = table.validator_bindings();
    let registry = match table.validator_registry_ref() {
        Some(r) => r,
        None => return false, // No registry → can't confirm nullable → fail closed.
    };

    for binding in bindings.iter() {
        let validator = match registry.get_by_id(&binding.validator_id) {
            Some(v) => v,
            None => continue,
        };
        if let Some(nullable) = validator.nullable_for_field(field) {
            return nullable;
        }
    }

    // No validator declared this field → assume NOT nullable (fail closed).
    false
}

/// Produce a new storage-bytes map with `field_id` set to Null.
pub(super) fn null_out_field(
    old_bytes: &Bytes,
    field_id: u64,
) -> Result<Bytes, shamir_storage::error::DbError> {
    let mut tree = InnerValue::from_bytes(old_bytes.clone()).map_err(|e| {
        shamir_storage::error::DbError::Codec(format!("fk_actions null_out_field decode: {e}"))
    })?;
    if let InnerValue::Map(ref mut m) = tree {
        let key = shamir_types::core::interner::InternerKey::new(field_id);
        m.insert(key, InnerValue::Null);
    }
    tree.to_bytes().map_err(|e| {
        shamir_storage::error::DbError::Codec(format!("fk_actions null_out_field encode: {e}"))
    })
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// Discover all child tables with Cascade or SetNull FKs pointing at
/// `parent_table`.
///
/// F-28 Step 4 (#831): reads through the repo's cached reverse-FK map
/// (`RepoInstance::fk_reverse_cache`) instead of re-running the O(tables)
/// `collect_fk_refs()` scan on every call/cascade-recursion-level — a cache
/// miss (never built, or invalidated by a schema mutation since the last
/// build) pays that scan exactly once and seeds every parent table's entry
/// in the repo, not just this one. Shares the SAME cache (and the same
/// underlying scan) `fk_restrict.rs`'s `discover_restrict_refs` populates —
/// the cache carries every `on_delete` action; each caller filters what it
/// needs.
pub(super) async fn discover_action_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
) -> Result<Vec<ReverseFkEntry>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_actions: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_actions".to_string()),
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
            message: format!("fk_actions: reverse-FK scan failed: {e}"),
            code: Some("fk_actions".to_string()),
        })?
        .by_parent(&parent_table_ref.table);

    let is_self_ref_table = &parent_table_ref.table;
    Ok(all_for_parent
        .iter()
        .filter(|e| {
            if !(e.on_delete == FkAction::Cascade || e.on_delete == FkAction::SetNull) {
                return false;
            }
            // Self-referential CASCADE is not supported by the table-name
            // cascade cycle-guard (it would false-positive as a cycle even
            // for the shallowest self-ref case); it is rejected at DDL time
            // (`validate_no_self_referential_cascade`) and skipped here as
            // defense-in-depth for any pre-existing schema predating that
            // check. Self-referential SET NULL is safe — it is a
            // single-level mutation that never recurses (see the
            // "Recurse for grandchildren (Cascade only)" gate below).
            let is_self_ref = &e.child_table == is_self_ref_table;
            !(is_self_ref && e.on_delete == FkAction::Cascade)
        })
        .cloned()
        .collect())
}

// ── Scan helpers (mirrors fk_restrict.rs) ────────────────────────────────────

/// Scan the parent table for rows matching `where_clause` and extract the
/// DISTINCT values of the given `ref_fields`.
///
/// Returns a map: `field_name -> TFxSet<QueryValue>` — deduped, so that many
/// matched parent rows sharing the same ref_field value collapse to one
/// downstream probe fed to `classify_row` instead of one per row.
pub(super) async fn collect_parent_values(
    table: &TableManager,
    where_clause: &Filter,
    ctx: &FilterContext<'_>,
    ref_fields: &[&str],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<
    shamir_collections::TFxMap<String, shamir_collections::TFxSet<QueryValue>>,
> {
    let interner = table.interner().get().await?;
    let callback = compile_filter(where_clause, interner);
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

    // F-28 Step 2: tx-aware scan — overlays this tx's own staged writes so a
    // parent row deleted/updated EARLIER in the same tx is correctly hidden/
    // reflected (read-your-own-writes), closing D1 for this probe.
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
                    // dropping this row's value from the CASCADE/SET NULL
                    // candidate set (decode-error twin of the F-65/#891
                    // read-error fix — see `extract_ref_field_value`'s doc).
                    if let Some(qv) = extract_ref_field_value(&bytes, path)? {
                        #[expect(
                            clippy::unwrap_used,
                            reason = "guard: result entry is pre-populated for every field in ref_fields"
                        )]
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
/// fail on the raw bytes: a genuinely corrupt row must abort the cascade
/// scan, not be silently dropped from the candidate value set — the
/// decode-error twin of the F-65/#891 read-error fail-closed fix (see
/// `fk_indexed_action_read_error_tests.rs`).
pub(super) fn extract_ref_field_value(
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
            "fk_actions: row decode failed (RecordView and InnerValue both failed): {e}"
        ))),
    }
}

pub(super) fn cow_to_bytes(cow: RecordCow) -> shamir_storage::error::DbResult<Bytes> {
    match cow {
        RecordCow::Borrowed(b) => Ok(b),
        RecordCow::Owned(tree) => tree.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("fk_actions scan serialize: {e}"))
        }),
    }
}

fn filter_matches(bytes: &Bytes, callback: &FilterNode, ctx: &FilterContext<'_>) -> bool {
    match RecordView::new(bytes) {
        Ok(view) => callback.matches(&view, ctx),
        Err(_) => {
            if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                callback.matches(&tree, ctx)
            } else {
                false
            }
        }
    }
}

fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    // Delegate to the EXACT numeric equality predicate (`scalar_ref_numeric_eq`,
    // RV-FUNCLIB-5FA455F7 S2) — Int↔F64(1.0) matches, Int↔Big is exact beyond
    // 2^53 (the old lossy `scalar_ref_cmp_qv` collapsed distinct integers
    // through `as f64`), NaN matches nothing.
    scalar_ref_numeric_eq(*actual, value)
}

/// Row decoded once for testing against multiple probes — see `decode_row`.
enum DecodedRow<'a> {
    View(RecordView<'a>),
    Tree(InnerValue),
}

impl DecodedRow<'_> {
    fn scalar_at(
        &self,
        path: &[shamir_types::core::interner::InternerKey],
    ) -> Option<ScalarRef<'_>> {
        match self {
            DecodedRow::View(v) => v.scalar_at(path),
            DecodedRow::Tree(t) => t.scalar_at(path),
        }
    }
}

/// Decode `bytes` once via `RecordView`, falling back to `InnerValue` for
/// non-map records — shared across every probe tested against the SAME row
/// (closes the redundant per-probe re-decode `classify_row` used to do by
/// calling a by-value decode helper once per probe for one row).
fn decode_row(bytes: &[u8]) -> Option<DecodedRow<'_>> {
    if let Ok(view) = RecordView::new(bytes) {
        return Some(DecodedRow::View(view));
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(bytes)) {
        return Some(DecodedRow::Tree(tree));
    }
    None
}

/// F-53c: per-row action resolution shared by the index fast-path and the
/// `list_stream_tx` scan fallback in `plan_cascade_recursive` and
/// `plan_cascade_for_ids`. Mutates `cascade_ids` / `setnull_ids` exactly as
/// the original inline loop did: Cascade dominates SetNull (a deleted row
/// can't be nulled). Extracting this into a shared helper guarantees the
/// two paths produce byte-for-byte identical results whether or not a
/// supporting index exists on the FK column.
pub(super) fn classify_row(
    id: RecordId,
    bytes: &[u8],
    resolved_probes: &[(u64, &QueryValue, FkAction, &str)],
    cascade_ids: &mut Vec<RecordId>,
    setnull_ids: &mut Vec<(RecordId, String)>,
) {
    // Decode the row ONCE regardless of how many probes it's tested
    // against — the previous by-value helper re-decoded these SAME bytes
    // from scratch on every probe iteration below.
    let Some(row) = decode_row(bytes) else {
        // Corrupt row: every probe below would already have failed its own
        // decode identically, so "no match for this row" is the same
        // outcome as before.
        return;
    };
    // Check each probe against this row.  Cascade dominates SetNull
    // (a deleted row can't be nulled).
    let mut row_action: Option<FkAction> = None;
    let mut row_setnull_field: Option<String> = None;
    for (field_id, value, action, field_name) in resolved_probes {
        let key = shamir_types::core::interner::InternerKey::new(*field_id);
        let path = std::slice::from_ref(&key);
        let matches = row
            .scalar_at(path)
            .is_some_and(|actual| scalar_ref_matches_qv(&actual, value));
        if matches {
            match action {
                FkAction::Cascade => {
                    row_action = Some(FkAction::Cascade);
                    break;
                }
                FkAction::SetNull if row_action.is_none() => {
                    row_action = Some(FkAction::SetNull);
                    row_setnull_field = Some((*field_name).to_string());
                }
                _ => {}
            }
        }
    }
    match row_action {
        Some(FkAction::Cascade) => cascade_ids.push(id),
        Some(FkAction::SetNull) => {
            if let Some(f) = row_setnull_field {
                setnull_ids.push((id, f));
            }
        }
        _ => {}
    }
}

/// F-53c: index fast-path candidate gathering, mirroring
/// `fk_restrict::child_has_reference`'s index lookup exactly.
///
/// For each `(field, value)` probe, resolves `field` to its BASE interner id,
/// checks for a single-field index on it (via `find_single_field_index`, the
/// SAME check `child_has_reference` uses for RESTRICT), and looks up the
/// committed RecordIds whose indexed value equals `value`. Returns the UNION
/// of all probes' candidate IDs (deduped) when EVERY probe is index-covered,
/// or `None` as soon as any probe's field lacks a usable single-field index
/// (or any value is `Null` / a non-indexable scalar) — signaling the caller
/// to fall back to the tx-aware `list_stream_tx` scan. `Null` is deliberately
/// treated as non-indexable so the fast-path and scan-fallback produce
/// identical results even for a (theoretically) nullable referenced field.
///
/// The returned IDs reflect COMMITTED state only — the index is maintained
/// at commit, never mid-tx. The caller MUST additionally gate on "no staged
/// writes to this child table" before treating the set as complete; staged
/// inserts/updates/deletes are invisible to the index and are correctly
/// handled only by the scan fallback. The caller MUST have already recorded
/// the RI-barrier token at entry (before this call), preserving the F-40b
/// mutual-serialization ordering exactly as `child_has_reference` does.
pub(super) async fn index_candidate_ids(
    table: &TableManager,
    interner: &shamir_types::core::interner::Interner,
    probes: &[(&str, &QueryValue)],
) -> shamir_storage::error::DbResult<Option<Vec<RecordId>>> {
    let mut all: BTreeSet<RecordId> = BTreeSet::new();
    for (field, value) in probes {
        let inner = match qv_scalar_to_inner(value) {
            Some(InnerValue::Null) | None => return Ok(None),
            Some(iv) => iv,
        };
        let base_field_id = match interner.get_ind(field) {
            Some(id) => id,
            None => return Ok(None),
        };
        let field_path = [base_field_id.id()];
        let idx_name = match table.find_single_field_index(&field_path) {
            Some(n) => n,
            None => return Ok(None),
        };
        let ids = match table
            .index_manager_ref()
            .lookup_by_index(idx_name, std::slice::from_ref(&inner))
            .await?
        {
            // P0-3a (#1011): `None` = a DROP of this index is in its drain→sweep
            // window — the index is momentarily unusable. Bail to a full scan
            // (`Ok(None)`) rather than treating it as "no matches", which would
            // silently drop FK-referenced rows from the candidate set.
            Some(ids) => ids,
            None => return Ok(None),
        };
        all.extend(ids.iter().copied());
    }
    Ok(Some(all.into_iter().collect()))
}

/// F-53c: index fast-path helper — converts a probe's `QueryValue` to its
/// indexable `InnerValue` form. Mirrors the identical helper in
/// `fk_restrict.rs` / `fk_on_update.rs`.
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

/// F-28 Step 2: resolve `field` to its interner id, checking `base` first
/// and then this tx's `interner_overlay` — mirrors
/// `ValidatorDb::exists_in_self`'s staged-writes step (and
/// `shamir_tx::LayeredInterner::get_id`, reimplemented inline here since
/// `LayeredInterner` borrows `&TxContext` fields directly and this call site
/// only has `tx: &shamir_tx::TxContext` on hand, not a pre-built wrapper).
///
/// A child/grandchild row inserted or updated EARLIER IN THE SAME tx may use
/// a field name that has never been touched outside this tx — such a name is
/// allocated only in `tx.interner_overlay`, never in `base`, until commit
/// merges it. A base-only lookup would silently return `None` for it even
/// though `list_stream_tx` already made the staged row itself visible,
/// causing every probe against that field to falsely report "no match".
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

fn scalar_ref_to_qv(sr: ScalarRef<'_>) -> QueryValue {
    match sr {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(b),
        ScalarRef::Int(i) => QueryValue::Int(i),
        ScalarRef::F64(f) => QueryValue::F64(f),
        ScalarRef::Str(s) => QueryValue::Str(s.to_owned()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.to_vec()),
    }
}
