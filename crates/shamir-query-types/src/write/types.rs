//! Write operation types.
//!
//! Core types for database write operations.

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use shamir_types::types::value::QueryValue;

use crate::filter::Filter;
use crate::TableRef;

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSelect {
    #[serde(default)]
    pub return_mode: UpdateReturnMode,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
}

// ============================================================================
// WRITE OPERATIONS
// ============================================================================

/// Insert operation - inserts new records into a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertOp {
    /// Target table (optionally qualified with repo).
    pub insert_into: TableRef,

    /// Records to insert (format-agnostic; deserialized directly from wire).
    pub values: Vec<QueryValue>,

    /// Each element is ONE record's id-keyed storage msgpack (the bytes
    /// `query_value_to_storage_bytes` emits). Used by the pass-through write
    /// path for fully-literal, client-interned records; records containing
    /// `$fn`/computed markers stay on `values`. Mutually-exclusive-per-record
    /// with `values` semantically; both may be present in one op (different
    /// records).
    ///
    /// Serializes as msgpack `bin` (not seq-of-u8) via `serde_bytes`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records_idmsgpack: Vec<ByteBuf>,
}

/// Update operation - updates records matching a filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateOp {
    /// Target table (optionally qualified with repo).
    pub update: TableRef,

    /// Filter condition (all records if omitted).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "where")]
    pub where_clause: Option<Filter>,

    /// Fields to update (partial) or full record (format-agnostic).
    pub set: QueryValue,

    /// Optional select configuration for returning updated records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<UpdateSelect>,
}

/// Set operation - upsert by key (update if exists, insert if not).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetOp {
    /// Target table (optionally qualified with repo).
    pub set: TableRef,

    /// Key to match (id or unique field, format-agnostic).
    pub key: QueryValue,

    /// Value to set (merged with existing on update, format-agnostic).
    pub value: QueryValue,
}

/// Delete operation - deletes records matching a filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteOp {
    /// Target table (optionally qualified with repo).
    pub delete_from: TableRef,

    /// Filter condition (required for safety).
    #[serde(rename = "where")]
    pub where_clause: Filter,
}
