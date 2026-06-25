//! Declarative schema DDL operations: set/add/remove/get table schema.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

use super::fk_action::FkAction;

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
    /// Literal default value stamped on INSERT for an absent field
    /// (Phase ②.4b — surface only; stamp-enforcement lands in ②.4c).
    /// Carried as a constant `QueryValue`; computed defaults (`now()`,
    /// scalars) are out of scope (would need the mutating-validator
    /// framework — see DDL-EVOLUTION-PLAN §②.4a variant A).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<QueryValue>,
    /// Array element type constraint (e.g. `"string"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array_of: Option<String>,

    /// Phase B — scalar-bridge: name of a registered scalar (built-in
    /// funclib or user) used as a predicate over the field value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scalar: Option<String>,

    /// Phase B — named format check (`"email"` / `"url"` / `"uuid"` / `"date"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Phase B — cross-field comparison against another path in the same
    /// record (e.g. `{ "other": ["end"], "op": ">=" }`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<CompareDto>,

    /// Phase C2 — forward-only foreign-key reference.
    /// `{ "ref_table": "parent_table", "ref_field": "id" }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_key: Option<ForeignKeyDto>,

    /// Phase C3 — unique constraint.  The field value must not duplicate any
    /// existing row in the same table.  Requires an index on the column at
    /// DDL time (fail-closed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique: Option<bool>,
}

/// Foreign-key reference descriptor (wire form).
///
/// `ref_table` and `ref_field` are flat (de-interned) names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignKeyDto {
    /// The parent table name (flat, same repo).
    pub ref_table: String,
    /// The field in the parent table that must contain the referenced value.
    pub ref_field: String,
    /// Referential action on parent delete (Phase D).
    ///
    /// Serde default is [`FkAction::NoAction`] (so legacy schemas stored
    /// without `on_delete` round-trip unchanged and do not alter delete
    /// behavior on reload). Omitted from the wire via
    /// `skip_serializing_if = "FkAction::is_no_action"`. The *builder* default
    /// for a new foreign key is `Restrict` (safe-by-default), set explicitly by
    /// the builder — not via this serde default.
    #[serde(default, skip_serializing_if = "FkAction::is_no_action")]
    pub on_delete: FkAction,
    /// Referential action on parent update (Phase ②.2a — surface only;
    /// enforcement lands in ②.2b).
    ///
    /// Symmetric to [`on_delete`](Self::on_delete): same serde-default /
    /// `skip_serializing_if = "FkAction::is_no_action"` split so legacy schemas
    /// persisted without `on_update` deserialize to [`FkAction::NoAction`] and
    /// round-trip byte-identical. The builder default for a *new* foreign key
    /// is `NoAction` (additive — existing FK callers keep current behavior);
    /// callers wanting a non-default update action use
    /// [`foreign_key_on_update`](shamir_query_builder::ddl::schema::FieldBuilder::foreign_key_on_update)
    /// or
    /// [`foreign_key_with_actions`](shamir_query_builder::ddl::schema::FieldBuilder::foreign_key_with_actions).
    #[serde(default, skip_serializing_if = "FkAction::is_no_action")]
    pub on_update: FkAction,
}

/// Cross-field comparison descriptor (wire form).
///
/// `other` is the path of the field to compare against; `op` is the
/// comparison operator as a string (`"<"`, `"<="`, `"=="`, `"!="`,
/// `">="`, `">"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareDto {
    /// The other field path (flat string segments, NOT interned).
    pub other: Vec<String>,
    /// Comparison operator: `"<"` / `"<="` / `"=="` / `"!="` / `">="` / `">"`.
    pub op: String,
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
