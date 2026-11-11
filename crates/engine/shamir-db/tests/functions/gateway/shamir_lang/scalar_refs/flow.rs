use super::support::{create, invoke, native_exists, setup};

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_types::types::value::QueryValue;

const ALL_SCALARS: &str = r#"@entry procedure copy() -> i64 {
    let b0 = batch_read(batch(), "source", "source_rows", { id: 1 });
    const row: {
        id: i64,
        text_value: string,
        bool_value: bool,
        bytes_value: bytes,
        f64_value: f64,
        i32_value: i32,
        i64_value: i64
    } = {
        id: 2,
        text_value: query_ref_field(b0, "source", 0, "text_value"),
        bool_value: query_ref_field(b0, "source", 0, "bool_value"),
        bytes_value: query_ref_field(b0, "source", 0, "bytes_value"),
        f64_value: query_ref_field(b0, "source", 0, "f64_value"),
        i32_value: query_ref_field(b0, "source", 0, "i32_value"),
        i64_value: query_ref_field(b0, "source", 0, "i64_value")
    };
    let b1 = batch_insert(b0, "dependent", "sink_rows", row);
    let response = db_execute(request(b1));
    let rows: [{ id: i64 }]? = response_inserted(response, "dependent");
    if (is_null(rows)) { return -1; }
    return unwrap(rows)[0].id;
}"#;

const READ_TO_READ_I64: &str = r#"@entry procedure match_value() -> i64 {
    let b0 = batch_read(batch(), "source", "source_rows", { id: 1 });
    const filter: { value: i64 } = {
        value: query_ref_field(b0, "source", 0, "value")
    };
    let b1 = batch_read(b0, "matched", "match_rows", filter);
    let response = db_execute(request(b1));
    let rows: [{ id: i64, value: i64 }]? = response_rows(response, "matched");
    if (is_null(rows)) { return -1; }
    let found = unwrap(rows);
    if (len(found) != 1) { return -2; }
    return found[0].id;
}"#;

#[tokio::test]
async fn real_read_to_read_i64_equality_matches_exact_target_row() {
    const REFERENCED_VALUE: i64 = 9_007_199_254_740_993;
    let db = setup(&["source_rows", "match_rows"]).await;
    super::support::seed_number(&db, "source_rows", 1, QueryValue::Int(REFERENCED_VALUE)).await;
    super::support::seed_number(&db, "match_rows", 2, QueryValue::Int(REFERENCED_VALUE)).await;
    super::support::seed_number(&db, "match_rows", 3, QueryValue::Int(9_007_199_254_740_992)).await;
    create(&db, "read_to_read_i64", READ_TO_READ_I64).await;

    assert_eq!(
        invoke(&db, "read_to_read_i64")
            .await
            .expect("read-to-read i64 reference"),
        QueryValue::Int(2),
        "the referenced i64 equality must select only the matching target row"
    );
    assert_eq!(
        super::support::native_i64(&db, "match_rows", 2).await,
        Some(REFERENCED_VALUE),
        "native builder read confirms the matching fixture row remains persisted"
    );
    assert_eq!(
        super::support::native_i64(&db, "match_rows", 3).await,
        Some(9_007_199_254_740_992),
        "native builder read confirms the distinct neighboring i64 row remains persisted"
    );
}

#[tokio::test]
async fn all_six_non_null_scalar_reference_kinds_work_through_real_database() {
    let db = setup(&["source_rows", "sink_rows"]).await;
    let mut seed = Batch::new();
    seed.id("seed_scalars");
    seed.insert(
        "source",
        insert("source_rows").rows([doc! {
            "id" => 1,
            "text_value" => "scalar-string",
            "bool_value" => true,
            "bytes_value" => QueryValue::Bin(vec![0x53, 0x48]),
            "f64_value" => QueryValue::F64(6.25),
            "i32_value" => 2_147_483_647i64,
            "i64_value" => 9_007_199_254_740_993i64,
        }]),
    );
    db.execute(
        super::support::DB_NAME,
        &seed.to_request_via_msgpack().expect("seed request"),
    )
    .await
    .expect("seed all scalar source fields");
    create(&db, "all_scalar_refs", ALL_SCALARS).await;

    assert_eq!(
        invoke(&db, "all_scalar_refs")
            .await
            .expect("six guarded scalar references"),
        QueryValue::Int(2)
    );
    let mut verify = Batch::new();
    verify.id("verify_scalars");
    verify.query(
        "rows",
        shamir_query_builder::query::Query::from("sink_rows").where_eq("id", 2),
    );
    let response = db
        .execute(
            super::support::DB_NAME,
            &verify
                .to_request_via_msgpack()
                .expect("verification request"),
        )
        .await
        .expect("native builder read of inserted row");
    let rows = &response.results["rows"].records;
    assert_eq!(rows.len(), 1, "native verification finds the dependent row");
    assert_eq!(rows[0].get_value_str("text_value"), Some("scalar-string"));
    assert_eq!(rows[0].get_value_bool("bool_value"), Some(true));
    assert_eq!(
        rows[0].get_value("bytes_value"),
        Some(&QueryValue::Bin(vec![0x53, 0x48]))
    );
    assert_eq!(rows[0].get_value("f64_value"), Some(&QueryValue::F64(6.25)));
    assert_eq!(rows[0].get_value_i64("i32_value"), Some(2_147_483_647));
    assert_eq!(
        rows[0].get_value_i64("i64_value"),
        Some(9_007_199_254_740_993)
    );
    assert!(native_exists(&db, "sink_rows", 2).await);
}
