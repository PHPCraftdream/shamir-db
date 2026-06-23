//! Declarative schema storage + interning — Phase A2.
//!
//! Ties together the table catalogue (`system_store`), the repo interner,
//! and the validator registry to persist, compile, and bind declarative
//! schemas.
//!
//! Key entry points:
//! - [`parse_schema`] — de-intern a persisted `List[Map]` schema into
//!   `Vec<FieldRule>` (used by boot-pass and DDL).
//! - [`ShamirDb::compile_table_schema`] — register + auto-bind a
//!   declarative schema validator for one table.
//! - The boot-pass in `core.rs::init` calls [`ShamirDb::boot_compile_schemas`]
//!   after `load_validators` to materialise all persisted declarative schemas.

use std::sync::Arc;

use crate::{DbError, DbResult};
use shamir_engine::validator::schema::{FieldRule, SchemaValidator};
use shamir_engine::validator::{RecordValidator, ValidatorBinding, WriteOp};
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use super::ShamirDb;

// ── Catalogue field names (stable on-disk contract) ─────────────────────

/// Catalogue field: the declarative schema (List of Map rules).
pub const SCHEMA_FIELD: &str = "schema";
/// Catalogue field: persistent RecordId of the schema validator.
pub const SCHEMA_VALIDATOR_ID_FIELD: &str = "schema_validator_id";
/// Catalogue field: monotonic schema version for optimistic concurrency.
/// Used by ALTER DDL (Phase A — DDL sub-unit); defined here as the stable
/// on-disk contract so both boot-pass and DDL share the same constant.
#[allow(dead_code)]
pub const SCHEMA_VERSION_FIELD: &str = "schema_version";

/// Auto-binding priority for declarative schema validators.
/// Below user-range [1000, 9999] so schema checks run first.
pub const SCHEMA_VALIDATOR_PRIORITY: u16 = 500;

// ── parse_schema ────────────────────────────────────────────────────────

/// De-intern a persisted schema `List[Map]` into `Vec<FieldRule>`.
///
/// Each map entry has the shape:
/// ```text
/// { "path": List[Int(id), ...], "type": Str, "required": Bool,
///   "min": Int|F64, "max": Int|F64, "len": Int, "max_len": Int,
///   "min_len": Int, "unsigned": Bool, "nullable": Bool,
///   "one_of": List[...], "array_of": Str }
/// ```
///
/// `path` values are interned field-name ids; they are resolved against
/// `interner` to produce string names.  A missing/unknown id is a hard
/// error (indicates catalogue corruption or interner desync).
pub fn parse_schema(schema: &QueryValue, interner: &Interner) -> DbResult<Vec<FieldRule>> {
    let items = match schema {
        QueryValue::List(items) => items,
        _ => {
            return Err(DbError::Validation("schema must be a List".to_string()));
        }
    };

    let mut rules = Vec::with_capacity(items.len());
    for item in items {
        rules.push(parse_one_rule(item, interner)?);
    }
    Ok(rules)
}

/// Parse one rule map, de-interning the `path` field ids.
fn parse_one_rule(item: &QueryValue, interner: &Interner) -> DbResult<FieldRule> {
    use shamir_engine::validator::schema::constraints::Constraints;
    use shamir_types::core::interner::InternerKey;

    // ── path (required) ────────────────────────────────────────────
    let path_ids = item
        .get("path")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DbError::Validation("schema rule missing 'path' List".to_string()))?;

    let mut path = Vec::with_capacity(path_ids.len());
    for id_val in path_ids {
        let id = id_val
            .as_i64()
            .ok_or_else(|| DbError::Validation("schema path element must be Int".to_string()))?;
        let key = InternerKey::new(id as u64);
        let name = interner.get_str(&key).ok_or_else(|| {
            DbError::Validation(format!("schema path: unknown interner id {}", id))
        })?;
        path.push(name.to_string());
    }

    // ── type (required) ────────────────────────────────────────────
    let type_str = item
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DbError::Validation("schema rule missing 'type' Str".to_string()))?;
    let ty = parse_type_tag(type_str)?;

    // ── constraints (all optional) ─────────────────────────────────
    let required = item
        .get("required")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let nullable = item
        .get("nullable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let unsigned = item
        .get("unsigned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let min = parse_num_constraint(item, "min");
    let max = parse_num_constraint(item, "max");

    let len = item.get("len").and_then(|v| v.as_i64()).map(|v| v as u64);
    let max_len = item
        .get("max_len")
        .and_then(|v| v.as_i64())
        .map(|v| v as u64);
    let min_len = item
        .get("min_len")
        .and_then(|v| v.as_i64())
        .map(|v| v as u64);

    let one_of = item.get("one_of").and_then(|v| {
        if let QueryValue::List(items) = v {
            Some(items.clone())
        } else {
            None
        }
    });

    let array_of = item
        .get("array_of")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_type_tag(s).ok());

    // Phase B — scalar-bridge, format, cross-field compare.
    let scalar = item
        .get("scalar")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let format = item
        .get("format")
        .and_then(|v| v.as_str())
        .and_then(shamir_engine::validator::schema::FormatKind::parse);
    let compare = item.get("compare").and_then(parse_cross_field_compare);
    let foreign_key = item.get("foreign_key").and_then(parse_foreign_key_ref);
    let unique = item
        .get("unique")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let constraints = Constraints {
        required,
        nullable,
        min,
        max,
        len,
        max_len,
        min_len,
        unsigned,
        one_of,
        array_of,
        scalar,
        format,
        compare,
        foreign_key,
        unique,
    };

    Ok(FieldRule {
        path,
        ty,
        constraints,
    })
}

/// Map a persisted type-tag string to [`TypeTag`].
fn parse_type_tag(s: &str) -> DbResult<shamir_engine::validator::schema::type_tag::TypeTag> {
    use shamir_engine::validator::schema::type_tag::TypeTag;
    match s {
        "string" => Ok(TypeTag::String),
        "int" => Ok(TypeTag::Int),
        "f64" => Ok(TypeTag::F64),
        "dec" => Ok(TypeTag::Dec),
        "bool" => Ok(TypeTag::Bool),
        "bin" => Ok(TypeTag::Bin),
        "list" => Ok(TypeTag::List),
        "map" => Ok(TypeTag::Map),
        "set" => Ok(TypeTag::Set),
        "null" => Ok(TypeTag::Null),
        "any" => Ok(TypeTag::Any),
        other => Err(DbError::Validation(format!(
            "unknown schema type tag: '{}'",
            other
        ))),
    }
}

/// Parse a numeric constraint from a rule map.
fn parse_num_constraint(
    item: &QueryValue,
    field: &str,
) -> Option<shamir_engine::validator::schema::constraints::Num> {
    use shamir_engine::validator::schema::constraints::Num;
    let v = item.get(field)?;
    if let Some(i) = v.as_i64() {
        Some(Num::Int(i))
    } else {
        v.as_f64().map(Num::F64)
    }
}

/// Parse a foreign-key reference from a catalogue Map.
///
/// Catalogue shape:
/// ```text
/// { "foreign_key": { "ref_table": "parent", "ref_field": "id" } }
/// ```
fn parse_foreign_key_ref(
    v: &QueryValue,
) -> Option<shamir_engine::validator::schema::ForeignKeyRef> {
    let map = v.as_object()?;
    let ref_table = map.get("ref_table")?.as_str()?.to_string();
    let ref_field = map.get("ref_field")?.as_str()?.to_string();
    Some(shamir_engine::validator::schema::ForeignKeyRef::new(
        ref_table, ref_field,
    ))
}

/// Parse a cross-field compare constraint from a rule map.
///
/// Catalogue shape:
/// ```text
/// { "compare": { "other": List[Str, ...], "op": "<" | "<=" | "==" | "!=" | ">=" | ">" } }
/// ```
/// The `other` path is a list of plain field-name strings (NOT interned ids —
/// cross-field paths are declarative and not stored on the interned hot path).
fn parse_cross_field_compare(
    v: &QueryValue,
) -> Option<shamir_engine::validator::schema::CrossFieldCompare> {
    use shamir_engine::validator::schema::{CompareOp, CrossFieldCompare};

    let map = v.as_object()?;
    let other_arr = map.get("other")?.as_array()?;
    let other: Vec<String> = other_arr
        .iter()
        .filter_map(|s| s.as_str().map(String::from))
        .collect();
    if other.is_empty() {
        return None;
    }
    let op_str = map.get("op")?.as_str()?;
    let op = match op_str {
        "<" => CompareOp::Lt,
        "<=" => CompareOp::Le,
        "==" => CompareOp::Eq,
        "!=" => CompareOp::Ne,
        ">=" => CompareOp::Ge,
        ">" => CompareOp::Gt,
        _ => return None,
    };
    Some(CrossFieldCompare::new(other, op))
}

// ── Schema validator name ───────────────────────────────────────────────

/// Canonical name for a table's auto-generated schema validator.
/// Format: `"__schema__/<db>/<repo>/<table>"`.
pub fn schema_validator_name(db: &str, repo: &str, table: &str) -> String {
    format!("__schema__/{}/{}/{}", db, repo, table)
}

// ── ShamirDb integration ────────────────────────────────────────────────

impl ShamirDb {
    /// Compile and register a declarative schema validator for one table.
    ///
    /// Called from:
    /// - **boot-pass** (`boot_compile_schemas`) for tables with a persisted
    ///   `schema` + `schema_validator_id`.
    /// - **DDL** (`add_table_as` with schema, `set_table_schema`) for first
    ///   compile or ALTER.
    ///
    /// The validator is registered under `schema_validator_id` with
    /// `ArtifactKind::Declarative` and auto-bound to all write ops at
    /// priority 500.
    pub(crate) async fn compile_table_schema(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        schema_validator_id: RecordId,
        rules: Vec<FieldRule>,
    ) -> DbResult<()> {
        let name = schema_validator_name(db_name, repo_name, table_name);
        let validator: Arc<dyn RecordValidator> = Arc::new(SchemaValidator::new(rules));

        // If already registered (e.g. ALTER replacing), swap the artifact.
        if self.validators.id_for_name(&name).is_some() {
            self.validators
                .replace_artifact(&schema_validator_id, validator);
        } else {
            self.validators
                .register(schema_validator_id, &name, validator)
                .map_err(|e| DbError::Validation(e.to_string()))?;
        }

        // Auto-bind to the table (idempotent — the table's info-twin
        // deduplicates by validator_id).
        let table = self.get_table(db_name, repo_name, table_name).await?;
        let binding = ValidatorBinding {
            validator_id: schema_validator_id,
            ops: vec![
                WriteOp::Insert,
                WriteOp::Update,
                WriteOp::Upsert,
                WriteOp::Delete,
            ]
            .into(),
            priority: SCHEMA_VALIDATOR_PRIORITY,
        };
        table.add_validator_binding(binding).await?;

        // Track in the global registry's bound_in.
        let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
        self.validators
            .add_binding(&schema_validator_id, &table_ref);

        Ok(())
    }

    /// Boot-pass: compile declarative schemas for all tables that have a
    /// persisted `schema` field.
    ///
    /// Called from `init()` after `load_validators()` (WASM validators) so
    /// declarative validators coexist with code validators.
    pub(super) async fn boot_compile_schemas(&self, table_records: &[QueryValue]) -> DbResult<()> {
        for trec in table_records {
            // Skip tables without a schema.
            let schema_val = match trec.get(SCHEMA_FIELD) {
                Some(s) if !matches!(s, QueryValue::Null) => s,
                _ => continue,
            };

            let db_name = trec["db_name"].as_str().unwrap_or_default();
            let repo_name = trec["repo_name"].as_str().unwrap_or_default();
            let table_name = trec["table_name"].as_str().unwrap_or_default();

            // Recover the persisted schema_validator_id.
            let id = match trec.get(SCHEMA_VALIDATOR_ID_FIELD).and_then(|v| v.as_str()) {
                Some(id_str) => match id_str.parse::<RecordId>() {
                    Ok(rid) => rid,
                    Err(e) => {
                        log::warn!(
                            "shamir_db::boot_compile_schemas: table '{}/{}/{}' \
                             bad schema_validator_id '{}': {}",
                            db_name,
                            repo_name,
                            table_name,
                            id_str,
                            e
                        );
                        continue;
                    }
                },
                None => {
                    log::warn!(
                        "shamir_db::boot_compile_schemas: table '{}/{}/{}' has \
                         schema but no schema_validator_id — skipping",
                        db_name,
                        repo_name,
                        table_name,
                    );
                    continue;
                }
            };

            // Resolve the repo interner for de-interning path ids.
            let interner_mgr = match self.resolve_repo_interner(db_name, repo_name).await {
                Ok(i) => i,
                Err(e) => {
                    log::warn!(
                        "shamir_db::boot_compile_schemas: cannot resolve interner \
                         for '{}/{}': {} — skipping schema for '{}'",
                        db_name,
                        repo_name,
                        e,
                        table_name,
                    );
                    continue;
                }
            };
            let interner = match interner_mgr.get().await {
                Ok(i) => i,
                Err(e) => {
                    log::warn!(
                        "shamir_db::boot_compile_schemas: cannot load interner \
                         for '{}/{}': {} — skipping schema for '{}'",
                        db_name,
                        repo_name,
                        e,
                        table_name,
                    );
                    continue;
                }
            };

            let rules = match parse_schema(schema_val, interner) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "shamir_db::boot_compile_schemas: failed to parse schema \
                         for '{}/{}/{}': {}",
                        db_name,
                        repo_name,
                        table_name,
                        e,
                    );
                    continue;
                }
            };

            if let Err(e) = self
                .compile_table_schema(db_name, repo_name, table_name, id, rules)
                .await
            {
                log::warn!(
                    "shamir_db::boot_compile_schemas: failed to compile schema \
                     for '{}/{}/{}': {}",
                    db_name,
                    repo_name,
                    table_name,
                    e,
                );
            }
        }
        Ok(())
    }

    /// Remove the declarative schema validator for a table (DROP cleanup).
    ///
    /// Called from `drop_table_cleaning_validators` when a table with a
    /// schema is dropped.  Removes the auto-binding and the registry entry
    /// so there is no id/name leak.
    pub(super) fn drop_schema_validator(&self, db_name: &str, repo_name: &str, table_name: &str) {
        let name = schema_validator_name(db_name, repo_name, table_name);
        if let Some(id) = self.validators.id_for_name(&name) {
            // Remove bound_in tracking.
            let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
            self.validators.remove_binding(&id, &table_ref);
            // Remove the validator itself.
            self.validators.remove(&id);
        }
    }

    /// Resolve the repo interner manager for a given db/repo pair.
    ///
    /// Used by `boot_compile_schemas` and DDL paths that need to
    /// de-intern schema path ids.
    pub(crate) async fn resolve_repo_interner(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<shamir_engine::table::interner_manager::InternerManager> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.repo_interner().await
    }
}
