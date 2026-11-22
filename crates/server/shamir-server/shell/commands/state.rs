//! State and narrow data view commands (design `admin-shell.md` §4.2, §4.4,
//! §9 PR3): `state status`, `version show`, `replication subscriptions`,
//! `data count`, `data head`, `data get`.
//!
//! Mapping evidence (all OBSERVED on this branch):
//! - `state status` / `version show` → the typed top-level
//!   `DbRequest::Status` (wire `db_message.rs`) through the SDK's
//!   `Client::status` (`client/api/admin.rs`); the reply is
//!   `DbResponse::Status { status: OperatorStatus }`. The observed format
//!   epoch and stamp outcome were captured by the server LAUNCHER at boot;
//!   the shell never reads the data directory.
//! - `replication subscriptions` → the EXISTING `ReplicationStatus` admin op
//!   through the existing batch path (no wire change), built with
//!   `shamir_query_builder::ddl::replication::replication_status()`.
//! - `data count` → `Query::with_repo(..).select([count_all])`, limit only,
//!   no ORDER BY, through the batch path (`BatchOp::Read`).
//! - `data head` → `Query::with_repo(..).limit(min(rows, 100))`, no ORDER BY
//!   (no ordering promise), through the batch path; shell-side caps:
//!   100 rows, 256 KiB of projected row bytes, and a 5 s MONOTONIC
//!   whole-operation budget (request + projection) sharing one deadline
//!   `ShellError::HeadWallTime`, exit 7, outcome unknown).
//! - `data get` → the typed top-level `DbRequest::GetRecord` through the
//!   SDK's `Client::get_record` (`client/api/request.rs`); NOT a batch op
//!   and NOT a cursor. The KEY is a Base58 `RecordId`, validated HERE
//!   (before any connection) with the fixed typed usage refusal.
//!
//! Errors: server failures route through `crate::shell::connect::request_error`
//! (allowlisted stable codes only, never a verbatim server string). A typed
//! `not_found` from the P2 point read maps to `ShellError::NotFound`.
//! Every authorized string is sanitized through `output::value` /
//! `output::sanitize`; peer-supplied text is never echoed raw.

use std::time::Duration;

use serde_json::{json, Value};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::replication::replication_status;
use shamir_query_builder::select::count_all;
use shamir_query_builder::Query;
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_query_types::wire::db_message::{
    OperatorStatus, StatusRepository, StatusRole, StatusStampOutcome, StatusWal,
};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use super::super::output::{self, value, ShellError};
use crate::shell::connect::{request_error, SessionClient};

/// `data head` row cap (design §4.4): at most 100 rows are requested and
/// projected; a larger `rows` argument is clamped and the clamp is visible
/// in the output (`rows_requested`).
const HEAD_MAX_ROWS: u64 = 100;

/// `data head` projected-row byte cap: once the serialized rows exceed
/// this, no further rows are added and the bytes marker is set.
const HEAD_MAX_ROW_BYTES: usize = 256 * 1024;

/// `data head` wall-clock budget covering the WHOLE operation — the request
/// wait AND the local projection loop — as ONE monotonic deadline: expiry
/// is the typed `head_wall_time` outcome-unknown refusal (exit 7), never a
/// partial result. The request phase uses `tokio::time::timeout_at`; the
/// projection loop compares the same deadline INSIDE the loop (a `tokio`
/// timeout cannot preempt a ready CPU-bound projection loop), before and
/// after the last row.
const HEAD_WALL: Duration = Duration::from_secs(5);

/// `data get` KEY cap in bytes before decoding: a Base58 `RecordId` is
/// exactly 22 characters (ceil(log_58(2^128)) = 22); anything longer is
/// refused without parsing — the decoder under `RecordId::from_str` has no
/// internal length bound, so the shell caps first. The server applies its
/// own typed-key cap as well.
const GET_KEY_MAX_BYTES: usize = 22;

/// Response alias for the single-entry batch (`data count`, `data head`,
/// `replication subscriptions`).
const RESULT_ALIAS: &str = "result";

/// Fixed merged-gap caveat (design §4.2): the shell NEVER uses the word
/// "lag"; the field is named and its non-authoritative nature stated.
const GAP_CAVEAT: &str = "gap_versions_merged is a merged, non-authoritative \
aggregate per repository; age is always unknown; this is not a measurement \
against a specific leader";

/// Fixed count caveat, shown ONLY when the statistics do not distinguish
/// the path (`index_used` absent): the ground truth is the engine dispatch
/// marker `index_used == Some("__record_counter__")` (O(1) counter);
/// any other `index_used` is a scan.
const COUNT_CAVEAT: &str = "the server statistics do not report whether the \
O(1) counter path or a scan produced this count";

/// Fixed no-ordering caveat for `data head` (design §4.4).
const HEAD_CAVEAT: &str = "rows are in server scan order; no ordering is \
promised and none may be assumed";

/// Fixed age caveat (design §6): the age field is always null; the shell
/// NEVER uses the word "lag" for it.
const AGE_CAVEAT: &str = "age is not sampled; zero can mean not sampled";

/// Allowlisted fields of one `replication subscriptions` entry (design
/// §4.2: identities, state, local consistent version, bookmarks, gap
/// endpoints). Anything else the peer emits — including raw
/// `status_message`, `last_terminal_reason` and source path fields — is
/// dropped, never projected raw.
const SUBSCRIPTION_FIELDS: &[&str] = &[
    "db",
    "repo",
    "name",
    "state",
    "installation_state",
    "recorded_identity",
    "observed_identity",
    "upstream_bookmark",
    "next_pull_version",
    "local_consistent_version",
    "certified_gap_first",
    "certified_gap_through",
    "artifact_digest",
    "source_lineage_id",
    "source_history_epoch",
    "seed_source_version",
    "local_baseline_version",
];

/// R5 step 3 state and narrow data view commands.
#[derive(Debug, Clone)]
pub(in crate::shell) enum StateCommand {
    /// `state status` — the P1 operator state snapshot.
    Status,
    /// `version show` — the P1 view: observed epoch and stamp outcome.
    VersionShow,
    /// `replication subscriptions` — the existing `ReplicationStatus` op.
    ReplicationSubscriptions,
    /// `data count DB REPO TABLE`.
    Count {
        db: String,
        repo: String,
        table: String,
    },
    /// `data head DB REPO TABLE N` — first N rows in server scan order.
    Head {
        db: String,
        repo: String,
        table: String,
        rows: u64,
    },
    /// `data get DB REPO TABLE KEY` — the P2 exact-key point read; KEY is
    /// Base58, validated before any connection.
    Get {
        db: String,
        repo: String,
        table: String,
        key: String,
    },
}

impl StateCommand {
    /// Wire operation label (JSON v1 `op` field, §5). The stable command ID
    /// is supplied by the dispatcher (`parser::grammar::command_id`).
    pub(in crate::shell) fn op(&self) -> &'static str {
        match self {
            StateCommand::Status | StateCommand::VersionShow => "Status",
            StateCommand::ReplicationSubscriptions => "ReplicationStatus",
            StateCommand::Count { .. } | StateCommand::Head { .. } => "Read",
            StateCommand::Get { .. } => "GetRecord",
        }
    }

    /// Whether the command needs the profile's routing database floor
    /// (`data …` names its database explicitly; `status`/`version show`
    /// are server-wide; `replication subscriptions` runs through the batch
    /// path and carries the routing database floor, §4.2).
    pub(in crate::shell) fn requires_routing_db(&self) -> bool {
        matches!(self, StateCommand::ReplicationSubscriptions)
    }

    /// Pre-connection validation with the fixed typed refusal policy:
    /// a malformed Base58 `data get` KEY (or an over-long one) and a zero
    /// `data head` row count are local usage errors refused BEFORE any
    /// dial or request.
    pub(in crate::shell) fn validate(&self) -> Result<(), ShellError> {
        match self {
            StateCommand::Head { rows, .. } => {
                if *rows == 0 {
                    return Err(ShellError::Usage);
                }
                Ok(())
            }
            StateCommand::Get { key, .. } => {
                if key.is_empty() || key.len() > GET_KEY_MAX_BYTES {
                    return Err(ShellError::InvalidRecordId);
                }
                // Base58 `RecordId` decode: a malformed key is the typed
                // `invalid_record_id` refusal (exit 1), never a panic and
                // never a scan.
                key.parse::<RecordId>()
                    .map(|_| ())
                    .map_err(|_| ShellError::InvalidRecordId)
            }
            _ => Ok(()),
        }
    }

    /// Execute the command. `routing_db` is the profile database floor and
    /// is required only when [`requires_routing_db`](Self::requires_routing_db)
    /// is true. Authorization happens exclusively on the normal remote
    /// route; this code sends no privilege material.
    /// cancel-safe: NO — a sent request may still execute; do not retry on
    /// timeout.
    pub(in crate::shell) async fn execute(
        &self,
        client: &SessionClient,
        routing_db: Option<&str>,
    ) -> Result<Value, ShellError> {
        match self {
            StateCommand::Status => {
                let snapshot = client.status().await?;
                let payload = project_status(&snapshot);
                client.check_catalogue_output(&payload)?;
                Ok(payload)
            }
            StateCommand::VersionShow => {
                let snapshot = client.status().await?;
                let payload = project_version(&snapshot);
                client.check_catalogue_output(&payload)?;
                Ok(payload)
            }
            StateCommand::ReplicationSubscriptions => {
                let db = routing_db.ok_or(ShellError::Usage)?;
                let payload = self.run_batch(client, db, replication_status()).await?;
                client.check_catalogue_output(&payload)?;
                Ok(payload)
            }
            StateCommand::Count { db, repo, table } => {
                let query = Query::with_repo(repo.clone(), table.clone())
                    .select([count_all(RESULT_ALIAS)])
                    .build();
                let result = self.run_read(client, db, query).await?;
                let payload = project_count(&result)?;
                client.check_catalogue_output(&payload)?;
                Ok(payload)
            }
            StateCommand::Head {
                db,
                repo,
                table,
                rows,
            } => {
                let requested = *rows;
                // Shell-side cap: never request more than 100 rows.
                let limit = requested.min(HEAD_MAX_ROWS);
                let query = Query::with_repo(repo.clone(), table.clone())
                    .limit(limit)
                    .build();
                // One monotonic deadline covers request + projection: the
                // read is bounded by `timeout_at` (elapsed maps to the typed
                // `head_wall_time`), and the SAME deadline bounds the local
                // CPU work in `project_head`.
                let deadline = tokio::time::Instant::now() + HEAD_WALL;
                let result = tokio::time::timeout_at(deadline, self.run_read(client, db, query))
                    .await
                    .map_err(|_| ShellError::HeadWallTime)?;
                let result = result?;
                // The credential-reflection scan is part of the SAME
                // head deadline (projection + guard, one budget, no
                // retry — a sent request is never resent on expiry).
                let payload = project_head(&result, requested, limit, deadline)?;
                client.check_catalogue_output(&payload)?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(ShellError::HeadWallTime);
                }
                Ok(payload)
            }
            StateCommand::Get {
                db,
                repo,
                table,
                key,
            } => {
                // validate() already parsed this key; the parse is repeated
                // here because `key` is owned text and validate borrows.
                let id: RecordId = key.parse().map_err(|_| ShellError::InvalidRecordId)?;
                let record = client.get_record(db, repo, table, id).await?;
                let payload = match record {
                    QueryRecord::Direct(qv) => value(&qv)?,
                    _ => return Err(ShellError::Server),
                };
                client.check_catalogue_output(&payload)?;
                Ok(payload)
            }
        }
    }

    /// Send one single-op batch (`replication subscriptions`) on the routing
    /// database and project the single direct result record through the
    /// STRICT allowlisted subscription shape.
    async fn run_batch(
        &self,
        client: &SessionClient,
        db: &str,
        op: shamir_query_types::batch::BatchOp,
    ) -> Result<Value, ShellError> {
        let mut batch = Batch::new();
        batch.insert(RESULT_ALIAS, op);
        // Client-side validation failure is a local usage problem.
        let request = batch.try_build().map_err(|_| ShellError::Usage)?;
        let response = client.execute(db, request).await.map_err(request_error)?;
        let result = take_result(response)?;
        project_subscriptions(&result)
    }

    /// Send one single-query batch and return the UNPROJECTED
    /// [`QueryResult`] (records stay typed `QueryValue`s; projection is the
    /// caller's bounded job).
    async fn run_read(
        &self,
        client: &SessionClient,
        db: &str,
        query: shamir_query_types::read::ReadQuery,
    ) -> Result<QueryResult, ShellError> {
        let mut batch = Batch::new();
        batch.query(RESULT_ALIAS, query);
        let request = batch.try_build().map_err(|_| ShellError::Usage)?;
        let response = client.execute(db, request).await.map_err(request_error)?;
        take_result(response)
    }
}

/// Extract the one-alias batch result as an owned, unprojected
/// [`QueryResult`]. A missing or skipped alias is a protocol mismatch,
/// reported as `Server` without echoing peer-supplied detail.
fn take_result(response: BatchResponse) -> Result<QueryResult, ShellError> {
    let mut results = response.results;
    let result = results.get_mut(RESULT_ALIAS).ok_or(ShellError::Server)?;
    if result.skipped {
        return Err(ShellError::Server);
    }
    Ok(std::mem::take(result))
}

/// The single direct record of a one-alias batch, as a typed `QueryValue`.
fn single_direct_value(result: &QueryResult) -> Result<&QueryValue, ShellError> {
    let [record] = result.records.as_slice() else {
        return Err(ShellError::Server);
    };
    match record {
        QueryRecord::Direct(qv) => Ok(qv),
        _ => Err(ShellError::Server),
    }
}

/// STRICT projection of `replication subscriptions`: the result must be
/// exactly one direct map with exactly the `subscriptions` list; each entry
/// must be a map and is re-projected through the typed sanitizer with ONLY
/// the [`SUBSCRIPTION_FIELDS`] allowlist. No raw server `QueryValue` map
/// projection: unknown peer fields (raw `status_message`, terminal
/// reasons, source paths) are dropped.
fn project_subscriptions(result: &QueryResult) -> Result<Value, ShellError> {
    let qv = single_direct_value(result)?;
    let QueryValue::Map(fields) = qv else {
        return Err(ShellError::Server);
    };
    if fields.len() != 1 {
        return Err(ShellError::Server);
    }
    let QueryValue::List(entries) = fields.get("subscriptions").ok_or(ShellError::Server)? else {
        return Err(ShellError::Server);
    };
    let mut projected = Vec::with_capacity(entries.len());
    for entry in entries {
        let QueryValue::Map(entry_fields) = entry else {
            return Err(ShellError::Server);
        };
        let mut item = serde_json::Map::new();
        for field in SUBSCRIPTION_FIELDS {
            if let Some(element) = entry_fields.get(*field) {
                item.insert((*field).to_string(), value(element)?);
            }
        }
        projected.push(Value::Object(item));
    }
    Ok(json!({ "subscriptions": projected }))
}

/// Project the `data head` output under the byte budget: rows are held
/// UNPROJECTED in the [`QueryResult`] and projected one at a time; the
/// first row that would overflow the serialized-bytes cap stops the
/// projection and sets `bytes_truncated`. Byte accumulation uses checked
/// arithmetic and exact separators (one comma between rows, the
/// surrounding brackets). `rows_truncated` is marked ONLY when more rows
/// existed beyond the returned window AND the request exceeded the hard
/// row cap; `rows_capped` independently records pure request clamping.
/// The `more exist` signal is the typed pagination metadata of the actual
/// paged read route (`pagination.has_next`), never a
/// `records.len() == limit` inference. The rows are constrained to the
/// requested limit with `.take(limit)` so a dishonest peer's extra rows
/// are dropped, not projected. The shared monotonic deadline is checked
/// INSIDE the loop, before and after the last row (the tail CPU work);
/// expiry is the typed `head_wall_time` refusal (exit 7), never a partial
/// success.
fn project_head(
    result: &QueryResult,
    requested: u64,
    limit: u64,
    deadline: tokio::time::Instant,
) -> Result<Value, ShellError> {
    let mut rows: Vec<Value> = Vec::new();
    // Exact serialization size: `[` + rows joined by `,` + `]`.
    let mut bytes = 2usize;
    let mut bytes_truncated = false;
    let max_rows = usize::try_from(limit).map_err(|_| ShellError::Server)?;
    for record in result.records.iter().take(max_rows) {
        if tokio::time::Instant::now() >= deadline {
            return Err(ShellError::HeadWallTime);
        }
        let QueryRecord::Direct(qv) = record else {
            return Err(ShellError::Server);
        };
        let rendered = value(qv)?;
        let width = serde_json::to_string(&rendered)
            .map(|text| text.len())
            .map_err(|_| ShellError::ProjectionLimit)?;
        let separator: usize = if rows.is_empty() { 0 } else { 1 };
        let needed = separator
            .checked_add(width)
            .and_then(|needed| bytes.checked_add(needed))
            .ok_or(ShellError::ProjectionLimit)?;
        if needed > HEAD_MAX_ROW_BYTES {
            bytes_truncated = true;
            break;
        }
        bytes = needed;
        rows.push(rendered);
    }
    // The tail CPU work after the last row is also inside the budget.
    if tokio::time::Instant::now() >= deadline {
        return Err(ShellError::HeadWallTime);
    }
    // `more exist`: the typed pagination metadata of the paged read route —
    // never inferred from the returned row count (a full page does not
    // prove more rows, and extra peer rows are dropped by `.take` above).
    let more_exist = result.pagination.as_ref().is_some_and(|p| p.has_next);
    let rows_truncated = more_exist && requested > HEAD_MAX_ROWS;
    let mut out = json!({
        "rows": rows,
        "rows_requested": requested.to_string(),
        "rows_returned": rows.len().to_string(),
        "rows_truncated": rows_truncated,
        "bytes_truncated": bytes_truncated,
        "caveat": HEAD_CAVEAT,
    });
    if requested > HEAD_MAX_ROWS {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("rows_capped".into(), json!(true));
        }
    }
    Ok(out)
}

/// Project the `data count` result. The count row is exactly one direct
/// record: a map with EXACTLY the named alias `result` carrying a
/// nonnegative integer, rendered as a decimal string. The engine dispatch
/// ground truth: `QueryStats` present with
/// `index_used == Some("__record_counter__")` is the O(1) counter path;
/// statistics present with `records_scanned > 0` is a scan (the plain
/// `CountAll` path); anything else stays `unknown` and carries the fixed
/// caveat — the shell never invents a scan marker.
fn project_count(result: &QueryResult) -> Result<Value, ShellError> {
    let qv = single_direct_value(result)?;
    let QueryValue::Map(fields) = qv else {
        return Err(ShellError::Server);
    };
    if fields.len() != 1 {
        return Err(ShellError::Server);
    }
    let QueryValue::Int(count) = fields.get(RESULT_ALIAS).ok_or(ShellError::Server)? else {
        return Err(ShellError::Server);
    };
    if *count < 0 {
        return Err(ShellError::Server);
    }
    let count_path = match &result.stats {
        Some(stats) if stats.index_used.as_deref() == Some("__record_counter__") => "counter",
        Some(stats) if stats.records_scanned > 0 => "scan",
        _ => "unknown",
    };
    let mut out = json!({
        "count": count.to_string(),
        "count_path": count_path,
    });
    if count_path == "unknown" {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("caveat".into(), json!(COUNT_CAVEAT));
        }
    }
    Ok(out)
}

/// Project the P1 snapshot with sanitized strings; EVERY u64 counter,
/// epoch, uptime and age renders as a decimal string (design §5 payload
/// conventions — no large-number fidelity loss through JSON floats).
fn project_status(snapshot: &OperatorStatus) -> Value {
    json!({
        "server_version": output::sanitize(&snapshot.server_version),
        "format_epoch": snapshot.format_epoch.to_string(),
        "stamp_outcome": stamp_outcome(snapshot.stamp_outcome),
        "uptime_ms": snapshot.uptime_ms.to_string(),
        "role": role(snapshot.role),
        "node_id": snapshot.node_id.as_deref().map(output::sanitize),
        "gap_caveat": GAP_CAVEAT,
        "age_caveat": AGE_CAVEAT,
        "replication": snapshot
            .replication
            .iter()
            .map(|entry| {
                json!({
                    "db": output::sanitize(&entry.db),
                    "repo": output::sanitize(&entry.repo),
                    "gap_versions_merged": entry.gap_versions_merged.to_string(),
                    "age": Value::Null,
                    "receipt_age_ns": entry
                        .receipt_age_ns
                        .map(|age| age.to_string()),
                })
            })
            .collect::<Vec<Value>>(),
        "repositories": snapshot
            .repositories
            .iter()
            .map(project_repository)
            .collect::<Vec<Value>>(),
    })
}

/// `version show`: the boot-observed epoch and outcome, the binary
/// version they belong to, and the OBSERVED role/node id (the same
/// launcher-captured values `status` reports — the shell never reads the
/// data directory and never invents an identity).
fn project_version(snapshot: &OperatorStatus) -> Value {
    json!({
        "server_version": output::sanitize(&snapshot.server_version),
        "format_epoch": snapshot.format_epoch.to_string(),
        "stamp_outcome": stamp_outcome(snapshot.stamp_outcome),
        "role": role(snapshot.role),
        "node_id": snapshot.node_id.as_deref().map(output::sanitize),
    })
}

/// One repository: effective vs stored durability and the WAL telemetry,
/// with the semantics of `durability_export.rs`. Budget bytes render as the
/// documented large decimal strings; absent override leaves are `null`.
fn project_repository(repo: &StatusRepository) -> Value {
    json!({
        "db": output::sanitize(&repo.db),
        "repo": output::sanitize(&repo.repo),
        "effective": {
            "wal_commit": repo.effective.wal_commit.map(wal_commit),
            "wal_retention_max_bytes": repo
                .effective
                .wal_retention_max_bytes
                .map(|budget| budget.get().to_string()),
            "on_wal_retention_exceeded": repo
                .effective
                .on_wal_retention_exceeded
                .map(retention_overflow),
            "replication_serve": repo.effective.replication_serve.map(serve),
        },
        "stored": repo.stored.map(|stored| {
            json!({
                "wal_commit": stored.wal_commit.map(wal_commit),
                "wal_retention_max_bytes": stored
                    .wal_retention_max_bytes
                    .map(|budget| budget.get().to_string()),
                "on_wal_retention_exceeded": stored
                    .on_wal_retention_exceeded
                    .map(retention_overflow),
                "replication_serve": stored.replication_serve.map(serve),
            })
        }),
        "wal": project_wal(&repo.wal),
    })
}

/// WAL telemetry, rendered verbatim in field names; `pending_versions` is a
/// bound, not an exact count of unflushed records.
fn project_wal(wal: &StatusWal) -> Value {
    json!({
        "backend_mem": wal.backend_mem,
        "watermark_w": wal.watermark_w.to_string(),
        "recoverable_through": wal.recoverable_through.to_string(),
        "pending_versions": wal.pending_versions.to_string(),
        "first_pending_version": wal.first_pending_version.map(|v| v.to_string()),
        "first_pending_age_ns": wal.first_pending_age_ns.map(|age| age.to_string()),
        "sync_now_calls": wal.sync_now_calls.to_string(),
        "sync_rounds": wal.sync_rounds.to_string(),
        "sync_coalesced": wal.sync_coalesced.to_string(),
        "sync_errors": wal.sync_errors.to_string(),
        "sync_duration_ns": wal.sync_duration_ns.to_string(),
    })
}

/// Stable snake_case rendering of the boot stamp outcome (mirrored enum,
/// `db_message.rs`).
fn stamp_outcome(outcome: StatusStampOutcome) -> &'static str {
    match outcome {
        StatusStampOutcome::Created => "created",
        StatusStampOutcome::Adopted => "adopted",
        StatusStampOutcome::Unchanged => "unchanged",
        StatusStampOutcome::VersionUpdated => "version_updated",
        StatusStampOutcome::InspectedStampless => "inspected_stampless",
        StatusStampOutcome::InspectedOlder => "inspected_older",
    }
}

/// Stable snake_case rendering of the replication role.
fn role(role: StatusRole) -> &'static str {
    match role {
        StatusRole::Follower => "follower",
        StatusRole::Unknown => "unknown",
    }
}

fn wal_commit(tier: shamir_query_types::durability::WalCommitTier) -> &'static str {
    match tier {
        shamir_query_types::durability::WalCommitTier::Buffered => "buffered",
        shamir_query_types::durability::WalCommitTier::Synced => "synced",
    }
}

fn retention_overflow(action: shamir_query_types::durability::RetentionOverflow) -> &'static str {
    match action {
        shamir_query_types::durability::RetentionOverflow::RefuseWrites => "refuse_writes",
        shamir_query_types::durability::RetentionOverflow::DeclareGap => "declare_gap",
    }
}

fn serve(mode: shamir_query_types::durability::ReplicationServe) -> &'static str {
    match mode {
        shamir_query_types::durability::ReplicationServe::Durable => "durable",
        shamir_query_types::durability::ReplicationServe::CompletedReseed => "completed_reseed",
    }
}

#[cfg(test)]
#[path = "tests/state_projection_tests.rs"]
mod state_projection_tests;
