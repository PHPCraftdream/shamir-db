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
//! - **With an ORDER BY** (single column): the seek key from the last row
//!   of the previous page is AND-combined into the query's `where` as a
//!   `Gt`/`Lt` boundary (direction-dependent), and pagination stays
//!   `LimitOffset { offset: 0, limit: page_size }` — the boundary filter
//!   does the seeking, the LIMIT just caps the page. This reproduces the
//!   same "ties on the boundary value are skipped" behavior as
//!   `Pagination::After` without a tie-breaker (a pre-existing, documented
//!   limitation elsewhere in this codebase, not a new one).
//! - **Without an ORDER BY**: there is no field to build a boundary filter
//!   on, so the bookmark is a plain row-count `offset` that advances by
//!   `page_size` each `FetchNext`, resumed via `Pagination::LimitOffset`.
//!   This relies on the pinned-snapshot full scan being stable across
//!   calls at the SAME pinned version (no concurrent write can be observed
//!   — see the AsOf pin above — so the enumeration order the engine
//!   produces for a fixed `(table, version)` pair is deterministic).

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

use crate::byte_budget::stash_guard;
use crate::cursor_registry::{Cursor, CursorRegistryError};

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

/// CR-A5: gate a cursor page against BOTH the per-page byte-size cap
/// (`query_limits.max_result_size_bytes`) and the RI-15 global in-flight
/// response-byte budget, mirroring `ShamirDbHandler::execute`'s exact block
/// (`handler.rs`'s `DbRequest::Execute` path) — measure the serialized
/// `page` ONCE (matching `execute()`'s choice to measure the payload alone,
/// i.e. `BatchResponse`/here `QueryResult`, not the full `DbResponse`
/// envelope), then either reject (too large — no budget acquired, there is
/// nothing to write) or acquire from `self.byte_budget` and stash the guard
/// for the writer task to release after the socket write completes.
///
/// Returns `Err(too_large_error)` when the page must be rejected; `Ok(())`
/// when the caller may proceed to return the `CursorPage` response (a guard
/// has already been stashed for it, if the budget is bounded).
async fn enforce_page_budget(
    handler: &ShamirDbHandler,
    page: &QueryResult,
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
        // still goes out, just without budget accounting for it.
        return Ok(());
    };

    if cap_active && bytes.len() > handler.query_limits.max_result_size_bytes {
        return Err(BatchError::CursorPageTooLarge {
            size: bytes.len(),
            max: handler.query_limits.max_result_size_bytes,
        });
    }

    if budget_active {
        let guard = handler.byte_budget.acquire(bytes.len()).await;
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

/// Build the boundary filter `field > seek_key` (ASC) / `field < seek_key`
/// (DESC) for the SOLE ORDER BY column, AND-combined with the caller's
/// original `where` (if any). Only single-segment field paths are
/// supported — mirrors the brief's guidance that a keyset seek needs the
/// ORDER BY column's value; multi-column ORDER BY / nested field paths
/// fall back to the `None` (row-count `offset`) bookmark instead (see
/// `seek_key_for_query`).
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
        OrderDirection::Asc => Filter::Gt {
            field: item.field.clone(),
            value,
        },
        OrderDirection::Desc => Filter::Lt {
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
/// `false`, `FetchNext` falls back to the row-count `offset` bookmark.
fn has_simple_single_column_order_by(query: &ReadQuery) -> bool {
    match &query.order_by {
        Some(ob) => ob.items.len() == 1 && ob.items[0].field.len() == 1,
        None => false,
    }
}

/// Extract the seek value (the sole ORDER BY column's value on the LAST
/// row of a page) for the next `FetchNext`'s boundary filter. `None` when
/// the field is absent from the projected row (e.g. not selected) — the
/// caller then falls back to the row-count bookmark for correctness rather
/// than silently repeating page 1.
fn seek_value_from_last_record(query: &ReadQuery, last: &QueryRecord) -> Option<QueryValue> {
    let order_by = query.order_by.as_ref()?;
    if order_by.items.len() != 1 || order_by.items[0].field.len() != 1 {
        return None;
    }
    last.get_value(&order_by.items[0].field[0]).cloned()
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

        let mut first_query = query.clone();
        first_query.pagination = Pagination::LimitOffset {
            limit: Some(page_size as u64),
            offset: 0,
        };
        first_query.temporal = Temporal::AsOf {
            at: At::Version(pinned_version),
        };

        let page = match table
            .read_with_encoding(&first_query, &ctx, Default::default())
            .await
        {
            Ok(p) => p,
            Err(e) => return error_response(&wrap_engine_err(e)),
        };

        // CR-A5: gate the page against the per-page byte-size cap and the
        // RI-15 global byte budget BEFORE deciding whether to register the
        // cursor — a rejected page must not mint a registered cursor (there
        // is nothing to `FetchNext` against; the client never receives this
        // page's bytes at all), and this covers BOTH success returns below
        // (the CR-A2 "exhausted first page, not registered" early return and
        // the normal registered return) since both hand the SAME `page` back
        // over the wire.
        if let Err(e) = enforce_page_budget(self, &page).await {
            return error_response(&e);
        }

        let has_more = page.records.len() as u64 >= page_size as u64;
        let seek_key = if has_more && has_simple_single_column_order_by(&query) {
            page.records
                .last()
                .and_then(|r| seek_value_from_last_record(&query, r))
        } else {
            None
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
    pub(super) async fn fetch_next(
        &self,
        session: &Session,
        cursor_id: CursorId,
        page_size: u32,
    ) -> DbResponse {
        // CR-A3: validate page_size BEFORE the registry lookup — it doesn't
        // need the cursor, and this avoids a wasted registry hit (and,
        // critically, avoids ever running the has_more == 0 >= 0 → true
        // infinite-loop computation below) for a malformed request. A bad
        // page_size on one FetchNext call must not corrupt or close the
        // cursor — it isn't looked up at all here, so it stays untouched.
        let max_page_size = self.cursor_limits.max_cursor_page_size;
        if page_size == 0 || page_size > max_page_size {
            return error_response(&BatchError::InvalidPageSize {
                page_size,
                max: max_page_size,
            });
        }

        let cursor = match self
            .cursor_registry
            .get_owned(cursor_id.0, &session.session_id)
        {
            Ok(c) => c,
            Err(e) => return cursor_registry_error_response(cursor_id, e),
        };

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
        let mut next_query = base_query.clone();
        let boundary = state
            .seek_key
            .as_ref()
            .and_then(|k| boundary_filter(&base_query, k));
        match boundary {
            Some(filter) => {
                // Boundary-filter bookmark: the seek does the work, LIMIT
                // just caps the page.
                next_query.r#where = Some(filter);
                next_query.pagination = Pagination::LimitOffset {
                    limit: Some(page_size as u64),
                    offset: 0,
                };
            }
            None => {
                // Row-count bookmark: no ORDER BY, the seek field wasn't in
                // the projected row on a prior page, or the seek value's
                // type has no `FilterValue` equivalent — any case where a
                // boundary filter can't be built. Falling back here (rather
                // than silently reusing a stale/empty `where`) is what
                // keeps this correct instead of re-returning page 1 forever.
                next_query.pagination = Pagination::LimitOffset {
                    limit: Some(page_size as u64),
                    offset: state.offset,
                };
            }
        }
        next_query.temporal = Temporal::AsOf {
            at: At::Version(cursor.pinned_version()),
        };

        let page = match table
            .read_with_encoding(&next_query, &ctx, Default::default())
            .await
        {
            Ok(p) => p,
            Err(e) => return error_response(&wrap_engine_err(e)),
        };

        // CR-A5: gate BEFORE mutating the cursor's bookmark state
        // (seek_key/offset/exhausted) or removing it from the registry on
        // exhaustion — a rejected page must leave the cursor exactly as it
        // was before this call, so the client can retry `FetchNext` (e.g.
        // with a smaller `page_size`) against an untouched bookmark instead
        // of one that silently advanced past records it never received.
        if let Err(e) = enforce_page_budget(self, &page).await {
            drop(state);
            return error_response(&e);
        }

        let has_more = page.records.len() as u64 >= page_size as u64;
        let new_seek_key = if has_more && has_simple_single_column_order_by(&base_query) {
            page.records
                .last()
                .and_then(|r| seek_value_from_last_record(&base_query, r))
        } else {
            None
        };
        state.seek_key = new_seek_key;
        state.offset += page.records.len() as u64;
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
