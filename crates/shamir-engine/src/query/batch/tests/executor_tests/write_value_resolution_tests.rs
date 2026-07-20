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
use shamir_query_builder::val::{add, col, cond, func, param, qref};
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::filter::FilterValue;
use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::query::batch::execute_batch;
use crate::query::batch::param_subst::{resolve_write_value, WriteValueError};
use crate::query::filter::FilterContext;

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
// Regression (CI-fix #01): a $fn call whose args contain a same-document
// $ref must pass through resolve_write_value COMPLETELY UNCHANGED, so it
// reaches the table layer's resolve_computed_record (write_helpers.rs),
// which resolves $ref against the row's own literal sibling fields. This
// resolver has no real per-row record, so it must not attempt (and fail)
// resolution itself — see param_subst.rs's own "$ref is out of scope" doc.
// ============================================================================

/// `FilterValue` and `QueryValue` share the same serde wire encoding (the
/// same convention `Doc::set` relies on) — round-trip a marker-shaped
/// `FilterValue` (e.g. `func(...)`, `cond(...)`) into the `QueryValue` a
/// write op actually carries on the wire.
fn fv_to_qv(fv: &FilterValue) -> QueryValue {
    let bytes =
        rmp_serde::to_vec_named(fv).expect("FilterValue msgpack serialization is infallible");
    rmp_serde::from_slice(&bytes).expect("FilterValue→QueryValue round-trip is infallible")
}

/// Unit-level: `resolve_write_value` passes a `$fn`+`$ref` marker through
/// byte-for-byte unresolved (same shape in, same shape out) rather than
/// erroring or altering it.
#[test]
fn resolve_write_value_passes_through_fn_call_with_field_ref_unchanged() {
    let interner = Interner::new();
    let resolved_refs = new_map();
    let ctx = FilterContext::new(&interner, &resolved_refs);

    let value = fv_to_qv(&func("strings/lower", [col("email")]));
    let resolved = resolve_write_value(&value, &ctx).expect(
        "a $fn+$ref marker must pass through unresolved, not error — \
         resolving $ref is the table layer's job, not this resolver's",
    );
    assert_eq!(
        resolved, value,
        "the $fn marker's shape must be COMPLETELY UNCHANGED (not resolved, \
         not altered) when its args contain a $ref anywhere"
    );
}

/// Unit-level: a `$fn` call with a `$ref` nested two levels down (inside an
/// `$expr` that is itself an argument of the `$fn`) is also detected and
/// passed through unresolved — the "anywhere, recursively" requirement.
#[test]
fn resolve_write_value_passes_through_fn_call_with_nested_field_ref_unchanged() {
    use shamir_query_builder::val::add;

    let interner = Interner::new();
    let resolved_refs = new_map();
    let ctx = FilterContext::new(&interner, &resolved_refs);

    let value = fv_to_qv(&func("math/abs", [add(col("a"), 1_i64)]));
    let resolved = resolve_write_value(&value, &ctx).expect(
        "a $fn call with a $ref nested inside an $expr argument must still \
         pass through unresolved",
    );
    assert_eq!(
        resolved, value,
        "the outer $fn marker must be left completely unchanged when a $ref \
         is found anywhere in its (possibly nested) args"
    );
}

/// Non-regression: a `$fn` call with NO `$ref` anywhere in its args (the
/// #641 case this resolver already supports) still resolves fully, exactly
/// as before this fix.
#[test]
fn resolve_write_value_still_resolves_fn_call_without_field_ref() {
    let interner = Interner::new();
    let resolved_refs = new_map();
    let ctx = FilterContext::new(&interner, &resolved_refs);

    let value = fv_to_qv(&func("strings/lower", [FilterValue::from("EVE")]));
    let resolved = resolve_write_value(&value, &ctx).unwrap();
    assert_eq!(
        resolved,
        QueryValue::Str("eve".to_string()),
        "a $fn call with no $ref in its args must still resolve to its real \
         computed value (unaffected by the $ref pass-through fix)"
    );
}

/// Integration/end-to-end: a single INSERT row carries BOTH a `$fn`+`$ref`
/// field (must be left for the table layer's `resolve_computed_record`) AND
/// a genuine `$query` field (must be resolved by `resolve_write_value` as
/// #641 already does) — both must end up correct after the full insert
/// pipeline. This is the exact regression shape from `functions_e2e.rs`'s
/// `seed_users`, plus a `$query` sibling to prove the two resolution paths
/// coexist correctly in the same row.
#[tokio::test]
async fn insert_row_with_fn_ref_and_query_ref_siblings_both_resolve_correctly() {
    let resolver = setup_resolver().await;

    let mut seed = Batch::new();
    seed.id(1);
    seed.op_silent(
        "seed_users",
        write::insert("users").row(doc().set("name", "Judy")),
    );
    let seed_req = seed.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    let users = b.query("users", Query::from("users"));
    let make_row = b.op_silent(
        "make_row",
        write::insert("orders").row(
            doc()
                .set("email", "J@X.COM")
                // Table-layer computed value: $fn+$ref against the SAME row's
                // sibling "email" field — must be left untouched here and
                // resolved by write_helpers::resolve_computed_record instead.
                .set("email_norm", func("strings/lower", [col("email")]))
                // Batch-level resolution: a genuine $query ref to another
                // query's result in this same batch — must be resolved HERE
                // by resolve_write_value, exactly as #641 already does.
                .set("owner", users.first().field("name")),
        ),
    );
    let check = b.query("check", Query::from("orders"));
    b.after(&check, &make_row);
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("email_norm").cloned(),
        Some(QueryValue::Str("j@x.com".to_string())),
        "the $fn+$ref field must be resolved by the table layer against the \
         row's own literal 'email' field"
    );
    assert_eq!(
        rows[0].as_value().get("owner").cloned(),
        Some(QueryValue::Str("Judy".to_string())),
        "the sibling $query field must still resolve at the batch level, \
         proving the $ref pass-through fix doesn't break #641's own \
         resolution path when both marker kinds appear in the same row"
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

// ============================================================================
// §2 gap 1: Top-level `$expr` in a write value (ZERO prior tests — the most
// direct mirror of the shipped `$fn`+`$ref` bug per report 08).
// ============================================================================

/// A top-level `$expr` with LITERAL args (no `$ref`) resolves to the computed
/// value. Exercises `param_subst.rs`'s msgpack round-trip into
/// `FilterValue::Expr` (line ~224) and `resolve_filter_query`'s `Expr` arm
/// (`resolve.rs:281` → `eval_filter_expr` → `FilterExprOp::Add`).
#[tokio::test]
async fn insert_value_with_top_level_expr_literal_resolves() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "make_row",
        write::insert("orders").row(doc().set("total", add(2_i64, 3_i64))),
    );
    b.query("check", Query::from("orders"));
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let rows = &resp.results["check"].records;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_value().get("total").cloned(),
        Some(QueryValue::Int(5)),
        "the $expr marker inside the INSERT value must resolve to the REAL \
         computed value (2+3=5), not be stored as the literal marker map"
    );
}

// ============================================================================
// §2 gap 2a: `$expr`+`$ref` is asymmetric — the pass-through check is
// FnCall-only (param_subst.rs:244), so an Expr containing $ref resolves
// against the dummy Null record, the FieldRef misses, and the whole expr
// collapses to None → MalformedMarker. Pin this exact error.
// ============================================================================

/// Unit-level: a top-level `$expr` whose args contain `$ref` (a record-field
/// reference) hard-errors with `WriteValueError::MalformedMarker`. This is the
/// KNOWN, DOCUMENTED asymmetry: the pass-through-to-table-layer check
/// (`param_subst.rs:244-248`) fires only for `FilterValue::FnCall`, so an
/// `Expr` containing `$ref` does NOT pass through — it proceeds to
/// `resolve_filter_query`, resolves the `$ref` against `DUMMY_RECORD` (Null),
/// the field misses (returns `None`), and `eval_filter_expr` short-circuits to
/// `None` → `resolve_write_value` maps that to `MalformedMarker`.
///
/// This is fail-closed (arguably fine — a `$ref` in a write value IS invalid
/// at this resolution stage since there's no real record yet), but no test
/// asserted it before. Pinning it makes a future change deliberate and visible.
#[test]
fn resolve_write_value_top_level_expr_with_field_ref_errors() {
    let interner = Interner::new();
    let resolved_refs = new_map();
    let ctx = FilterContext::new(&interner, &resolved_refs);

    // add(col("a"), col("b")) — both args are $ref, which miss against Null.
    let value = fv_to_qv(&add(col("a"), col("b")));
    let err = resolve_write_value(&value, &ctx).expect_err(
        "a top-level $expr containing $ref must error: the pass-through check \
         is FnCall-only, so the Expr resolves against the dummy Null record \
         where $ref always misses, collapsing the whole expr to None",
    );
    assert!(
        matches!(err, WriteValueError::MalformedMarker(_)),
        "expected MalformedMarker for a $expr+$ref write value, got: {err:?}"
    );
}

// ============================================================================
// §2 gap 2b: `$cond` whose CONDITION references a record field (not a
// ValueCompare) silently picks the else branch — the condition compiles
// against the dummy Null record where the field is absent, so it NEVER
// matches. Pin this current behavior.
// ============================================================================

/// Unit-level: a top-level `$cond` whose `condition` is a field-based
/// comparison (`filter::eq("total", 100)`, NOT `filter::value_eq`/`value_gte`)
/// evaluates its condition against the dummy Null record. The record has no
/// "total" field, so the condition compiles but never matches — the ELSE
/// branch is selected every time, regardless of the comparison value.
///
/// This is a KNOWN, DOCUMENTED asymmetry (param_subst.rs's own doc comment,
/// lines 24-38, explicitly states `$ref` is out of scope and "always misses"
/// against the dummy record; a `$cond`'s field-based condition is the same
/// class of "no real record available" limitation). The existing `$cond`
/// tests (:381, :412) sidestep this by using `filter::value_gte` — a
/// value-vs-value comparison that needs no record. This test pins the
/// field-condition case explicitly so a future change is a visible diff.
///
/// NOT a bug: the resolver deliberately has no per-row record at this stage
/// (the row itself is what's being constructed), mirroring `when`'s
/// `resolve_skip` exclusion. A future task could thread real partial-document
/// context through if field-condition `$cond` support is ever requested.
#[test]
fn resolve_write_value_cond_with_field_condition_always_picks_else_branch() {
    let interner = Interner::new();
    let resolved_refs = new_map();
    let ctx = FilterContext::new(&interner, &resolved_refs);

    // Condition references record field "total" — against DUMMY Null record,
    // "total" is absent, condition never matches, always picks else ("low").
    let value = fv_to_qv(&cond(filter::eq("total", 100_i64), "high", "low"));
    let resolved = resolve_write_value(&value, &ctx).expect(
        "a $cond with a field-based condition resolves without error — it \
         silently picks the else branch because the field is absent on the \
         dummy Null record",
    );
    assert_eq!(
        resolved,
        QueryValue::Str("low".to_string()),
        "the condition `total == 100` evaluated against the Null record (no \
         'total' field) never matches, so the ELSE branch is always selected"
    );

    // Pin the asymmetry: inverting the comparison (total != 100) ALSO picks
    // else, because the field is still absent — the condition's operator is
    // irrelevant when the field doesn't exist on the record.
    let value_inverted = fv_to_qv(&cond(filter::ne("total", 100_i64), "high", "low"));
    let resolved_inverted = resolve_write_value(&value_inverted, &ctx).unwrap();
    assert_eq!(
        resolved_inverted,
        QueryValue::Str("low".to_string()),
        "even a negated field condition picks else — the field is absent on \
         the Null record regardless of operator"
    );
}

// ============================================================================
// §2 gap 3: `SetOp.key` markers untested — only `.value` was exercised
// (write_value_resolution_tests.rs:146). The resolver IS called on the key
// (query_runner.rs:1235), but no test pins it.
// ============================================================================

/// A `$param` marker inside an upsert's `key` field (the row-identity path)
/// IS resolved by `resolve_write_value` — proven via an unbound param: if the
/// key were NOT passed through `resolve_write_value` (query_runner.rs:1235),
/// the literal marker map `{"$param": "missing"}` would be silently stored as
/// the key and the batch would SUCCEED. The fact that it ERRORS with
/// `unbound_param` is the observable proof that the resolver was called on the
/// key and attempted name→value lookup.
///
/// This is the strongest non-vacuous evidence: the test FAILS if anyone
/// removes the `resolve_write_value(&op.key, &ctx)` call site, because without
/// it the unbound param would never be looked up and the batch would succeed.
#[tokio::test]
async fn upsert_key_with_unbound_param_errors_proving_key_is_resolved() {
    let resolver = setup_resolver().await;

    let mut inner = Batch::new();
    inner.id(1);
    inner.op_silent(
        "upsert_row",
        write::upsert("orders")
            .key(doc().set("name", param("missing")))
            .value(doc().set("total", 100)),
    );
    let inner_req = inner.build();

    // Empty bind map → "missing" is unbound.
    let mut outer = Batch::new();
    outer.id(2);
    outer.sub_batch("sub", inner_req, new_map());
    let req = outer.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err(
            "an unbound $param in the upsert KEY must error — this proves \
             resolve_write_value IS called on SetOp.key (query_runner.rs:1235). \
             If it were NOT called, the literal marker map would be silently \
             stored as the key and the batch would succeed.",
        );
    let msg = err.to_string();
    assert!(
        msg.contains("unbound_param") || msg.contains("missing"),
        "expected an unbound_param error from the $param in the upsert key, \
         got: {msg}"
    );
}

// ============================================================================
// §2 gap 4: Nesting combinations untested — the resolver recurses through
// Maps/Lists (param_subst.rs:257-270), but no test exercises a marker inside
// a nested Map, a List element, or a $query ref nested inside a $fn arg.
// ============================================================================

/// Unit-level: a `$param` marker nested two levels deep inside a Map
/// (`{"outer": {"inner": {"$param": "x"}}}`) resolves correctly. Exercises
/// the resolver's Map-recursion path (param_subst.rs:257-262) — the outer
/// and inner maps are NOT markers themselves, so `is_marker_map` returns
/// false at both levels and the resolver recurses into their values until
/// it reaches the `{"$param": "x"}` leaf.
#[test]
fn resolve_write_value_marker_nested_two_levels_deep_in_map() {
    let interner = Interner::new();
    let resolved_refs = new_map();
    let mut params = new_map();
    params.insert("x".to_string(), QueryValue::Int(42));
    let ctx = FilterContext::new(&interner, &resolved_refs).with_params(&params);

    // {"outer": {"inner": {"$param": "x"}}}
    let param_marker = mpack!({"$param": "x"});
    let value = mpack!({"outer": {"inner": @param_marker}});

    let resolved = resolve_write_value(&value, &ctx).unwrap();
    let outer = resolved.get("outer").expect("outer map present");
    let inner = outer.get("inner").expect("inner map present");
    assert_eq!(
        inner,
        &QueryValue::Int(42),
        "the $param marker nested two levels deep must resolve to the bound \
         value, proving the resolver recurses through non-marker maps"
    );
}

/// Unit-level: a `$fn` marker inside a List element
/// (`{"items": [{"$fn": ...}, "literal"]}`) resolves correctly, and the
/// non-marker literal sibling is left unchanged. Exercises the resolver's
/// List-recursion path (param_subst.rs:264-270) — each element is resolved
/// independently; the `$fn` element resolves to its computed value while
/// the plain string passes through via the fast-path `other => Ok(other.clone())`.
#[test]
fn resolve_write_value_marker_inside_list_element() {
    let interner = Interner::new();
    let resolved_refs = new_map();
    let ctx = FilterContext::new(&interner, &resolved_refs);

    // {"items": [{"$fn": strings/lower("EVE")}, "literal"]}
    let fn_marker = fv_to_qv(&func("strings/lower", [FilterValue::from("EVE")]));
    let value = mpack!({"items": [@fn_marker, "literal"]});

    let resolved = resolve_write_value(&value, &ctx).unwrap();
    let items = match resolved.get("items") {
        Some(QueryValue::List(arr)) => arr,
        other => panic!("expected List, got {other:?}"),
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0],
        QueryValue::Str("eve".to_string()),
        "the $fn marker inside the list element must resolve to the computed \
         value"
    );
    assert_eq!(
        items[1],
        QueryValue::Str("literal".to_string()),
        "the non-marker literal sibling must be left unchanged"
    );
}

/// A `$query` ref NESTED inside a `$fn`'s args (referencing an unknown alias)
/// is caught at PLAN time — `collect_query_refs` (batch.rs:1011) recursively
/// walks ALL map values, so the nested `$query` key is found even inside a
/// `$fn` payload. This extends the existing top-level unknown-alias test
/// (:447) to the nested case report 08 flags as untested.
#[tokio::test]
async fn insert_value_with_nested_query_ref_inside_fn_arg_unknown_alias_fails() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    // $query ref nested inside a $fn's arg — collect_query_refs must find it
    // at plan time and reject the unknown alias, same as a top-level $query.
    b.op_silent(
        "make_row",
        write::insert("orders").row(doc().set(
            "owner",
            func("strings/lower", [qref("does_not_exist", "[0].name")]),
        )),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err(
            "a $query ref nested inside a $fn arg must be caught at plan time \
             — collect_query_refs recursively walks all map values",
        );
    let msg = err.to_string();
    assert!(
        msg.contains("does_not_exist") || msg.contains("Unknown alias"),
        "expected an unknown-alias error for the nested $query ref, got: {msg}"
    );
}
