use super::support::{
    assert_reference_failure, create, invoke, invoke_with_id, native_exists, native_text, setup,
};
use shamir_db::{DbError, DbResult};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_types::types::value::QueryValue;

const UNIQUE_FAILURE: &str = r#"@entry procedure run() -> i64 {
    let b0 = batch_tx();
    let b1 = batch_insert(b0, "earlier", "independent_rows", { id: 101, value: "must rollback" });
    let b2 = batch_insert(b1, "duplicate", "sink_rows", { id: 102, code: "already-used" });
    let response = db_execute(request(b2));
    let rows: [{ id: i64 }]? = response_inserted(response, "earlier");
    if (is_null(rows)) { return -1; }
    return 1;
}"#;

const REFERENCE_FAILURE: &str = r#"@entry procedure run(id: i64) -> i64 {
    let b0 = batch_tx();
    let b1 = batch_insert(b0, "earlier", "independent_rows", { id: id, value: "must rollback" });
    let b2 = batch_read(b1, "source", "source_rows", { id: id });
    const row: { id: i64, value: string } = {
        id: id,
        value: query_ref_field(b2, "source", 0, "value")
    };
    let b3 = batch_insert(b2, "dependent", "sink_rows", row);
    let response = db_execute(request(b3));
    let rows: [{ id: i64 }]? = response_inserted(response, "earlier");
    if (is_null(rows)) { return -1; }
    return 1;
}"#;

const COMMIT_ALL: &str = r#"@entry procedure run() -> i64 {
    let b0 = batch_tx();
    let b1 = batch_insert(b0, "first", "independent_rows", { id: 201, value: "committed-first" });
    let b2 = batch_insert(b1, "second", "sink_rows", { id: 202, code: "committed-second" });
    let response = db_execute(request(b2));
    let first: [{ id: i64 }]? = response_inserted(response, "first");
    let second: [{ id: i64 }]? = response_inserted(response, "second");
    if (is_null(first)) { return -1; }
    if (is_null(second)) { return -2; }
    return 1;
}"#;

fn assert_generic_transaction_failure(result: DbResult<QueryValue>) {
    let error = result.expect_err("failed transactional execute must fail the procedure");
    let DbError::Function(message) = error else {
        panic!("expected kind-2 guest function error, got {error:?}");
    };
    assert!(
        message.contains("compute error: guest invalid argument: transactional batch aborted"),
        "expected the stable generic transactional-abort message, got: {message}"
    );
}

async fn add_unique_code_index(db: &shamir_db::ShamirDb) {
    let mut batch = Batch::new();
    batch.id("unique_code_index");
    batch.create_index(
        "unique_code",
        // A plain index backs the schema-level unique rule (same as the
        // declarative-schema tests): the rule requires a single-field index.
        ddl::create_index("unique_code", "sink_rows").field("code"),
    );
    db.execute(
        super::support::DB_NAME,
        &batch
            .to_request_via_msgpack()
            .expect("unique index request"),
    )
    .await
    .expect("create unique index for rollback fixture");

    let mut schema = Batch::new();
    schema.id("unique_code_schema");
    schema.set_table_schema(
        "unique_code_schema",
        ddl::set_table_schema("sink_rows").rules([ddl::field(["code"])
            .string()
            .required()
            .unique()
            .build()]),
    );
    db.execute(
        super::support::DB_NAME,
        &schema
            .to_request_via_msgpack()
            .expect("unique code schema request"),
    )
    .await
    .expect("set unique schema rule for rollback fixture");

    let mut seed = Batch::new();
    seed.id("seed_duplicate");
    seed.insert(
        "seed",
        insert("sink_rows").rows([doc! { "id" => 1, "code" => "already-used" }]),
    );
    db.execute(
        super::support::DB_NAME,
        &seed.to_request_via_msgpack().expect("unique seed request"),
    )
    .await
    .expect("seed existing unique value");
}

#[tokio::test]
async fn unique_violation_rolls_back_prior_transactional_insert() {
    let db = setup(&["sink_rows", "independent_rows"]).await;
    add_unique_code_index(&db).await;
    create(&db, "atomic_unique_failure", UNIQUE_FAILURE).await;

    assert_generic_transaction_failure(invoke(&db, "atomic_unique_failure").await);

    assert!(
        !native_exists(&db, "independent_rows", 101).await,
        "the earlier write must be rolled back after the unique violation"
    );
    let mut verify = Batch::new();
    verify.id("verify_unique_rollback");
    verify.query("rows", Query::from("sink_rows"));
    let response = db
        .execute(
            super::support::DB_NAME,
            &verify
                .to_request_via_msgpack()
                .expect("verify unique rollback request"),
        )
        .await
        .expect("native builder verification after unique violation");
    let sink_rows = &response.results["rows"].records;
    assert_eq!(sink_rows.len(), 1, "the pre-existing sink row is unchanged");
    assert_eq!(sink_rows[0].get_value_i64("id"), Some(1));
    assert_eq!(sink_rows[0].get_value_str("code"), Some("already-used"));
}

#[tokio::test]
async fn failed_reference_guard_rolls_back_prior_transactional_insert() {
    let db = setup(&["source_rows", "sink_rows", "independent_rows"]).await;
    create(&db, "atomic_reference_failure", REFERENCE_FAILURE).await;

    assert_reference_failure(invoke_with_id(&db, "atomic_reference_failure", 301).await);

    assert!(
        !native_exists(&db, "independent_rows", 301).await,
        "the earlier insert must roll back with the failed scalar-reference guard"
    );
    assert!(
        !native_exists(&db, "sink_rows", 301).await,
        "the guarded dependent insert must not be written"
    );
}

#[tokio::test]
async fn successful_transactional_execute_commits_every_insert() {
    let db = setup(&["sink_rows", "independent_rows"]).await;
    create(&db, "atomic_commit_all", COMMIT_ALL).await;

    assert_eq!(
        invoke(&db, "atomic_commit_all")
            .await
            .expect("transaction commits"),
        QueryValue::Int(1)
    );
    assert!(native_exists(&db, "independent_rows", 201).await);
    assert_eq!(
        native_text(&db, "independent_rows", 201).await.as_deref(),
        Some("committed-first")
    );
    assert!(native_exists(&db, "sink_rows", 202).await);
    let mut verify = Batch::new();
    verify.id("verify_committed_rows");
    verify.query("rows", Query::from("sink_rows"));
    let response = db
        .execute(
            super::support::DB_NAME,
            &verify
                .to_request_via_msgpack()
                .expect("verify committed rows request"),
        )
        .await
        .expect("native builder verification of committed sink row");
    let sink_rows = &response.results["rows"].records;
    assert_eq!(sink_rows.len(), 1);
    assert_eq!(sink_rows[0].get_value_i64("id"), Some(202));
    assert_eq!(sink_rows[0].get_value_str("code"), Some("committed-second"));
}
