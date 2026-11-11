use super::support::{
    assert_reference_failure, create, invoke_with_id, native_exists, native_i64, seed_number, setup,
};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_types::types::value::QueryValue;

const GUARDED_F64: &str = r#"@entry procedure copy(id: i64) -> i64 {
    let b0 = batch_read(batch(), "source", "source_rows", { id: id });
    const row: { id: i64, value: f64 } = {
        id: id,
        value: query_ref_field(b0, "source", 0, "value")
    };
    let b1 = batch_insert(b0, "dependent", "sink_rows", row);
    let b2 = batch_insert(b1, "independent", "independent_rows", { id: id, value: "independent" });
    let response = db_execute(request(b2));
    let rows: [{ id: i64, value: f64 }]? = response_inserted(response, "dependent");
    if (is_null(rows)) { return -1; }
    return 1;
}"#;

const GUARDED_I32: &str = r#"@entry procedure copy(id: i64) -> i64 {
    let b0 = batch_read(batch(), "source", "source_rows", { id: id });
    const row: { id: i64, value: i32 } = {
        id: id,
        value: query_ref_field(b0, "source", 0, "value")
    };
    let b1 = batch_insert(b0, "dependent", "sink_rows", row);
    let response = db_execute(request(b1));
    let rows: [{ id: i64, value: i32 }]? = response_inserted(response, "dependent");
    if (is_null(rows)) { return -1; }
    return 1;
}"#;

async fn failure_fixture() -> shamir_db::ShamirDb {
    setup(&["source_rows", "sink_rows", "independent_rows"]).await
}

async fn assert_failed_for_id(db: &shamir_db::ShamirDb, id: i64) {
    assert_reference_failure(invoke_with_id(db, "guarded_copy", id).await);
    assert!(
        !native_exists(db, "sink_rows", id).await,
        "dependent row must not dispatch for id={id}"
    );
    assert!(
        native_exists(db, "independent_rows", id).await,
        "independent insert must remain committed for id={id}"
    );
}

#[tokio::test]
async fn missing_row_and_missing_field_fail_closed_without_dependent_dispatch() {
    let db = failure_fixture().await;
    create(&db, "guarded_copy", GUARDED_F64).await;

    assert_failed_for_id(&db, 11).await; // no source row
    let mut seed = Batch::new();
    seed.id("missing_field");
    seed.insert("row", insert("source_rows").rows([doc! { "id" => 12 }]));
    db.execute(
        super::support::DB_NAME,
        &seed
            .to_request_via_msgpack()
            .expect("missing-field request"),
    )
    .await
    .expect("seed row without referenced field");
    assert_failed_for_id(&db, 12).await;
}

#[tokio::test]
async fn null_and_wrong_integer_type_fail_closed_without_dependent_dispatch() {
    let db = failure_fixture().await;
    create(&db, "guarded_copy", GUARDED_F64).await;
    seed_number(&db, "source_rows", 21, QueryValue::Null).await;
    seed_number(&db, "source_rows", 22, QueryValue::Int(8)).await;

    assert_failed_for_id(&db, 21).await;
    assert_failed_for_id(&db, 22).await;
}

#[tokio::test]
async fn out_of_range_i32_fails_but_i32_max_succeeds() {
    let db = failure_fixture().await;
    seed_number(&db, "source_rows", 31, QueryValue::Int(2_147_483_648)).await;
    seed_number(&db, "source_rows", 32, QueryValue::Int(2_147_483_647)).await;
    create(&db, "guarded_copy", GUARDED_I32).await;

    assert_reference_failure(invoke_with_id(&db, "guarded_copy", 31).await);
    assert!(
        !native_exists(&db, "sink_rows", 31).await,
        "out-of-range i32 reference must not dispatch"
    );
    assert_eq!(
        invoke_with_id(&db, "guarded_copy", 32)
            .await
            .expect("i32::MAX is a supported scalar"),
        QueryValue::Int(1)
    );
    assert!(
        native_exists(&db, "sink_rows", 32).await,
        "dependent insert at i32::MAX must persist"
    );
    assert_eq!(native_i64(&db, "sink_rows", 32).await, Some(2_147_483_647));
}

#[tokio::test]
async fn fresh_invocation_succeeds_after_a_reference_failure() {
    let db = failure_fixture().await;
    create(&db, "guarded_copy", GUARDED_F64).await;
    assert_failed_for_id(&db, 41).await;
    seed_number(&db, "source_rows", 42, QueryValue::F64(42.5)).await;
    assert_eq!(
        invoke_with_id(&db, "guarded_copy", 42)
            .await
            .expect("later independent invocation succeeds"),
        QueryValue::Int(1)
    );
    assert!(native_exists(&db, "sink_rows", 42).await);
    assert_eq!(
        super::support::native_i64(&db, "sink_rows", 42).await,
        None,
        "a float-valued reference must persist as F64 rather than an integer"
    );
    let mut check = shamir_query_builder::batch::Batch::new();
    check.id("verify_f64");
    check.query(
        "rows",
        shamir_query_builder::query::Query::from("sink_rows").where_eq("id", 42),
    );
    let response = db
        .execute(
            super::support::DB_NAME,
            &check
                .to_request_via_msgpack()
                .expect("f64 verification request"),
        )
        .await
        .expect("verify persisted float through native builder");
    assert_eq!(
        response.results["rows"].records[0].get_value("value"),
        Some(&QueryValue::F64(42.5))
    );
    assert_eq!(
        super::support::native_text(&db, "independent_rows", 42)
            .await
            .as_deref(),
        Some("independent")
    );
}
