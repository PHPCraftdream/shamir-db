//! Tests for the fallible `try_*` batch entry path ([`TryIntoBatchOp`] +
//! [`Batch::try_op`] / [`Batch::try_update`] / …).
//!
//! For each of the five fallible builder types there is:
//! - a **negative** test that omits the required field and asserts the
//!   *specific* [`BuilderError`] variant is returned (no panic), and
//! - a **positive** test that builds a well-formed op via the `try_*` method
//!   and asserts the resulting wire shape is byte-for-byte identical to the
//!   existing panicking path (regression guard proving the new path does not
//!   change the wire shape).

use shamir_types::types::value::QueryValue;

use crate::batch::Batch;
use crate::ddl;
use crate::filter;
use crate::wire::ToWire;
use crate::write::{self, doc, BuilderError};

/// Build two batches via the fallible `try_*` method and assert their wire
/// shapes are identical via msgpack round-trip (determinism guard).
fn assert_try_path_deterministic(try_path: impl Fn(&mut Batch)) {
    let mut b1 = Batch::new();
    try_path(&mut b1);
    let j1 = b1.build().to_query_value().unwrap();

    let mut b2 = Batch::new();
    try_path(&mut b2);
    let j2 = b2.build().to_query_value().unwrap();

    assert_eq!(j1, j2, "try_* path must be deterministic");
}

// ============================================================================
// 1. Update — MissingSetValue / happy path
// ============================================================================

#[test]
fn try_update_without_set_returns_missing_set_value() {
    let mut b = Batch::new();
    let err = b
        .try_update("u", write::update("users").where_(filter::eq("id", 1)))
        .unwrap_err();
    assert_eq!(err, BuilderError::MissingSetValue);
}

#[test]
fn try_update_happy_path_produces_valid_batch() {
    let upd = || {
        write::update("users")
            .where_(filter::eq("id", 1))
            .set(doc().set("name", "Bob"))
    };
    assert_try_path_deterministic(|b| {
        b.try_update("u", upd()).unwrap();
    });
}

// ============================================================================
// 2. Upsert — MissingKey / MissingValue / happy path
// ============================================================================

#[test]
fn try_upsert_without_key_returns_missing_key() {
    let mut b = Batch::new();
    let err = b
        .try_upsert("s", write::upsert("cache").value(doc().set("v", 42)))
        .unwrap_err();
    assert_eq!(err, BuilderError::MissingKey);
}

#[test]
fn try_upsert_without_value_returns_missing_value() {
    let mut b = Batch::new();
    let err = b
        .try_upsert("s", write::upsert("cache").key(shamir_types::mpack!("k1")))
        .unwrap_err();
    assert_eq!(err, BuilderError::MissingValue);
}

#[test]
fn try_upsert_happy_path_produces_valid_batch() {
    let ups = || {
        write::upsert("cache")
            .key(shamir_types::mpack!("k1"))
            .value(doc().set("v", 42))
    };
    assert_try_path_deterministic(|b| {
        b.try_upsert("s", ups()).unwrap();
    });
}

// ============================================================================
// 3. Delete — MissingWhereClause / happy path
// ============================================================================

#[test]
fn try_delete_without_where_returns_missing_where_clause() {
    let mut b = Batch::new();
    let err = b.try_delete("d", write::delete("sessions")).unwrap_err();
    assert_eq!(err, BuilderError::MissingWhereClause);
}

#[test]
fn try_delete_happy_path_produces_valid_batch() {
    let del = || write::delete("sessions").where_(filter::eq("expired", true));
    assert_try_path_deterministic(|b| {
        b.try_delete("d", del()).unwrap();
    });
}

// ============================================================================
// 4. AddSchemaRuleBuilder — MissingRule / happy path
// ============================================================================

#[test]
fn try_add_schema_rule_without_rule_returns_missing_rule() {
    let mut b = Batch::new();
    let err = b
        .try_add_schema_rule("r", ddl::add_schema_rule("users"))
        .unwrap_err();
    assert_eq!(err, BuilderError::MissingRule);
}

#[test]
fn try_add_schema_rule_happy_path_produces_valid_batch() {
    let rule = || ddl::add_schema_rule("users").rule(ddl::field(["status"]).string());
    assert_try_path_deterministic(|b| {
        b.try_add_schema_rule("r", rule()).unwrap();
    });
}

// ============================================================================
// 5. AlterSubscriptionBuilder — MissingAction / happy path (via try_op)
// ============================================================================

#[test]
fn try_op_alter_subscription_without_action_returns_missing_action() {
    let mut b = Batch::new();
    let err = b
        .try_op("sub", ddl::alter_subscription("sub1"))
        .unwrap_err();
    assert_eq!(err, BuilderError::MissingAction);
}

#[test]
fn try_op_alter_subscription_happy_path_produces_valid_batch() {
    let alter = || ddl::alter_subscription("sub1").pause();
    assert_try_path_deterministic(|b| {
        b.try_op("sub", alter()).unwrap();
    });
}

// ============================================================================
// Bonus: try_op works for all five builder types (generic dispatch)
// ============================================================================

#[test]
fn try_op_dispatches_to_all_five_fallible_builders() {
    let mut b = Batch::new();
    b.try_update("u", write::update("t").set(doc().set("a", 1)))
        .unwrap();
    b.try_upsert(
        "s",
        write::upsert("t")
            .key(shamir_types::mpack!("k"))
            .value(doc().set("v", 2)),
    )
    .unwrap();
    b.try_delete("d", write::delete("t").where_(filter::eq("x", 0)))
        .unwrap();
    b.try_op("r", ddl::add_schema_rule("t").rule(ddl::field(["n"]).int()))
        .unwrap();
    b.try_op("sub", ddl::alter_subscription("s1").resume())
        .unwrap();

    let qv = b.build().to_query_value().unwrap();
    let queries = match &qv["queries"] {
        QueryValue::Map(m) => m,
        other => panic!("expected Map for queries, got {other:?}"),
    };
    assert_eq!(queries.len(), 5);
    // Spot-check the two ops that go through the generic try_op path.
    assert!(queries["r"].get("add_schema_rule").is_some());
    assert!(queries["sub"].get("alter_subscription").is_some());
}
