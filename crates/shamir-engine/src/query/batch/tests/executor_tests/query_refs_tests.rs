//! Tests for $query reference dependencies between batch ops.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::batch::ResultEncoding;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch;

use super::common::setup_resolver;

// ============================================================================
// Dependent queries: $query ref
// ============================================================================

#[tokio::test]
async fn test_dependent_query_ref() {
    let resolver = setup_resolver().await;

    // Seed users
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "active")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Query 1: get active users
    // Query 2: get users where name == first active user's name (via $query ref)
    let mut b = Batch::new();
    b.id(1);
    let active = b.query("active", Query::from("users").where_eq("status", "active"));
    b.query(
        "first_active",
        Query::from("users").where_eq("name", active.first().field("name")),
    );
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Two stages: [active], [first_active]
    assert_eq!(resp.execution_plan.len(), 2);
    assert_eq!(resp.results["active"].records.len(), 2); // Alice + Carol
    assert_eq!(resp.results["first_active"].records.len(), 1); // Alice
}

// ============================================================================
// Finding 1.4 — $query ref resolution under ResultEncoding::Id
// ============================================================================
//
// This settles the Rust-vs-TS divergence in `execute_with_touch`:
//
//   - Rust ALWAYS set `result_encoding = Id` on v2.
//   - TS SKIPPED id-encoding whenever a batch contained $query/$param refs or
//     a sub-batch (with a comment: those "rely on server-side intermediate
//     results staying name-keyed").
//
// The server encodes a read result's records into `QueryRecord::IdBytes`
// (opaque id-keyed msgpack) when `result_encoding == Id`. `IdBytes.as_value()`
// returns `QueryValue::Null` and its field accessors return `None` — so a
// downstream query that resolves `$query @dep[i].field` against an IdBytes
// intermediate gets Null, silently breaking path resolution.
//
// Conclusion (settled by this test): the ENGINE does NOT resolve $query refs
// against IdBytes intermediates. Under `ResultEncoding::Id` the `active` read
// is encoded to `QueryRecord::IdBytes` (opaque, `as_value() == Null`), so
// `first_active`'s ref resolves to Null and the dependent query returns 0 rows.
//
// Therefore RUST's client had the latent bug (it ALWAYS set Id on v2) and TS
// was correct to skip Id-encoding when a batch carries $query/$param/sub-batch
// references. The client-side fix (Rust `execute_with_touch` now matches TS)
// keeps such batches name-keyed. This test PINS the engine constraint that
// makes that client rule necessary — if the engine is ever taught to resolve
// refs through IdBytes, flip this assertion.

/// Characterises the Finding-1.4 engine constraint: a $query-ref batch run
/// with `ResultEncoding::Id` does NOT resolve its ref (the IdBytes intermediate
/// is opaque), so the dependent query yields 0 rows. This is why both clients
/// must keep ref-bearing batches name-keyed. The companion positive case is
/// `test_dependent_query_ref`, which resolves correctly under the default
/// (Name) encoding.
#[tokio::test]
async fn query_ref_does_not_resolve_under_id_encoding() {
    let resolver = setup_resolver().await;

    // Seed users.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "active")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Same $query-ref batch as `test_dependent_query_ref`, but negotiated with
    // ResultEncoding::Id (the smart-write path the clients use on v2 servers).
    let mut b = Batch::new();
    b.id(1);
    b.result_encoding(ResultEncoding::Id);
    let active = b.query("active", Query::from("users").where_eq("status", "active"));
    b.query(
        "first_active",
        Query::from("users").where_eq("name", active.first().field("name")),
    );
    let req = b.build();

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // The IdBytes intermediate is opaque → the ref resolves to Null → 0 rows.
    // This is the constraint that forces clients to keep ref-bearing batches
    // name-keyed (see Rust `Client::execute_with_touch` / TS `batchHasRefs`).
    assert_eq!(
        resp.results["first_active"].records.len(),
        0,
        "under ResultEncoding::Id the $query ref cannot resolve (IdBytes is opaque) — \
         clients MUST keep ref-bearing batches name-keyed (Finding 1.4)"
    );
}
