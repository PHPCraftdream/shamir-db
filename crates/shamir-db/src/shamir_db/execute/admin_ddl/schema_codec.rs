//! Schema-catalogue codec helpers: DTO <-> `QueryValue` conversion.
//!
//! Split out of `admin_schema.rs` during the layout sweep. This module owns
//! the serialisation of `FieldRuleDto` rule sets into the catalogue record
//! form and the reverse parsing (catalogue `QueryValue` -> DTOs), plus the
//! `map_insert` utility used when assembling catalogue maps.

use crate::query::admin::{
    CompareDto, ConstraintsDto, FieldRuleDto, FkAction, ForeignKeyDto, NumDto,
};
use crate::query::filter::{
    filter_value_to_query_value as fv_to_qv_literal,
    query_value_to_filter_value as qv_to_fv_literal, FilterValue,
};
use crate::types::common::{new_map, TMap};
use crate::types::value::QueryValue;
use shamir_storage::error::DbError;
use shamir_types::core::interner::{Interner, InternerKey};

use crate::shamir_db::shamir_db::schema_management::SCHEMA_VERSION_FIELD;

/// The table record's schema version; absent or non-integer reads as 0.
pub(super) fn schema_version_of(rec: &QueryValue) -> u64 {
    rec.get(SCHEMA_VERSION_FIELD)
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as u64
}

/// Insert a `(key, value)` pair into a `QueryValue::Map` in place.
///
/// `QueryValue` exposes `as_object` (immutable) but no `as_object_mut`;
/// this helper does the in-place mutation via pattern matching.
pub(super) fn map_insert(qv: &mut QueryValue, key: &str, value: QueryValue) {
    if let QueryValue::Map(m) = qv {
        m.insert(key.to_string(), value);
    }
}
// ── serialisation helpers ──────────────────────────────────────────────────

/// Serialise a list of DTO rules into the **catalogue form** (paths as
/// interned-id `List[Int]`, exactly the shape `parse_schema` reads).
/// Fallible: a base-intern over the per-repo quota surfaces the typed
/// `interner_quota_exceeded` error (RU8 acceptance 8 — no panics).
pub(super) fn serialise_rules(
    rules: &[FieldRuleDto],
    interner: &Interner,
) -> Result<QueryValue, DbError> {
    let items: Vec<QueryValue> = rules
        .iter()
        .map(|r| serialise_one_rule_catalogue(r, interner))
        .collect::<Result<_, DbError>>()?;
    Ok(QueryValue::List(items))
}

/// Serialise one rule into the catalogue Map form (interned path ids).
fn serialise_one_rule_catalogue(
    rule: &FieldRuleDto,
    interner: &Interner,
) -> Result<QueryValue, DbError> {
    let mut m = new_map();

    // path: List[Int] (interned ids).
    let path_ids: Vec<QueryValue> = rule
        .path
        .iter()
        .map(|seg| {
            let touch = interner
                .touch_ind(seg)
                .map_err(shamir_storage::error::DbError::from)?;
            Ok(QueryValue::Int(touch.into_key().id() as i64))
        })
        .collect::<Result<Vec<_>, DbError>>()?;
    m.insert("path".to_string(), QueryValue::List(path_ids));

    // type: Str.
    m.insert("type".to_string(), QueryValue::Str(rule.r#type.clone()));

    // Constraints (optional fields, only inserted when present).
    insert_constraint_fields(&mut m, &rule.constraints);

    // F-17 (#810) — persist `keyset_safe` only when true (omit when false
    // so legacy catalogue rows stay byte-identical).
    if rule.keyset_safe {
        m.insert("keyset_safe".to_string(), QueryValue::Bool(true));
    }

    Ok(QueryValue::Map(m))
}

/// Serialise a list of DTO rules into the **flat (de-interned) wire form**
/// — paths as `List[Str]`, suitable for `GetTableSchema` responses.
pub(super) fn serialise_rules_flat(rules: &[FieldRuleDto]) -> QueryValue {
    let items: Vec<QueryValue> = rules.iter().map(serialise_one_rule_flat).collect();
    QueryValue::List(items)
}

/// Serialise one rule into the flat wire form (string paths).
fn serialise_one_rule_flat(rule: &FieldRuleDto) -> QueryValue {
    let mut m = new_map();

    let path_strs: Vec<QueryValue> = rule
        .path
        .iter()
        .map(|s| QueryValue::Str(s.clone()))
        .collect();
    m.insert("path".to_string(), QueryValue::List(path_strs));
    m.insert("type".to_string(), QueryValue::Str(rule.r#type.clone()));

    insert_constraint_fields(&mut m, &rule.constraints);

    // F-17 (#810) — include `keyset_safe` when true (same omit-when-false
    // contract as the catalogue form).
    if rule.keyset_safe {
        m.insert("keyset_safe".to_string(), QueryValue::Bool(true));
    }

    QueryValue::Map(m)
}

/// Insert the optional constraint fields from a DTO into a catalogue/flat
/// Map. Shared by both serialisers — the constraint fields are identical in
/// both forms (only `path` differs).
fn insert_constraint_fields(m: &mut TMap<String, QueryValue>, c: &ConstraintsDto) {
    if let Some(v) = c.required {
        m.insert("required".to_string(), QueryValue::Bool(v));
    }
    if let Some(v) = c.nullable {
        m.insert("nullable".to_string(), QueryValue::Bool(v));
    }
    if let Some(v) = c.unsigned {
        m.insert("unsigned".to_string(), QueryValue::Bool(v));
    }
    if let Some(n) = &c.min {
        m.insert(
            "min".to_string(),
            match n {
                NumDto::Int(i) => QueryValue::Int(*i),
                NumDto::F64(f) => QueryValue::F64(*f),
            },
        );
    }
    if let Some(n) = &c.max {
        m.insert(
            "max".to_string(),
            match n {
                NumDto::Int(i) => QueryValue::Int(*i),
                NumDto::F64(f) => QueryValue::F64(*f),
            },
        );
    }
    if let Some(v) = c.len {
        m.insert("len".to_string(), QueryValue::Int(v as i64));
    }
    if let Some(v) = c.max_len {
        m.insert("max_len".to_string(), QueryValue::Int(v as i64));
    }
    if let Some(v) = c.min_len {
        m.insert("min_len".to_string(), QueryValue::Int(v as i64));
    }
    if let Some(items) = &c.one_of {
        m.insert("one_of".to_string(), QueryValue::List(items.clone()));
    }
    // ③.2c — default (literal or expression; extends ②.4b literal-only).
    // WRITE: FilterValue → QueryValue.
    // Literals use a direct match (no allocation, no msgpack).
    // Expression defaults ($fn/$ref/$expr/$cond/$param/$query) have no direct
    // QueryValue equivalent and are serialised via msgpack round-trip, which
    // preserves the untagged-serde shape faithfully.
    // On failure (malformed expression) we log a warning instead of silently
    // dropping — boot-resilience is preserved (the field is omitted rather
    // than aborting), but the failure is now visible in the log.
    if let Some(fv) = &c.default {
        let qv_opt: Option<QueryValue> = if let Some(qv) = fv_to_qv_literal(fv) {
            // Literal path: direct, zero-copy conversion.
            Some(qv)
        } else {
            // Expression path: msgpack round-trip.
            match rmp_serde::to_vec_named(fv)
                .ok()
                .and_then(|bytes| rmp_serde::from_slice::<QueryValue>(&bytes).ok())
            {
                Some(qv) => Some(qv),
                None => {
                    log::warn!(
                        "admin_schema::insert_constraint_fields: \
                         failed to serialise default FilterValue to QueryValue — \
                         field omitted from catalogue. value = {:?}",
                        fv
                    );
                    None
                }
            }
        };
        if let Some(qv) = qv_opt {
            m.insert("default".to_string(), qv);
        }
    }
    if let Some(s) = &c.array_of {
        m.insert("array_of".to_string(), QueryValue::Str(s.clone()));
    }
    // Phase B fields.
    if let Some(s) = &c.scalar {
        m.insert("scalar".to_string(), QueryValue::Str(s.clone()));
    }
    if let Some(s) = &c.format {
        m.insert("format".to_string(), QueryValue::Str(s.clone()));
    }
    if let Some(cmp) = &c.compare {
        let mut cmp_m = new_map();
        let other_list: Vec<QueryValue> = cmp
            .other
            .iter()
            .map(|s| QueryValue::Str(s.clone()))
            .collect();
        cmp_m.insert("other".to_string(), QueryValue::List(other_list));
        cmp_m.insert("op".to_string(), QueryValue::Str(cmp.op.clone()));
        m.insert("compare".to_string(), QueryValue::Map(cmp_m));
    }
    // Phase C3 — unique.
    if let Some(v) = c.unique {
        m.insert("unique".to_string(), QueryValue::Bool(v));
    }
    // ③.2d — server-stamping flags (omit when false — legacy rows stay unchanged).
    if c.auto_now {
        m.insert("auto_now".to_string(), QueryValue::Bool(true));
    }
    if c.auto_now_add {
        m.insert("auto_now_add".to_string(), QueryValue::Bool(true));
    }
    // Phase C2 — foreign_key.
    if let Some(fk) = &c.foreign_key {
        let mut fk_m = new_map();
        fk_m.insert(
            "ref_table".to_string(),
            QueryValue::Str(fk.ref_table.clone()),
        );
        fk_m.insert(
            "ref_field".to_string(),
            QueryValue::Str(fk.ref_field.clone()),
        );
        // Phase D — persist the ON DELETE action so reverse-FK discovery
        // (RESTRICT / CASCADE / SET NULL) survives the catalogue round-trip.
        // Mirrors `foreign_key_dto_from_qv` read mapping. NoAction is omitted
        // so legacy rows (and the common default) stay byte-identical.
        let on_delete = match fk.on_delete {
            FkAction::Restrict => Some("restrict"),
            FkAction::Cascade => Some("cascade"),
            FkAction::SetNull => Some("set_null"),
            FkAction::NoAction => None,
        };
        if let Some(action) = on_delete {
            fk_m.insert("on_delete".to_string(), QueryValue::Str(action.to_string()));
        }
        // Phase ②.2a — persist the ON UPDATE action (surface only; enforcement
        // lands in ②.2b). Symmetric to `on_delete` above; NoAction is omitted
        // so legacy rows (and the default) stay byte-identical.
        let on_update = match fk.on_update {
            FkAction::Restrict => Some("restrict"),
            FkAction::Cascade => Some("cascade"),
            FkAction::SetNull => Some("set_null"),
            FkAction::NoAction => None,
        };
        if let Some(action) = on_update {
            fk_m.insert("on_update".to_string(), QueryValue::Str(action.to_string()));
        }
        m.insert("foreign_key".to_string(), QueryValue::Map(fk_m));
    }
}

// ── deserialisation helper (catalogue → DTO, de-interned) ──────────────────

pub(super) fn checked_catalogue_rules(
    value: Option<&QueryValue>,
    interner: &Interner,
) -> Result<Vec<FieldRuleDto>, crate::query::batch::BatchError> {
    match value {
        Some(qv) => dto_list_from_catalogue(qv, interner).map_err(|message| {
            crate::query::batch::BatchError::QueryError {
                alias: String::new(),
                message,
                code: Some("bad_schema".to_string()),
            }
        }),
        None => Ok(Vec::new()),
    }
}

/// Parse a catalogue-form schema (`List[Map]` with interned path ids) back
/// into the wire DTO form (flat string paths). Used by add/remove/get.
pub(super) fn dto_list_from_catalogue(
    qv: &QueryValue,
    interner: &Interner,
) -> Result<Vec<FieldRuleDto>, String> {
    let items = match qv {
        QueryValue::List(l) => l,
        _ => return Ok(Vec::new()),
    };
    for item in items {
        if let Some(fk) = item.get("foreign_key") {
            let map = fk.as_object().ok_or("invalid foreign key record")?;
            for field in ["on_delete", "on_update"] {
                if let Some(value) = map.get(field) {
                    match value.as_str() {
                        Some("no_action" | "restrict" | "cascade" | "set_null") => {}
                        _ => return Err(format!("invalid foreign key {field}: {value:?}")),
                    }
                }
            }
        }
    }
    Ok(items
        .iter()
        .filter_map(|item| dto_one_from_catalogue(item, interner))
        .collect())
}

/// Parse one catalogue rule Map into a DTO (de-interning the path).
fn dto_one_from_catalogue(item: &QueryValue, interner: &Interner) -> Option<FieldRuleDto> {
    let m = item.as_object()?;

    // path: List[Int] → List<String>.
    let path_ids = m.get("path")?.as_array()?;
    let path: Vec<String> = path_ids
        .iter()
        .map(|id_val| {
            let id = id_val.as_i64()?;
            let key = InternerKey::new(id as u64);
            interner.get_str(&key).map(|s| s.to_string())
        })
        .collect::<Option<Vec<_>>>()?;

    let r#type = m.get("type")?.as_str()?.to_string();

    let constraints = ConstraintsDto {
        required: m.get("required").and_then(|v| v.as_bool()),
        nullable: m.get("nullable").and_then(|v| v.as_bool()),
        unsigned: m.get("unsigned").and_then(|v| v.as_bool()),
        min: m.get("min").and_then(num_dto_from_qv),
        max: m.get("max").and_then(num_dto_from_qv),
        len: m.get("len").and_then(|v| v.as_i64()).map(|v| v as u64),
        max_len: m.get("max_len").and_then(|v| v.as_i64()).map(|v| v as u64),
        min_len: m.get("min_len").and_then(|v| v.as_i64()).map(|v| v as u64),
        one_of: m.get("one_of").and_then(|v| {
            if let QueryValue::List(items) = v {
                Some(items.clone())
            } else {
                None
            }
        }),
        // ③.2c — default (literal or expression; extends ②.4b literal-only).
        // READ: QueryValue → FilterValue.
        // Literals use a direct match (no msgpack). Map (expression defaults
        // like {"$fn":...}) fall back to msgpack. On genuine decode failure
        // we log a warning — the default is dropped (boot-resilience preserved),
        // but the failure is visible in the log instead of being silent.
        default: m
            .get("default")
            .and_then(|qv| catalogue_qv_to_filter_value(qv, "dto_one_from_catalogue", "default")),
        array_of: m.get("array_of").and_then(|v| v.as_str()).map(String::from),
        scalar: m.get("scalar").and_then(|v| v.as_str()).map(String::from),
        format: m.get("format").and_then(|v| v.as_str()).map(String::from),
        compare: m.get("compare").and_then(compare_dto_from_qv),
        foreign_key: m.get("foreign_key").and_then(foreign_key_dto_from_qv),
        unique: m.get("unique").and_then(|v| v.as_bool()),
        // ③.2d — server-stamping flags (absent = false for legacy rows).
        auto_now: m.get("auto_now").and_then(|v| v.as_bool()).unwrap_or(false),
        auto_now_add: m
            .get("auto_now_add")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    Some(FieldRuleDto {
        path,
        r#type,
        constraints,
        // F-17 (#810) — absent in pre-fix rows = false (unproven).
        keyset_safe: m
            .get("keyset_safe")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Extract a `NumDto` from a catalogue QueryValue.
fn num_dto_from_qv(v: &QueryValue) -> Option<NumDto> {
    match v {
        QueryValue::Int(i) => Some(NumDto::Int(*i)),
        QueryValue::F64(f) => Some(NumDto::F64(*f)),
        _ => None,
    }
}

/// Extract a `ForeignKeyDto` from a catalogue Map.
fn foreign_key_dto_from_qv(v: &QueryValue) -> Option<ForeignKeyDto> {
    let m = v.as_object()?;
    let ref_table = m.get("ref_table")?.as_str()?.to_string();
    let ref_field = m.get("ref_field")?.as_str()?.to_string();
    // Legacy catalogue rows written before on_delete exist — default to
    // NoAction (the serde/wire default) so they round-trip unchanged.
    let on_delete = match m.get("on_delete").and_then(|v| v.as_str()) {
        Some("no_action") => FkAction::NoAction,
        Some("restrict") => FkAction::Restrict,
        Some("cascade") => FkAction::Cascade,
        Some("set_null") => FkAction::SetNull,
        _ => FkAction::default(),
    };
    // Phase ②.2a — read on_update (surface only; enforcement lands in ②.2b).
    // Legacy rows without the field default to NoAction, mirroring on_delete.
    let on_update = match m.get("on_update").and_then(|v| v.as_str()) {
        Some("no_action") => FkAction::NoAction,
        Some("restrict") => FkAction::Restrict,
        Some("cascade") => FkAction::Cascade,
        Some("set_null") => FkAction::SetNull,
        _ => FkAction::default(),
    };
    Some(ForeignKeyDto {
        ref_table,
        ref_field,
        on_delete,
        on_update,
    })
}

/// Convert a catalogue [`QueryValue`] back to a [`FilterValue`].
///
/// Strategy (READ path):
/// 1. Literal variants (Null/Bool/Int/F64/Str/Bin/List) → direct match via
///    [`qv_to_fv_literal`] (no msgpack, zero-copy for common case).
/// 2. `Map` (expression defaults such as `{"$fn": ...}`) → msgpack round-trip,
///    which preserves the untagged-serde shape that encodes expression variants.
/// 3. Other exotic types (Dec/Big/Set) → msgpack round-trip as last resort.
/// 4. Genuine failure → `log::warn!` with context (caller + field name) and
///    `None` (default is dropped, boot-resilience preserved — no panic).
fn catalogue_qv_to_filter_value(qv: &QueryValue, caller: &str, field: &str) -> Option<FilterValue> {
    // Tier 1: direct literal match.
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
    log::warn!(
        "admin_schema::{}: catalogue default decode failed for field '{}' — \
         dropped. qv = {:?}",
        caller,
        field,
        qv
    );
    None
}

/// Extract a `CompareDto` from a catalogue Map.
fn compare_dto_from_qv(v: &QueryValue) -> Option<CompareDto> {
    let m = v.as_object()?;
    let other_arr = m.get("other")?.as_array()?;
    let other: Vec<String> = other_arr
        .iter()
        .filter_map(|s| s.as_str().map(String::from))
        .collect();
    if other.is_empty() {
        return None;
    }
    let op = m.get("op")?.as_str()?.to_string();
    Some(CompareDto { other, op })
}
