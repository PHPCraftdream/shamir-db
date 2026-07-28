//! Phase D.1 — `ON DELETE RESTRICT` gate for the batch delete path.
//!
//! Before a `BatchOp::Delete` dispatches to `execute_delete_tx`, this gate
//! checks whether any child table has a foreign-key constraint referencing the
//! parent table being deleted from with `on_delete = Restrict`.  If a child
//! row still references one of the parent rows about to be deleted, the
//! operation is rejected with `fk_restrict`.
//!
//! ## TOCTOU caveat
//!
//! F-28 Step 2 closes the **in-tx** read-your-own-writes gap (D1): the row
//! probes below (`collect_parent_values`, `child_has_reference`) read via
//! `TableManager::list_stream_tx(Some(tx), ..)`, which overlays the calling
//! transaction's own staged writes on top of the committed stream — a
//! staged-but-uncommitted delete of a child row is hidden, a staged insert is
//! injected. This closes the deterministic bug where a transactional
//! `[delete child; delete parent]` batch under `on_delete = Restrict` was
//! wrongly rejected because the check saw the child as still committed.
//!
//! ## Cross-transaction race — CLOSED (F-28 Step 5, S3-C)
//!
//! The **cross-transaction** race — a genuinely concurrent OTHER transaction
//! inserting a new child reference between this check and the eventual
//! commit of the parent delete — is now closed via targeted Serializable
//! isolation + SSI footprint widening (S3-C, decided in the Step 3 spike,
//! `docs/dev-artifacts/research/f28-s3-mechanism-decision.md`):
//!
//! - **Parent side**: `query_runner.rs`'s implicit DELETE arm checks
//!   `FkReverseCache::is_fk_parent_with_delete_action` and opens the implicit tx as
//!   `Serializable` (instead of `Snapshot`) when this table is an FK parent
//!   with a non-`NoAction` `on_delete` action. Under Serializable,
//!   `child_has_reference`'s `list_stream_tx` call above records a real
//!   `PredicateDep::TableScan` — including when the field id can't be
//!   resolved (the scan still runs; see that function's doc).
//! - **Child side**: the engine's insert/update/set staging path
//!   (`query_runner.rs`'s `require_footprint_if_fk_child`) calls
//!   `tx.require_footprint_for` on any write into a table flagged as an FK
//!   child, regardless of that writer's OWN isolation level — so a plain
//!   Snapshot insert into the child table still publishes a footprint the
//!   Serializable delete's Phase 2-bis phantom check can conflict against.
//!
//! With both sides wired, the race resolves to: the racing insert commits
//! first and the delete's commit aborts with `PhantomConflict` (surfaced as
//! `tx_conflict`, with a bounded internal retry for the common
//! already-resolved case — see `query_runner.rs`'s `retry_on_tx_conflict`);
//! or the delete's own check/commit wins the race and the insert (if it
//! runs after) observes the parent already gone. Neither interleaving can
//! leave "delete succeeded AND a dangling child reference exists" — see
//! `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
//! for the deterministic end-to-end proof.
//!
//! Residual scope: this closes the race for the **implicit** (autocommit)
//! delete path, which is where F-28's brief scoped the fix. An EXPLICIT
//! transaction that stays Snapshot for its own reasons does not get an
//! automatic upgrade here — a caller wanting Serializable protection for an
//! explicit tx opens it as `Serializable` itself (`begin_tx`), which this
//! same machinery already protects once wired.

use bytes::Bytes;
use futures::StreamExt;

use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_query_types::filter::Filter;
use shamir_types::record_view::{scalar_ref_cmp_qv, RecordRef, RecordView, ScalarRef};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::TableResolver;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};
use crate::query::TableRef;
use crate::repo::{build_reverse_fk_entries, ReverseFkEntry};
use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

/// Pre-delete RESTRICT gate.
///
/// Scans the repo for child tables whose schema declares a foreign key
/// referencing `parent_table` with `on_delete = Restrict`.  If any child row
/// still references a parent row matched by the delete's `where_clause`, the
/// delete is rejected.
///
/// Returns `Ok(())` to proceed with the delete, or `Err(BatchError)` to reject.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn check_fk_restrict(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
    parent_table: &TableManager,
    delete_where: &Filter,
    ctx: &FilterContext<'_>,
    alias: &str,
    tx: &shamir_tx::TxContext,
) -> Result<(), BatchError> {
    // 1. Discover reverse-FK references with Restrict.
    let restrict_refs = discover_restrict_refs(resolver, parent_table_ref).await?;
    if restrict_refs.is_empty() {
        return Ok(());
    }

    // 2. Collect the parent ref_field values from rows about to be deleted.
    let parent_ref_fields: Vec<&str> = {
        let mut fields: Vec<&str> = restrict_refs
            .iter()
            .map(|r| r.parent_ref_field.as_str())
            .collect();
        fields.sort_unstable();
        fields.dedup();
        fields
    };

    let parent_values =
        collect_parent_values(parent_table, delete_where, ctx, &parent_ref_fields, tx)
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: format!("fk_restrict: parent scan failed: {e}"),
                code: Some("fk_restrict".to_string()),
            })?;

    if parent_values.is_empty() {
        return Ok(());
    }

    // 3. For each restrict ref, check if any child row references one of the
    //    parent values.
    for rref in &restrict_refs {
        let values_for_field = match parent_values.get(rref.parent_ref_field.as_str()) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        let child_table_ref = TableRef::with_repo(&parent_table_ref.repo, &rref.child_table);
        let child_table =
            resolver
                .resolve(&child_table_ref)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!(
                        "fk_restrict: cannot resolve child table '{}': {e}",
                        rref.child_table
                    ),
                    code: Some("fk_restrict".to_string()),
                })?;

        for parent_val in values_for_field {
            let exists = child_has_reference(&child_table, &rref.child_field, parent_val, tx)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("fk_restrict: child scan failed: {e}"),
                    code: Some("fk_restrict".to_string()),
                })?;

            if exists {
                return Err(BatchError::query_coded(
                    alias,
                    "fk_restrict",
                    format!(
                        "cannot delete from '{}': row is still referenced by '{}.{}'",
                        parent_table_ref.table, rref.child_table, rref.child_field,
                    ),
                ));
            }
        }
    }

    Ok(())
}

/// Discover all child tables in the repo that have a FK pointing at
/// `parent_table` with `on_delete = Restrict`.
///
/// F-28 Step 4 (#831): reads through the repo's cached reverse-FK map
/// (`RepoInstance::fk_reverse_cache`) instead of re-running the O(tables)
/// `collect_fk_refs()` scan on every call — a cache miss (never built, or
/// invalidated by a schema mutation since the last build) pays that scan
/// exactly once and seeds every parent table's entry in the repo, not just
/// this one.
async fn discover_restrict_refs(
    resolver: &dyn TableResolver,
    parent_table_ref: &TableRef,
) -> Result<Vec<ReverseFkEntry>, BatchError> {
    let repo = resolver
        .resolve_repo(&parent_table_ref.repo)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_restrict: resolve_repo({}): {e}", parent_table_ref.repo),
            code: Some("fk_restrict".to_string()),
        })?;

    let repo_name = parent_table_ref.repo.clone();
    let all_for_parent = repo
        .fk_reverse_cache()
        .get_or_build_by_parent(&parent_table_ref.table, || {
            // F-36: `Fn` (re-callable) build closure so a
            // generation-mismatched scan can retry under the same
            // single-flight lock. Clone `repo_name` per call.
            let repo_name = repo_name.clone();
            async move { build_reverse_fk_entries(resolver, &repo_name).await }
        })
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("fk_restrict: reverse-FK scan failed: {e}"),
            code: Some("fk_restrict".to_string()),
        })?;

    Ok(all_for_parent
        .into_iter()
        .filter(|e| e.on_delete == FkAction::Restrict)
        .collect())
}

/// Scan the parent table for rows matching `where_clause` and extract the
/// values of the given `ref_fields`.
///
/// Returns a map: `field_name -> Vec<QueryValue>` (the set of distinct values
/// from the matched rows).
async fn collect_parent_values(
    table: &TableManager,
    where_clause: &Filter,
    ctx: &FilterContext<'_>,
    ref_fields: &[&str],
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<shamir_collections::TFxMap<String, Vec<QueryValue>>> {
    let interner = table.interner().get().await?;
    let callback = compile_filter(where_clause, interner);
    let batch_size = 1000;

    let mut result: shamir_collections::TFxMap<String, Vec<QueryValue>> =
        shamir_collections::TFxMap::default();
    for &field in ref_fields {
        result.insert(field.to_string(), Vec::new());
    }

    // F-28 Step 2: resolve each ref_field's id through the tx-layered
    // interner (base, then this tx's `interner_overlay`) once, up front —
    // same rationale as `child_has_reference`: a ref_field touched for the
    // first time earlier in this SAME tx only exists in the overlay until
    // commit, and a base-only lookup would silently drop it from every
    // matched row's extracted values.
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

            // Extract the ref_field values from the matched row.
            for &field in ref_fields {
                if let Some(&field_id) = field_ids.get(field) {
                    let key = shamir_types::core::interner::InternerKey::new(field_id);
                    let path = std::slice::from_ref(&key);
                    if let Ok(view) = RecordView::new(&bytes) {
                        if let Some(scalar) = view.scalar_at(path) {
                            let qv = scalar_ref_to_qv(scalar);
                            result.get_mut(field).unwrap().push(qv);
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Check whether any row in `child_table` has `field == value`.
///
/// Mirrors `ValidatorDb::exists_in_table` semantics: index lookup if covered,
/// else full scan with field match — PLUS (F-28 Step 2) a staged-overlay
/// probe of this tx's own `write_set` for the child table, since a
/// staged-but-not-yet-committed insert/update is never reflected in the index
/// (indexing happens at commit) and the fallback scan below is already
/// tx-aware via `list_stream_tx`.
async fn child_has_reference(
    table: &TableManager,
    field: &str,
    value: &QueryValue,
    tx: &shamir_tx::TxContext,
) -> shamir_storage::error::DbResult<bool> {
    // F-40b (RI barrier): record this child-table scan REGARDLESS of
    // isolation, so an EXPLICIT `Snapshot`-isolation parent delete — which
    // never records a Serializable `predicate_set` entry — still gets a
    // commit-time re-check (`pre_commit.rs` Phase 2-bis) against concurrent
    // committers that touched this child table. This closes the
    // cross-transaction FK TOCTOU race for the explicit-Snapshot path.
    //
    // Recorded at function ENTRY (before the index fast-path below) so it
    // fires even when a supporting index short-circuits the scan and
    // `list_stream_tx` (which records the Serializable `TableScan` predicate)
    // is never reached — a case the existing Serializable path itself misses
    // for a well-formed FK whose referenced field has a supporting index.
    tx.record_ri_barrier(table.table_token());

    let interner = table.interner().get().await?;

    // F-28 Step 2: resolve the field id through the tx-layered interner
    // (base, then this tx's `interner_overlay`) — a field first interned
    // EARLIER IN THE SAME tx (e.g. a brand-new child row's FK field, never
    // touched by this table before) only exists in the overlay until commit;
    // a base-only lookup would silently miss it and every probe below would
    // wrongly report "no match". Mirrors `ValidatorDb::exists_in_self`'s
    // staged-writes step.
    let field_id = resolve_field_id_layered(interner, tx, field).await;

    // Fast path: single-field index lookup (only meaningful for a base id —
    // an overlay-only id was never indexed and never will be pre-commit).
    if let Some(inner_value) = qv_scalar_to_inner(value) {
        if let Some(base_field_id) = interner.get_ind(field) {
            let field_path = [base_field_id.id()];
            if let Some(idx_name) = table.find_single_field_index(&field_path) {
                let ids = table
                    .index_manager_ref()
                    .lookup_by_index(idx_name, std::slice::from_ref(&inner_value))
                    .await?;
                if !ids.is_empty() {
                    return Ok(true);
                }
                // Index says no match — authoritative for the COMMITTED
                // store, but cannot rule out a same-tx staged insert/update
                // (never in the index until commit). Mirrors
                // `ValidatorDb::exists_in_table`'s staged-overlay fallback.
                if let Some(field_id) = field_id {
                    let key = shamir_types::core::interner::InternerKey::new(field_id);
                    return Ok(staged_field_matches(tx, table.table_token(), &key, value));
                }
                return Ok(false);
            }
        }
    }

    // Fallback: full scan with field match. Tx-aware: overlays this tx's own
    // staged writes so a staged insert/update/delete on the child table is
    // correctly reflected (read-your-own-writes, closing D1 for this probe).
    //
    // F-28 Step 5 (memo §2.4): `list_stream_tx` MUST run unconditionally here
    // — even when `field_id` is unresolvable — because it records the
    // Serializable `TableScan` predicate at CONSTRUCTION time (before the
    // stream is even polled; see `TableManager::list_stream_tx`). A
    // brand-new/empty child table whose FK field was never interned is
    // exactly the common case a RESTRICT/CASCADE check hits, and skipping
    // the scan entirely there would skip recording the predicate too — the
    // scan (and its predicate recording) must run; only the row-level
    // MATCH is skipped when the field id can't be resolved.
    let batch_size = 1000;
    // Construct the stream UNCONDITIONALLY — `list_stream_tx` records the
    // Serializable `TableScan` predicate synchronously at construction time,
    // before this line returns, regardless of whether `field_id` resolved or
    // whether the stream is ever polled below.
    let stream = table.list_stream_tx(Some(tx), batch_size);
    let Some(field_id) = field_id else {
        // Field doesn't exist anywhere (base or this tx's overlay) — no row,
        // committed or staged, can possibly reference it. The predicate is
        // already recorded (above); no row can match an unresolvable field,
        // so there is nothing left to poll.
        return Ok(false);
    };
    let key = shamir_types::core::interner::InternerKey::new(field_id);
    futures::pin_mut!(stream);
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for (_, cow) in batch {
            let bytes: Bytes = cow_to_bytes(cow)?;
            if record_field_matches_by_id(bytes.as_ref(), &key, value) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// F-28 Step 2: resolve `field` to its interner id, checking `base` first
/// and then this tx's `interner_overlay`. See the identical helper + doc
/// comment in `fk_actions.rs` for the full rationale (a field name first
/// interned earlier in the SAME tx only lives in `tx.interner_overlay` until
/// commit; a base-only lookup misses it even though the staged row using it
/// is already visible via `list_stream_tx`).
async fn resolve_field_id_layered(
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
/// `ValidatorDb::staged_field_matches` — needed here because the index
/// fast-path never sees a staged-but-uncommitted insert (indexing happens at
/// commit only).
fn staged_field_matches(
    tx: &shamir_tx::TxContext,
    target_token: u64,
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let Some(staging) = tx.write_set.get(&target_token) else {
        return false;
    };
    staging.snapshot_ops().into_iter().any(|op| match op {
        shamir_storage::types::KvOp::Set(_, ref bytes) => {
            record_field_matches_by_id(bytes.as_ref(), field_id, value)
        }
        shamir_storage::types::KvOp::Remove(_) => false,
    })
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn cow_to_bytes(cow: RecordCow) -> shamir_storage::error::DbResult<Bytes> {
    match cow {
        RecordCow::Borrowed(b) => Ok(b),
        RecordCow::Owned(tree) => tree.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("fk_restrict scan serialize: {e}"))
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

/// Keyed by an already-resolved `InternerKey` instead of a field name — the
/// field id must come from [`resolve_field_id_layered`] (base + this tx's
/// overlay), not a plain by-name interner lookup, since the matched row may
/// be tx-overlay-sourced (`list_stream_tx`) and use an overlay-only field id.
fn record_field_matches_by_id(
    record_bytes: &[u8],
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let path = std::slice::from_ref(field_id);
    if let Ok(view) = RecordView::new(record_bytes) {
        if let Some(actual) = view.scalar_at(path) {
            return scalar_ref_matches_qv(&actual, value);
        }
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(record_bytes)) {
        if let Some(actual) = tree.scalar_at(path) {
            return scalar_ref_matches_qv(&actual, value);
        }
    }
    false
}

fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    // Delegate to the cross-type-comparing `scalar_ref_cmp_qv` so that a parent
    // key stored as `Int(5)` matches a child FK field stored as `F64(5.0)` (and
    // vice-versa) — consistent with every other comparison layer in the engine.
    scalar_ref_cmp_qv(*actual, value) == Some(std::cmp::Ordering::Equal)
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
