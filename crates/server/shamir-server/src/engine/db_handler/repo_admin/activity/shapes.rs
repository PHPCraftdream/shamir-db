//! `list_shape_stats` / `reset_shape_stats` — the R4b step 4 per-shape
//! statistics surface (the second operational-views admin surface).
//!
//! Inherent methods on [`ShamirDbHandler`], routed by `execute` as the
//! single, standalone, non-transactional op of an admitted batch. Both ops
//! address LOCAL, in-memory, per-repository telemetry ([`ShapeRegistry`]):
//! no data path, no journal, no snapshot, no lock any engine writer takes,
//! and no node-mode (follower) gate — `is_write()` is `false` for both, so
//! a read-only replica serves them exactly like `ListActivity`.
//!
//! Permission (D13/D19): `ActivityView` + `Action::Manage` through the
//! normal `authorize_access` path, checked FIRST — before the page limit is
//! validated, before the repository is resolved and before the registry is
//! touched. The registry aggregates every user's workload, so there is no
//! own-scope view and no ordinary-user surface: a caller without `Manage`
//! is refused even when the repository name in the request does not exist
//! (the refusal is never an existence oracle).
//!
//! Privacy: a page carries the canonical shape hash (`shape_hex`, 32
//! lowercase hex characters) and its counters only. The hash is folded from
//! interned field ids, static tags and the table name, so it cannot render
//! a table name, a field name, a literal, a parameter name or a user back
//! to the caller; no such value can enter a row by construction. The reset
//! is audited as an admin operation and its record carries the repository
//! name only.
//!
//! [`ShapeRegistry`]: shamir_db::engine::repo::instance::support::metrics::ShapeRegistry

use std::fmt::Write as _;

use shamir_connect::server::session::Session;
use shamir_db::engine::repo::instance::support::metrics::SHAPE_ROWS_CAP;
use shamir_db::query::batch::BatchError;
use shamir_query_types::admin::shape_stats::{ListShapeStatsReq, ResetShapeStatsReq};
use shamir_query_types::admin::{ShapeStatsPage, ShapeStatsRow, ShapeStatsTotals};
use shamir_query_types::wire::DbResponse;
use shamir_types::access::{Action, ResourcePath};

use crate::engine::db_handler::cursor_handlers::error_response;
use crate::engine::db_handler::cursor_support::resolve_repo;
use crate::engine::db_handler::handler::{session_actor, ShamirDbHandler};
use crate::engine::db_handler::repo_audit::enqueue_shape_reset_audit;

/// Default page size when the request carries no `limit`.
const DEFAULT_SHAPE_LIMIT: u32 = 50;

/// Fixed refusal text. Deliberately generic: it names the permission and
/// the scope rule, never a repository, table or path from the request.
const MANAGE_REQUIRED: &str =
    "shape statistics aggregate every user's workload; ActivityView manage permission is \
     required";

fn error(code: &str, message: &str) -> DbResponse {
    DbResponse::Error {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

/// Lowercase hex rendering of the 16-byte canonical shape key — the exact
/// `ShapeKey::to_hex` form (32 characters), applied to the raw key bytes a
/// [`ShapeStatsSnapshot`] carries.
///
/// [`ShapeStatsSnapshot`]: shamir_db::engine::repo::instance::support::metrics::ShapeStatsSnapshot
fn shape_hex(key: &[u8; 16]) -> String {
    let mut text = String::with_capacity(32);
    for byte in key {
        let _ = write!(&mut text, "{byte:02x}");
    }
    text
}

impl ShamirDbHandler {
    /// Serve one bounded per-shape statistics page for one repository.
    ///
    /// Order of work is a contract, not an implementation detail:
    /// permission → page-cap validation → repository resolution → registry
    /// snapshot. A caller without `ActivityView`/`Manage` therefore learns
    /// nothing about which repositories exist, and an unauthorized caller's
    /// reply is byte-identical regardless of the requested names.
    ///
    /// A `limit` of `0` is a typed `invalid_request` (the same rule as
    /// `list.rs`'s `page_cap`); any larger value clamps to the registry cap
    /// ([`SHAPE_ROWS_CAP`]) instead of being rejected. Rows are ordered by
    /// `total_nanos` desc, then `executions` desc, then `shape_hex` asc — a
    /// total, deterministic order that never depends on hash iteration.
    pub(in crate::engine::db_handler) async fn list_shape_stats(
        &self,
        session: &Session,
        db_name: &str,
        req: &ListShapeStatsReq,
    ) -> DbResponse {
        // 1. Permission FIRST — before any limit check, repository lookup
        //    or registry touch.
        if self
            .db
            .authorize_access(
                &session_actor(session),
                &ResourcePath::ActivityView,
                Action::Manage,
            )
            .await
            .is_err()
        {
            return error("permission_denied", MANAGE_REQUIRED);
        }

        // 2. Page cap: form validation before any lookup or registry work.
        let limit = match req.limit {
            Some(0) => return error("invalid_request", "limit must be at least 1"),
            Some(n) => n.min(SHAPE_ROWS_CAP as u32),
            None => DEFAULT_SHAPE_LIMIT,
        } as usize;

        // 3. Resolve the repository. A failure keeps the existing stable
        //    typed refusal (`unknown_db` / `unknown_repo`): the shared
        //    `error_response` conversion, never a new code and never a raw
        //    string.
        let repo = match resolve_repo(&self.db, db_name, &req.repo) {
            Ok(repo) => repo,
            Err(e) => return error_response(&e),
        };

        // 4. Cold-path snapshot of the lock-free registry (in memory).
        let registry = repo.query_metrics().shape_stats();
        let snapshot = registry.snapshot();

        let mut rows: Vec<ShapeStatsRow> = snapshot
            .rows
            .iter()
            .map(|(key, counters)| ShapeStatsRow {
                shape_hex: shape_hex(key),
                executions: counters.executions,
                total_nanos: counters.total_nanos,
                errors: counters.errors,
            })
            .collect();
        // Total order over the rendered counters only: no reliance on the
        // hash map's iteration order, so two calls against unchanged
        // counters always render the same page.
        rows.sort_by(|a, b| {
            b.total_nanos
                .cmp(&a.total_nanos)
                .then_with(|| b.executions.cmp(&a.executions))
                .then_with(|| a.shape_hex.cmp(&b.shape_hex))
        });
        let truncated = rows.len() > limit;
        rows.truncate(limit);

        DbResponse::ShapeStatsPage {
            page: ShapeStatsPage {
                rows,
                other: ShapeStatsTotals {
                    executions: snapshot.other.executions,
                    total_nanos: snapshot.other.total_nanos,
                    errors: snapshot.other.errors,
                },
                tracked: snapshot.tracked as u32,
                truncated,
            },
        }
    }

    /// Wipe one repository's local shape registry (rows AND the `(other)`
    /// fold), then answer the unit acknowledgement.
    ///
    /// Same permission-first ordering as [`Self::list_shape_stats`]: the
    /// wipe is never reached by a caller without `ActivityView`/`Manage`,
    /// and an unknown repository is refused with the same stable typed
    /// error (never a silent no-op).
    ///
    /// The audit record is appended through the operator-cancel audit path
    /// ([`enqueue_shape_reset_audit`]): a detached, bounded, at-most-N
    /// queue whose details carry the repository name ONLY. An audit-less
    /// handler skips the append silently, exactly like the cancel path, and
    /// the reply never awaits audit I/O.
    ///
    /// Cancellation: the wipe itself is the commit point, so the reset
    /// claims the commit slot ([`shamir_tx::CancelHandle::try_commit`])
    /// before touching the registry — the same idiom the batch runner
    /// uses for independent effects. A cancel that already won makes this
    /// reset answer the typed `cancelled` and wipe nothing; a cancel
    /// arriving after the claim answers `already_committing`.
    pub(in crate::engine::db_handler) async fn reset_shape_stats(
        &self,
        session: &Session,
        db_name: &str,
        req: &ResetShapeStatsReq,
        cancel: &shamir_tx::CancelHandle,
    ) -> DbResponse {
        // 1. Permission FIRST — before the repository lookup and before any
        //    registry state is touched.
        if self
            .db
            .authorize_access(
                &session_actor(session),
                &ResourcePath::ActivityView,
                Action::Manage,
            )
            .await
            .is_err()
        {
            return error("permission_denied", MANAGE_REQUIRED);
        }

        // 2. Resolve the repository (stable `unknown_db` / `unknown_repo`).
        let repo = match resolve_repo(&self.db, db_name, &req.repo) {
            Ok(repo) => repo,
            Err(e) => return error_response(&e),
        };

        // 3. Wipe the node-local telemetry, then enqueue the detached admin
        //    audit. Nothing in this path touches data state, so no
        //    transaction/visibility wait applies.
        //
        // The wipe is the commit point: claim it first, so a cancellation
        // that already won is answered instead of being overwritten by an
        // irreversible side effect.
        #[cfg(test)]
        self.shape_reset_pause_at(false);
        if !cancel.try_commit() && cancel.state() != shamir_tx::CancelState::Committing {
            return error_response(&BatchError::Cancelled);
        }
        #[cfg(test)]
        self.shape_reset_pause_at(true);
        repo.query_metrics().shape_stats().reset();
        if let Some(audit) = &self.audit {
            drop(enqueue_shape_reset_audit(
                &self.cancel_refusal_audit,
                audit,
                session,
                &req.repo,
            ));
        }
        DbResponse::ShapeStatsReset
    }
}
