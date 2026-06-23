//! Declarative schema DDL operations: set/add/remove/get table schema.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

fn default_repo() -> String {
    "main".to_string()
}

// ── DTO: wire-level rule representation ────────────────────────────────

/// Numeric bound for `min` / `max` constraints on the wire.
///
/// Mirrors [`shamir_engine::validator::schema::constraints::Num`] but
/// lives in the DTO layer so `shamir-query-types` does not depend on
/// `shamir-engine`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumDto {
    /// Integer bound.
    Int(i64),
    /// Floating-point bound.
    F64(f64),
}

/// A single field-rule as it travels over the wire (DDL payload).
///
/// `path` uses flat string names (de-interned on the client side).
/// The server interns them before persisting to the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldRuleDto {
    /// Field path segments (e.g. `["address", "zip"]`).
    pub path: Vec<String>,
    /// Type tag: `"string"`, `"int"`, `"f64"`, `"dec"`, `"bool"`, `"bin"`,
    /// `"list"`, `"map"`, `"set"`, `"null"`, `"any"`.
    pub r#type: String,
    /// Optional constraints (flattened into the same JSON/msgpack object).
    #[serde(flatten)]
    pub constraints: ConstraintsDto,
}

/// Constraint fields carried alongside a [`FieldRuleDto`].
///
/// All fields are optional; absent = unconstrained.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConstraintsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsigned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<NumDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<NumDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_len: Option<u64>,
    /// Enum constraint: the value must be one of these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<QueryValue>>,
    /// Array element type constraint (e.g. `"string"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array_of: Option<String>,
}

// ── Ops ────────────────────────────────────────────────────────────────

/// Whole-replace a table's declarative schema.
///
/// ```text
/// { "set_table_schema": "users", "repo": "main",
///   "schema": [ {path, type, ...} ], "expected_version": 3 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetTableSchemaOp {
    pub set_table_schema: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    /// The new schema (complete replacement).
    pub schema: Vec<FieldRuleDto>,
    /// Optimistic concurrency: if present, the server checks that the
    /// current `schema_version` matches before applying.  Mismatch
    /// produces `version_conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
}

/// Add (or replace) a single rule in a table's declarative schema.
///
/// Upsert by `path`: if a rule with the same path exists it is replaced,
/// otherwise appended.
///
/// ```text
/// { "add_schema_rule": "users", "repo": "main",
///   "rule": {path, type, ...} }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddSchemaRuleOp {
    pub add_schema_rule: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub rule: FieldRuleDto,
}

/// Remove a single rule from a table's declarative schema by path.
///
/// ```text
/// { "remove_schema_rule": "users", "repo": "main",
///   "path": ["email"] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoveSchemaRuleOp {
    pub remove_schema_rule: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub path: Vec<String>,
}

/// Read a table's declarative schema (introspection).
///
/// ```text
/// { "get_table_schema": "users", "repo": "main" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetTableSchemaOp {
    pub get_table_schema: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}
