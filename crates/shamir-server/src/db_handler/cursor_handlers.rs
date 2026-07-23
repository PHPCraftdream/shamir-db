//! FG-5b — `CreateCursor` / `FetchNext` / `CancelCursor` wire handlers.
//!
//! Mirrors `tx_handlers.rs`'s shape (registry lookup → engine call →
//! `error_code`-classified `DbResponse::Error` on failure) but calls
//! `TableManager::read_with_encoding` DIRECTLY against a pinned
//! `TableManager` + hand-built `FilterContext`, bypassing the batch
//! planner entirely (see the brief §1/§2 — a cursor is a bookmark, not a
//! live `Stream`, and `FetchNext` re-runs the SAME read at a pinned
//! snapshot version with a mutated bookmark).
//!
//! # Bookmark strategy — why NOT `Pagination::After` directly
//!
//! `Pagination::After { key, .. }` is the shape `crates/shamir-query-types`
//! documents for keyset seeks, but `Pagination::resolve()` maps it to a bare
//! `(skip=0, take=limit)` pair — the seek `key` itself is only consumed by
//! the engine's sorted-INDEX keyset-seek fast path
//! (`TableManager::try_plan_keyset_seek`, `read_exec.rs`), which is reached
//! only for `Temporal::Latest` reads. A cursor's `FetchNext` reads
//! `Temporal::AsOf { at: At::Version(pinned) }` (so it never observes a
//! write committed after `CreateCursor` — see the module's snapshot-
//! stability tests), and `Temporal::AsOf`'s own pipeline
//! (`TableManager::read_as_of` / `read_temporal.rs`) NEVER consults the
//! sorted-index seek path — it always ORDER-BY-sorts the in-memory matched
//! set and slices it with `Pagination::resolve()`'s plain `(skip, take)`.
//! Handing it a bare `Pagination::After` would therefore always resolve to
//! `(skip=0, take=limit)` — i.e. return PAGE ONE FOREVER, never advancing.
//!
//! Instead, the bookmark is built explicitly:
//! - **With an ORDER BY** (single column, `PaginationMode::Keyset`): the
//!   seek key from the last row of the previous page is AND-combined into
//!   the query's `where` as an INCLUSIVE `Gte`/`Lte` boundary
//!   (direction-dependent), and pagination stays
//!   `LimitOffset { offset: 0, limit }` — the boundary filter does the
//!   seeking, the LIMIT just caps the internal fetch. CR-A4 (#764): the
//!   boundary is inclusive (not `Gt`/`Lt`) specifically so a run of ROWS
//!   TIED on the ORDER BY value straddling a page boundary is never
//!   silently dropped — see `boundary_filter` and `fetch_next`'s
//!   skip-past-tie-run logic below for the mechanics.
//! - **Without an ORDER BY** (`PaginationMode::Offset`): there is no field
//!   to build a boundary filter on, so the bookmark is a plain row-count
//!   `offset` that advances by `page_size` each `FetchNext`, resumed via
//!   `Pagination::LimitOffset`. This relies on the pinned-snapshot full
//!   scan being stable across calls at the SAME pinned version (no
//!   concurrent write can be observed — see the AsOf pin above — so the
//!   enumeration order the engine produces for a fixed `(table, version)`
//!   pair is deterministic).
//!
//! `PaginationMode` is decided ONCE, at `create_cursor` time (see
//! `CursorState::mode`) — never re-derived per `FetchNext` — so a later
//! page can never flip coordinate systems mid-scroll.
//!
//! # CR-A4 (#764) — keyset tie-breaker for duplicate ORDER BY values
//!
//! Brief: `docs/dev-artifacts/prompts/post-alpha/11-cr-a4-keyset-tie-breaker.md`.
//!
//! Problem: a bare `field > last_value` / `field < last_value` boundary
//! silently skips every row tied with `last_value` once the page boundary
//! falls inside a run of equal ORDER BY values — permanent, silent data
//! loss, no error.
//!
//! `_id`/`RecordId` is NOT available as a comparable field on this read
//! path: `TableManager::read_as_of`'s non-fast-path projection helpers
//! (`apply_select_value_bytes` / `try_project_page_only_bytes` in
//! `shamir-engine`'s `read_exec.rs`) explicitly discard the `RecordId` of
//! each matched row before projecting — confirmed by grep across the
//! engine crate: `_id` is attached ONLY on the write-result path
//! (`InsertedRecord`) and the Latest-temporal SORTED-INDEX keyset-seek
//! fast path (`try_plan_keyset_seek`), which `Temporal::AsOf` structurally
//! cannot reach (`read_impl` routes `AsOf` straight to `read_as_of` before
//! any index-scan planning). Building a Filter-level `_id` comparison to
//! plug this gap would be a materially bigger, riskier change than this
//! task needs (the brief explicitly rules it out).
//!
//! Instead: the boundary filter is made INCLUSIVE (`Gte`/`Lte` instead of
//! `Gt`/`Lt`), so the previous page's exact boundary row (and every row
//! tied with it) is refetched. `CursorState::tie_skip` then counts how
//! many rows AT THAT EXACT VALUE have already been handed to the client —
//! `fetch_next` skips that many equal-valued rows from the front of the
//! (stably re-sorted) refetch before handing out `page_size` new rows.
//! This is sound because `list_stream`'s enumeration order and
//! `apply_order_by_qv`'s sort (`Vec::sort_by`, stable per Rust std) are
//! both deterministic across repeat `read_as_of` calls at the SAME pinned
//! version with no concurrent write — so the Nth tied row (by return
//! order) is the same physical row every time, making "skip the first
//! `tie_skip` tied rows" behave exactly like "skip past last_id" would.
//! (A concurrent DELETE/INSERT between calls could disturb this guarantee
//! — that's CR-B1/#767's territory, not this task's.)
//!
//! When the tie run at the boundary is itself larger than one internal
//! fetch, `fetch_next` retries with a doubled internal limit (capped by
//! `cursor_limits.max_cursor_page_size`) until either `page_size` new rows
//! are collected or the fetch stops growing (true end of data).

use shamir_connect::server::session::Session;
use shamir_db::engine::query::filter::eval_context::FilterContext;
use shamir_db::engine::repo::RepoInstance;
use shamir_db::engine::table::TableManager;
use shamir_db::query::batch::BatchError;
use shamir_db::query::filter::Filter;
use shamir_db::query::read::{
    At, OrderDirection, Pagination, QueryRecord, QueryResult, ReadQuery, Temporal,
};
use shamir_query_types::filter::query_value_to_filter_value;
use shamir_query_types::wire::CursorId;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use crate::byte_budget::{stash_guard, ByteBudgetGuard};
use crate::cursor_registry::{Cursor, CursorRegistryError, PaginationMode};

use super::handler::{session_actor, DbResponse, ShamirDbHandler};

/// Resolve `(db_name, query.from.repo)` down to a `RepoInstance`, mirroring
/// `tx_begin_as`'s `db.get_db(...)`/`db.get_repo(...)` idiom exactly
/// (`crates/shamir-db/src/shamir_db/execute/db_tx.rs:70-81`).
fn resolve_repo(
    db: &shamir_db::ShamirDb,
    db_name: &str,
    repo_name: &str,
) -> Result<RepoInstance, BatchError> {
    let dbi = db.get_db(db_name).ok_or_else(|| BatchError::QueryError {
        alias: String::new(),
        message: format!("Database '{}' not found", db_name),
        code: None,
    })?;
    dbi.get_repo(repo_name)
        .ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Repository '{}' not found", repo_name),
            code: None,
        })
}

/// Authorize `actor` for `Action::Read` on the target table, mirroring the
/// exact two-call shape `execute_as` uses for the normal batch path
/// (`crates/shamir-db/src/shamir_db/execute/db_execute.rs::execute_as`,
/// ~lines 35-65): a `Database` check up front, then a `Table` check for the
/// specific target. `authorize_access`'s own ancestor-walk already covers the
/// `Store` link internally, so these two calls are sufficient — no more, no
/// less. `Actor::System`/`Actor::Admin` short-circuit to `Ok(())` inside
/// `authorize_access` itself (admin bypass, same as everywhere else).
async fn authorize_cursor_read(
    db: &shamir_db::ShamirDb,
    actor: &shamir_db::access::Actor,
    db_name: &str,
    repo_name: &str,
    table_name: &str,
) -> Result<(), BatchError> {
    db.authorize_access(
        actor,
        &shamir_db::access::ResourcePath::Database {
            db: db_name.to_string(),
        },
        shamir_db::access::Action::Read,
    )
    .await
    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

    db.authorize_access(
        actor,
        &shamir_db::access::ResourcePath::Table {
            db: db_name.to_string(),
            store: repo_name.to_string(),
            table: table_name.to_string(),
        },
        shamir_db::access::Action::Read,
    )
    .await
    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

    Ok(())
}

/// Wrap an engine [`shamir_storage::error::DbError`] (or any `Display`
/// error) in the same `BatchError::QueryError` shape every other cursor
/// error path uses, so `error_code()` classifies it uniformly.
fn wrap_engine_err(e: impl std::fmt::Display) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: e.to_string(),
        code: None,
    }
}

fn error_response(e: &BatchError) -> DbResponse {
    DbResponse::Error {
        code: super::handler::error_code(e).to_string(),
        message: e.to_string(),
    }
}

/// CR-B2: reserve an upfront, pessimistic slice of the RI-15 global
/// in-flight response-byte budget BEFORE running the pinned-version read for
/// a cursor page, using `handler.query_limits.max_result_size_bytes` (the
/// natural upper bound for one page — `CursorPageTooLarge` already rejects
/// anything past it) as the estimate.
///
/// Returns `None` when there is no natural upfront estimate to reserve
/// against — either the RI-15 budget is unbounded (nothing to gate) or the
/// per-page size cap itself is inactive (`usize::MAX`, e.g. some unit-test
/// configs): inventing an estimate out of nothing in that case would be
/// over-engineering an unbounded case (mirrors the brief's own CR-A4
/// precedent for not fabricating bounds that don't exist). Callers fall
/// back to [`enforce_page_budget`]'s existing post-hoc-only acquire in that
/// case.
async fn reserve_page_budget_upfront(handler: &ShamirDbHandler) -> Option<ByteBudgetGuard> {
    let budget_active = handler.byte_budget.cap().is_some();
    let cap_active = handler.query_limits.max_result_size_bytes < usize::MAX;
    if !budget_active || !cap_active {
        return None;
    }
    Some(
        handler
            .byte_budget
            .acquire(handler.query_limits.max_result_size_bytes)
            .await,
    )
}

/// CR-A5 (+ CR-B2): gate a cursor page against BOTH the per-page byte-size
/// cap (`query_limits.max_result_size_bytes`) and the RI-15 global in-flight
/// response-byte budget, mirroring `ShamirDbHandler::execute`'s exact block
/// (`handler.rs`'s `DbRequest::Execute` path) — measure the serialized
/// `page` ONCE (matching `execute()`'s choice to measure the payload alone,
/// i.e. `BatchResponse`/here `QueryResult`, not the full `DbResponse`
/// envelope), then either reject (too large — no budget acquired/held,
/// there is nothing to write) or finalize the budget reservation and stash
/// the guard for the writer task to release after the socket write
/// completes.
///
/// `upfront_guard` is whatever [`reserve_page_budget_upfront`] already
/// reserved before the page was built (`Some` when both gates were active
/// at that point) — this function SHRINKS it down to the actual serialized
/// size rather than acquiring fresh. When `upfront_guard` is `None` (the
/// per-page cap was inactive, so there was no natural upfront estimate),
/// this falls back to the pre-CR-B2 post-hoc-only acquire.
///
/// The too-large rejection check itself is unchanged by CR-B2: it still
/// runs only after the real page is built (it inherently needs the actual
/// size) — the upfront reserve only affects WHEN the RI-15 budget is
/// acquired, never the `CursorPageTooLarge` rejection logic.
///
/// Returns `Err(too_large_error)` when the page must be rejected; `Ok(())`
/// when the caller may proceed to return the `CursorPage` response (a guard
/// has already been stashed for it, if the budget is bounded).
async fn enforce_page_budget(
    handler: &ShamirDbHandler,
    page: &QueryResult,
    upfront_guard: Option<ByteBudgetGuard>,
) -> Result<(), BatchError> {
    // Only serialize when at least one of the two gates is actually active —
    // an unbounded budget AND an effectively-unlimited size cap (the UNIT
    // TEST default) must stay a pure no-op, same as `execute()`'s
    // `self.byte_budget.cap().is_some()` short-circuit.
    let budget_active = handler.byte_budget.cap().is_some();
    let cap_active = handler.query_limits.max_result_size_bytes < usize::MAX;
    if !budget_active && !cap_active {
        return Ok(());
    }

    let Ok(bytes) = rmp_serde::to_vec_named(page) else {
        // Mirrors `execute()`: a serialization failure here is swallowed
        // (the `if let Ok(...)` in `execute()` silently skips the acquire
        // on `Err`) rather than treated as a hard error — the response
        // still goes out, just without budget accounting for it. Any
        // upfront reservation is simply dropped as-is (releasing the
        // pessimistic estimate untouched).
        return Ok(());
    };

    if cap_active && bytes.len() > handler.query_limits.max_result_size_bytes {
        // Rejected — no budget is acquired/held for a page that is never
        // going out over the wire. Drop any upfront reservation (releases
        // it back to the budget via `ByteBudgetGuard::Drop`).
        return Err(BatchError::CursorPageTooLarge {
            size: bytes.len(),
            max: handler.query_limits.max_result_size_bytes,
        });
    }

    if budget_active {
        let guard = match upfront_guard {
            Some(mut guard) => {
                // Overshoot edge case (CR-B2, mirrors `execute()`'s
                // handling): the per-page cap and the actual serialized
                // size are two independently-configured numbers, so in
                // principle `bytes.len()` could exceed what was reserved
                // upfront (e.g. an operator changed the cap between the
                // upfront reserve and this point in a hot-reload scenario) —
                // guard against that with `grow_unchecked` rather than
                // assume `shrink_to`'s no-op-on-grow branch is always safe
                // to silently under-reserve through.
                if bytes.len() >= guard.bytes_reserved() {
                    guard.grow_unchecked(bytes.len() - guard.bytes_reserved());
                } else {
                    guard.shrink_to(bytes.len());
                }
                guard
            }
            None => handler.byte_budget.acquire(bytes.len()).await,
        };
        stash_guard(guard);
    }

    Ok(())
}

/// Build the `FilterContext` a cursor's `FetchNext` reads through — mirrors
/// the non-tx bare-single-read shape `query_runner.rs` builds (empty
/// `resolved_refs`/`params`, actor injected).
///
/// Scope note: this uses `FilterContext::new`'s default scalar resolver
/// (`ScalarResolver::builtins_only()`) rather than the per-DB resolver with
/// user-registered scalars (`DbTableResolver::scalar_resolver()` in
/// `shamir-db`, which needs a direct `shamir-funclib` dependency this
/// crate does not otherwise carry). A cursor's WHERE clause calling a
/// user-registered scalar function is therefore out of scope for FG-5b —
/// the same narrow limitation the brief accepts for temporal reads; a
/// future task can thread the per-DB resolver through if this proves
/// necessary in practice.
async fn build_filter_context<'a>(
    table: &'a TableManager,
    actor: shamir_db::access::Actor,
    resolved_refs: &'a TMap<String, shamir_db::query::read::QueryResult>,
) -> Result<FilterContext<'a>, BatchError> {
    let interner = table.interner().get().await.map_err(wrap_engine_err)?;
    Ok(FilterContext::new(interner, resolved_refs).with_actor(actor))
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
fn boundary_filter(base_query: &ReadQuery, seek_key: &QueryValue) -> Option<Filter> {
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
fn pagination_mode_for_query(query: &ReadQuery) -> PaginationMode {
    match &query.order_by {
        Some(ob) if ob.items.len() == 1 && ob.items[0].field.len() == 1 => PaginationMode::Keyset,
        _ => PaginationMode::Offset,
    }
}

/// Extract the sole ORDER BY column's value from a record. `None` when the
/// field is absent from the projected row (e.g. not selected) — callers
/// treat this as "can't build/refresh a keyset bookmark from this page"
/// (see call sites).
///
/// Precondition: only meaningful when `pagination_mode_for_query(query) ==
/// PaginationMode::Keyset` (single-column simple ORDER BY) — callers only
/// invoke this in that case.
fn order_by_field_value(query: &ReadQuery, record: &QueryRecord) -> Option<QueryValue> {
    let order_by = query.order_by.as_ref()?;
    if order_by.items.len() != 1 || order_by.items[0].field.len() != 1 {
        return None;
    }
    record.get_value(&order_by.items[0].field[0]).cloned()
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
fn same_boundary_value(a: &QueryValue, b: &QueryValue) -> bool {
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
struct KeysetPage {
    result: QueryResult,
    next_seek_key: Option<QueryValue>,
    next_tie_skip: u64,
    has_more: bool,
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
fn bookmark_from_tail(
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
            (Some(last_value), count)
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
async fn fetch_keyset_page(
    table: &TableManager,
    ctx: &FilterContext<'_>,
    base_query: &ReadQuery,
    seek_key: &QueryValue,
    tie_skip: u64,
    page_size: u32,
    pinned_version: u64,
    limit_ceiling: u32,
) -> Result<KeysetPage, BatchError> {
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

            return Ok(finish_keyset_page(
                base_query, page, skip_count, take, has_more,
            ));
        }

        if internal_limit >= ceiling_u64 {
            // Hit the retry ceiling with still not enough usable rows — the
            // tie run genuinely exceeds the configured cap. Return what we
            // have; `has_more` reflects whatever is left after this
            // (already-maxed) fetch.
            let consumed_from_front = skip_count + usable_len;
            let has_more = consumed_from_front < page.records.len();
            return Ok(finish_keyset_page(
                base_query, page, skip_count, usable_len, has_more,
            ));
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
fn finish_keyset_page(
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

impl ShamirDbHandler {
    /// FG-5b CREATE — resolve the query's table, open an MVCC snapshot
    /// pinned for the cursor's whole lifetime, run the first page, and
    /// park the cursor in the registry bound to this session.
    pub(super) async fn create_cursor(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        query: ReadQuery,
        page_size: u32,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }

        // CR-A3: reject page_size == 0 (would make has_more's
        // `page.records.len() as u64 >= page_size as u64` compute
        // `0 >= 0 → true` forever, looping the client indefinitely) and
        // page_size above the configured cap (unbounded materialize/
        // serialize hazard) up front — before any registry/engine work.
        let max_page_size = self.cursor_limits.max_cursor_page_size;
        if page_size == 0 || page_size > max_page_size {
            return error_response(&BatchError::InvalidPageSize {
                page_size,
                max: max_page_size,
            });
        }

        // Scope cut (FG-5b): only Temporal::Latest cursors are supported.
        // AsOf/History are rejected outright — never silently downgraded.
        if !matches!(query.temporal, Temporal::Latest) {
            return error_response(&BatchError::CursorTemporalNotSupported);
        }

        // Scope cut (CR-B5, #771): a cursor's every internal read (this
        // first page and every later `fetch_next`) rewrites `temporal` to
        // `Temporal::AsOf { at: At::Version(pinned_version) }` below — and
        // that read path hard-codes `versions: None` on its `QueryResult`
        // (`TableManager::read_as_of` in `shamir-engine`'s
        // `read_temporal.rs`). Honoring `with_version = true` here would
        // therefore silently produce NO per-record versions, breaking the
        // FG-2 optimistic-CAS contour a client might expect to still work
        // after switching a `.withVersion()` read to a cursor. Reject
        // outright rather than silently drop the flag — see
        // `BatchError::CursorWithVersionNotSupported`'s doc comment.
        if query.with_version {
            return error_response(&BatchError::CursorWithVersionNotSupported);
        }

        let repo_name = query.from.repo.clone();
        let table_name = query.from.table.clone();

        let actor = session_actor(session);
        if let Err(e) =
            authorize_cursor_read(&self.db, &actor, db_name, &repo_name, &table_name).await
        {
            return error_response(&e);
        }

        let repo = match resolve_repo(&self.db, db_name, &repo_name) {
            Ok(r) => r,
            Err(e) => return error_response(&e),
        };
        let table = match repo.get_table(&table_name).await {
            Ok(t) => t,
            Err(e) => return error_response(&wrap_engine_err(e)),
        };
        let gate = match repo.tx_gate().await {
            Ok(g) => g,
            Err(e) => return error_response(&wrap_engine_err(e)),
        };
        let guard = gate.open_snapshot().await;
        let pinned_version = guard.version();

        // Temporal-read drain caveat (query_runner.rs): flush the repo's
        // in-memory overlay to durable history once, up front, so the
        // pinned version's `AsOf` reads are coherent for the cursor's
        // whole lifetime (the pinned version never changes across fetches,
        // so a single drain here suffices — unlike a one-shot AsOf/History
        // read, which drains on every call).
        if let Err(e) = repo.drainer().drain_all(&repo).await {
            tracing::warn!(?e, db = db_name, repo = %repo_name, "create_cursor: drain_all failed");
        }

        let empty_refs: TMap<String, shamir_db::query::read::QueryResult> = new_map();
        let ctx = match build_filter_context(&table, actor.clone(), &empty_refs).await {
            Ok(c) => c,
            Err(e) => return error_response(&e),
        };

        // CR-B4: fetch one extra "peek" row beyond the client-visible
        // `page_size` so the true end-of-data can be told apart from "the
        // result set happens to be an exact multiple of page_size" — without
        // the peek, `fetched == page_size` is ambiguous, and the OLD
        // `fetched >= page_size` heuristic reported `has_more: true` on the
        // genuine last page, causing one spurious empty round-trip (the
        // `DbResponse::CursorPage::has_more` doc comment promises `true`
        // means "a further FetchNext will return AT LEAST ONE more record" —
        // widen to `u64` before the `+1` since `page_size` is validated only
        // against `max_cursor_page_size` as a `u32`, not against the wider
        // internal fetch limit; `Pagination::LimitOffset::limit` is already
        // `Option<u64>`, so this cannot overflow.
        let internal_limit = (page_size as u64).saturating_add(1);
        let mut first_query = query.clone();
        first_query.pagination = Pagination::LimitOffset {
            limit: Some(internal_limit),
            offset: 0,
        };
        first_query.temporal = Temporal::AsOf {
            at: At::Version(pinned_version),
        };

        // CR-B2: reserve an upfront, pessimistic slice of the RI-15 budget
        // BEFORE running the pinned-version read below — this is what
        // actually bounds execution-time memory for a cursor page, not
        // just write-path residency. `None` when there's no natural
        // upfront estimate (unbounded budget or inactive per-page cap);
        // `enforce_page_budget` falls back to its post-hoc-only acquire in
        // that case.
        let budget_guard = reserve_page_budget_upfront(self).await;

        let mut page = match table
            .read_with_encoding(&first_query, &ctx, Default::default())
            .await
        {
            Ok(p) => p,
            Err(e) => return error_response(&wrap_engine_err(e)),
        };

        // CR-B4: the peek row (if present) proves there's at least one more
        // record beyond `page_size` — trim it off BEFORE the page goes out
        // over the wire or is measured for the byte budget, so both the
        // client-visible payload and the budget accounting reflect only the
        // `page_size` rows actually returned.
        let has_more = page.records.len() as u64 > page_size as u64;
        if has_more {
            page.records.truncate(page_size as usize);
            if let Some(stats) = page.stats.as_mut() {
                stats.records_returned = page.records.len() as u64;
            }
        }

        // CR-A5 (+ CR-B2): gate the page against the per-page byte-size cap
        // and the RI-15 global byte budget BEFORE deciding whether to
        // register the cursor — a rejected page must not mint a registered
        // cursor (there is nothing to `FetchNext` against; the client never
        // receives this page's bytes at all), and this covers BOTH success
        // returns below (the CR-A2 "exhausted first page, not registered"
        // early return and the normal registered return) since both hand
        // the SAME `page` back over the wire. `budget_guard` (reserved
        // upfront, before the read above) is shrunk to the actual size here
        // rather than acquired fresh.
        if let Err(e) = enforce_page_budget(self, &page, budget_guard).await {
            return error_response(&e);
        }

        let mode = pagination_mode_for_query(&query);
        let (seek_key, tie_skip) = if has_more && mode == PaginationMode::Keyset {
            match page
                .records
                .last()
                .and_then(|r| order_by_field_value(&query, r))
            {
                Some(last_value) => {
                    // CR-A4: how many rows in THIS page already share the
                    // last row's ORDER BY value — that many must be
                    // skipped from the front of the next (inclusive)
                    // refetch so they aren't handed to the client twice.
                    let tie_skip = page
                        .records
                        .iter()
                        .filter(|r| {
                            order_by_field_value(&query, r)
                                .is_some_and(|v| same_boundary_value(&v, &last_value))
                        })
                        .count() as u64;
                    (Some(last_value), tie_skip)
                }
                None => (None, 0),
            }
        } else {
            (None, 0)
        };
        let offset = page.records.len() as u64;

        let cursor_id = self.next_cursor_id();

        if !has_more {
            // The entire result fit on the first page — no `FetchNext` will
            // ever be issued (both the Rust and TS SDKs stop iterating as
            // soon as `has_more == false`). Registering it anyway would park
            // a live `SnapshotGuard` MVCC pin and a per-session registry
            // slot for no reason until the idle-timeout reaper eventually
            // reclaims it. Returning here instead lets `page` (built above)
            // go out with the response while `guard` (never wrapped into a
            // `Cursor` for this branch) drops immediately via RAII, and the
            // per-session cursor cap is never touched by an already-
            // exhausted cursor. The minted `cursor_id` is handed to the
            // client unregistered: a later `FetchNext`/`CancelCursor` against
            // it falls through to the existing not-found / idempotent-close
            // paths, which is the accurate answer for an id that never
            // existed in the registry.
            return DbResponse::CursorPage {
                cursor_id: CursorId(cursor_id),
                page,
                has_more,
            };
        }

        let cursor = Cursor::new(
            query,
            mode,
            guard,
            pinned_version,
            page_size,
            session.session_id,
            db_name.to_string(),
            repo_name,
        );
        {
            let mut state = cursor.state().lock().await;
            state.seek_key = seek_key;
            state.tie_skip = tie_skip;
            state.offset = offset;
            state.exhausted = !has_more;
        }

        match self.cursor_registry.register(
            cursor_id,
            session.session_id,
            cursor,
            self.cursor_limits.max_cursors_per_session as u32,
        ) {
            Ok(_) => DbResponse::CursorPage {
                cursor_id: CursorId(cursor_id),
                page,
                has_more,
            },
            Err(CursorRegistryError::CursorLimitExceeded { limit }) => {
                error_response(&BatchError::CursorLimitExceeded { limit })
            }
            Err(_) => DbResponse::Error {
                code: "cursor_error".into(),
                message: "could not register cursor".into(),
            },
        }
    }

    /// FG-5b FETCH_NEXT — look up the cursor, re-run the pinned read at the
    /// current bookmark, advance the bookmark, reply with the page.
    ///
    /// CR-B3 (#769): `page_size` is `Some(n)` for an explicit per-call
    /// override (unchanged, client-controlled backpressure) or `None` to
    /// fall back to the cursor's stored `CreateCursor`-time default
    /// (`Cursor::default_page_size`). The `Some(n)` case is still validated
    /// BEFORE the registry lookup (CR-A3's existing property — avoids a
    /// wasted registry hit and avoids ever reaching the `has_more`
    /// infinite-loop computation for a malformed request). The `None` case
    /// has nothing to validate yet at this point — the stored default is
    /// only known AFTER the registry lookup succeeds — so validation for
    /// that path is deferred to just after the lookup, below.
    pub(super) async fn fetch_next(
        &self,
        session: &Session,
        cursor_id: CursorId,
        page_size: Option<u32>,
    ) -> DbResponse {
        let max_page_size = self.cursor_limits.max_cursor_page_size;
        if let Some(requested) = page_size {
            // CR-A3: validate an explicit page_size BEFORE the registry
            // lookup — it doesn't need the cursor, and this avoids a wasted
            // registry hit (and, critically, avoids ever running the
            // has_more == 0 >= 0 → true infinite-loop computation below) for
            // a malformed request. A bad page_size on one FetchNext call
            // must not corrupt or close the cursor — it isn't looked up at
            // all here, so it stays untouched.
            if requested == 0 || requested > max_page_size {
                return error_response(&BatchError::InvalidPageSize {
                    page_size: requested,
                    max: max_page_size,
                });
            }
        }

        let cursor = match self
            .cursor_registry
            .get_owned(cursor_id.0, &session.session_id)
        {
            Ok(c) => c,
            Err(e) => return cursor_registry_error_response(cursor_id, e),
        };

        // CR-B3: resolve the effective page size now that the cursor is
        // known — `None` falls back to the stored `CreateCursor`-time
        // default. Validate the resolved value again (defense-in-depth):
        // the default was already validated once at `CreateCursor` time via
        // CR-A3, so this should always pass in practice, but a stored value
        // is never silently trusted. A failure here is a normal
        // `InvalidPageSize` rejection — same shape as the `Some(n)` case
        // above — and must NOT mutate/close the cursor (the "a bad
        // page_size on one call must not corrupt the cursor" invariant
        // CR-A3 already established): the lookup above only cloned an
        // `Arc<Cursor>`, it did not touch registry/cursor state, so simply
        // returning here leaves everything untouched.
        let effective_page_size = page_size.unwrap_or_else(|| cursor.default_page_size());
        if effective_page_size == 0 || effective_page_size > max_page_size {
            return error_response(&BatchError::InvalidPageSize {
                page_size: effective_page_size,
                max: max_page_size,
            });
        }

        let repo = match resolve_repo(&self.db, cursor.db(), cursor.repo()) {
            Ok(r) => r,
            Err(e) => return error_response(&e),
        };

        let mut state = cursor.state().lock().await;
        if state.exhausted {
            self.cursor_registry.remove(cursor_id.0);
            drop(state);
            return DbResponse::Error {
                code: "cursor_not_found".into(),
                message: format!(
                    "cursor {} is already exhausted (no more pages) and has been closed",
                    cursor_id
                ),
            };
        }

        let table_name = state.query.from.table.clone();

        // Re-authorize on every FetchNext, not just at CreateCursor time: a
        // pinned snapshot only bounds WHAT DATA a cursor can see, not
        // whether the actor SHOULD still be allowed to see it — a
        // permission revoked mid-scroll (between CreateCursor and a later
        // FetchNext) must close the read here, same class of gap as the
        // CreateCursor check above. Cheap: no I/O beyond the existing
        // `resource_meta` catalog reads every other authorize call already
        // does.
        let actor = session_actor(session);
        if let Err(e) =
            authorize_cursor_read(&self.db, &actor, cursor.db(), cursor.repo(), &table_name).await
        {
            self.cursor_registry.remove(cursor_id.0);
            drop(state);
            return error_response(&e);
        }

        let table = match repo.get_table(&table_name).await {
            Ok(t) => t,
            Err(e) => return error_response(&wrap_engine_err(e)),
        };

        let empty_refs: TMap<String, shamir_db::query::read::QueryResult> = new_map();
        let ctx = match build_filter_context(&table, actor, &empty_refs).await {
            Ok(c) => c,
            Err(e) => return error_response(&e),
        };

        let base_query = state.query.clone();

        // CR-A4 (#764): `state.mode` was pinned once at `create_cursor`
        // time and never re-derived here — see `PaginationMode`'s doc
        // comment for why flip-flopping bookmark kinds mid-scroll is
        // unsafe. `Keyset` additionally requires a `seek_key` to seek
        // from; a `Keyset`-mode cursor with `seek_key == None` (e.g. the
        // ORDER BY field was absent from a page's projection) falls back
        // to the row-count bookmark for THIS call only, matching the
        // pre-CR-A4 "can't build a seek from this page" safety net.
        // CR-B2: reserve an upfront, pessimistic slice of the RI-15 budget
        // BEFORE running the pinned-version read below (either branch of
        // the match) — bounds execution-time memory for this page, not
        // just write-path residency. `None` when there's no natural
        // upfront estimate (unbounded budget or inactive per-page cap);
        // `enforce_page_budget` falls back to its post-hoc-only acquire in
        // that case.
        let budget_guard = reserve_page_budget_upfront(self).await;

        let (page, new_seek_key, new_tie_skip, has_more, new_offset);
        match (state.mode, state.seek_key.clone()) {
            (PaginationMode::Keyset, Some(seek_key)) => {
                let outcome = match fetch_keyset_page(
                    &table,
                    &ctx,
                    &base_query,
                    &seek_key,
                    state.tie_skip,
                    effective_page_size,
                    cursor.pinned_version(),
                    max_page_size,
                )
                .await
                {
                    Ok(o) => o,
                    Err(e) => {
                        drop(state);
                        return error_response(&e);
                    }
                };
                new_offset = state.offset + outcome.result.records.len() as u64;
                page = outcome.result;
                new_seek_key = outcome.next_seek_key;
                new_tie_skip = outcome.next_tie_skip;
                has_more = outcome.has_more;
            }
            _ => {
                // CR-B4: same peek-ahead-by-one-row trick as `create_cursor`'s
                // first page — fetch `effective_page_size + 1` internally so
                // the true end-of-data can be told apart from an exact
                // multiple of `effective_page_size`. Widen to `u64` before
                // the `+1`; `Pagination::LimitOffset::limit` is `Option<u64>`,
                // so this cannot overflow.
                let internal_limit = (effective_page_size as u64).saturating_add(1);
                let mut next_query = base_query.clone();
                next_query.pagination = Pagination::LimitOffset {
                    limit: Some(internal_limit),
                    offset: state.offset,
                };
                next_query.temporal = Temporal::AsOf {
                    at: At::Version(cursor.pinned_version()),
                };
                let mut fetched = match table
                    .read_with_encoding(&next_query, &ctx, Default::default())
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        drop(state);
                        return error_response(&wrap_engine_err(e));
                    }
                };
                // The peek row (if present) proves at least one more record
                // remains beyond `effective_page_size` — trim it off before
                // it's handed to the client or counted toward the returned
                // stats, and advance the offset bookmark by the RETURNED
                // count only (advancing by the peek-inflated fetched count
                // would skip a row on the next page).
                has_more = fetched.records.len() as u64 > effective_page_size as u64;
                if has_more {
                    fetched.records.truncate(effective_page_size as usize);
                    if let Some(stats) = fetched.stats.as_mut() {
                        stats.records_returned = fetched.records.len() as u64;
                    }
                }
                new_offset = state.offset + fetched.records.len() as u64;
                page = fetched;
                new_seek_key = None;
                new_tie_skip = 0;
            }
        }

        // CR-A5 (+ CR-B2): gate BEFORE mutating the cursor's bookmark state
        // (seek_key/offset/exhausted) or removing it from the registry on
        // exhaustion — a rejected page must leave the cursor exactly as it
        // was before this call, so the client can retry `FetchNext` (e.g.
        // with a smaller `page_size`) against an untouched bookmark instead
        // of one that silently advanced past records it never received.
        // `budget_guard` (reserved upfront, before the read above) is
        // shrunk to the actual size here rather than acquired fresh.
        if let Err(e) = enforce_page_budget(self, &page, budget_guard).await {
            drop(state);
            return error_response(&e);
        }

        state.seek_key = new_seek_key;
        state.tie_skip = new_tie_skip;
        state.offset = new_offset;
        state.exhausted = !has_more;
        drop(state);
        cursor.bump_activity();

        if !has_more {
            self.cursor_registry.remove(cursor_id.0);
        }

        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        }
    }

    /// FG-5b CANCEL — idempotent close. Canceling an unknown/already-closed
    /// cursor is NOT an error (CURSORS.md) — reply `CursorClosed` either way.
    pub(super) async fn cancel_cursor(&self, session: &Session, cursor_id: CursorId) -> DbResponse {
        // Only remove when it's actually ours; a cross-session cancel
        // attempt is silently treated the same as "already closed" (no
        // information leak about another session's cursor existing).
        if let Ok(cursor) = self
            .cursor_registry
            .get_owned(cursor_id.0, &session.session_id)
        {
            drop(cursor);
            self.cursor_registry.remove(cursor_id.0);
        }
        DbResponse::CursorClosed { cursor_id }
    }
}

fn cursor_registry_error_response(cursor_id: CursorId, e: CursorRegistryError) -> DbResponse {
    match e {
        CursorRegistryError::CursorExpired => {
            error_response(&BatchError::CursorExpired { cursor_id })
        }
        CursorRegistryError::CursorNotFound | CursorRegistryError::CursorOwnershipMismatch => {
            error_response(&BatchError::CursorNotFound { cursor_id })
        }
        CursorRegistryError::CursorLimitExceeded { limit } => {
            error_response(&BatchError::CursorLimitExceeded { limit })
        }
    }
}
