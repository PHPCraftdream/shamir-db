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
use shamir_engine::query::filter::{query_value_to_filter_value as qv_to_fv_literal, FilterValue};
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

/// Convert a catalogue [`QueryValue`] `default` field back to a [`FilterValue`].
///
/// Strategy (boot-path READ):
/// 1. Literal variants → direct match via [`qv_to_fv_literal`] (no msgpack).
/// 2. `Map` (expression defaults such as `{"$fn": ...}`) → msgpack round-trip.
/// 3. Genuine failure → `log::warn!` with the rule path for context; returns
///    `None` so the boot-pass continues (boot-resilience) but the failure is
///    now visible rather than silently discarded.
fn parse_one_rule_default(qv: &QueryValue, item: &QueryValue) -> Option<FilterValue> {
    // Tier 1: direct literal match (no allocation).
    if let Some(fv) = qv_to_fv_literal(qv) {
        return Some(fv);
    }
    // Tier 2: msgpack for Map (expression defaults) and exotic types.
    if let Some(fv) = rmp_serde::to_vec_named(qv)
        .ok()
        .and_then(|bytes| rmp_serde::from_slice::<FilterValue>(&bytes).ok())
    {
        return Some(fv);
    }
    // Tier 3: genuine decode failure — log visibly, drop gracefully.
    let path_hint = item
        .get("path")
        .map(|p| format!("{:?}", p))
        .unwrap_or_else(|| "<unknown>".to_string());
    log::warn!(
        "schema_management::parse_one_rule: catalogue default decode failed \
         for field path {} — default dropped. qv = {:?}",
        path_hint,
        qv
    );
    None
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

    // ③.2c — default (literal or expression; extends ②.4b literal-only).
    // READ: QueryValue → FilterValue.
    // Literals use a direct match (no msgpack allocation). Map (expression
    // defaults like {"$fn":...}) and exotic types fall back to msgpack.
    // On genuine decode failure we log a warning and drop the default so
    // the boot-pass continues — failure is visible, not silent.
    let default: Option<FilterValue> = item
        .get("default")
        .and_then(|qv| parse_one_rule_default(qv, item));

    // F-4: `array_of`/`format`/`compare`/`foreign_key` now distinguish "field
    // absent" (legitimate default / None) from "field present but does not
    // parse" (hard error). Previously a typo'd or malformed value silently
    // collapsed to `None` (or `NoAction` for FK actions), making the DDL
    // report success while dropping the constraint.

    // `array_of`: present-but-unparseable type tag → error (was silent None).
    let array_of = match item.get("array_of") {
        Some(v) => {
            let s = v.as_str().ok_or_else(|| {
                DbError::Validation(format!("schema 'array_of' must be a Str, got {:?}", v))
            })?;
            Some(parse_type_tag(s).map_err(|_e: DbError| {
                DbError::Validation(format!("schema 'array_of' has unknown type tag: '{s}'"))
            })?)
        }
        None => None,
    };

    // Phase B — scalar-bridge, format, cross-field compare.
    let scalar = item
        .get("scalar")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    // `format`: present-but-unrecognised name → error (was silent None).
    let format = match item.get("format") {
        Some(v) => {
            let s = v.as_str().ok_or_else(|| {
                DbError::Validation(format!("schema 'format' must be a Str, got {:?}", v))
            })?;
            Some(
                shamir_engine::validator::schema::FormatKind::parse(s).ok_or_else(|| {
                    DbError::Validation(format!("schema 'format' has unknown format: '{s}'"))
                })?,
            )
        }
        None => None,
    };
    // `compare`/`foreign_key`: outer key absent → Ok(None); present-but-
    // malformed inside → Err (was silent None).
    let compare = parse_cross_field_compare(item)?;
    let foreign_key = parse_foreign_key_ref(item)?;
    let unique = item
        .get("unique")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // ③.2d — server-stamping flags (absent in legacy catalogue rows = false).
    let auto_now = item
        .get("auto_now")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let auto_now_add = item
        .get("auto_now_add")
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
        default,
        array_of,
        scalar,
        format,
        compare,
        foreign_key,
        unique,
        auto_now,
        auto_now_add,
    };

    // F-17 (#810) — `keyset_safe` proof: absent in pre-fix catalogue rows
    // (serde/manual default = false = unproven). The DDL handlers stamp
    // `true` only when `table.count() == 0` at bind time.
    let keyset_safe = item
        .get("keyset_safe")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(FieldRule {
        path,
        ty,
        constraints,
        keyset_safe,
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

/// Parse a foreign-key reference from a catalogue rule Map.
///
/// Catalogue shape:
/// ```text
/// { "foreign_key": { "ref_table": "parent", "ref_field": "id" } }
/// ```
///
/// Returns:
/// - `Ok(None)` when the `"foreign_key"` key is absent (no FK constraint).
/// - `Err(DbError::Validation)` when the key is present but malformed (not a
///   Map, missing `ref_table`/`ref_field`, or an unrecognised `on_delete`/
///   `on_update` action string) — F-4: was previously silently collapsed to
///   `None`, conflating "garbled" with "absent".
/// - `Ok(Some(..))` on success.
fn parse_foreign_key_ref(
    item: &QueryValue,
) -> DbResult<Option<shamir_engine::validator::schema::ForeignKeyRef>> {
    // Outer key absent → no FK constraint.
    let v = match item.get("foreign_key") {
        Some(v) => v,
        None => return Ok(None),
    };
    let map = v
        .as_object()
        .ok_or_else(|| DbError::Validation("schema 'foreign_key' must be a Map".to_string()))?;
    let ref_table = map
        .get("ref_table")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DbError::Validation("schema 'foreign_key.ref_table' must be a Str".to_string())
        })?
        .to_string();
    let ref_field = map
        .get("ref_field")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DbError::Validation("schema 'foreign_key.ref_field' must be a Str".to_string())
        })?
        .to_string();
    // Phase D / ②.2a: parse on_delete / on_update actions. Legacy catalogue
    // rows without the fields get NoAction (the FkAction serde/wire default),
    // preserving backward compatibility. A PRESENT-but-unrecognised string is
    // now a hard error (F-4: was previously silently downgraded to NoAction).
    let on_delete = parse_fk_action(v, "on_delete")?;
    let on_update = parse_fk_action(v, "on_update")?;
    Ok(Some(
        shamir_engine::validator::schema::ForeignKeyRef::with_actions(
            ref_table, ref_field, on_delete, on_update,
        ),
    ))
}

/// Parse an `on_delete` / `on_update` referential action from a foreign-key
/// Map value.
///
/// - Field **absent** → `FkAction::NoAction` (documented backward-compat
///   default for legacy rows; do not change — see `parse_foreign_key_ref`).
/// - Field **present** but not a `Str`, or an unrecognised action string →
///   hard `DbError::Validation` (F-4: was previously silently downgraded to
///   `NoAction`, conflating "garbled" with "absent").
fn parse_fk_action(
    fk_map: &QueryValue,
    field: &str,
) -> DbResult<shamir_engine::validator::schema::FkAction> {
    use shamir_engine::validator::schema::FkAction;
    let Some(v) = fk_map.get(field) else {
        return Ok(FkAction::NoAction);
    };
    let s = v.as_str().ok_or_else(|| {
        DbError::Validation(format!(
            "schema 'foreign_key.{field}' must be a Str, got {:?}",
            v
        ))
    })?;
    let action = match s {
        "no_action" => FkAction::NoAction,
        "restrict" => FkAction::Restrict,
        "cascade" => FkAction::Cascade,
        "set_null" => FkAction::SetNull,
        _ => {
            return Err(DbError::Validation(format!(
                "schema 'foreign_key.{field}' has unknown action: '{s}'"
            )));
        }
    };
    Ok(action)
}

/// Parse a cross-field compare constraint from a catalogue rule Map.
///
/// Catalogue shape:
/// ```text
/// { "compare": { "other": List[Str, ...], "op": "<" | "<=" | "==" | "!=" | ">=" | ">" } }
/// ```
/// The `other` path is a list of plain field-name strings (NOT interned ids —
/// cross-field paths are declarative and not stored on the interned hot path).
///
/// Returns:
/// - `Ok(None)` when the `"compare"` key is absent (no compare constraint).
/// - `Err(DbError::Validation)` when the key is present but malformed (not a
///   Map, `other` missing/not-a-List/empty/non-string elements, `op` missing/
///   not-a-Str/unrecognised) — F-4: was previously silently collapsed to
///   `None`.
/// - `Ok(Some(..))` on success.
fn parse_cross_field_compare(
    item: &QueryValue,
) -> DbResult<Option<shamir_engine::validator::schema::CrossFieldCompare>> {
    use shamir_engine::validator::schema::{CompareOp, CrossFieldCompare};

    // Outer key absent → no compare constraint.
    let v = match item.get("compare") {
        Some(v) => v,
        None => return Ok(None),
    };
    let map = v
        .as_object()
        .ok_or_else(|| DbError::Validation("schema 'compare' must be a Map".to_string()))?;
    let other_arr = map.get("other").and_then(|v| v.as_array()).ok_or_else(|| {
        DbError::Validation("schema 'compare.other' must be a List[Str]".to_string())
    })?;
    let other: Vec<String> = other_arr
        .iter()
        .map(|s| {
            s.as_str().map(String::from).ok_or_else(|| {
                DbError::Validation("schema 'compare.other' must be a List[Str]".to_string())
            })
        })
        .collect::<DbResult<Vec<_>>>()?;
    if other.is_empty() {
        return Err(DbError::Validation(
            "schema 'compare.other' must not be empty".to_string(),
        ));
    }
    let op_str = map
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DbError::Validation("schema 'compare.op' must be a Str".to_string()))?;
    let op = match op_str {
        "<" => CompareOp::Lt,
        "<=" => CompareOp::Le,
        "==" => CompareOp::Eq,
        "!=" => CompareOp::Ne,
        ">=" => CompareOp::Ge,
        ">" => CompareOp::Gt,
        _ => {
            return Err(DbError::Validation(format!(
                "schema 'compare.op' has unknown operator: '{op_str}'"
            )));
        }
    };
    Ok(Some(CrossFieldCompare::new(other, op)))
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
    ///
    /// **Rollback symmetry (F-24, #817):** if this is a FRESH registration
    /// (not an ALTER's `replace_artifact`) and a LATER step in the same call
    /// (`get_table` / `add_validator_binding`) fails, the fresh registration
    /// is undone (`ValidatorRegistry::remove`) before returning `Err`, so no
    /// orphaned validator survives in the registry.
    ///
    /// **ALTER rollback symmetry (F-27b, #827):** an ALTER's
    /// `replace_artifact` swaps the LIVE compiled validator in place — the
    /// NEW rules start being enforced the instant the swap runs, before
    /// `activation_result` is even known. If a LATER step then fails, the
    /// OLD compiled artifact (captured just before the swap) is put straight
    /// back via a second `replace_artifact` call, so the registry's live
    /// artifact matches the catalogue's `rec_prev` rollback (still pointing
    /// at the SAME `schema_validator_id`, expecting the OLD rules to be
    /// live) — see the inline rationale.
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

        // F-24 (#817): track whether THIS call performed a FRESH registration
        // (the table's first-ever schema, or a boot-pass materialisation) vs
        // an ALTER's `replace_artifact` (a previously-active, still-valid
        // schema version whose validator already existed before this call).
        // Only a fresh registration is undone on later failure — see the
        // rollback at the bottom of this function.
        //
        // WHY the `replace_artifact` (ALTER) branch must NEVER be undone via
        // a bare `remove` here: it swaps an EXISTING, previously-working
        // validator's artifact in place, preserving its name + table bindings
        // (RCU). Rolling it back needs the OLD artifact restored (F-27b,
        // below), not a bare removal — `ValidatorRegistry::remove` would
        // instead DELETE the validator entirely, orphaning the unchanged
        // catalogue reference and breaking the previously-working schema.
        let freshly_registered = self.validators.id_for_name(&name).is_none();

        // F-27b (#827): capture the artifact that's about to be replaced, so
        // it can be put back if a later step fails — see the restore at the
        // bottom of this function. `None` when `freshly_registered` (nothing
        // to restore — the fresh-registration rollback removes instead).
        //
        // `prior_artifact` should only be `None` here if the registry's
        // name→id and id→artifact maps have desynced (a `name_to_id` entry
        // with no matching `by_id` entry) — `register` always inserts both
        // together, so this registry invariant should already prevent it.
        // If it ever happens there is simply nothing to restore; that's a
        // safe no-op, not a case worth inventing extra handling for.
        let prior_artifact: Option<Arc<dyn RecordValidator>> = if freshly_registered {
            None
        } else {
            self.validators.get_by_id(&schema_validator_id)
        };

        if freshly_registered {
            self.validators
                .register(schema_validator_id, &name, validator)
                .map_err(|e| DbError::Validation(e.to_string()))?;
        } else {
            // F-24 (#817) secondary hardening: `replace_artifact` returns
            // `false` (a silent no-op) when the passed `schema_validator_id`
            // is NOT the one registered under `name` — exactly the
            // stale-name-collision shape the escalation analysis hinged on
            // (a failed first-schema call orphaning a validator under `name`,
            // then a retry minting a NEW id and no-op'ing here). The primary
            // fix below (undoing fresh registrations on failure) already
            // eliminates the known path to such a collision; this assertion
            // makes any FUTURE path that could recreate one fail loudly
            // instead of leaving the catalogue pointing at an unregistered
            // validator id.
            let replaced = self
                .validators
                .replace_artifact(&schema_validator_id, validator);
            if !replaced {
                return Err(DbError::Validation(format!(
                    "schema_validator_id {schema_validator_id} is not the id registered \
                     under name '{name}' (stale name collision) — refusing to activate"
                )));
            }
        }

        // Activation: resolve the table, bind the validator to it, and record
        // the global `bound_in` reference. Captured as a single `Result` so a
        // failure in ANY later step can be unwound symmetrically below —
        // mirroring the catalogue-level `rec_prev` rollback the caller
        // (`admin_schema.rs`) performs on this function's `Err`.
        let activation_result: DbResult<()> = async {
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
        .await;

        // F-24 (#817): if a FRESH registration succeeded above but a LATER
        // step (`get_table` / `add_validator_binding`) failed, undo exactly
        // that registration — restoring the registry to its pre-call state so
        // no orphan survives. Safe because `add_binding` (the LAST step above)
        // never ran before the failure, so the freshly-registered validator
        // was never bound to anything; `ValidatorRegistry::remove` does not
        // itself check binding state, but there is nothing to be bound here.
        //
        // F-27b (#827): if the ALTER branch's `replace_artifact` ran above but
        // a LATER step then failed, put the OLD compiled validator back —
        // symmetric with the catalogue's `rec_prev` rollback, which still
        // points at this same `schema_validator_id` expecting the OLD rules
        // to be what's live. Without this, the live registry would keep
        // enforcing the NEW (never-fully-activated) rules under an id the
        // catalogue believes still holds the OLD schema.
        if activation_result.is_err() {
            if freshly_registered {
                self.validators.remove(&schema_validator_id);
            } else if let Some(old) = prior_artifact {
                self.validators.replace_artifact(&schema_validator_id, old);
            }
        }

        activation_result
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
