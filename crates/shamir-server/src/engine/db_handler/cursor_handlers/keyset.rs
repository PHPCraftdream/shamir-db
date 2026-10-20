//! Keyset-bookmark plumbing: boundary filters, ORDER BY probing, keyset
//! page fetches and the tail-to-bookmark conversion.

use shamir_db::engine::query::filter::eval_context::FilterContext;
use shamir_db::engine::table::TableManager;
use shamir_db::engine::validator::schema::TypeTag;
use shamir_db::query::batch::BatchError;
use shamir_db::query::filter::Filter;
use shamir_db::query::read::{
    At, OrderDirection, Pagination, QueryRecord, QueryResult, ReadQuery, Temporal,
};
use shamir_query_types::filter::query_value_to_filter_value;
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::cursor_registry::PaginationMode;

use super::budget::wrap_engine_err;

/// Build the `FilterContext` a cursor's `FetchNext` reads through — mirrors
/// the non-tx bare-single-read shape `query_runner.rs` builds (empty
/// `resolved_refs`/`params`). Built engine-side (S5c-X F1, Round 6; default
/// `builtins_only()` scalar resolver — see the FG-5b scope note). The
/// returned borrow is tied to the MANAGER lifetime (not the temporary read view), so it composes with the caller's `table.read(...)`.
pub(super) async fn build_filter_context<'a>(
    table: &'a TableManager,
    resolved_refs: &'a TMap<String, shamir_db::query::read::QueryResult>,
) -> Result<FilterContext<'a>, BatchError> {
    table
        .info_store_read_view()
        .filter_context(resolved_refs)
        .await
        .map_err(wrap_engine_err)
}

/// Build the INCLUSIVE boundary filter `field >= seek_key` (ASC) /
/// `field <= seek_key` (DESC) for the SOLE ORDER BY column, AND-combined
/// with the caller's original `where` (if any). Only single-segment field
/// paths are supported — mirrors the brief's guidance that a keyset seek
/// needs the ORDER BY column's value; multi-column ORDER BY / nested field
/// paths fall back to the `None` (row-count `offset`) bookmark instead
/// (see `pagination_mode_for_query`).
///
/// CR-A4 (#764): INCLUSIVE (`Gte`/`Lte`), not `Gt`/`Lt` — the boundary row
/// itself (and every row tied with it) is deliberately refetched so
/// `fetch_next`'s skip-past-tie-run logic can distinguish "already
/// returned" ties from "not yet returned" ties by COUNTING, since a real
/// per-row identity (`_id`/`RecordId`) is not available on this read path
/// (see the module doc comment).
pub(super) fn boundary_filter(base_query: &ReadQuery, seek_key: &QueryValue) -> Option<Filter> {
    let order_by = base_query.order_by.as_ref()?;
    if order_by.items.len() != 1 {
        return None;
    }
    let item = &order_by.items[0];
    if item.field.len() != 1 {
        return None;
    }
    let value = query_value_to_filter_value(seek_key)?;
    let boundary = match item.direction {
        OrderDirection::Asc => Filter::Gte {
            field: item.field.clone(),
            value,
        },
        OrderDirection::Desc => Filter::Lte {
            field: item.field.clone(),
            value,
        },
    };
    Some(match &base_query.r#where {
        Some(existing) => Filter::And {
            filters: vec![existing.clone(), boundary],
        },
        None => boundary,
    })
}

/// Whether this query's ORDER BY is a single, simple (top-level-field)
/// column — the only shape [`boundary_filter`] can build a seek from. When
/// `false`, the cursor is pinned to [`PaginationMode::Offset`] (the
/// row-count bookmark) for its WHOLE lifetime (CR-A4 #764: decided once at
/// `create_cursor` time, never re-derived per `FetchNext` — see the module
/// doc comment on why flip-flopping bookmark kinds mid-scroll is unsafe).
pub(super) fn pagination_mode_for_query(query: &ReadQuery) -> PaginationMode {
    match &query.order_by {
        Some(ob) if ob.items.len() == 1 && ob.items[0].field.len() == 1 => PaginationMode::Keyset,
        _ => PaginationMode::Offset,
    }
}

/// F-53b Step 4 (#880): `create_cursor`-time eligibility probe for
/// `PaginationMode::IndexSeek` — an EXACT port of the F-53b Step 3 spike's
/// `probe_index_seek_eligible` (`crates/shamir-engine/src/table/tests/
/// f53b_step3_cursor_after_spike.rs`, lines 171-190), unchanged apart from
/// the module path its imports now resolve through. Returns the interned
/// sorted-index name when:
///
/// - `query.r#where.is_none()` — mirrors `TableManager::try_plan_keyset_seek`
///   (`read_planner.rs:472-525`)'s hard requirement, shared by BOTH the
///   `Temporal::Latest` fast path and the AsOf arm this mode reaches. A
///   cursor whose ORIGINAL query already carries a caller `WHERE` can NEVER
///   reach the seek arm, no matter what pagination shape `fetch_next` sends
///   — see the module doc comment / the settled design memo §1.1 for why
///   extending the seek machinery to accept a residual `WHERE` is out of
///   scope.
/// - `query.order_by` is `Some` with exactly one single-segment field,
/// - a sorted index covers that field.
///
/// Deliberately does NOT check `query.pagination` — at `create_cursor` time
/// there is no `Pagination::After` bookmark yet to inspect; that's exactly
/// why this can't just delegate to `try_plan_keyset_seek` itself even
/// setting `where` aside.
///
/// Built ENTIRELY from APIs already reachable today: `TableManager::
/// sorted_index_definition_by_field` (the S5c-X read-only view over
/// `SortedIndexManager::find_by_field`) and
/// `shamir_engine::query::filter::resolve::intern_field_path`
/// (see the design memo §1.2 for the full
/// grep proof of reachability from this crate).
///
/// Wired as the FIRST branch in `create_cursor`'s mode-decision chain: when
/// `Some`, `IndexSeek` is pinned and the existing `pagination_mode_for_query`
/// -> schema-typed-gate -> null-probe chain is skipped entirely — the seek
/// arm's own per-candidate MVCC classifier (`read_as_of_keyset_seek`)
/// already handles the data-shape edge cases those two `Keyset`-specific
/// safety probes exist for, so running them here would be unnecessary work,
/// not a correctness gap (design memo §2.1).
pub(super) async fn probe_index_seek_eligible(
    table: &TableManager,
    query: &ReadQuery,
) -> shamir_db::DbResult<Option<u64>> {
    if query.r#where.is_some() {
        return Ok(None);
    }
    let Some(order_by) = query.order_by.as_ref() else {
        return Ok(None);
    };
    if order_by.items.len() != 1 {
        return Ok(None);
    }
    let item = &order_by.items[0];
    if item.field.len() != 1 {
        return Ok(None);
    }
    // S5c-X F1 (Round 6): the path is interned through the engine's
    // read-only view (`get_ind` lookups only) — no `&Interner` parameter.
    let field_path = table
        .info_store_read_view()
        .intern_field_path(&item.field)
        .await?;
    let Some(field_path) = field_path else {
        return Ok(None);
    };
    let def = table.sorted_index_definition_by_field(&field_path);
    Ok(def.map(|d| d.name_interned))
}

/// F-1 (#792, release blocker): is `query`'s sole ORDER BY column provably
/// homogeneous (no second `QueryValue` type possible, and never `NaN`) by a
/// bound schema, so the keyset boundary-filter scheme is safe to attempt?
///
/// # F-17 (#810): the `keyset_safe` proof
///
/// The TypeTag check alone is **not sufficient**: `add_schema_rule`/
/// `set_table_schema` validate the new rule's *shape* but never scan the
/// table's EXISTING rows against it. A schemaless table with mixed-type
/// data can have a schema declared *after the fact*. The fix stamps
/// `keyset_safe = (table.count() == 0)` at bind time — persisted in the
/// catalogue, preserved for unchanged rules on re-declaration. The gate
/// requires BOTH an accepted `TypeTag` AND `keyset_safe == true`.
///
/// # F-18 (#811): `TypeTag::Bin` excluded from the accepted set
///
/// The accepted set is `Int | Bool | String` — `Bin` is deliberately NOT
/// accepted. `shamir-engine`'s `compare_values` (`query/filter/resolve.rs`)
/// has no `(Bin, Bin)` arm, so `safe_seek_key` (this file) always returns
/// `None` for a `Bin`-typed seek key: a `Bin` column could never actually
/// benefit from `PaginationMode::Keyset`, it would only pay for the
/// null-probe read at `create_cursor` time before silently degrading to the
/// per-call offset-bookmark fallback (W-2) on every `FetchNext` past the
/// first. Excluding it at the gate is correctness-neutral (the fallback
/// already prevented row loss) but avoids that wasted work.
///
/// # Conservative defaults
///
/// Returns `false` for: no validator bound, no matching `FieldRule`,
/// disagreeing types across validators, an excluded `TypeTag` (including
/// `Bin`, see above), or a matched rule whose `keyset_safe` proof is
/// `false`.
pub(super) fn order_by_column_is_schema_typed_scalar(table: &TableManager, field: &[String]) -> bool {
    // S5c-X F2 (Round 6): read-only view — no `&Arc<ValidatorRegistry>`
    // (with its register/replace/rename/remove mutators) escapes.
    let Some(registry) = table.validator_registry_read_view() else {
        return false;
    };
    let bindings = table.validator_bindings();

    // Collect the `TypeTag` from every bound SchemaValidator's FieldRule
    // that matches `field` exactly. Ambiguous (disagreeing) or absent
    // matches both fall back to "not schema-typed" — see the doc comment.
    let mut matched_ty: Option<TypeTag> = None;
    // F-17 (#810) — every matching rule must carry `keyset_safe == true`
    // for the gate to pass. A single unproven rule (schema declared onto a
    // non-empty table) is enough to disqualify the column.
    let mut all_keyset_safe = true;
    for binding in bindings.iter() {
        let Some(validator) = registry.get_by_id(&binding.validator_id) else {
            continue;
        };
        let Some(rules) = validator.as_schema_rules() else {
            continue;
        };
        for rule in rules {
            if rule.path == field {
                if !rule.keyset_safe {
                    all_keyset_safe = false;
                }
                match matched_ty {
                    None => matched_ty = Some(rule.ty),
                    Some(existing) if existing == rule.ty => {}
                    // Two bound schema validators disagree on the type for
                    // the same path — conservative: treat as not
                    // schema-typed rather than trying to reconcile.
                    Some(_) => return false,
                }
            }
        }
    }

    all_keyset_safe
        && matches!(
            matched_ty,
            Some(TypeTag::Int | TypeTag::Bool | TypeTag::String)
        )
}

/// CR-D2 (#783, release blocker): does `query`'s sole ORDER BY column
/// contain ANY `Null`/missing value, checked once against the SAME pinned
/// snapshot `create_cursor` is about to scan?
///
/// # The bug this closes
///
/// The keyset bookmark's boundary filter is an inclusive `field >=
/// seek_key` (ASC) / `field <= seek_key` (DESC), evaluated through
/// `compare_values` (`shamir-engine`'s `query/filter/resolve.rs`). That
/// function's `(Value::Null, _)` case falls through to its catch-all `_ =>
/// None` arm — a `Null`/missing ORDER BY value can never be proven `>=` or
/// `<=` anything, so the filter is `false` for that row. Meanwhile
/// `QvSortKey`'s sort (`shamir-engine`'s `query/read/order.rs`,
/// `compare_qv_sort_keys`) places `Null` LAST under the ASC default (FIRST
/// under DESC) — so an ASC scan's page 1 (no boundary yet) returns only the
/// leading real-valued rows, and EVERY subsequent page's boundary filter
/// permanently excludes the null/missing rows. The scan looks like it "ran
/// out" (`has_more: false` eventually), but those rows were never returned
/// at all — silent data loss, no error. (DESC happens to work today, since
/// nulls sort FIRST there and the boundary scan reaches them naturally —
/// but this check does not special-case direction: a null-containing column
/// is treated as unsafe for keyset regardless, the conservative choice.)
///
/// # Why a probe, not a static check
///
/// `pagination_mode_for_query` decides `Keyset` purely from the query's
/// SHAPE (a single simple-field ORDER BY) — whether the column's DATA
/// happens to contain a null/missing value cannot be known without asking
/// the data. This runs ONE cheap existence-check read at `create_cursor`
/// time, against the SAME pinned MVCC version the cursor's first page
/// reads: `WHERE <order_by_field> IS NULL LIMIT 1`. `Filter::IsNull`
/// compiles to `FilterNode::IsNull`, evaluated via `RecordView::is_null_at`
/// (`shamir-types`), which treats an explicitly-`Null` field and a
/// COMPLETELY MISSING field identically (`matches!(present_kind_at(path),
/// None | Some(Kind::Null))`) — so this one probe closes BOTH sub-cases in
/// the brief (explicit `Null` and absent field) with a single filter shape.
///
/// # Cost
///
/// `Temporal::AsOf`'s read path (`TableManager::read_as_of`) is a full
/// tombstone-inclusive streaming scan of the table regardless of `limit` —
/// the `LIMIT 1` here does NOT make the underlying scan early-terminate
/// (verified against `read_as_of`'s source: the WHERE filter is applied
/// per-record INSIDE the scan loop, and pagination/limit is only applied to
/// the fully-materialized match set afterward). This probe is therefore the
/// SAME cost class as `create_cursor`'s own first-page read (also a full
/// `read_as_of` scan) and the `drain_all` step already run once per cursor
/// creation — an acceptable ONE-TIME cost at cursor creation, not a
/// per-page cost. `LIMIT 1` is still the cheapest correct query SHAPE
/// (smallest possible result to materialize/serialize internally), even
/// though it doesn't change the scan's asymptotic cost here.
pub(super) async fn order_by_column_contains_null(
    table: &TableManager,
    ctx: &FilterContext<'_>,
    query: &ReadQuery,
    pinned_version: u64,
) -> Result<bool, BatchError> {
    let Some(order_by) = query.order_by.as_ref() else {
        return Ok(false);
    };
    if order_by.items.len() != 1 || order_by.items[0].field.len() != 1 {
        return Ok(false);
    }
    let field = order_by.items[0].field.clone();
    let is_null = Filter::IsNull { field };
    // AND-combined with the caller's own WHERE (if any) — a null-containing
    // ORDER BY value in a row the caller's own filter already excludes can
    // never reach the keyset boundary scan in the first place, so it must
    // not force an unnecessary Offset fallback (mirrors `boundary_filter`'s
    // own AND-combination of the boundary with the caller's `where`).
    let probe_where = match &query.r#where {
        Some(existing) => Filter::And {
            filters: vec![existing.clone(), is_null],
        },
        None => is_null,
    };

    // Base the probe on a clone of the caller's query (mirrors
    // `create_cursor`'s own `first_query = query.clone()` idiom) so
    // `from`/`select` and any other shape stay identical — only
    // `where`/`pagination`/`temporal` are overridden for the probe's own
    // purpose.
    let mut probe = query.clone();
    probe.r#where = Some(probe_where);
    probe.pagination = Pagination::LimitOffset {
        limit: Some(1),
        offset: 0,
    };
    probe.temporal = Temporal::AsOf {
        at: At::Version(pinned_version),
    };

    let result = table
        .read_with_encoding(&probe, ctx, Default::default())
        .await
        .map_err(wrap_engine_err)?;
    Ok(!result.records.is_empty())
}

/// Extract the sole ORDER BY column's value from a record. `None` when the
/// field is absent from the projected row (e.g. not selected) — callers
/// treat this as "can't build/refresh a keyset bookmark from this page"
/// (see call sites).
///
/// Precondition: only meaningful when `pagination_mode_for_query(query) ==
/// PaginationMode::Keyset` (single-column simple ORDER BY) — callers only
/// invoke this in that case.
///
/// F-53b Step 4 (#880): uses `QueryRecord::get_value_owned` (variant-
/// agnostic), not `get_value` (which only handles `Direct`, returning `None`
/// for `Inserted`) — `PaginationMode::IndexSeek`'s successful-seek-arm rows
/// come back as `QueryRecord::Inserted` (see `read_as_of_keyset_seek`), so
/// this widening is required for `IndexSeek` to extract its `seek_key`
/// bookmark at all. Strictly additive for `Keyset` mode, whose rows are
/// always `Direct` (the generic AsOf full-scan projection) — `get_value_
/// owned`'s `Direct` arm behaves identically to the old `get_value` call.
pub(super) fn order_by_field_value(query: &ReadQuery, record: &QueryRecord) -> Option<QueryValue> {
    let order_by = query.order_by.as_ref()?;
    if order_by.items.len() != 1 || order_by.items[0].field.len() != 1 {
        return None;
    }
    record.get_value_owned(&order_by.items[0].field[0])
}

/// F-53b Step 4 (#880): extract the real `RecordId` a
/// `PaginationMode::IndexSeek` row carries. Only `QueryRecord::Inserted`
/// (the wire shape `read_as_of_keyset_seek`/`read_keyset_seek` both use to
/// surface `_id`) carries one — `QueryRecord::Direct` (the generic AsOf
/// full-scan/offset projection) never does, matching the module doc
/// comment's CR-A4 note on why `Keyset` mode needs `tie_skip` instead.
pub(super) fn row_record_id(record: &QueryRecord) -> Option<RecordId> {
    match record {
        QueryRecord::Inserted(rec) => rec.id,
        _ => None,
    }
}

/// W-2/W-3 (#789, Wave D review): is `candidate` safe to hand to
/// `boundary_filter` as a keyset `seek_key`?
///
/// Two independent gaps closed here, both symptoms of the same root cause —
/// `seek_key` was previously set from whatever raw `QueryValue` the ORDER BY
/// column held, with no check that this specific VALUE can actually
/// round-trip through the boundary-filter scheme:
///
/// - **W-3** (`Dec`/`Big`/anything else `query_value_to_filter_value`
///   refuses): these do not convert to a `FilterValue` at all, so
///   `boundary_filter` would return `None` and `fetch_keyset_page`'s
///   `let Some(filter) = boundary_filter(..) else { return Err(..) }` would
///   hard-error on every `FetchNext` past page 1 — even though `Dec`/`Big`
///   values ARE perfectly comparable via `compare_values`, the bookmark
///   mechanism just can't express them as a `Filter` literal.
/// - **W-2** (`Bin`/`List`): these DO convert to a `FilterValue`
///   (`Binary`/`Array`), so `boundary_filter` builds a filter that LOOKS
///   valid — but `compare_values` (`shamir-engine`'s `query/filter/
///   resolve.rs`) has no `(Bin, Bin)`/`(List, List)` comparison arm (its own
///   doc comment: "only the scalar arms... participate"), so the boundary
///   filter can never actually match any row — permanent silent data loss
///   past page 1, the exact CR-D2 failure shape. Investigated whether
///   `order.rs`'s `QvSortKey` machinery already established a working total
///   order for `Bin`/`List` that this could mirror/reuse (the more complete
///   fix, option (a) from the brief): it does NOT — `QvSortKey::
///   from_query_value` maps `Bin`/`Set`/`List`/`Map` ALL to `QvSortKey::
///   Other`, an explicitly UNSORTABLE bucket that Rust's stable sort merely
///   preserves in insertion order (see that function's own doc comment:
///   "Bin`/`Set` map to `Other` (unsortable, preserving insertion order via
///   stable sort)"). There is no existing total order to extend
///   `compare_values` with — inventing one (e.g. lexicographic `Vec<u8>`/
///   recursive `List` comparison) would ALSO have to change ORDER BY's own
///   sort semantics to stay consistent (today's ORDER BY does not actually
///   sort `Bin`/`List` at all), which is a materially bigger change than
///   this polish-batch task's scope warrants. So: option (b) — detect
///   `Bin`/`List` as unsafe-for-keyset the same way as an unconvertible
///   value, converting the silent data loss into the SAME documented
///   offset-mode degradation CR-A4's existing safety net already provides
///   for "ORDER BY field absent from projection".
///
/// Both sub-cases are treated identically by callers: a `None` return here
/// is handed straight to the EXISTING "no seek_key" safety net (a
/// `Keyset`-mode cursor with `seek_key == None` falls back to the row-count
/// offset bookmark for that call) — no new fallback mechanism invented.
pub(super) fn safe_seek_key(candidate: QueryValue) -> Option<QueryValue> {
    match candidate {
        // W-2: convertible to a FilterValue but not comparable via
        // `compare_values` — would silently match nothing past page 1.
        QueryValue::Bin(_) | QueryValue::List(_) => None,
        other => query_value_to_filter_value(&other).map(|_| other),
    }
}

/// Whether two `QueryValue`s represent the SAME ORDER BY boundary value,
/// for CR-A4's tie-run counting. Compares through `FilterValue` (the same
/// conversion `boundary_filter` already uses to build the `Gte`/`Lte`
/// comparison) rather than inventing a new equality relation — `FilterValue`
/// derives `PartialEq`. A value with no `FilterValue` equivalent (e.g.
/// `Map`/`Set`/`Dec`/`Big`) can never compare equal here; that's fine,
/// because such a value could not have produced a boundary filter in the
/// first place (`query_value_to_filter_value` would already have returned
/// `None` in `boundary_filter`, falling back to the offset bookmark).
pub(super) fn same_boundary_value(a: &QueryValue, b: &QueryValue) -> bool {
    match (
        query_value_to_filter_value(a),
        query_value_to_filter_value(b),
    ) {
        (Some(fa), Some(fb)) => fa == fb,
        _ => false,
    }
}

/// Outcome of [`fetch_keyset_page`]: the up-to-`page_size` NEW rows for
/// this `FetchNext` call (as a full `QueryResult`, carrying whatever
/// `stats`/etc. the last internal fetch produced — `stats.records_returned`
/// is corrected to the actual post-skip/post-take count), plus the
/// refreshed bookmark (`next_seek_key`/`next_tie_skip`) and whether more
/// data remains beyond this page.
pub(super) struct KeysetPage {
    pub(super) result: QueryResult,
    pub(super) next_seek_key: Option<QueryValue>,
    pub(super) next_tie_skip: u64,
    pub(super) has_more: bool,
}

/// CR-D1 (#782, release blocker): what [`fetch_keyset_page`] produced.
///
/// `StuckAtCeiling` is the fix for the tie-run-ceiling livelock: when the
/// retry ceiling (`limit_ceiling = cursor_limits.max_cursor_page_size`) is
/// hit AND the post-skip usable slice is EXACTLY `0` (`usable_len == 0` —
/// not just "hit the ceiling but still made some progress", which stays a
/// normal [`KeysetPage`]), there is NO way to make forward progress via the
/// keyset boundary-filter scheme: every row in the current tie run up to
/// `internal_limit` has already been handed to the client
/// (`tie_skip >= internal_limit`), so re-running the SAME `>= seek_key`
/// fetch at the SAME capped `internal_limit` can only ever reproduce the
/// identical zero-progress result — the exact silent-livelock shape this
/// task fixes (see the module-level `fetch_next` doc comment on the
/// keyset->offset fallback for how the caller recovers from this signal).
pub(super) enum KeysetOutcome {
    // Boxed: `KeysetPage` embeds a full `QueryResult`, making it far larger
    // than the unit `StuckAtCeiling` variant (clippy::large_enum_variant) —
    // indirection here keeps every `KeysetOutcome` on the stack cheap to
    // move regardless of which variant is live.
    Page(Box<KeysetPage>),
    StuckAtCeiling,
}

/// Compute the next bookmark (`next_seek_key`, `next_tie_skip`) for the tail
/// of a just-produced page.
///
/// `full_fetch` is the WHOLE internal fetch this call made (before slicing
/// out the client-visible `out` rows) and `consumed_from_front` is how many
/// of `full_fetch`'s LEADING rows are accounted for by `out` PLUS whatever
/// was skipped ahead of it (i.e. `skip_count + out.len()`) — every row in
/// `full_fetch[..consumed_from_front]` has now been handed to the client at
/// some point (this call or an earlier one).
///
/// CR-A4 (#764) correctness point: `next_tie_skip` must count ties RELATIVE
/// TO THE BOUNDARY, not just within `out` — when the new seek value is the
/// SAME as the value at `full_fetch[0]` (i.e. the boundary didn't move
/// because the last row handed out is still tied with earlier-consumed
/// rows), the count must include those earlier-consumed rows too, or a
/// later `FetchNext` will under-count `tie_skip`, re-skip too few ties, and
/// re-return rows that were already handed out — this manifested as an
/// infinite duplicate-returning loop before this fix (verified while
/// debugging this task: a `bookmark_from_last`-from-`out`-only version
/// caused `create_fetch_cancel`-style drains to never terminate on an
/// all-tied result set). Counting from `full_fetch[0]` forward is safe
/// specifically because everything at or before `consumed_from_front` is,
/// by construction, tied with the SAME boundary value the bookmark is
/// currently seeking on (the WHERE clause is `>= seek_key`, so nothing
/// before that value could appear).
pub(super) fn bookmark_from_tail(
    base_query: &ReadQuery,
    full_fetch: &[QueryRecord],
    consumed_from_front: usize,
) -> (Option<QueryValue>, u64) {
    match full_fetch
        .get(..consumed_from_front)
        .and_then(|s| s.last())
        .and_then(|r| order_by_field_value(base_query, r))
    {
        Some(last_value) => {
            let count = full_fetch[..consumed_from_front]
                .iter()
                .rev()
                .take_while(|r| {
                    order_by_field_value(base_query, r)
                        .is_some_and(|v| same_boundary_value(&v, &last_value))
                })
                .count() as u64;
            // W-2/W-3 (#789): the tie count above is computed against the
            // RAW extracted value (equality via `same_boundary_value` is
            // fine for any value shape), but the value actually returned as
            // the bookmark must first clear `safe_seek_key` — an
            // unconvertible (`Dec`/`Big`) or uncomparable-via-`compare_values`
            // (`Bin`/`List`) value is treated exactly like "no seek_key",
            // routing this call's caller straight into the existing
            // Keyset-with-`seek_key == None` offset-bookmark fallback for
            // THIS call only (see `safe_seek_key`'s doc comment).
            (safe_seek_key(last_value), count)
        }
        None => (None, 0),
    }
}

/// CR-A4 (#764) core: run the inclusive-boundary keyset seek, skip past the
/// `tie_skip` rows already handed to the client on prior pages, and retry
/// with a growing internal fetch limit when the boundary's tie run is
/// itself larger than one internal fetch.
///
/// `seek_key`/`tie_skip` are the CURRENT bookmark (from `CursorState`);
/// `page_size` is the number of NEW rows the caller wants; `limit_ceiling`
/// bounds how large the internal fetch may grow (reusing
/// `cursor_limits.max_cursor_page_size` per the brief, so this can't
/// runaway).
///
/// Each internal fetch asks for `internal_limit + 1` rows (one extra "peek"
/// row) so the TRUE end-of-data can be told apart from "the fetch just
/// happened to return exactly `internal_limit` rows" — without the peek,
/// `fetched == internal_limit` is ambiguous (could mean "that's all there
/// is" or "there's more, ask for a bigger limit"), which caused a genuine
/// infinite retry loop when a tie run's total size was an exact multiple of
/// a fetch's limit (found and fixed while building this task).
#[allow(clippy::too_many_arguments)]
pub(super) async fn fetch_keyset_page(
    table: &TableManager,
    ctx: &FilterContext<'_>,
    base_query: &ReadQuery,
    seek_key: &QueryValue,
    tie_skip: u64,
    page_size: u32,
    pinned_version: u64,
    limit_ceiling: u32,
) -> Result<KeysetOutcome, BatchError> {
    let page_size_u64 = page_size as u64;
    let ceiling_u64 = limit_ceiling as u64;
    // The internal fetch must be AT LEAST big enough to contain every
    // already-consumed tied row (`tie_skip`) plus one more — otherwise the
    // skip-count walk below can run out of fetched rows before it finds all
    // `tie_skip` ties, is forced to fall back to "skip nothing" (brief §5's
    // documented safety net for a GENUINELY missing boundary row), and would
    // then re-return already-consumed rows forever on a tie run bigger than
    // `page_size` (found and fixed while building this task: a tie run of
    // 10 with `page_size: 2` looped without this floor, since `tie_skip`
    // grows past a bare `page_size`-sized fetch long before the retry
    // doubling catches up). Starting at `max(page_size, tie_skip + 1)`
    // guarantees the very first fetch in this call can already account for
    // every previously-consumed tie.
    let mut internal_limit: u64 = page_size_u64
        .max(tie_skip.saturating_add(1))
        .min(ceiling_u64)
        .max(1);

    // boundary_filter only returns None when the seek value has no
    // `FilterValue` equivalent — callers only reach this function with a
    // `seek_key` that already produced one at bookmark-build time (see
    // `create_cursor`/`fetch_next`), so this is infallible in practice;
    // treat a (theoretical) `None` as a hard engine error rather than
    // silently mis-seeking.
    let Some(filter) = boundary_filter(base_query, seek_key) else {
        return Err(BatchError::QueryError {
            alias: String::new(),
            message: "cursor: keyset seek key has no comparable filter form".to_string(),
            code: None,
        });
    };

    loop {
        let mut query = base_query.clone();
        query.r#where = Some(filter.clone());
        // Peek-ahead: fetch one MORE row than `internal_limit` so a fetch
        // returning exactly `internal_limit` rows can be told apart from
        // one that ran out of data early (see the peek-ahead doc comment
        // above). `page.records` may therefore hold up to
        // `internal_limit + 1` rows; everything from `internal_limit`
        // onward is peek-only and never handed to the client directly by
        // this fetch (it's re-fetched for real on a LATER internal
        // iteration or a later `FetchNext`, if still needed).
        query.pagination = Pagination::LimitOffset {
            limit: Some(internal_limit.saturating_add(1)),
            offset: 0,
        };
        query.temporal = Temporal::AsOf {
            at: At::Version(pinned_version),
        };

        let page = table
            .read_with_encoding(&query, ctx, Default::default())
            .await
            .map_err(wrap_engine_err)?;

        let fetched = page.records.len() as u64;
        // Strictly fewer rows than we asked for (limit+1) means the
        // underlying data ran out somewhere at or before `internal_limit`
        // — i.e. there is NOTHING beyond what `page.records` already holds.
        let data_exhausted = fetched <= internal_limit;

        // Skip past the `tie_skip` rows exactly equal to `seek_key` that a
        // prior page already returned. If fewer than `tie_skip` matching
        // rows are found (brief §5: the boundary row was concurrently
        // deleted, or some other R-1/#767-territory anomaly), do NOT skip
        // anything — treating the whole `>=` result as new risks a
        // duplicate, which is the documented, strictly-less-bad failure
        // mode versus silently losing a row.
        let mut skip_count = 0usize;
        let mut ties_seen = 0u64;
        for record in &page.records {
            if ties_seen >= tie_skip {
                break;
            }
            match order_by_field_value(base_query, record) {
                Some(v) if same_boundary_value(&v, seek_key) => {
                    ties_seen += 1;
                    skip_count += 1;
                }
                _ => break,
            }
        }
        if ties_seen < tie_skip {
            skip_count = 0;
        }

        // Only rows strictly within `internal_limit` (excluding the
        // peek-only tail) are real candidates to hand to the client this
        // iteration.
        let usable_len = (fetched.min(internal_limit) as usize).saturating_sub(skip_count);

        // Stop retrying once either the post-skip usable slice already
        // covers a full page, or the underlying fetch came back short of
        // what we asked for (the data is genuinely exhausted — growing the
        // limit further would fetch nothing new).
        if usable_len as u64 >= page_size_u64 || data_exhausted {
            let take = (page_size_u64 as usize).min(usable_len);
            let consumed_from_front = skip_count + take;

            // More pages remain iff there's any row beyond
            // `consumed_from_front` in THIS fetch (including the peek row,
            // or unused usable rows we chose not to take because `take`
            // was capped at `page_size`).
            let has_more = consumed_from_front < page.records.len();

            return Ok(KeysetOutcome::Page(Box::new(finish_keyset_page(
                base_query, page, skip_count, take, has_more,
            ))));
        }

        if internal_limit >= ceiling_u64 {
            // Hit the retry ceiling with still not enough usable rows — the
            // tie run genuinely exceeds the configured cap.
            //
            // CR-D1 (#782, release blocker): `usable_len == 0` here means
            // this call can make ZERO forward progress — every row this
            // (already-maxed) fetch could see is either already-consumed
            // tie (`skip_count`) or peek-only tail beyond `internal_limit`.
            // Returning a normal zero-`take` page in that case is exactly
            // the livelock this task fixes: `finish_keyset_page` would
            // recompute the SAME `(seek_key, tie_skip)` bookmark this call
            // started with (the tail of the consumed prefix is still
            // wholly within the SAME tie run), so the very next `FetchNext`
            // would re-run this identical fetch and get the identical
            // empty-page-with-`has_more:true` result, forever. Signal the
            // caller instead so it can fall back to the offset bookmark
            // (see `fetch_next`'s Keyset dispatch arm).
            //
            // When `usable_len > 0` (the ceiling was hit but SOME progress
            // was still made this call), the existing behavior is correct
            // and unchanged — that page is real progress, not a livelock.
            if usable_len == 0 {
                return Ok(KeysetOutcome::StuckAtCeiling);
            }
            let consumed_from_front = skip_count + usable_len;
            let has_more = consumed_from_front < page.records.len();
            return Ok(KeysetOutcome::Page(Box::new(finish_keyset_page(
                base_query, page, skip_count, usable_len, has_more,
            ))));
        }

        internal_limit = internal_limit.saturating_mul(2).min(ceiling_u64);
    }
}

/// Build the final [`KeysetPage`] from a raw internal fetch: slice out
/// `page.records[skip_count..skip_count + take]` as the NEW rows for this
/// call, correct `stats.records_returned` to match, and compute the
/// refreshed bookmark from the tail of `page.records` up through
/// `skip_count + take` (see [`bookmark_from_tail`] for why the count must
/// span the whole consumed prefix, not just the newly-returned `out` rows).
pub(super) fn finish_keyset_page(
    base_query: &ReadQuery,
    mut page: QueryResult,
    skip_count: usize,
    take: usize,
    has_more: bool,
) -> KeysetPage {
    let consumed_from_front = skip_count + take;
    let (next_seek_key, next_tie_skip) = if has_more {
        bookmark_from_tail(base_query, &page.records, consumed_from_front)
    } else {
        (None, 0)
    };
    let out: Vec<QueryRecord> = page
        .records
        .drain(skip_count..consumed_from_front)
        .collect();
    if let Some(stats) = page.stats.as_mut() {
        stats.records_returned = out.len() as u64;
    }
    page.records = out;
    KeysetPage {
        result: page,
        next_seek_key,
        next_tie_skip,
        has_more,
    }
}

