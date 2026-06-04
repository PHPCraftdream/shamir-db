//! Batch assembler + typed `Handle`/`RowRef` dependency references.
//!
//! `Batch` accumulates queries/writes under string aliases and produces a
//! `shamir_query_types::batch::BatchRequest`. Each `query()`/`insert()`/…
//! call returns a typed `Handle` whose `column()`, `row()`, `first()`,
//! `all()` methods emit `FilterValue::QueryRef` values that the planner
//! treats as inter-query dependencies.

use serde_json::Value;
use shamir_query_types::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::ReadQuery;
use shamir_query_types::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use shamir_types::types::common::{new_map, TMap};

use crate::val::{qref, qref_all, IntoFieldPath};

// ============================================================================
// Isolation / Durability enums
// ============================================================================

/// Transaction isolation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Reads see a consistent snapshot; writes use last-writer-wins.
    Snapshot,
    /// Read-set validated at commit; concurrent write conflict aborts.
    Serializable,
}

impl Isolation {
    fn as_str(self) -> &'static str {
        match self {
            Isolation::Snapshot => "snapshot",
            Isolation::Serializable => "serializable",
        }
    }
}

/// Per-request durability level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Ack after in-memory buffer; durable on background tick.
    Buffered,
    /// Flush durable backing before ack.
    Synced,
}

impl Durability {
    fn as_str(self) -> &'static str {
        match self {
            Durability::Buffered => "buffered",
            Durability::Synced => "synced",
        }
    }
}

// ============================================================================
// Handle / RowRef
// ============================================================================

/// A typed reference to a query registered in a [`Batch`].
///
/// Returned by `Batch::query`, `Batch::insert`, etc. Its methods produce
/// `FilterValue::QueryRef` values that the engine's batch planner
/// interprets as inter-query dependencies.
#[derive(Debug, Clone)]
pub struct Handle {
    alias: String,
}

impl Handle {
    /// The bare alias (without `@` prefix).
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Reference a column across all result rows.
    ///
    /// Produces a `$query` path like `"[].field"` (or `"[].a.b"` for
    /// nested fields).
    pub fn column(&self, field: impl IntoFieldPath) -> FilterValue {
        let segments = field.into_field_path();
        let dotted = segments.join(".");
        let path = format!("[].{dotted}");
        qref(&self.alias, path)
    }

    /// Reference a specific result row by index.
    pub fn row(&self, index: usize) -> RowRef {
        RowRef {
            alias: self.alias.clone(),
            index,
        }
    }

    /// Shorthand for `row(0)`.
    pub fn first(&self) -> RowRef {
        self.row(0)
    }

    /// Reference the entire result (no path).
    pub fn all(&self) -> FilterValue {
        qref_all(&self.alias)
    }
}

/// A reference to a specific row of a query result.
#[derive(Debug, Clone)]
pub struct RowRef {
    alias: String,
    index: usize,
}

impl RowRef {
    /// Reference a field on this row.
    ///
    /// Produces a `$query` path like `"[0].field"`.
    pub fn field(&self, field: impl IntoFieldPath) -> FilterValue {
        let segments = field.into_field_path();
        let dotted = segments.join(".");
        let path = format!("[{}].{dotted}", self.index);
        qref(&self.alias, path)
    }

    /// Reference the whole row (no field).
    ///
    /// Produces a `$query` path like `"[0]"`.
    pub fn get(&self) -> FilterValue {
        let path = format!("[{}]", self.index);
        qref(&self.alias, path)
    }
}

// ============================================================================
// IntoBatchOp trait
// ============================================================================

/// Anything convertible into a [`BatchOp`].
pub trait IntoBatchOp {
    /// Convert into a batch operation.
    fn into_batch_op(self) -> BatchOp;
}

impl IntoBatchOp for BatchOp {
    fn into_batch_op(self) -> BatchOp {
        self
    }
}

impl IntoBatchOp for ReadQuery {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Read(self)
    }
}

impl IntoBatchOp for crate::query::Query {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Read(self.build())
    }
}

impl IntoBatchOp for InsertOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Insert(self)
    }
}

impl IntoBatchOp for crate::write::Insert {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Insert(self.build())
    }
}

impl IntoBatchOp for UpdateOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Update(self)
    }
}

impl IntoBatchOp for crate::write::Update {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Update(self.build())
    }
}

impl IntoBatchOp for SetOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Set(self)
    }
}

impl IntoBatchOp for crate::write::Upsert {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Set(self.build())
    }
}

impl IntoBatchOp for DeleteOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Delete(self)
    }
}

impl IntoBatchOp for crate::write::Delete {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Delete(self.build())
    }
}

// ============================================================================
// BuildError
// ============================================================================

/// Client-side validation error from [`Batch::try_build`].
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    /// A `$query` ref points to an alias not present in the batch.
    UnknownAlias {
        /// The alias that was referenced.
        alias: String,
        /// The alias of the entry that contains the bad reference.
        referenced_by: String,
    },
    /// A `$query` ref inside an entry points back to itself.
    SelfReference {
        /// The alias that references itself.
        alias: String,
    },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::UnknownAlias {
                alias,
                referenced_by,
            } => write!(
                f,
                "unknown alias '{}' referenced by '{}'",
                alias, referenced_by
            ),
            BuildError::SelfReference { alias } => {
                write!(f, "alias '{}' references itself", alias)
            }
        }
    }
}

impl std::error::Error for BuildError {}

// ============================================================================
// Batch assembler
// ============================================================================

/// Fluent batch assembler.
///
/// Accumulates query/write entries under string aliases and produces a
/// [`BatchRequest`].
#[derive(Debug, Clone)]
pub struct Batch {
    id: Value,
    name: Option<String>,
    transactional: bool,
    isolation: Option<String>,
    durability: Option<String>,
    queries: TMap<String, QueryEntry>,
    return_all: bool,
    return_only: Option<Vec<String>>,
    limits: BatchLimits,
}

impl Default for Batch {
    fn default() -> Self {
        Self::new()
    }
}

impl Batch {
    /// Create an empty batch with default settings.
    pub fn new() -> Self {
        Self {
            id: Value::Null,
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries: new_map(),
            return_all: true,
            return_only: None,
            limits: BatchLimits::default(),
        }
    }

    /// Create a named batch.
    pub fn named(name: impl Into<String>) -> Self {
        let mut b = Self::new();
        b.name = Some(name.into());
        b
    }

    // ── config (chainable) ─────────────────────────────────────────

    /// Set the batch name.
    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// Set the client-provided request id.
    pub fn id(&mut self, id: impl Into<Value>) -> &mut Self {
        self.id = id.into();
        self
    }

    /// Enable transactional semantics (MVCC).
    pub fn transactional(&mut self) -> &mut Self {
        self.transactional = true;
        self
    }

    /// Set the isolation level.
    pub fn isolation(&mut self, iso: Isolation) -> &mut Self {
        self.isolation = Some(iso.as_str().to_owned());
        self
    }

    /// Set the durability level.
    pub fn durability(&mut self, d: Durability) -> &mut Self {
        self.durability = Some(d.as_str().to_owned());
        self
    }

    /// Return all results (resets `return_only`).
    pub fn return_all(&mut self) -> &mut Self {
        self.return_all = true;
        self.return_only = None;
        self
    }

    /// Return only the listed aliases.
    pub fn return_only(
        &mut self,
        aliases: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        self.return_only = Some(aliases.into_iter().map(Into::into).collect());
        self.return_all = false;
        self
    }

    /// Override default execution limits.
    pub fn limits(&mut self, limits: BatchLimits) -> &mut Self {
        self.limits = limits;
        self
    }

    // ── entry insertion ────────────────────────────────────────────

    /// Add a read query (returned in the response).
    pub fn query(&mut self, alias: impl Into<String>, q: impl Into<ReadQuery>) -> Handle {
        self.add_entry(alias, BatchOp::Read(q.into()), true)
    }

    /// Add a read query (silent — not returned in the response).
    pub fn query_silent(&mut self, alias: impl Into<String>, q: impl Into<ReadQuery>) -> Handle {
        self.add_entry(alias, BatchOp::Read(q.into()), false)
    }

    /// Add an insert operation.
    pub fn insert(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an update operation.
    pub fn update(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an upsert (set) operation.
    pub fn upsert(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add a delete operation.
    pub fn delete(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Escape hatch: add any `BatchOp` (returned in the response).
    pub fn op(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Escape hatch: add any `BatchOp` (silent).
    pub fn op_silent(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), false)
    }

    // ── build ──────────────────────────────────────────────────────

    // ── wire encoding (build + encode in one step) ─────────────────

    /// Build and encode as a JSON `serde_json::Value`.
    pub fn to_json_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        crate::wire::ToWire::to_json_value(&self.build())
    }

    /// Build and encode as a compact JSON string.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        crate::wire::ToWire::to_json_string(&self.build())
    }

    /// Build and encode as a pretty-printed JSON string.
    pub fn to_json_string_pretty(&self) -> Result<String, serde_json::Error> {
        crate::wire::ToWire::to_json_string_pretty(&self.build())
    }

    /// Build and encode as msgpack (named fields).
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec_named(&self.build())
    }

    // ── build ──────────────────────────────────────────────────────

    /// Infallible build — clones accumulated state into a [`BatchRequest`].
    pub fn build(&self) -> BatchRequest {
        BatchRequest {
            id: self.id.clone(),
            name: self.name.clone(),
            transactional: self.transactional,
            isolation: self.isolation.clone(),
            durability: self.durability.clone(),
            queries: self.queries.clone(),
            return_all: self.return_all,
            return_only: self.return_only.clone(),
            limits: self.limits.clone(),
        }
    }

    /// Build with client-side validation.
    ///
    /// Serializes each entry's op to JSON, walks for `"$query"` string
    /// values, normalizes the base alias (strip `@`, cut at `[`/`.`), and
    /// checks:
    /// - the base alias exists as a key in `queries`
    /// - the base alias is not the referencing entry's own alias
    pub fn try_build(&self) -> Result<BatchRequest, BuildError> {
        for (alias, entry) in &self.queries {
            let json =
                serde_json::to_value(&entry.op).expect("BatchOp serialization is infallible");
            let mut refs = Vec::new();
            collect_query_refs(&json, &mut refs);
            for raw_ref in &refs {
                let base = extract_base_alias(raw_ref);
                if base == *alias {
                    return Err(BuildError::SelfReference {
                        alias: alias.clone(),
                    });
                }
                if !self.queries.contains_key(&base) {
                    return Err(BuildError::UnknownAlias {
                        alias: base,
                        referenced_by: alias.clone(),
                    });
                }
            }
        }
        Ok(self.build())
    }

    // ── internal ───────────────────────────────────────────────────

    fn add_entry(&mut self, alias: impl Into<String>, op: BatchOp, return_result: bool) -> Handle {
        let alias = alias.into();
        self.queries
            .insert(alias.clone(), QueryEntry { op, return_result });
        Handle { alias }
    }
}

// ============================================================================
// $query ref walking helpers (mirrors planner.rs logic)
// ============================================================================

/// Collect all `$query` string values from a JSON tree.
fn collect_query_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(qv) = map.get("$query") {
                if let Some(s) = qv.as_str() {
                    out.push(s.to_owned());
                }
            }
            for v in map.values() {
                collect_query_refs(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_query_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Strip leading `@` and cut at the first `[` or `.`.
fn extract_base_alias(s: &str) -> String {
    let s = s.strip_prefix('@').unwrap_or(s);
    s.find(['[', '.'])
        .map(|pos| s[..pos].to_string())
        .unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod tests;
