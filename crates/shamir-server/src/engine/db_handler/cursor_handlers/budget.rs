//! Byte-budget + error-shaping helpers for the cursor wire handlers.

use shamir_db::query::batch::BatchError;

use crate::byte_budget::{stash_guard, ByteBudgetGuard};
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};

/// Wrap an engine [`shamir_storage::error::DbError`] (or any `Display`
/// error) in the same `BatchError::QueryError` shape every other cursor
/// error path uses, so `error_code()` classifies it uniformly.
pub(super) fn wrap_engine_err(e: impl std::fmt::Display) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: e.to_string(),
        code: None,
    }
}

pub(in crate::engine::db_handler) fn error_response(e: &BatchError) -> DbResponse {
    if let BatchError::AsOfVisibilityTimeout {
        requested,
        visible_through,
        blocking_version,
        ..
    } = e
    {
        return DbResponse::AsOfVisibilityTimeout {
            code: "as_of_visibility_timeout".into(),
            message: e.to_string(),
            requested: *requested,
            visible_through: *visible_through,
            blocking_version: *blocking_version,
        };
    }
    DbResponse::Error {
        code: crate::db_handler::handler::error_code(e).to_string(),
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
pub(super) async fn reserve_page_budget_upfront(
    handler: &ShamirDbHandler,
) -> Option<ByteBudgetGuard> {
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

/// CR-A5 (+ CR-B2, CR-D4): gate a cursor page against BOTH the per-page
/// byte-size cap (`query_limits.max_result_size_bytes`) and the RI-15 global
/// in-flight response-byte budget, mirroring `ShamirDbHandler::execute`'s
/// exact block (`handler.rs`'s `DbRequest::Execute` path) — measure the
/// serialized `response` (the FULL `DbResponse::CursorPage { cursor_id,
/// page, has_more }` wire envelope, not just the inner `page`/`QueryResult`
/// payload — this is what `execute()` itself actually measures for the
/// plain path too: `&final_response`, i.e. the whole `DbResponse`) ONCE,
/// then either reject (too large — no budget acquired/held, there is
/// nothing to write) or finalize the budget reservation.
///
/// CR-D4 (#785, N-4): unlike the pre-existing version of this function, this
/// does NOT call `stash_serialized_response` itself — it only RETURNS the
/// serialized bytes on success. The caller (`create_cursor`/`fetch_next`)
/// stashes them only once it has confirmed the `response` value passed in
/// here really is the one being returned to the client. This matters
/// concretely for `create_cursor`'s `has_more` branch: after this function
/// accepts the page, the caller still calls `self.cursor_registry.register`,
/// which can ITSELF fail and produce a COMPLETELY DIFFERENT `DbResponse`
/// (`CursorLimitExceeded`/`cursor_error`). Stashing here, before that
/// outcome is known, would leak stale `CursorPage` bytes into the wire
/// response on a `register()` failure — a real wire-corruption bug, not
/// just a missed optimization. See `create_cursor`'s call site for exactly
/// where the stash actually happens.
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
/// Returns `Err(too_large_error)` when the page must be rejected; on
/// success, returns the serialized bytes for `response` — the RI-15 guard
/// (if any) has already been stashed via [`stash_guard`], but the SERIALIZED
/// BYTES have deliberately not been stashed (see above).
pub(super) async fn enforce_page_budget(
    handler: &ShamirDbHandler,
    response: &DbResponse,
    upfront_guard: Option<ByteBudgetGuard>,
) -> Result<Option<Vec<u8>>, BatchError> {
    // Only serialize when at least one of the two gates is actually active —
    // an unbounded budget AND an effectively-unlimited size cap (the UNIT
    // TEST default) must stay a pure no-op, same as `execute()`'s
    // `self.byte_budget.cap().is_some()` short-circuit.
    let budget_active = handler.byte_budget.cap().is_some();
    let cap_active = handler.query_limits.max_result_size_bytes < usize::MAX;
    if !budget_active && !cap_active {
        return Ok(None);
    }

    let Ok(bytes) = rmp_serde::to_vec_named(response) else {
        // Mirrors `execute()`: a serialization failure here is swallowed
        // (the `if let Ok(...)` in `execute()` silently skips the acquire
        // on `Err`) rather than treated as a hard error — the response
        // still goes out, just without budget accounting for it (and
        // without any bytes to hand back for stashing). Any upfront
        // reservation is simply dropped as-is (releasing the pessimistic
        // estimate untouched).
        return Ok(None);
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
                if bytes.len() > guard.bytes_reserved() {
                    // No unchecked growth: release the old reservation before
                    // acquiring the actual size, avoiding self-deadlock.
                    drop(guard);
                    handler.byte_budget.acquire(bytes.len()).await
                } else {
                    guard.shrink_to(bytes.len());
                    guard
                }
            }
            None => handler.byte_budget.acquire(bytes.len()).await,
        };
        stash_guard(guard);
    }

    Ok(Some(bytes))
}
