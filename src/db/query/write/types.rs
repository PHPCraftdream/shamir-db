//! Write operation types.
//!
//! Core types for database write operations.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::query::filter::Filter;

// ============================================================================
// UPDATE SELECT TYPES
// ============================================================================

/// Mode for returning records from UPDATE operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateReturnMode {
    /// Return all records that matched the filter.
    All,

    /// Return only records that were actually changed.
    #[default]
    Changed,

    /// Return only records that matched but were not changed.
    Unchanged,
}

/// Configuration for selecting results from UPDATE operation.
///
/// Controls which records are returned and which fields.
///
/// # Example
///
/// ```json
/// {
///   "return": "changed",
///   "fields": ["id", "name", "status"]
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSelect {
    /// Which records to return: "all", "changed", or "unchanged".
    ///
    /// - `"all"` - All records that matched the filter
    /// - `"changed"` - Only records that were actually modified
    /// - `"unchanged"` - Only records that matched but data was already the same
    #[serde(default)]
    pub return_mode: UpdateReturnMode,

    /// Fields to return (optional, all fields if omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
}

// ============================================================================
// WRITE OPERATIONS
// ============================================================================

/// Insert operation - inserts new records into a table.
///
/// # Example
///
/// ```json
/// {
///   "insert_into": "users",
///   "values": [
///     { "name": "Alice", "email": "alice@example.com" },
///     { "name": "Bob", "email": "bob@example.com" }
///   ]
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertOp {
    /// Target table name.
    pub insert_into: String,

    /// Records to insert.
    pub values: Vec<Value>,
}

/// Update operation - updates records matching a filter.
///
/// # Example
///
/// ```json
/// // Partial update by fields
/// {
///   "update": "users",
///   "where": { "op": "eq", "field": "id", "value": 1 },
///   "set": { "name": "New Name", "status": "active" }
/// }
///
/// // Full record replacement
/// {
///   "update": "users",
///   "where": { "op": "eq", "field": "id", "value": 1 },
///   "set": { "id": 1, "name": "Full", "email": "full@example.com", "status": "active" }
/// }
///
/// // Update with returning changed records
/// {
///   "update": "users",
///   "where": { "op": "eq", "field": "status", "value": "inactive" },
///   "set": { "status": "active" },
///   "select": {
///     "return_mode": "changed",
///     "fields": ["id", "name", "status"]
///   }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateOp {
    /// Target table name.
    pub update: String,

    /// Filter condition (all records if omitted).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "where")]
    pub where_clause: Option<Filter>,

    /// Fields to update (partial) or full record.
    pub set: Value,

    /// Optional select configuration for returning updated records.
    ///
    /// When specified, the update operation returns records based on the mode:
    /// - `"all"` - All records that matched the filter
    /// - `"changed"` - Only records that were actually modified (default)
    /// - `"unchanged"` - Only records that matched but data was already the same
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<UpdateSelect>,
}

/// Set operation - upsert by key (update if exists, insert if not).
///
/// Works only with primary key (`id`) or unique keys.
///
/// # Example
///
/// ```json
/// // By primary key
/// {
///   "set": "users",
///   "key": { "id": 1 },
///   "value": { "name": "Alice", "email": "alice@example.com" }
/// }
///
/// // By unique field
/// {
///   "set": "users",
///   "key": { "email": "alice@example.com" },
///   "value": { "name": "Alice Updated" }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetOp {
    /// Target table name.
    pub set: String,

    /// Key to match (id or unique field).
    pub key: Value,

    /// Value to set (merged with existing on update).
    pub value: Value,
}

/// Delete operation - deletes records matching a filter.
///
/// # Example
///
/// ```json
/// {
///   "delete_from": "users",
///   "where": { "op": "eq", "field": "status", "value": "inactive" }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteOp {
    /// Target table name.
    pub delete_from: String,

    /// Filter condition (required for safety).
    #[serde(rename = "where")]
    pub where_clause: Filter,
}
