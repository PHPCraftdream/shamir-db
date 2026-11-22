//! Operator-state and narrow data read handlers (admin-shell PR3).
//!
//! Holds BOTH PR3 halves: the P2 exact-key point read
//! ([`DbRequest::GetRecord`] → [`DbResponse::Record`]) and the P1 typed
//! `status` request ([`DbRequest::Status`] → [`DbResponse::Status`]).
//! The module name is the design's (`operator_state.rs`), mounted
//! through the same `#[path = "repo_admin/..."]` mechanism as
//! `cursor_support` / `execute` / `tx_handlers`, because `db_handler/`
//! itself is over its folder-entry limit while `repo_admin/` has room.
//!
//! # P2 semantics (design §4.4/§4.11)
//!
//! A typed, top-level point read — NOT a `BatchOp`, no cursor is
//! created and no cursor path is engaged:
//!
//! 1. Name shape FIRST: a `db`/`repo`/`table` name of 0 or more than 255
//!    UTF-8 bytes is a fixed-text `invalid_request` that never echoes the
//!    input, so the error envelope is bounded by construction. Then
//!    authorization before any lookup: the exact two-call ACL of
//!    [`authorize_cursor_read`] (`Action::Read` on the `Database`, then
//!    on the `Table`; the ancestor walk inside `authorize_access` covers
//!    the `Store` link). Authorizing before resolution keeps an
//!    unauthorized caller's response indistinguishable from a genuine
//!    not-found (same property `get_ddl_op_status` relies on).
//! 2. The same repository quarantine probe as the cursor read path
//!    ([`quarantine_cursor_error`] — one `read_quarantine_reason`
//!    atomic-map probe, the same reason the batch path carries inside
//!    `BatchError::ResyncRequired`). The stored reason is deliberately
//!    NOT forwarded on this path: an identity-fence reason can name
//!    recorded and observed replication identities, which a table reader
//!    without subscription-admin rights must not receive, so `GetRecord`
//!    answers with the fixed [`quarantined_repo_response`] text under the
//!    same typed code. The shared batch path keeps its own reason.
//! 3. Per-session in-flight slot (`Session::try_acquire_request_slot`,
//!    after auth — an exhausted session gets the fixed-text `server_busy`
//!    rejection before any engine work; stateless handle-0 sessions skip
//!    the cap, same as `try_track_activity_request`).
//! 4. A single fixed-16-byte [`RecordId`] lookup. Resolve the parent
//!    database/repository with `resolve_repo` (typed `unknown_db` /
//!    `unknown_repo` from its explicit `BatchError.code`) and refuse an
//!    unconfigured table with `unknown_table` BEFORE the budget wait and
//!    before `ShamirDb::get_table` (whose cold resolver would insert a
//!    catalogue cell for the unknown name). Capture the authorized table
//!    REGISTRATION as a `TableRegistration` token (see
//!    `TableCatalog::table_registration`) plus its `store_id`. After the
//!    parking point (a RESPONSE-BUDGET wait, never a visibility wait) the
//!    same ACL is re-run, the name is re-resolved and the registration is
//!    compared with the captured token; `TableRegistration` is the
//!    catalogue cell the registry handed out, so it distinguishes ANY two
//!    registrations, including two legacy tables that both carry
//!    `store_id: None`. Three outcomes, in this order: (a) registration
//!    unchanged — read normally (a legacy table with no store id is
//!    readable: its cell exists, so it always pins); (b) registration
//!    replaced, or no identity could be pinned for it — the typed
//!    `access_denied` with the fixed [`changed_registration_response`]
//!    text (never a payload, never an identity); (c) the NAME is gone with
//!    no replacement — `unknown_table` ([`unknown_table_response`]):
//!    absence is reported as absence.
//!    Then acquire the table through `ShamirDb::get_table`, which
//!    rechecks the repository quarantine at acquisition; the table's own
//!    `get` rechecks its reopen fence. That last call is inside the
//!    documented residual DDL window between the comparison and the
//!    handle acquisition, which this handler does NOT close. A typed
//!    `DbError::NotFound` there is mapped by
//!    [`final_lookup_error_response`]: a parent that disappeared is
//!    reported with ITS typed name-resolution code (`unknown_db` /
//!    `unknown_repo`, the same as without the wait), only a `NotFound`
//!    whose parents still resolve is `unknown_table`; other error variants
//!    retain their distinct mappings. There is no table-list scan, record
//!    scan, or cursor.
//! 5. The record is converted through the SAME decoding the batch read
//!    path uses: `InnerValue` → `QueryValue` via the table's interner
//!    read view (`inner_to_query_value`, the changelog decoder's codec),
//!    wrapped as the batch path's canonical `QueryRecord::Direct`.
//! 6. The existing result controls, mirrored from
//!    `cursor_handlers/budget.rs` and the batch `execute` path: an
//!    upfront RI-15 reservation when the budget AND a finite result cap
//!    are active, one serialization of the full `DbResponse` envelope,
//!    rejection (`result_too_large`) when it exceeds
//!    `query_limits.max_result_size_bytes`, shrink of the reservation to
//!    the actual size, and a stash of the exact bytes via
//!    `byte_budget::stash_serialized_response` so
//!    `RequestHandler::handle` reuses them instead of re-encoding.

use core::num::NonZeroU64;
use std::time::{SystemTime, UNIX_EPOCH};

use shamir_collections::TFxMap;

use shamir_connect::server::session::Session;
use shamir_db::engine::repo::durability_policy::DurabilityMetricsSnapshot;
use shamir_db::engine::repo::DurabilityPolicy;
use shamir_query_types::durability::DurabilityOverride;
use shamir_query_types::read::QueryRecord;
use shamir_query_types::wire::{
    DbResponse, OperatorStatus, StatusReplication, StatusRepository, StatusRole,
    StatusStampOutcome, StatusWal,
};
use shamir_types::types::record_id::RecordId;

use super::activity::server_busy_response;
use super::config::NodeMode;
use crate::byte_budget::{stash_guard, stash_serialized_response, ByteBudgetGuard};
use crate::db_handler::cursor_handlers::error_response;
use crate::db_handler::cursor_support::{authorize_cursor_read, resolve_repo};
use crate::db_handler::handler::{session_actor, ShamirDbHandler};
use crate::db_handler::visibility_handlers::quarantine_cursor_error;

/// Longest accepted `db`/`repo`/`table` name, in UTF-8 bytes: the
/// PHYSICAL keyspace-name limit of the storage layer (fjall repo keyspace
/// names, `crates/storage/shamir-storage/src/stores/storage_fjall/repo.rs:22-26`,
/// mirrored for generated table store names in
/// `crates/engine/shamir-engine-table/src/support/model/names.rs:10-16`),
/// reused rather than invented.
///
/// This is NOT the logical DB/repository name limit: the DDL validators
/// (`crates/engine/shamir-db/src/shamir_db/execute/core/helpers.rs`,
/// `validate_ident`) accept longer names, so a database or repository whose
/// logical name exceeds 255 bytes can exist and still persist, while a
/// point read cannot ADDRESS it — the fixed [`invalid_name_response`] below
/// is then the honest answer. Do not claim parity with the logical limit
/// and do not tighten the DDL validators to match.
const MAX_NAME_BYTES: usize = u8::MAX as usize; // 255

/// Name-shape bound (1..=255 UTF-8 bytes, see [`MAX_NAME_BYTES`]).
fn name_in_bounds(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES
}

/// Fixed-text refusal for an out-of-bounds name. Never echoes the input, so
/// the `GetRecord` error envelope stays bounded by construction.
fn invalid_name_response() -> DbResponse {
    DbResponse::Error {
        code: "invalid_request".to_string(),
        message: "db, repo and table names must be 1..=255 UTF-8 bytes".to_string(),
    }
}

/// Fixed-text refusal for a table that is not CONFIGURED in the repository —
/// raised before the cold table resolver. `ShamirDb::get_table` inserts a
/// catalogue cell for an unregistered name before checking configuration
/// (`insert_table_cell`,
/// `crates/engine/shamir-engine/src/tx/commit/core/catalog_access.rs:186`,
/// retained by `crates/engine/shamir-engine-table/src/support/storage/catalog.rs:120`).
/// The batch path shares that retention defect; it is out of scope here.
/// The message never echoes the caller's name.
fn unknown_table_response() -> DbResponse {
    DbResponse::Error {
        code: "unknown_table".to_string(),
        message: "table is not registered in the repository".to_string(),
    }
}

/// Fixed-text refusal for a table whose REGISTRATION is no longer the one the
/// ACL approved, and for a registration that cannot be pinned at all. Same
/// typed code the ACL refusals use; the message never names the identity.
fn changed_registration_response() -> DbResponse {
    DbResponse::Error {
        code: "access_denied".to_string(),
        message: "the authorized table registration changed during the request".to_string(),
    }
}

/// Fixed-text refusal for a quarantined repository on the POINT-READ path. The
/// stored reason can name recorded and observed replication identities, which a
/// table reader without subscription-admin rights must not receive; the shared
/// batch path keeps its own detail-carrying message. Same typed code.
fn quarantined_repo_response() -> DbResponse {
    DbResponse::Error {
        code: "resync_required".to_string(),
        message: "repository is quarantined; resynchronize before reading".to_string(),
    }
}

/// Final-lookup miss handling. A parent (database or repository) that
/// disappeared between the registration check and the handle acquisition must
/// report ITS typed code (`unknown_db` / `unknown_repo`), never an
/// `unknown_table` mislabel. A `NotFound` whose parents still resolve is the
/// missing table itself.
fn final_lookup_error_response(
    db: &shamir_db::ShamirDb,
    db_name: &str,
    repo_name: &str,
    error: shamir_db::DbError,
) -> DbResponse {
    match error {
        shamir_db::DbError::NotFound(_) => match resolve_repo(db, db_name, repo_name) {
            Ok(_) => unknown_table_response(),
            Err(parent) => error_response(&parent),
        },
        other => db_error_response(other),
    }
}

/// Map an engine [`shamir_db::DbError`] onto the coarse wire `code` tag,
/// mirroring `handler.rs`'s `get_ddl_op_status` mapping (`validation` /
/// `not_found` / `query`) with one addition: the quarantine gate inside
/// `ShamirDb::get_table` surfaces as `resync_required` (defense in
/// depth — the probe above normally fires first).
fn db_error_response(e: shamir_db::DbError) -> DbResponse {
    let code = match &e {
        shamir_db::DbError::NotFound(_) => "not_found",
        shamir_db::DbError::Validation(_) => "validation",
        shamir_db::DbError::ResyncRequired(_) => "resync_required",
        _ => "query",
    };
    DbResponse::Error {
        code: code.to_string(),
        message: e.to_string(),
    }
}

/// CR-B2-shaped upfront RI-15 reservation, mirroring
/// `cursor_handlers/budget.rs::reserve_page_budget_upfront`: reserve the
/// pessimistic full-cap slice only when BOTH the RI-15 budget is active
/// AND `query_limits.max_result_size_bytes` is a finite bound; otherwise
/// `None` (an unbounded case must stay a pure no-op — there is no
/// natural estimate to invent, per the CR-A4 precedent).
async fn reserve_record_budget_upfront(handler: &ShamirDbHandler) -> Option<ByteBudgetGuard> {
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

impl ShamirDbHandler {
    /// P2 — `DbRequest::GetRecord`: the typed exact-key point read.
    /// See the module docs for the gate ordering and the budget story.
    pub(super) async fn get_record(
        &self,
        session: &Session,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        key: RecordId,
    ) -> DbResponse {
        // 1. Bound every caller-supplied name BEFORE anything can echo or
        //    resolve it: the `GetRecord` error envelope is bounded by
        //    construction, and an over-long name can never reach the
        //    catalogue.
        if !name_in_bounds(db_name) || !name_in_bounds(repo_name) || !name_in_bounds(table_name) {
            return invalid_name_response();
        }

        // 2. Authorization BEFORE any lookup (Database Read, then Table Read).
        //    Cheap refusals must not wait for the budget, so this stays ahead
        //    of the parking point below.
        let actor = session_actor(session);
        let authorized =
            authorize_cursor_read(&self.db, &actor, db_name, repo_name, table_name).await;
        if let Err(e) = authorized {
            return error_response(&e);
        }

        // 3. Per-session in-flight slot (AFTER auth, mirroring the batch
        //    `execute` path): an exhausted session gets the fixed-text
        //    `server_busy` rejection before any engine work. The stateless
        //    handle-0 fallback skips the cap exactly like
        //    `try_track_activity_request`. The slot is held to the end of
        //    the request (drop on every early-return path).
        let _request_slot = if session.activity_handle() == 0 {
            // Stateless token-REST fallback: no minted handle, no slot.
            None
        } else {
            match session
                .try_acquire_request_slot(self.query_limits.max_in_flight_requests_per_session)
            {
                Some(slot) => Some(slot),
                None => return server_busy_response(),
            }
        };

        // 4. The same repository quarantine the cursor read path probes
        //    (the batch path carries the same reason inside its own error).
        //    The stored reason is deliberately not forwarded on this path (it
        //    can name replication identities); only the point read is changed.
        if quarantine_cursor_error(&self.db, db_name, repo_name).is_some() {
            return quarantined_repo_response();
        }

        // 5. Typed parent resolution + configured-table check + capture of
        //    the authorized incarnation, all BEFORE the budget wait and
        //    before any `ShamirDb::get_table` call. `resolve_repo` gives the
        //    typed `unknown_db` / `unknown_repo` codes (authorized callers
        //    only — strictly after the two-call ACL). `has_table` is a cheap
        //    catalogue `configs.contains_key` read; an unregistered name is
        //    refused here because the cold table resolver would insert a
        //    catalogue cell for it (see `unknown_table_response`). The
        //    captured `store_id` is the incarnation identity of the table the
        //    ACL just approved — re-read after the parking point below.
        let repo = match resolve_repo(&self.db, db_name, repo_name) {
            Ok(repo) => repo,
            Err(parent) => return error_response(&parent),
        };
        if !repo.has_table(table_name) {
            return unknown_table_response();
        }
        let Some(authorized_config) = repo.table_config(table_name) else {
            return unknown_table_response();
        };
        let authorized_store_id = authorized_config.store_id;
        // Fail closed: a registration that cannot be pinned is never read
        // after a park. `store_id` alone is NOT an identity — a legacy table
        // has `store_id: None`, so two distinct legacy registrations compare
        // equal; the token is the catalogue cell this registration reserved
        // and is held until the response is built.
        let Some(authorized_registration) = repo.table_registration(table_name) else {
            return changed_registration_response();
        };

        // 6. Upfront RI-15 slice (both gates active only — see above),
        //    released by drop on every early-return path below.
        let budget_guard = reserve_record_budget_upfront(self).await;

        // 7. Re-run the SAME two-call ACL now that every parking point is
        //    behind us, then re-resolve and re-check the captured
        //    incarnation. WHY the seam is here and nowhere else: name-based
        //    authorization is a shared seam with the batch and cursor read
        //    paths, and the re-run only narrows the park→resolve window — it
        //    does NOT close a DDL racing between this re-check and the handle
        //    acquisition below. No new ACL seam may be invented here.
        if let Err(e) =
            authorize_cursor_read(&self.db, &actor, db_name, repo_name, table_name).await
        {
            return error_response(&e);
        }
        let repo = match resolve_repo(&self.db, db_name, repo_name) {
            Ok(repo) => repo,
            Err(parent) => return error_response(&parent),
        };
        // Absence is reported as absence: a table that disappeared with no
        // replacement stays `unknown_table`; only a replacement is the
        // changed-registration refusal.
        if !repo.has_table(table_name) {
            return unknown_table_response();
        }
        let Some(current_config) = repo.table_config(table_name) else {
            return unknown_table_response();
        };
        let Some(current_registration) = repo.table_registration(table_name) else {
            return changed_registration_response();
        };
        if current_config.store_id != authorized_store_id
            || !authorized_registration.same_registration(&current_registration)
        {
            return changed_registration_response();
        }

        // 8. Table acquisition goes through `ShamirDb::get_table` so its
        //    repository quarantine gate is re-checked AT ACQUISITION,
        //    closing the probe→lookup race. The table's own `get` preserves
        //    the reopen fence. A typed `NotFound` here can only be the
        //    missing table because parent resolution and `has_table` both
        //    succeeded; no list-table scan is needed. No record scan, no
        //    cursor.
        let table = match self.db.get_table(db_name, repo_name, table_name).await {
            Ok(table) => table,
            Err(e) => return final_lookup_error_response(&self.db, db_name, repo_name, e),
        };
        let inner = match table.get(key).await {
            Ok(inner) => inner,
            Err(e) => return db_error_response(e),
        };

        // 9. Same decoding as the batch read path: interner-backed
        //    `InnerValue` → `QueryValue`, wrapped as `QueryRecord::Direct`.
        let value = match table
            .info_store_read_view()
            .inner_to_query_value(&inner)
            .await
        {
            Ok(value) => value,
            Err(e) => return db_error_response(e),
        };
        let response = DbResponse::Record {
            record: QueryRecord::Direct(value),
        };

        // 10. Result controls — measure the FULL `DbResponse` envelope once,
        //    reject oversize (`result_too_large`, mirroring `execute`'s
        //    rejection shape), shrink/reacquire the RI-15 reservation, then
        //    stash the exact bytes so `RequestHandler::handle` reuses them
        //    (this `response` IS the final return value — nothing can
        //    replace it past this point, so stashing here is safe; the
        //    CR-D4 concern does not apply).
        let budget_active = self.byte_budget.cap().is_some();
        let cap_active = self.query_limits.max_result_size_bytes < usize::MAX;
        if budget_active || cap_active {
            let Ok(bytes) = rmp_serde::to_vec_named(&response) else {
                // Mirror `execute`/`enforce_page_budget`: a serialization
                // failure here is swallowed — the response still goes out,
                // just without budget accounting; any upfront reservation
                // is dropped as-is.
                return response;
            };
            if cap_active && bytes.len() > self.query_limits.max_result_size_bytes {
                // Rejected — release the pessimistic reservation; never
                // stash the rejected payload.
                drop(budget_guard);
                return DbResponse::Error {
                    code: "result_too_large".to_string(),
                    message: format!(
                        "encoded response is {} bytes; maximum is {}",
                        bytes.len(),
                        self.query_limits.max_result_size_bytes
                    ),
                };
            }
            if budget_active {
                let guard = match budget_guard {
                    Some(mut guard) => {
                        if bytes.len() > guard.bytes_reserved() {
                            // No unchecked growth: release before
                            // re-acquiring, avoiding self-deadlock.
                            drop(guard);
                            self.byte_budget.acquire(bytes.len()).await
                        } else {
                            guard.shrink_to(bytes.len());
                            guard
                        }
                    }
                    None => self.byte_budget.acquire(bytes.len()).await,
                };
                stash_guard(guard);
            }
            stash_serialized_response(bytes);
        }

        response
    }
}

/// Mirror the replication owner's `StampOutcome` onto the wire enum
/// (names kept in sync by hand — six variants, no invention).
fn status_stamp_outcome(outcome: shamir_sync::format_stamp::StampOutcome) -> StatusStampOutcome {
    match outcome {
        shamir_sync::format_stamp::StampOutcome::Created => StatusStampOutcome::Created,
        shamir_sync::format_stamp::StampOutcome::Adopted => StatusStampOutcome::Adopted,
        shamir_sync::format_stamp::StampOutcome::Unchanged => StatusStampOutcome::Unchanged,
        shamir_sync::format_stamp::StampOutcome::VersionUpdated => {
            StatusStampOutcome::VersionUpdated
        }
        shamir_sync::format_stamp::StampOutcome::InspectedStampless => {
            StatusStampOutcome::InspectedStampless
        }
        shamir_sync::format_stamp::StampOutcome::InspectedOlder => {
            StatusStampOutcome::InspectedOlder
        }
    }
}

/// Live WAL telemetry from the repo-owned durability snapshot, read at
/// the ACTUAL export indices (`durability_export.rs` renders gauges and
/// counters positionally; see `DurabilityMetricsSnapshot`'s slots).
///
/// First-pending pairing: the age is present exactly when the version is.
/// A zero age is a LOWER BOUND, not proof that the pending span was just
/// observed: the exporter
/// (`crates/engine/shamir-engine-repo/src/journal/backend/durability_policy/durability_metrics.rs`)
/// also yields zero when the sample is not comparable with the previous one
/// (a delayed or concurrent observation whose first-uncovered position
/// differs, so the earlier `since` watermark cannot be reused).
pub(super) fn status_wal(snapshot: &DurabilityMetricsSnapshot) -> StatusWal {
    let first_pending_version = (snapshot.gauges[9] != 0).then_some(snapshot.gauges[9]);
    StatusWal {
        backend_mem: snapshot.gauges[2] == 1,
        watermark_w: snapshot.gauges[4],
        recoverable_through: snapshot.gauges[5],
        pending_versions: snapshot.gauges[8],
        first_pending_version,
        first_pending_age_ns: first_pending_version.map(|_| snapshot.gauges[10]),
        sync_now_calls: snapshot.counters[0],
        sync_rounds: snapshot.counters[1],
        sync_coalesced: snapshot.counters[2],
        sync_errors: snapshot.counters[3],
        sync_duration_ns: snapshot.counters[4],
    }
}

/// Effective policy → override fields (the same projection as the ALTER
/// path's `effective_durability_record`: `effective_wal_commit()` is the
/// tier that actually applies on this backend).
fn effective_override(policy: &DurabilityPolicy) -> DurabilityOverride {
    DurabilityOverride {
        wal_commit: Some(policy.effective_wal_commit()),
        wal_retention_max_bytes: NonZeroU64::new(policy.wal_retention_max_bytes()),
        on_wal_retention_exceeded: Some(policy.on_wal_retention_exceeded()),
        replication_serve: Some(policy.replication_serve()),
    }
}

fn unix_now_ns() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
}

impl ShamirDbHandler {
    /// P1 — `DbRequest::Status`: the typed operator state snapshot
    /// (design §4.2). Superuser-class read; no stamp or data-dir
    /// inspection at request time (the system-catalogue override read MAY
    /// touch disk) — everything observed comes from the launcher's
    /// boot capture, the replication owner's exported snapshot, the
    /// open repos' durability telemetry, and ONE system-store catalogue
    /// read of the stored overrides. A handler built outside the server
    /// boot path (`operator_boot: None`, direct fixtures) is a typed
    /// `not_supported` — never faked.
    ///
    /// Accepted limits, stated explicitly:
    ///
    /// 1. The stored-override catalogue read happens at REQUEST time through
    ///    the running Fjall system store, so `status` (and `version show`)
    ///    can fail with a typed code if that read fails; only the boot STAMP
    ///    is never reread.
    /// 2. The replication observations come from a PROCESS-GLOBAL registry,
    ///    so two embedded launchers in one process observe each other (under
    ///    the per-repository caveat an overflow may show as `(other)/(other)`).
    /// 3. The role mapping is `NodeMode::ReadOnly → follower`, every other
    ///    configuration → `unknown` — never inferred from a local stream.
    pub(super) async fn operator_status(&self, session: &Session) -> DbResponse {
        if !session.permissions.is_superuser {
            return DbResponse::Error {
                code: "permission_denied".to_string(),
                message: "status requires superuser".to_string(),
            };
        }
        let Some(boot) = self.operator_boot.as_ref() else {
            return DbResponse::Error {
                code: "not_supported".to_string(),
                message: "status is unavailable outside the server launch path".to_string(),
            };
        };

        // Take the application request slot exactly like the other typed
        // admin requests that hold a result, with the same stateless-handle-0
        // skip the point read uses. Held (by drop at end of scope) until the
        // response below is built.
        let _request_slot = if session.activity_handle() == 0 {
            // Stateless token-REST fallback: no minted handle, no slot.
            None
        } else {
            match session
                .try_acquire_request_slot(self.query_limits.max_in_flight_requests_per_session)
            {
                Some(slot) => Some(slot),
                None => return server_busy_response(),
            }
        };

        let stored_overrides: TFxMap<(String, String), Option<DurabilityOverride>> = match self
            .db
            .system_store()
            .load_repository_durability_overrides()
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(db, repo, stored)| ((db, repo), stored))
                .collect(),
            Err(e) => return db_error_response(e),
        };

        // Design §6: configured ReadOnly node ⇒ Follower; every other
        // configuration (including ReadWrite / replication disabled) is
        // Unknown — nothing is invented.
        let role = match self.node_mode {
            NodeMode::ReadOnly => StatusRole::Follower,
            NodeMode::ReadWrite => StatusRole::Unknown,
        };

        let now_ns = unix_now_ns();
        let replication: Vec<StatusReplication> =
            shamir_sync::replication::replication_metrics_snapshot()
                .into_iter()
                .map(|s| StatusReplication {
                    db: s.db,
                    repo: s.repo,
                    gap_versions_merged: s.lag_versions,
                    // No timestamp plumbing exists: always `unknown`.
                    age: None,
                    receipt_age_ns: match now_ns {
                        Some(now) if s.last_receipt_ns != 0 => {
                            Some(now.saturating_sub(s.last_receipt_ns))
                        }
                        _ => None,
                    },
                })
                .collect();

        let mut repositories = Vec::new();
        for db_name in self.db.list_dbs() {
            let Some(db) = self.db.get_db(&db_name) else {
                continue;
            };
            for repo_name in db.list_repos() {
                let Some(repo) = db.get_repo(&repo_name) else {
                    continue;
                };
                let snapshot = repo.durability_metrics_snapshot();
                let policy = repo.durability_policy();
                let stored = stored_overrides
                    .get(&(db_name.clone(), repo_name.clone()))
                    .cloned()
                    .flatten();
                repositories.push(StatusRepository {
                    db: db_name.clone(),
                    repo: repo_name.clone(),
                    effective: effective_override(&policy),
                    stored,
                    wal: status_wal(&snapshot),
                });
            }
        }

        let response = DbResponse::Status {
            status: OperatorStatus {
                server_version: env!("CARGO_PKG_VERSION").to_string(),
                format_epoch: boot.observed_format_epoch,
                stamp_outcome: status_stamp_outcome(boot.format_stamp),
                uptime_ms: u64::try_from(boot.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
                role,
                node_id: boot.node_id.clone(),
                replication,
                repositories,
            },
        };

        // Same result-size cap the batch path applies: measure the encoded
        // reply once and refuse it whole — never a partial reply. A
        // serialization failure keeps today's behaviour (the response still
        // goes out, just unmeasured). The measured buffer is NOT handed on:
        // it is dropped here and the writer serializes this response once
        // more, so a `status` pays two encodings. That is accepted
        // duplicate work — weaker than the batch handoff on purpose, since
        // stashing would need a second ownership path through the generic
        // writer — and it never bypasses the cap (the reply measured and
        // the reply sent are the same value).
        if self.query_limits.max_result_size_bytes < usize::MAX {
            if let Ok(bytes) = rmp_serde::to_vec_named(&response) {
                if bytes.len() > self.query_limits.max_result_size_bytes {
                    return DbResponse::Error {
                        code: "result_too_large".to_string(),
                        message: format!(
                            "encoded response is {} bytes; maximum is {}",
                            bytes.len(),
                            self.query_limits.max_result_size_bytes
                        ),
                    };
                }
            }
        }

        response
    }
}
