//! Write operation types.
//!
//! Core types for database write operations.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::query::filter::Filter;
use crate::db::query::TableRef;

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

    /// Records to insert.
    pub values: Vec<Value>,
}

/// Update operation - updates records matching a filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateOp {
    /// Target table (optionally qualified with repo).
    pub update: TableRef,

    /// Filter condition (all records if omitted).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "where")]
    pub where_clause: Option<Filter>,

    /// Fields to update (partial) or full record.
    pub set: Value,

    /// Optional select configuration for returning updated records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<UpdateSelect>,
}

/// Set operation - upsert by key (update if exists, insert if not).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetOp {
    /// Target table (optionally qualified with repo).
    pub set: TableRef,

    /// Key to match (id or unique field).
    pub key: Value,

    /// Value to set (merged with existing on update).
    pub value: Value,
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
