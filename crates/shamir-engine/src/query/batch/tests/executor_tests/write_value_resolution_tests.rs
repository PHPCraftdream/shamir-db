//! #641 — write-value marker resolution: a `$query`/`$fn`/`$cond`/`$expr`
//! marker embedded inside an INSERT/UPDATE/SET(upsert) value must resolve
//! to the value it points to at EXECUTION time, not be written to the table
//! as the literal marker map. Also confirms the pre-existing `$param`-only
//! substitution behavior is unchanged (regression guard), and that a
//! malformed reserved-key marker is a hard error rather than a silent
//! literal pass-through.
//!
//! See `docs/dev-artifacts/prompts/gap-641/01-write-value-resolution.md`.

use shamir_collections::new_map;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::query::Query;
use shamir_query_builder::val::{cond, func, param};
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::filter::FilterValue;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::query::batch::execute_batch;

use super::common::setup_resolver;

// ============================================================================
// $query ref inside an INSERT value resolves at execution time
// ============================================================================

#[tokio::test]
async fn insert_value_with_query_ref_resolves_to_real_value() {
    let resolver = setup_resolver().await;

    // Seed `users` in a PRIOR batch — a read and a same-batch insert with no
    // dependency edge between them plan into the SAME stage (unordered
    // relative to each other), so seeding here (not in the batch below)
    // guarantees `users`'s read actually observes the row.
    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed_users",
        write::insert("users").row(doc().set("name", "Alice")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let users = b.query("users", Query::from("users"));
    // `make_order` embeds a $query ref to `users`' first row's `name` as
    // the `owner` field. Before the #641 fix this literal marker map would
    // be written verbatim as `owner`'s value; after the fix it must
    // resolve to the real string "Alice".
    let make_order = b.op_silent(
        "make_order",
        write::insert("orders").row(
            doc()
                .set("total", 100)
                .set("owner", users.first().field("name")),
        ),
    );
    // Read the order back to inspect what was actually stored. `check` has
    // no `$query`/data-flow edge onto `make_order` (a plain unfiltered
    // read), so without an explicit `after` it plans into the SAME stage as
    // `users`/`make_order` — running BEFORE the insert. `after` orders it
    // to run strictly once `make_order` has completed.
    let check = b.query("check", Query::from("orders"));
    b.after(&check, &make_order);

    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    let owner = rows[0].as_value().get("owner").cloned();
    assert_eq!(
        owner,
        Some(QueryValue::Str("Alice".to_string())),
        "the $query ref inside the INSERT value must resolve to the REAL \
         value it points to, not be stored as the literal marker map"
    );
}

/// Same, but for UPDATE — the `set` document (not merely `where`) carries the
/// `$query` marker.
#[tokio::test]
async fn update_set_value_with_query_ref_resolves_to_real_value() {
    let resolver = setup_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Bob").set("tag", "source"))
            .row(doc().set("name", "").set("tag", "target")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let source = b.query("source", Query::from("users").where_eq("tag", "source"));
    let copy_name = b.op_silent(
        "copy_name",
        write::update("users")
            .where_(filter::eq("tag", "target"))
            .set(doc().set("name", source.first().field("name"))),
    );
    // `check` has no `$query`/data-flow edge onto `copy_name` (it just reads
    // the same table by an unrelated filter), so without an explicit `after`
    // it plans into the SAME stage as `source` — running BEFORE `copy_name`
    // and observing pre-update data. `after` orders it to run strictly once
    // `copy_name` has completed.
    let check = b.query("check", Query::from("users").where_eq("tag", "target"));
    b.after(&check, &copy_name);
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("name").cloned(),
        Some(QueryValue::Str("Bob".to_string())),
        "UPDATE's `set` document must resolve an embedded $query ref to the \
         real value, not store the literal marker map"
    );
}

/// Same, but for SET (upsert) — both `key` and `value` accept markers; here
/// `value` carries the `$query` marker.
#[tokio::test]
async fn upsert_value_with_query_ref_resolves_to_real_value() {
    let resolver = setup_resolver().await;

    // Seed `users` in a PRIOR batch (same rationale as
    // `insert_value_with_query_ref_resolves_to_real_value`: a read and a
    // same-batch insert with no dependency edge between them plan into the
    // SAME stage, unordered relative to each other).
    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed_users",
        write::insert("users").row(doc().set("name", "Carol")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let users = b.query("users", Query::from("users"));
    let upsert_profile = b.op_silent(
        "upsert_profile",
        write::upsert("orders")
            .key(mpack!({"total": 200}))
            .value(doc().set("owner", users.first().field("name"))),
    );
    // See the `after` rationale in `insert_value_with_query_ref_resolves_to_real_value`.
    let check = b.query("check", Query::from("orders"));
    b.after(&check, &upsert_profile);

    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("owner").cloned(),
        Some(QueryValue::Str("Carol".to_string())),
        "SET(upsert)'s `value` must resolve an embedded $query ref to the \
         real value, not store the literal marker map"
    );
}

// ============================================================================
// $fn inside a write value resolves at execution time
// ============================================================================

/// `$fn` with a literal argument (no `$ref`, since same-document field refs
/// are out of scope for write-value resolution) resolves to the real
/// computed value.
#[tokio::test]
async fn insert_value_with_fn_call_literal_arg_resolves() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "make_user",
        write::insert("users").row(doc().set("name", "Eve").set(
            "name_norm",
            func("strings/lower", [FilterValue::from("EVE")]),
        )),
    );
    b.query("check", Query::from("users"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("name_norm").cloned(),
        Some(QueryValue::Str("eve".to_string())),
        "the $fn marker inside the INSERT value must resolve to the REAL \
         computed value, not be stored as the literal marker map"
    );
}

// ============================================================================
// $cond inside a write value resolves at execution time
// ============================================================================

#[tokio::test]
async fn insert_value_with_cond_resolves_true_branch() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    // `$cond`'s `if` uses `Filter::ValueCompare` (value-vs-value, no record
    // needed) — the same "no per-row record" constraint write-value
    // resolution has (mirrors `when`'s documented exclusion).
    let band = cond(filter::value_gte(100_i64, 50_i64), "high", "low");
    b.op_silent(
        "make_row",
        write::insert("orders").row(doc().set("total", 100).set("band", band)),
    );
    b.query("check", Query::from("orders"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("band").cloned(),
        Some(QueryValue::Str("high".to_string())),
        "the $cond marker inside the INSERT value must resolve its TRUE \
         branch to the real value, not be stored as the literal marker map"
    );
}

#[tokio::test]
async fn insert_value_with_cond_resolves_false_branch() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    let band = cond(filter::value_gte(10_i64, 50_i64), "high", "low");
    b.op_silent(
        "make_row",
        write::insert("orders").row(doc().set("total", 10).set("band", band)),
    );
    b.query("check", Query::from("orders"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("band").cloned(),
        Some(QueryValue::Str("low".to_string())),
    );
}

// ============================================================================
// Malformed marker → hard error, not silent literal pass-through
// ============================================================================

/// A `$query` marker referencing an alias that doesn't exist anywhere in the
/// batch's dependency graph now fails at PLAN time (`UnknownAlias`), before
/// the write executes at all — the observable proof that #641 also fixed
/// `extract_deps_from_value` to actually scan write values for `$query`
/// refs (dependency extraction), not just execution-time resolution.
#[tokio::test]
async fn insert_value_with_query_ref_to_unknown_alias_fails_at_plan_time() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "make_row",
        write::insert("orders").row(doc().set(
            "owner",
            shamir_query_builder::val::qref("does_not_exist", "[0].name"),
        )),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a $query ref to an unknown alias must fail, not silently insert garbage");
    let msg = err.to_string();
    assert!(
        msg.contains("does_not_exist") || msg.contains("Unknown alias"),
        "unexpected error message: {msg}"
    );
}

/// A malformed marker payload (a `$fn` value that is not a valid `FnCall`
/// shape — e.g. a bare integer instead of a string/complex-object) must
/// produce a clear, coded execution-time error rather than being written to
/// the table as the literal marker map.
#[tokio::test]
async fn insert_value_with_malformed_fn_marker_errors_clearly() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    // `{"$fn": 123}` — not a valid FnCall shape (FnCall is either a bare
    // string or {name, args}), so the msgpack round-trip into FilterValue
    // fails.
    let malformed = QueryValue::Map({
        let mut m = shamir_types::types::common::new_map();
        m.insert("$fn".to_string(), QueryValue::Int(123));
        m
    });
    b.op_silent(
        "make_row",
        write::insert("orders").row(doc().set_value("weird", malformed)),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("a malformed $fn marker must error, not silently write the literal map");
    let msg = err.to_string();
    assert!(
        msg.contains("malformed_marker") || msg.contains("marker"),
        "expected a clear malformed-marker error, got: {msg}"
    );
}

// ============================================================================
// Regression: existing $param-only behavior is unchanged
// ============================================================================

#[tokio::test]
async fn insert_value_with_param_still_resolves_via_sub_batch_bind() {
    let resolver = setup_resolver().await;

    let mut inner = Batch::new();
    inner.id(1);
    inner.op_silent(
        "insert_row",
        write::insert("users").row(doc().set("name", param("who"))),
    );
    let inner_req = inner.build();

    let mut bind = new_map();
    bind.insert("who".to_string(), FilterValue::from("Frank"));

    let mut outer = Batch::new();
    outer.id(2);
    outer.sub_batch("sub", inner_req, bind);
    outer.query("check", Query::from("users"));
    let req = outer.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("name").cloned(),
        Some(QueryValue::Str("Frank".to_string())),
        "$param resolution inside a write value (pre-existing behavior) \
         must be unchanged after generalizing the resolver"
    );
}

/// A `$param` name absent from the sub-batch's `bind` map still errors with
/// `unbound_param` (pre-existing behavior, unchanged).
#[tokio::test]
async fn insert_value_with_unbound_param_still_errors() {
    let resolver = setup_resolver().await;

    let mut inner = Batch::new();
    inner.id(1);
    inner.op_silent(
        "insert_row",
        write::insert("users").row(doc().set("name", param("missing"))),
    );
    let inner_req = inner.build();

    let mut outer = Batch::new();
    outer.id(2);
    outer.sub_batch("sub", inner_req, new_map());
    let req = outer.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("an unbound $param must still error (pre-existing behavior)");
    let msg = err.to_string();
    assert!(
        msg.contains("unbound_param") || msg.contains("missing"),
        "unexpected error message: {msg}"
    );
}

/// Plain literal writes (the overwhelming common case, no markers at all)
/// are completely unaffected by the generalized resolver — this pins the
/// fast-path behavior.
#[tokio::test]
async fn insert_plain_literal_values_unaffected() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "make_row",
        write::insert("users")
            .row(doc().set("name", "Grace").set("age", 41))
            .row(doc().set("name", "Heidi").set("age", 29)),
    );
    b.query("check", Query::from("users"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 2);
}
