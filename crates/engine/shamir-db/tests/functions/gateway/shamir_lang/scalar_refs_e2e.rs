use shamir_db::shamir_db::FunctionSource;
use shamir_db::{DbError, ShamirDb};
use shamir_engine::function::{CreateFunctionOptions, Params};

const GATED_MESSAGE: &str =
    "query_ref_field is gated: scalar reference validation is not implemented";

async fn assert_creation_rejected(db: &ShamirDb, name: &str, source: &str) {
    let error = db
        .create_function_with_opts(
            name,
            FunctionSource::Shamir(source),
            CreateFunctionOptions::default(),
        )
        .await
        .expect_err("valid scalar-reference procedure must remain gated in production");
    let DbError::Function(message) = error else {
        panic!("expected compiler Function error, got {error:?}");
    };
    assert!(
        message.contains("T010:") && message.contains(GATED_MESSAGE),
        "expected T010 {GATED_MESSAGE:?}, got: {message}"
    );
    assert!(
        matches!(
            db.invoke_function(name, Params::new()).await,
            Err(DbError::Function(message)) if message == format!("function '{name}' not found")
        ),
        "rejected procedure invocation must report the exact missing-function error"
    );
}

#[tokio::test]
async fn valid_read_to_read_reference_source_stays_gated_at_create() {
    let db = ShamirDb::init_memory().await.expect("in-memory database");
    let source = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    const filter: { name: string } = {
        name: query_ref_field(b0, "source", 0, "name")
    };
    let b1 = batch_read(b0, "matched", "people", filter);
    let response = db_execute(request(b1));
    return 1;
}"#;

    assert_creation_rejected(&db, "gated_read_to_read", source).await;
}

#[tokio::test]
async fn valid_read_to_insert_reference_source_stays_gated_at_create() {
    let db = ShamirDb::init_memory().await.expect("in-memory database");
    let source = r#"@entry procedure p() -> i64 {
    let b0 = batch_read(batch(), "source", "people", null);
    const row: { name: string } = {
        name: query_ref_field(b0, "source", 0, "name")
    };
    let b1 = batch_insert(b0, "dependent", "sink", row);
    let response = db_execute(request(b1));
    return 1;
}"#;

    assert_creation_rejected(&db, "gated_read_to_insert", source).await;
}
