//! S-read server capability tests — id-keyed msgpack read path.
//!
//! Exercises the `ResultEncoding::Id` branch of `read_with_encoding`:
//!
//! 1. **SELECT * convergence (id-keyed ≡ name-keyed)**: reading with
//!    `ResultEncoding::Id` yields `QueryRecord::IdBytes` whose bytes are the
//!    verbatim stored bytes, and de-interning those bytes produces the same
//!    `QueryValue` as the Name-path `Direct` rows for the same record.
//!
//! 2. **Field projection convergence**: `SELECT a, c` with `ResultEncoding::Id`
//!    yields `IdBytes`; de-interning those bytes == the Name-path projected
//!    `{ a, c }` QueryValue.
//!
//! 3. **Aggregate/GROUP BY fallback**: a query with `ResultEncoding::Id` that
//!    has an aggregate (`sum(age)`) falls back to the Name path and returns
//!    `QueryRecord::Direct` rows with the correct result.
//!
//! 4. **Default unchanged**: `ResultEncoding::Name` (or absent) → `Direct`
//!    rows exactly as before.

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{QueryRecord, ReadQuery, Select};
use crate::table::record_cow::RecordCow;
use crate::table::tests::write_exec_tests::setup_empty_table;
use shamir_query_builder::write;
use shamir_query_types::batch::ResultEncoding;
use shamir_query_types::read::{AggFunc, AggregateField, SelectItem};
use shamir_types::codecs::interned::record_view_to_query_value;
use shamir_types::mpack;
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map;

// ============================================================================
// Helpers
// ============================================================================

/// Insert one record `{ name: "Alice", age: 30, city: "NYC" }` into the
/// table and return the stored raw bytes (the one-and-only record).
async fn insert_alice_and_get_bytes(
    table: &crate::table::TableManager,
    repo: &crate::repo::RepoInstance,
) -> bytes::Bytes {
    let op = write::insert("users")
        .row(mpack!({ "name": "Alice", "age": 30, "city": "NYC" }))
        .build();

    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(
        shamir_types::access::Actor::System,
        "test_insert_alice",
        move |tx| {
            Box::pin(async move { owned_table.execute_insert_tx(&owned_op, tx, false).await })
        },
    )
    .await
    .unwrap();

    // Collect the raw stored bytes for the one record.
    use futures::StreamExt;
    let stream = table.list_stream(100);
    futures::pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        for (_rid, cow) in batch.unwrap() {
            let raw = match cow {
                RecordCow::Borrowed(b) => b,
                RecordCow::Owned(iv) => iv.to_bytes().unwrap(),
            };
            out.push(raw);
        }
    }
    assert_eq!(out.len(), 1, "expected exactly 1 stored record");
    out.remove(0)
}

// ============================================================================
// Test 1 — SELECT * convergence: id-keyed ≡ name-keyed
// ============================================================================

/// The KEY convergence test for S-read.
///
/// 1. Insert one record.
/// 2. Read with `ResultEncoding::Id` and a `SELECT *` query.
/// 3. Assert the row is `QueryRecord::IdBytes`.
/// 4. Assert IdBytes == stored bytes (verbatim pass-through).
/// 5. De-intern the IdBytes → QueryValue.
/// 6. Read the same row with `ResultEncoding::Name`.
/// 7. Assert de-intern(IdBytes) == Name-path QueryValue.
#[tokio::test]
async fn s_read_select_star_convergence() {
    let (table, repo) = setup_empty_table().await;

    let stored_bytes = insert_alice_and_get_bytes(&table, &repo).await;

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // --- Id path ---
    let q_id = ReadQuery::new("users"); // default SELECT * with ResultEncoding::Id
    let result_id = table
        .read_with_encoding(&q_id, &ctx, ResultEncoding::Id)
        .await
        .unwrap();

    assert_eq!(result_id.records.len(), 1, "expected 1 record (Id path)");
    let id_row = &result_id.records[0];

    // Must be IdBytes, not Direct.
    let id_bytes = match id_row {
        QueryRecord::IdBytes(b) => b.clone(),
        other => panic!("expected QueryRecord::IdBytes, got {other:?}"),
    };

    // Byte-identity: IdBytes == stored bytes (SELECT * verbatim pass-through).
    assert_eq!(
        id_bytes.as_ref(),
        stored_bytes.as_ref(),
        "SELECT* IdBytes must equal verbatim stored bytes"
    );

    // --- Name path ---
    let q_name = ReadQuery::new("users");
    let result_name = table
        .read_with_encoding(&q_name, &ctx, ResultEncoding::Name)
        .await
        .unwrap();

    assert_eq!(
        result_name.records.len(),
        1,
        "expected 1 record (Name path)"
    );
    let name_row = &result_name.records[0];

    // Name path must return Direct(QueryValue).
    let name_qv = match name_row {
        QueryRecord::Direct(qv) => qv.clone(),
        other => panic!("expected QueryRecord::Direct from Name path, got {other:?}"),
    };

    // De-intern the IdBytes via RecordView.
    let view = RecordView::new(&id_bytes).expect("IdBytes must be valid msgpack");
    let id_qv =
        record_view_to_query_value(&view, interner).expect("de-interning IdBytes must succeed");

    // Convergence: de-intern(IdBytes) == Name-path QueryValue.
    assert_eq!(
        id_qv, name_qv,
        "de-intern(IdBytes) must equal the Name-path QueryValue:\n  id:   {id_qv:?}\n  name: {name_qv:?}"
    );
}

// ============================================================================
// Test 2 — Field projection convergence: SELECT a, c with Id encoding
// ============================================================================

/// Field projection convergence for S-read.
///
/// 1. Insert one record `{ name, age, city }`.
/// 2. Read `SELECT name, city` with `ResultEncoding::Id`.
/// 3. Assert the row is `QueryRecord::IdBytes` with two fields.
/// 4. De-intern the IdBytes → QueryValue.
/// 5. Read `SELECT name, city` with `ResultEncoding::Name`.
/// 6. Assert de-intern(IdBytes) == Name-path QueryValue.
#[tokio::test]
async fn s_read_projection_convergence() {
    let (table, repo) = setup_empty_table().await;
    insert_alice_and_get_bytes(&table, &repo).await;

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let proj_select = Select::fields(["name", "city"]);

    // --- Id path ---
    let q_id = ReadQuery::new("users").select(proj_select.clone());
    let result_id = table
        .read_with_encoding(&q_id, &ctx, ResultEncoding::Id)
        .await
        .unwrap();

    assert_eq!(
        result_id.records.len(),
        1,
        "expected 1 record (Id projection)"
    );
    let id_row = &result_id.records[0];

    let id_bytes = match id_row {
        QueryRecord::IdBytes(b) => b.clone(),
        other => panic!("expected QueryRecord::IdBytes for projection, got {other:?}"),
    };

    // De-intern the projected IdBytes.
    let view = RecordView::new(&id_bytes).expect("projected IdBytes must be valid msgpack");
    let id_qv = record_view_to_query_value(&view, interner)
        .expect("de-interning projected IdBytes must succeed");

    // Projected IdBytes must contain exactly 2 fields (name and city).
    let id_map = match &id_qv {
        shamir_types::types::value::QueryValue::Map(m) => m,
        other => panic!("expected QueryValue::Map from de-interned IdBytes, got {other:?}"),
    };
    assert_eq!(
        id_map.len(),
        2,
        "projected IdBytes must have exactly 2 fields"
    );
    assert!(
        id_map.contains_key("name"),
        "projected result must contain 'name'"
    );
    assert!(
        id_map.contains_key("city"),
        "projected result must contain 'city'"
    );
    assert!(
        !id_map.contains_key("age"),
        "projected result must NOT contain 'age'"
    );

    // --- Name path ---
    let q_name = ReadQuery::new("users").select(proj_select);
    let result_name = table
        .read_with_encoding(&q_name, &ctx, ResultEncoding::Name)
        .await
        .unwrap();

    assert_eq!(
        result_name.records.len(),
        1,
        "expected 1 record (Name projection)"
    );
    let name_qv = match &result_name.records[0] {
        QueryRecord::Direct(qv) => qv.clone(),
        other => panic!("expected QueryRecord::Direct from Name projection, got {other:?}"),
    };

    // Convergence: de-intern(projected IdBytes) == Name-path projected QueryValue.
    assert_eq!(
        id_qv, name_qv,
        "de-intern(projected IdBytes) must equal Name-path projection:\n  id:   {id_qv:?}\n  name: {name_qv:?}"
    );
}

// ============================================================================
// Test 3 — Aggregate fallback: ResultEncoding::Id with SUM → Name path
// ============================================================================

/// An aggregate query with `ResultEncoding::Id` must fall back to the Name
/// path (because aggregates require server-side computation and de-interning).
/// The result is correct (`Direct`/`Json`) and not `IdBytes`.
#[tokio::test]
async fn s_read_aggregate_falls_back_to_name() {
    let (table, repo) = setup_empty_table().await;

    // Insert two records so the aggregate is non-trivial.
    let op = write::insert("users")
        .row(mpack!({ "name": "Alice", "age": 30 }))
        .row(mpack!({ "name": "Bob", "age": 25 }))
        .build();
    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(
        shamir_types::access::Actor::System,
        "test_insert_two",
        move |tx| {
            Box::pin(async move { owned_table.execute_insert_tx(&owned_op, tx, false).await })
        },
    )
    .await
    .unwrap();

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Build SELECT sum(age) AS total_age.
    let agg_select = Select {
        items: vec![SelectItem::Aggregate {
            func: AggFunc::Sum,
            field: AggregateField::Field(vec!["age".to_string()]),
            alias: Some("total_age".to_string()),
            distinct: false,
        }],
        distinct: false,
    };

    let q = ReadQuery::new("users").select(agg_select);
    let result = table
        .read_with_encoding(&q, &ctx, ResultEncoding::Id)
        .await
        .unwrap();

    // Aggregate fallback: must return 1 row, must NOT be IdBytes.
    assert_eq!(
        result.records.len(),
        1,
        "aggregate must return exactly 1 row"
    );
    let row = &result.records[0];
    assert!(
        !matches!(row, QueryRecord::IdBytes(_)),
        "aggregate with ResultEncoding::Id must fall back to Name path (not IdBytes), got {row:?}"
    );

    // The aggregate result must be correct: sum(30, 25) == 55.
    let qv = row.as_value().into_owned();
    assert_eq!(qv["total_age"], 55i64, "sum(age) must equal 55, got {qv:?}");
}

// ============================================================================
// Test 4 — Default unchanged: ResultEncoding::Name → Direct rows as before
// ============================================================================

/// `ResultEncoding::Name` (or the default, not specifying encoding) must
/// produce `QueryRecord::Direct(QueryValue)` rows, unchanged from the
/// pre-S-read behaviour.
#[tokio::test]
async fn s_read_default_name_encoding_unchanged() {
    let (table, repo) = setup_empty_table().await;
    insert_alice_and_get_bytes(&table, &repo).await;

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Use the plain `read()` method (Name encoding, pre-S-read behaviour).
    let q = ReadQuery::new("users");
    let result_plain = table.read(&q, &ctx).await.unwrap();

    assert_eq!(
        result_plain.records.len(),
        1,
        "expected 1 record (plain read)"
    );
    match &result_plain.records[0] {
        QueryRecord::Direct(_) => {} // correct
        other => panic!("plain read() must return Direct, got {other:?}"),
    }

    // Explicitly specifying Name must also return non-IdBytes.
    let result_name = table
        .read_with_encoding(&q, &ctx, ResultEncoding::Name)
        .await
        .unwrap();

    assert_eq!(
        result_name.records.len(),
        1,
        "expected 1 record (Name encoding)"
    );
    assert!(
        !matches!(&result_name.records[0], QueryRecord::IdBytes(_)),
        "ResultEncoding::Name must never return IdBytes"
    );

    // The name-keyed result must contain Alice's fields.
    let rec = &result_name.records[0];
    assert_eq!(rec.get_value_str("name"), Some("Alice"));
    assert_eq!(rec.get_value_i64("age"), Some(30));
}
