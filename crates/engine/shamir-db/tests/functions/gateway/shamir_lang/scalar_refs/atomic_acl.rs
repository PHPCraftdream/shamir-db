use super::support::{
    create_as, invoke_as, native_exists, open_acl_ancestors, restrict_table_to_owner, restricted,
    setup,
};
use shamir_engine::function::Security;
use shamir_types::access::{ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;

const TWO_TABLE_TRANSACTION: &str = r#"@entry procedure run() -> i64 {
    let b0 = batch_tx();
    let b1 = batch_insert(b0, "first", "first_rows", { id: 401, value: "first" });
    let b2 = batch_insert(b1, "second", "second_rows", { id: 402, value: "second" });
    let response = db_execute(request(b2));
    let first: [{ id: i64 }]? = response_inserted(response, "first");
    let second: [{ id: i64 }]? = response_inserted(response, "second");
    if (is_null(first)) { return -1; }
    if (is_null(second)) { return -2; }
    return 1;
}"#;

fn assert_gateway_create_denial(result: shamir_db::DbResult<QueryValue>) {
    let error = result
        .expect_err("Invoker without CREATE access must be denied before transaction execution");
    let shamir_db::DbError::Function(message) = error else {
        panic!("expected gateway DbError::Function, got {error:?}");
    };
    assert!(
        message.contains("compute error"),
        "expected the existing guest compute-error classification, got: {message}"
    );
    assert!(
        message.contains("db_execute: db gateway: permission denied:"),
        "expected normal db_execute gateway denial, got: {message}"
    );
    assert!(
        message.contains(
            "access denied: User(2002) cannot CREATE on db://scalar_ref_db/main/second_rows"
        ),
        "expected second_rows CREATE denial before transaction execution, got: {message}"
    );
}

#[tokio::test]
async fn definer_transaction_uses_effective_owner_rights() {
    let db = setup(&["first_rows", "second_rows"]).await;
    open_acl_ancestors(&db).await;
    restrict_table_to_owner(&db, "first_rows").await;
    restrict_table_to_owner(&db, "second_rows").await;
    create_as(
        &db,
        "atomic_definer_acl",
        TWO_TABLE_TRANSACTION,
        Security::Definer,
    )
    .await;

    assert_eq!(
        invoke_as(&db, "atomic_definer_acl", restricted())
            .await
            .expect("Definer batch is authorized as the function owner"),
        QueryValue::Int(1)
    );
    assert!(native_exists(&db, "first_rows", 401).await);
    assert!(native_exists(&db, "second_rows", 402).await);
}

#[tokio::test]
async fn invoker_second_table_create_denial_precedes_transaction_writes() {
    let db = setup(&["first_rows", "second_rows"]).await;
    open_acl_ancestors(&db).await;
    db.set_resource_meta(
        &ResourcePath::table(super::support::DB_NAME, "main", "first_rows"),
        &ResourceMeta::open(),
    )
    .await
    .expect("allow the Invoker's first CREATE to isolate second-table denial");
    restrict_table_to_owner(&db, "second_rows").await;
    create_as(
        &db,
        "atomic_invoker_acl",
        TWO_TABLE_TRANSACTION,
        Security::Invoker,
    )
    .await;

    assert!(
        !native_exists(&db, "first_rows", 401).await,
        "fixture confirms the first target row is absent before invocation"
    );
    assert!(
        !native_exists(&db, "second_rows", 402).await,
        "fixture confirms the denied target row is absent before invocation"
    );
    let tx_started_before = db
        .get_db(super::support::DB_NAME)
        .expect("test database")
        .get_repo("main")
        .expect("main repository")
        .tx_metrics()
        .snapshot()
        .txs_started;
    let denied = invoke_as(&db, "atomic_invoker_acl", restricted()).await;
    let tx_started_after = db
        .get_db(super::support::DB_NAME)
        .expect("test database after Invoker denial")
        .get_repo("main")
        .expect("main repository after Invoker denial")
        .tx_metrics()
        .snapshot()
        .txs_started;
    assert_eq!(
        tx_started_after, tx_started_before,
        "whole-tree CREATE authorization must reject before begin_tx"
    );
    assert_gateway_create_denial(denied);
    assert!(
        !native_exists(&db, "first_rows", 401).await,
        "authorized first insert must remain unwritten when second-table ACL denies the batch"
    );
    assert!(
        !native_exists(&db, "second_rows", 402).await,
        "denied second insert must remain unwritten"
    );
}
