//! Tests for the `QueryRunner` struct (tx: None path).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::batch::{EdgeKind, ResultEncoding};
use shamir_types::access::Actor;
use shamir_types::codecs::interned::record_view_to_query_value;
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map;

use crate::query::batch::query_runner::build_resolved_refs;
use crate::query::batch::QueryRunner;
use crate::query::read::{QueryRecord, QueryResult};

use super::common::setup_resolver;

// ============================================================================
// QueryRunner struct — tx: None path
// ============================================================================

#[tokio::test]
async fn test_query_runner_none_tx_insert_and_read() {
    let resolver = setup_resolver().await;

    // Insert via QueryRunner with tx: None
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "Eve").set("age", 28)),
    );
    let insert_req = b.build();
    let insert_entry = insert_req.queries.get("ins").unwrap().clone();
    let empty_params = shamir_types::types::common::new_map();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &empty_params,
        result_encoding: ResultEncoding::Name,
    };
    let result = runner
        .run(
            "ins",
            &insert_entry,
            &shamir_types::types::common::new_map(),
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);

    // Read via QueryRunner with tx: None
    let mut b = Batch::new();
    b.id(2);
    b.query("q", Query::from("users"));
    let read_req = b.build();
    let read_entry = read_req.queries.get("q").unwrap().clone();
    let empty_params2 = shamir_types::types::common::new_map();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &empty_params2,
        result_encoding: ResultEncoding::Name,
    };
    let result = runner
        .run("q", &read_entry, &shamir_types::types::common::new_map())
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
}

// ============================================================================
// ResultEncoding::Id — INSERT…RETURNING produces IdBytes
// ============================================================================

/// INSERT…RETURNING with `ResultEncoding::Id`:
///
/// 1. Insert a record with `ResultEncoding::Id`.
/// 2. Assert the RETURNING row is `QueryRecord::IdBytes`, not `Inserted`.
/// 3. Assert de-interning the IdBytes yields the same `QueryValue` as a
///    name-keyed insert (parity assertion).
#[tokio::test]
async fn insert_returning_id_encoding_yields_id_bytes() {
    let resolver = setup_resolver().await;

    // --- Id path: INSERT with ResultEncoding::Id ---
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_id",
        write::insert("users").row(
            doc()
                .set("name", "Alice")
                .set("age", 30_i64)
                .set("city", "NYC"),
        ),
    );
    let insert_req = b.build();
    let insert_entry = insert_req.queries.get("ins_id").unwrap().clone();
    let empty_params = shamir_types::types::common::new_map();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &empty_params,
        result_encoding: ResultEncoding::Id,
    };
    let result_id = runner
        .run(
            "ins_id",
            &insert_entry,
            &shamir_types::types::common::new_map(),
        )
        .await
        .unwrap();

    // Must have exactly one RETURNING record.
    assert_eq!(result_id.records.len(), 1, "expected 1 RETURNING record");

    // The record must be IdBytes when ResultEncoding::Id is negotiated.
    let id_bytes = match &result_id.records[0] {
        QueryRecord::IdBytes(b) => b.clone(),
        other => {
            panic!("INSERT RETURNING with ResultEncoding::Id must yield IdBytes, got {other:?}")
        }
    };

    // IdBytes must be valid msgpack that de-interns to the expected fields.
    let table = resolver.db.get_table("default", "users").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let view = RecordView::new(&id_bytes).expect("IdBytes must be valid id-keyed msgpack");
    let id_qv =
        record_view_to_query_value(&view, interner).expect("de-interning IdBytes must succeed");

    assert_eq!(
        id_qv.get("name").and_then(|v| v.as_str()),
        Some("Alice"),
        "de-interned IdBytes must contain name=Alice"
    );
    assert_eq!(
        id_qv.get("age").and_then(|v| v.as_i64()),
        Some(30),
        "de-interned IdBytes must contain age=30"
    );
    assert_eq!(
        id_qv.get("city").and_then(|v| v.as_str()),
        Some("NYC"),
        "de-interned IdBytes must contain city=NYC"
    );
}

/// Parity assertion: INSERT…RETURNING with `ResultEncoding::Name` and with
/// `ResultEncoding::Id` must yield the same logical record after decoding.
///
/// 1. Insert the same record twice via two separate resolvers.
/// 2. Name path → `Inserted` or `Direct` with name-keyed fields.
/// 3. Id path   → `IdBytes`; de-intern → same `QueryValue`.
#[tokio::test]
async fn insert_returning_id_vs_name_encoding_parity() {
    // --- Name path resolver ---
    let resolver_name = setup_resolver().await;
    let mut b = Batch::new();
    b.id(10);
    b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "Bob").set("score", 99_i64)),
    );
    let req = b.build();
    let entry = req.queries.get("ins").unwrap().clone();
    let params = shamir_types::types::common::new_map();
    let mut runner_name = QueryRunner {
        resolver: &resolver_name,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &params,
        result_encoding: ResultEncoding::Name,
    };
    let result_name = runner_name
        .run("ins", &entry, &shamir_types::types::common::new_map())
        .await
        .unwrap();

    assert_eq!(result_name.records.len(), 1);
    // Extract name-keyed fields from the Name-path record.
    let name_qv = result_name.records[0].as_value().into_owned();
    assert_eq!(
        name_qv.get("name").and_then(|v| v.as_str()),
        Some("Bob"),
        "Name path must expose name=Bob"
    );

    // --- Id path resolver (fresh resolver so interner is independent) ---
    let resolver_id = setup_resolver().await;
    let mut b2 = Batch::new();
    b2.id(11);
    b2.insert(
        "ins",
        write::insert("users").row(doc().set("name", "Bob").set("score", 99_i64)),
    );
    let req2 = b2.build();
    let entry2 = req2.queries.get("ins").unwrap().clone();
    let params2 = shamir_types::types::common::new_map();
    let mut runner_id = QueryRunner {
        resolver: &resolver_id,
        admin: None,
        invoker: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &params2,
        result_encoding: ResultEncoding::Id,
    };
    let result_id = runner_id
        .run("ins", &entry2, &shamir_types::types::common::new_map())
        .await
        .unwrap();

    assert_eq!(result_id.records.len(), 1);
    let id_bytes = match &result_id.records[0] {
        QueryRecord::IdBytes(b) => b.clone(),
        other => panic!("Id path must yield IdBytes, got {other:?}"),
    };

    // De-intern via the resolver_id's interner.
    let table_id = resolver_id.db.get_table("default", "users").await.unwrap();
    let interner_id = table_id.interner().get().await.unwrap();
    let view = RecordView::new(&id_bytes).expect("IdBytes must be valid msgpack");
    let id_qv =
        record_view_to_query_value(&view, interner_id).expect("de-interning IdBytes must succeed");

    // Parity: both paths must yield the same logical field values.
    assert_eq!(
        id_qv.get("name").and_then(|v| v.as_str()),
        name_qv.get("name").and_then(|v| v.as_str()),
        "name field must match between Id and Name paths"
    );
    assert_eq!(
        id_qv.get("score").and_then(|v| v.as_i64()),
        name_qv.get("score").and_then(|v| v.as_i64()),
        "score field must match between Id and Name paths"
    );
}

// ============================================================================
// build_resolved_refs — edge provenance (task #628 — Epic01/A)
// ============================================================================

/// An `Explicit`-only edge (pure `after`, no `$query`) must NOT contribute
/// its alias's result to `resolved_refs` — `after` is ordering only, never
/// a data-access grant.
#[test]
fn build_resolved_refs_excludes_explicit_only_edge() {
    let mut all_results: shamir_types::types::common::TMap<String, QueryResult> = new_map();
    all_results.insert(
        "a".to_string(),
        QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
        },
    );

    let mut provenance = new_map();
    provenance.insert("a".to_string(), EdgeKind::Explicit);

    let refs = build_resolved_refs(&all_results, Some(&provenance));
    assert!(
        refs.is_empty(),
        "Explicit-only edge must not leak its alias's result into resolved_refs"
    );
}

/// A `DataFlow` edge (real `$query` ref) DOES contribute its alias's result.
#[test]
fn build_resolved_refs_includes_dataflow_edge() {
    let mut all_results: shamir_types::types::common::TMap<String, QueryResult> = new_map();
    all_results.insert(
        "a".to_string(),
        QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
        },
    );

    let mut provenance = new_map();
    provenance.insert("a".to_string(), EdgeKind::DataFlow);

    let refs = build_resolved_refs(&all_results, Some(&provenance));
    assert!(
        refs.contains_key("a"),
        "DataFlow edge must resolve its alias's result"
    );
}

/// A `Both` edge (after + $query on the same alias) still resolves — the
/// `after` half is redundant ordering, but the real `$query` half still
/// grants data access.
#[test]
fn build_resolved_refs_includes_both_edge() {
    let mut all_results: shamir_types::types::common::TMap<String, QueryResult> = new_map();
    all_results.insert(
        "a".to_string(),
        QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
        },
    );

    let mut provenance = new_map();
    provenance.insert("a".to_string(), EdgeKind::Both);

    let refs = build_resolved_refs(&all_results, Some(&provenance));
    assert!(
        refs.contains_key("a"),
        "Both edge must still resolve its alias's result (real DataFlow half)"
    );
}
