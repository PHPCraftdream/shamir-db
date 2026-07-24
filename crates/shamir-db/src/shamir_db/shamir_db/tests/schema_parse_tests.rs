//! F-4 regression tests for `parse_schema` / `parse_one_rule` (Bug 2:
//! silent-None constraint parsing).
//!
//! Before F-4, a PRESENT-but-unparseable value for `array_of` / `format` /
//! `compare` / `foreign_key` / `on_delete` / `on_update` silently collapsed to
//! `None` (or `NoAction` for FK actions), making the DDL report success while
//! dropping the constraint. These tests exercise the catalogue-form parse path
//! directly (the same `parse_schema` the DDL handlers call as their precompile
//! gate, and the boot-pass calls on reopen) to prove:
//!
//! 1. A **garbled-but-present** value is now a hard `DbError::Validation`
//!    naming the offending field (not a silent success).
//! 2. A **genuinely-absent** field still parses to its documented default
//!    (`None` for the optional constraints; `NoAction` for `on_delete` /
//!    `on_update`) — no regression to the legitimate default path.
//!
//! These are unit-level because `on_delete` / `on_update` garbled strings are
//! NOT reachable via the typed DTO builder (`FkAction` is an enum) — they can
//! only arise from a hand-built (or corrupt) catalogue Map, which is exactly
//! what `parse_schema` reads. Testing here covers the full fixed surface in
//! one place; the DDL-level consequence (the DDL call returns the error
//! instead of silently succeeding) is covered by the e2e suite in
//! `declarative_schema_ddl_atomic_e2e.rs`.

use shamir_engine::validator::schema::FkAction;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::super::schema_management::parse_schema;

// ── helpers ──────────────────────────────────────────────────────────────

/// Build a minimal valid catalogue rule Map (path + type), with the field
/// name `name` interned so the path id resolves.
fn base_rule(interner: &Interner, name: &str, type_str: &str) -> QueryValue {
    let id = interner.touch_ind(name).unwrap().into_key().id() as i64;
    let mut m = new_map();
    m.insert(
        "path".to_string(),
        QueryValue::List(vec![QueryValue::Int(id)]),
    );
    m.insert("type".to_string(), QueryValue::Str(type_str.to_string()));
    QueryValue::Map(m)
}

/// Insert a constraint sub-field into a rule Map.
fn with_field(mut rule: QueryValue, key: &str, val: QueryValue) -> QueryValue {
    if let QueryValue::Map(m) = &mut rule {
        m.insert(key.to_string(), val);
    }
    rule
}

/// Wrap a single rule into the `List[Map]` catalogue form `parse_schema` expects.
fn schema_of(rule: QueryValue) -> QueryValue {
    QueryValue::List(vec![rule])
}

/// Assert `parse_schema` rejects the schema and the error names `field`.
fn assert_rejects(schema: &QueryValue, interner: &Interner, field: &str) {
    let err = parse_schema(schema, interner)
        .err()
        .unwrap_or_else(|| panic!("expected error naming '{field}', parsed Ok"));
    assert!(
        err.to_string().contains(field),
        "error should name '{field}', got: {err}"
    );
}

// ── array_of ─────────────────────────────────────────────────────────────

#[test]
fn garbled_array_of_rejected() {
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "tags", "list"),
        "array_of",
        QueryValue::Str("bogus_type".into()),
    );
    assert_rejects(&schema_of(rule), &interner, "array_of");
}

#[test]
fn absent_array_of_defaults_to_none() {
    let interner = Interner::new();
    let rule = base_rule(&interner, "tags", "list"); // no array_of key
    let rules = parse_schema(&schema_of(rule), &interner).expect("absent array_of parses");
    assert!(rules[0].constraints.array_of.is_none());
}

#[test]
fn valid_array_of_parses() {
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "tags", "list"),
        "array_of",
        QueryValue::Str("string".into()),
    );
    let rules = parse_schema(&schema_of(rule), &interner).expect("valid array_of parses");
    assert!(rules[0].constraints.array_of.is_some());
}

// ── format ───────────────────────────────────────────────────────────────

#[test]
fn garbled_format_rejected() {
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "email", "string"),
        "format",
        QueryValue::Str("not_a_format".into()),
    );
    assert_rejects(&schema_of(rule), &interner, "format");
}

#[test]
fn absent_format_defaults_to_none() {
    let interner = Interner::new();
    let rule = base_rule(&interner, "email", "string"); // no format key
    let rules = parse_schema(&schema_of(rule), &interner).expect("absent format parses");
    assert!(rules[0].constraints.format.is_none());
}

#[test]
fn valid_format_parses() {
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "email", "string"),
        "format",
        QueryValue::Str("email".into()),
    );
    let rules = parse_schema(&schema_of(rule), &interner).expect("valid format parses");
    assert!(rules[0].constraints.format.is_some());
}

// ── compare ──────────────────────────────────────────────────────────────

#[test]
fn garbled_compare_op_rejected() {
    let interner = Interner::new();
    let cmp = {
        let mut m = new_map();
        m.insert(
            "other".to_string(),
            QueryValue::List(vec![QueryValue::Str("end".into())]),
        );
        m.insert("op".to_string(), QueryValue::Str("bogus_op".into()));
        QueryValue::Map(m)
    };
    let rule = with_field(base_rule(&interner, "start", "int"), "compare", cmp);
    assert_rejects(&schema_of(rule), &interner, "compare");
}

#[test]
fn compare_not_a_map_rejected() {
    let interner = Interner::new();
    // `compare` present but not a Map → malformed.
    let rule = with_field(
        base_rule(&interner, "start", "int"),
        "compare",
        QueryValue::Str("garbage".into()),
    );
    assert_rejects(&schema_of(rule), &interner, "compare");
}

#[test]
fn absent_compare_defaults_to_none() {
    let interner = Interner::new();
    let rule = base_rule(&interner, "start", "int"); // no compare key
    let rules = parse_schema(&schema_of(rule), &interner).expect("absent compare parses");
    assert!(rules[0].constraints.compare.is_none());
}

#[test]
fn valid_compare_parses() {
    let interner = Interner::new();
    let cmp = {
        let mut m = new_map();
        m.insert(
            "other".to_string(),
            QueryValue::List(vec![QueryValue::Str("end".into())]),
        );
        m.insert("op".to_string(), QueryValue::Str("<=".into()));
        QueryValue::Map(m)
    };
    let rule = with_field(base_rule(&interner, "start", "int"), "compare", cmp);
    let rules = parse_schema(&schema_of(rule), &interner).expect("valid compare parses");
    assert!(rules[0].constraints.compare.is_some());
}

// ── foreign_key (outer shape) ────────────────────────────────────────────

#[test]
fn garbled_foreign_key_not_a_map_rejected() {
    let interner = Interner::new();
    // foreign_key present but not a Map → malformed.
    let rule = with_field(
        base_rule(&interner, "parent_id", "int"),
        "foreign_key",
        QueryValue::Str("garbage".into()),
    );
    assert_rejects(&schema_of(rule), &interner, "foreign_key");
}

#[test]
fn foreign_key_missing_ref_field_rejected() {
    let interner = Interner::new();
    let fk = {
        let mut m = new_map();
        m.insert("ref_table".to_string(), QueryValue::Str("parent".into()));
        // ref_field missing
        QueryValue::Map(m)
    };
    let rule = with_field(base_rule(&interner, "parent_id", "int"), "foreign_key", fk);
    assert_rejects(&schema_of(rule), &interner, "foreign_key");
}

#[test]
fn absent_foreign_key_defaults_to_none() {
    let interner = Interner::new();
    let rule = base_rule(&interner, "parent_id", "int"); // no foreign_key key
    let rules = parse_schema(&schema_of(rule), &interner).expect("absent fk parses");
    assert!(rules[0].constraints.foreign_key.is_none());
}

#[test]
fn valid_foreign_key_parses() {
    let interner = Interner::new();
    let fk = {
        let mut m = new_map();
        m.insert("ref_table".to_string(), QueryValue::Str("parent".into()));
        m.insert("ref_field".to_string(), QueryValue::Str("id".into()));
        QueryValue::Map(m)
    };
    let rule = with_field(base_rule(&interner, "parent_id", "int"), "foreign_key", fk);
    let rules = parse_schema(&schema_of(rule), &interner).expect("valid fk parses");
    let fk = rules[0]
        .constraints
        .foreign_key
        .as_ref()
        .expect("fk present");
    assert_eq!(fk.ref_table, "parent");
    assert_eq!(fk.ref_field, "id");
}

// ── on_delete / on_update (inside foreign_key) ───────────────────────────

fn fk_map_with(action_field: &str, action_val: &str) -> QueryValue {
    let mut m = new_map();
    m.insert("ref_table".to_string(), QueryValue::Str("parent".into()));
    m.insert("ref_field".to_string(), QueryValue::Str("id".into()));
    m.insert(
        action_field.to_string(),
        QueryValue::Str(action_val.to_string()),
    );
    QueryValue::Map(m)
}

#[test]
fn garbled_on_delete_rejected() {
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "parent_id", "int"),
        "foreign_key",
        fk_map_with("on_delete", "garbled"),
    );
    assert_rejects(&schema_of(rule), &interner, "on_delete");
}

#[test]
fn garbled_on_update_rejected() {
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "parent_id", "int"),
        "foreign_key",
        fk_map_with("on_update", "garbled"),
    );
    assert_rejects(&schema_of(rule), &interner, "on_update");
}

#[test]
fn absent_on_delete_on_update_default_to_no_action() {
    // foreign_key present but with NEITHER on_delete NOR on_update → both
    // default to NoAction (documented backward-compat default for legacy rows;
    // the F-4 fix only tightens what counts as "absent", not the default).
    let interner = Interner::new();
    let fk = {
        let mut m = new_map();
        m.insert("ref_table".to_string(), QueryValue::Str("parent".into()));
        m.insert("ref_field".to_string(), QueryValue::Str("id".into()));
        QueryValue::Map(m)
    };
    let rule = with_field(base_rule(&interner, "parent_id", "int"), "foreign_key", fk);
    let rules = parse_schema(&schema_of(rule), &interner).expect("absent actions parse");
    let fk = rules[0]
        .constraints
        .foreign_key
        .as_ref()
        .expect("fk present");
    assert_eq!(fk.on_delete, FkAction::NoAction);
    assert_eq!(fk.on_update, FkAction::NoAction);
}

#[test]
fn explicit_no_action_string_parses() {
    // The recognised "no_action" string must round-trip to NoAction (it is a
    // PRESENT, recognised value — not the absent-default path).
    let interner = Interner::new();
    let rule = with_field(
        base_rule(&interner, "parent_id", "int"),
        "foreign_key",
        fk_map_with("on_delete", "no_action"),
    );
    let rules = parse_schema(&schema_of(rule), &interner).expect("no_action parses");
    let fk = rules[0]
        .constraints
        .foreign_key
        .as_ref()
        .expect("fk present");
    assert_eq!(fk.on_delete, FkAction::NoAction);
}
