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
//! page can never flip coordinate systems OPPORTUNISTICALLY or repeatedly
//! mid-scroll. CR-D1 (#782) below is the one narrow, DELIBERATE exception:
//! a single, one-time, detected-failure-condition transition, not a
//! per-page re-derivation.
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

mod budget;
mod fetch;
mod handlers;
mod keyset;

pub(super) use budget::error_response;
