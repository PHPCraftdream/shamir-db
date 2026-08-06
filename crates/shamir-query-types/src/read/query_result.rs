//! QueryResult and QueryStats — query execution results.

use super::ddl::DdlOpState;
use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use std::str::FromStr;

use super::query_record::QueryRecord;
use super::PaginationInfo;

/// Plan type chosen by the read planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanType {
    /// Keyset-seek via sorted index (Pagination::After).
    KeysetSeek,
    /// ORDER BY + LIMIT K fast path via sorted index.
    OrderLimitFast,
    /// Index2 accelerated path (FTS / functional / vector).
    Index2,
    /// BTree index equality / In scan.
    IndexScan,
    /// Sorted index range scan (Between / Gte / Lte / Gt / Lt).
    SortedIndexScan,
    /// Range predicate extracted from AND filter + sorted index.
    AndRangeIndexScan,
    /// Counter shortcut (COUNT(*) without WHERE).
    CounterShortcut,
    /// MIN/MAX aggregate via sorted index.
    MinMaxIndex,
    /// Full table scan (streaming / collecting / counting).
    FullScan,
}

/// EXPLAIN plan preview — returned when `ReadQuery::explain == true`.
///
/// Contains the plan the planner WOULD choose, without materialising
/// any rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainPlan {
    /// Which execution strategy the planner selected.
    pub plan_type: PlanType,
    /// Name of the index used (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_used: Option<String>,
    /// Estimated records to scan (when the planner knows before execution).
    /// `None` when the estimate is unavailable without materialisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_rows: Option<u64>,
}

/// Query execution statistics
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryStats {
    /// Was an index used?
    pub index_used: Option<String>,
    /// Number of records scanned
    pub records_scanned: u64,
    /// Number of records returned
    pub records_returned: u64,
    /// Execution time in microseconds
    pub execution_time_us: u64,
}

/// A single record that failed to decode during a scan — reported instead
/// of silently dropped from the result set.
///
/// The row is still skipped from `QueryResult::records` (unchanged
/// behaviour); this struct only makes that skip visible to the caller
/// instead of the result count silently coming up one row short.
///
/// Wire shape: `{ "table": <str>, "id": <str> }` — `id` is the record's
/// base58 string form (via `RecordId::to_string`/`FromStr`), the SAME
/// convention every other `RecordId` uses on the wire (e.g.
/// `InsertedRecord`'s `_id`, and a read result row's own `_id` field) —
/// NOT raw msgpack bytes, despite `RecordId`'s own derived `Serialize`
/// impl emitting `bin`. The Rust-side field stays typed as `RecordId` for
/// ergonomic call sites; only the wire encoding is customised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorruptRecordRef {
    /// Name of the table the corrupt record belongs to.
    pub table: String,
    /// The record's id (still resolvable even though its VALUE failed to
    /// decode — ids are read independently of the value payload). Encoded
    /// on the wire as a base58 string (see struct doc comment), not raw
    /// bytes.
    #[serde(with = "id_as_base58_string")]
    pub id: RecordId,
}

/// Custom (de)serialization for `CorruptRecordRef::id` — base58 string on
/// the wire, typed `RecordId` in Rust. Mirrors `InsertedRecord`'s
/// `id.to_string()` convention (`crates/shamir-query-types/src/write/
/// inserted_record.rs`) rather than `RecordId`'s own derived `Serialize`
/// (which emits raw msgpack `bin`).
mod id_as_base58_string {
    use super::{DeError, Deserializer, FromStr, RecordId, Serializer};
    use serde::Deserialize;

    pub fn serialize<S: Serializer>(id: &RecordId, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(id)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<RecordId, D::Error> {
        let s = String::deserialize(deserializer)?;
        RecordId::from_str(&s).map_err(DeError::custom)
    }
}

/// Query result
///
/// Derives `Default` (F-10, #800) so that adding a field here does not force
/// hand-editing every one of the ~90 existing `QueryResult { .. }` literal
/// construction sites scattered across the workspace — every field already
/// has a natural `Default` (`Vec::new()`, `None`, `false`), so this derive is
/// purely mechanical, not a behavioural change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    /// Result records.
    pub records: Vec<QueryRecord>,
    /// Execution statistics
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<QueryStats>,
    /// Pagination metadata (present when pagination was used)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationInfo>,
    /// Non-tabular value returned by a stored procedure / callable function.
    ///
    /// When a `CallOp` returns a scalar, object, or array the answer lives
    /// here; `records` is empty. `None` for regular DML/DDL results.
    /// Skipped from serialization when absent (backward-compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<QueryValue>,
    /// EXPLAIN plan preview (present only when `ReadQuery::explain == true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<ExplainPlan>,
    /// Conditional-execution status (Epic03/B, #645): `true` when this
    /// alias's op did NOT run — either its own `when` evaluated `false`, or
    /// it was cascade-skipped because a `DataFlow`/`Both`-provenance
    /// dependency was itself skipped. `false` (the default, omitted from
    /// the wire) means the op executed normally; existing peers that don't
    /// know this field never observe it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
    /// Per-record version, index-aligned with `records` (i.e.
    /// `versions[i]` is the version of `records[i]`). `Some` only when the
    /// originating `ReadQuery::with_version == true`; `None`/omitted
    /// otherwise (backward-compatible — existing peers that don't ask for
    /// versions never see this field).
    ///
    /// The version is the canonical per-key committed version reported by
    /// `MvccStore::version_of` (the same source SSI read-set validation
    /// uses). A client captures this value, mutates conditionally, and
    /// supplies it back as `UpdateOp`/`DeleteOp::expected_version` for
    /// optimistic concurrency control (CAS). Paths that cannot
    /// structurally attribute a single record version to a result row
    /// (aggregates, ORDER BY / DISTINCT which reorder rows after the
    /// version is read, or tables without an MVCC backing store) leave
    /// this field `None` even when `with_version == true` — opt-in
    /// assistance, never a correctness contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<u64>>,
    /// Records that failed to decode during this scan — dropped from
    /// `records`, but reported here as `(table, id)` pairs instead of
    /// silently vanishing. Empty (and omitted from the wire) on the common
    /// case where nothing was corrupt.
    ///
    /// SDK note (F-22, #815): `crates/shamir-client` and `shamir-sdk` return
    /// this exact `QueryResult` type directly (no separate "SDK response"
    /// wrapper struct) — any Rust consumer already has direct, typed field
    /// access to `result.corrupt_records` / `CorruptRecordRef` with no
    /// additional plumbing. The TypeScript SDK mirrors this shape as
    /// `CorruptRecordRef` / `QueryResult.corrupt_records` in
    /// `crates/shamir-client-ts/src/core/types/batch.ts`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrupt_records: Vec<CorruptRecordRef>,
    /// DDL operation ID for recoverable DDL operations (minted by the server).
    ///
    /// Present only for DDL ops that have crash-recovery tombstones (e.g.,
    /// `DROP INDEX` / `RENAME INDEX`). The client uses this to poll the
    /// operation's status via `DbRequest::GetDdlOpStatus` — especially
    /// important for crash-recovered ops where the synchronous response was
    /// lost or never sent.
    ///
    /// Backward-compatible: `#[serde(default, skip_serializing_if = "Option::is_none")]`
    /// → old peers never see this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<RecordId>,
    /// DDL operation status (populated for the synchronous `Succeeded` case).
    ///
    /// This field is authoritative ONLY for the inline-succeeded common case.
    /// For crash-recovered operations, the client MUST poll via `op_id` and
    /// `DbRequest::GetDdlOpStatus` to see `SucceededViaCrashRecovery` — the
    /// synchronous response (if any) does NOT carry that state.
    ///
    /// Backward-compatible: `#[serde(default, skip_serializing_if = "Option::is_none")]`
    /// → old peers never see this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ddl_status: Option<DdlOpState>,
}
