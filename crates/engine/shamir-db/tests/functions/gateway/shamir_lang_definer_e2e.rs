//! End-to-end checks that `Security::Definer` reaches the real shamir DB gateway.

use shamir_db::shamir_db::FunctionSource;
use shamir_db::{DbError, DbResult, ShamirDb};
use shamir_engine::function::{CreateFunctionOptions, Params, Security, Visibility};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_types::access::{Action, Actor, ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;

const OWNER_ID: u64 = 1001;
const RESTRICTED_ID: u64 = 2002;
const DB_NAME: &str = "testdb";
const TABLE: &str = "secrets";

const ALL_PATHS_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let inserted: { id: i64, label: string } = db_insert("secrets", { id: 10, label: "definer" });
    let found: { id: i64, label: string }? = db_get("secrets", 10);
    if (is_null(found)) { return -1; }
    let b = batch_read(batch(), "rows", "secrets", { id: 10 });
    let resp = db_execute(request(b));
    let rows: [{ id: i64, label: string }]? = response_rows(resp, "rows");
    if (is_null(rows)) { return -2; }
    return inserted.id + unwrap(found).id + unwrap(rows)[0].id;
}"#;

const INSERT_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let inserted: { id: i64, label: string } = db_insert("secrets", { id: 2, label: "inserted" });
    return inserted.id;
}"#;

const GET_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let found: { id: i64, label: string }? = db_get("secrets", 1);
    if (is_null(found)) { return -1; }
    return unwrap(found).id;
}"#;

const BATCH_READ_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let b = batch_read(batch(), "rows", "secrets", null);
    let resp = db_execute(request(b));
    let rows: [{ id: i64 }]? = response_rows(resp, "rows");
    if (is_null(rows)) { return -1; }
    return to_i64(len(unwrap(rows)));
}"#;

async fn setup() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("in-memory database");
    db.create_db(DB_NAME).await.expect("create database");

    let mut create = Batch::new();
    create.id("create_repo");
    create.create_repo(
        "repo",
        ddl::create_repo("main").engine("in_memory").tables([TABLE]),
    );
    db.execute(
        DB_NAME,
        &create
            .to_request_via_msgpack()
            .expect("create-repo request"),
    )
    .await
    .expect("create repository and table");

    // The DAC traversal rule needs Execute on every ancestor; keep db/store open
    // so the test isolates the table-level READ/WRITE decision.
    let open = ResourceMeta::open();
    db.set_resource_meta(&ResourcePath::database(DB_NAME), &open)
        .await
        .expect("open database ancestor");
    db.set_resource_meta(&ResourcePath::store(DB_NAME, "main"), &open)
        .await
        .expect("open store ancestor");

    // POSIX-style owner/other classes: only the privileged owner has table rights.
    db.set_resource_meta(
        &ResourcePath::table(DB_NAME, "main", TABLE),
        &ResourceMeta {
            owner: owner(),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("restrict table to function owner");

    let table_path = ResourcePath::table(DB_NAME, "main", TABLE);
    for action in [Action::Read, Action::Write] {
        let error = db
            .authorize_access(&restricted(), &table_path, action)
            .await
            .expect_err("restricted caller must have no direct table rights");
        let action_name = match action {
            Action::Read => "READ",
            Action::Write => "WRITE",
            Action::Create | Action::Delete | Action::Execute | Action::List | Action::Manage => {
                unreachable!("only READ and WRITE are checked here")
            }
        };
        assert_eq!(
            error.to_string(),
            format!("access denied: User({RESTRICTED_ID}) cannot {action_name} on db://testdb/main/secrets")
        );
    }

    db
}

fn owner() -> Actor {
    Actor::User(OWNER_ID)
}

fn restricted() -> Actor {
    Actor::User(RESTRICTED_ID)
}

async fn create_procedure(db: &ShamirDb, name: &str, source: &str, security: Security) {
    let options = CreateFunctionOptions {
        replace: false,
        visibility: Visibility::Public,
        security,
        secret_grants: Vec::new(),
        net_grants: Vec::new(),
        ..CreateFunctionOptions::default()
    };
    db.create_function_with_opts_as(name, FunctionSource::Shamir(source), options, owner())
        .await
        .unwrap_or_else(|error| panic!("create {name}: {error}"));

    // Invocation is gated by Execute on the function ResourcePath. Public is
    // persisted visibility metadata; the platform's actual invocation right is
    // granted here through the function's other-execute DAC bit (no setuid bit).
    db.set_resource_meta(
        &ResourcePath::function(name),
        &ResourceMeta {
            owner: owner(),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("grant execute on {name}: {error}"));
}

async fn invoke(db: &ShamirDb, name: &str) -> DbResult<QueryValue> {
    db.invoke_function_in_db_as(DB_NAME, "main", name, Params::new(), restricted())
        .await
}

async fn seed_row(db: &ShamirDb, id: i64, label: &str) {
    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "insert",
        insert(TABLE).rows([doc! { "id" => id, "label" => label }]),
    );
    db.execute(
        DB_NAME,
        &seed.to_request_via_msgpack().expect("seed request"),
    )
    .await
    .expect("seed table row");
}

async fn assert_native_row(db: &ShamirDb, id: i64, label: &str) {
    let mut read = Batch::new();
    read.id("verify");
    read.query(
        "rows",
        shamir_query_builder::query::Query::from(TABLE).where_eq("id", id),
    );
    let response = db
        .execute(
            DB_NAME,
            &read.to_request_via_msgpack().expect("verification request"),
        )
        .await
        .expect("native Batch verification query");
    let rows = &response.results["rows"].records;
    assert_eq!(rows.len(), 1, "native query must find exactly one row");
    assert_eq!(rows[0].get_value_i64("id"), Some(id));
    assert_eq!(rows[0].get_value_str("label"), Some(label));
}

fn assert_gateway_denial(result: DbResult<QueryValue>, operation: &str, action: &str) {
    let error = match result {
        Ok(value) => panic!("expected {operation} denial, got successful result {value:?}"),
        Err(error) => error,
    };
    let DbError::Function(message) = error else {
        panic!(
            "expected guest compute error carrying the gateway permission denial, got {error:?}"
        );
    };
    assert!(
        message.contains("compute error"),
        "expected the guest compute error kind, got: {message}"
    );
    assert!(
        message.contains(&format!("{operation}: db gateway: permission denied:")),
        "expected the gateway Permission denial for {operation}, got: {message}"
    );
    let denial =
        format!("access denied: User({RESTRICTED_ID}) cannot {action} on db://testdb/main/secrets");
    assert!(
        message.contains(&denial),
        "expected exact table/action denial `{denial}`, got: {message}"
    );
}

#[tokio::test]
async fn shamir_definer_uses_owner_for_direct_and_typed_gateway_paths() {
    let db = setup().await;
    create_procedure(&db, "all_paths", ALL_PATHS_SOURCE, Security::Definer).await;

    let result = invoke(&db, "all_paths")
        .await
        .expect("Definer procedure should use its owner's table rights");
    assert_eq!(result, QueryValue::Int(30));
    // Verify committed storage independently through the native query builder.
    assert_native_row(&db, 10, "definer").await;
}

#[tokio::test]
async fn shamir_invoker_db_insert_is_denied_independently() {
    let db = setup().await;
    create_procedure(&db, "insert_definer", INSERT_SOURCE, Security::Definer).await;
    create_procedure(&db, "insert_invoker", INSERT_SOURCE, Security::Invoker).await;

    assert_eq!(
        invoke(&db, "insert_definer").await.expect("Definer insert"),
        QueryValue::Int(2)
    );
    assert_native_row(&db, 2, "inserted").await;
    assert_gateway_denial(invoke(&db, "insert_invoker").await, "db_insert", "WRITE");
}

#[tokio::test]
async fn shamir_invoker_db_get_is_denied_independently() {
    let db = setup().await;
    seed_row(&db, 1, "seeded").await;
    create_procedure(&db, "get_definer", GET_SOURCE, Security::Definer).await;
    create_procedure(&db, "get_invoker", GET_SOURCE, Security::Invoker).await;

    assert_eq!(
        invoke(&db, "get_definer").await.expect("Definer get"),
        QueryValue::Int(1)
    );
    assert_native_row(&db, 1, "seeded").await;
    assert_gateway_denial(invoke(&db, "get_invoker").await, "db_get", "READ");
}

#[tokio::test]
async fn shamir_invoker_batch_read_execute_is_denied_independently() {
    let db = setup().await;
    seed_row(&db, 1, "seeded").await;
    create_procedure(&db, "batch_definer", BATCH_READ_SOURCE, Security::Definer).await;
    create_procedure(&db, "batch_invoker", BATCH_READ_SOURCE, Security::Invoker).await;

    assert_eq!(
        invoke(&db, "batch_definer")
            .await
            .expect("Definer batch read"),
        QueryValue::Int(1)
    );
    assert_native_row(&db, 1, "seeded").await;
    assert_gateway_denial(invoke(&db, "batch_invoker").await, "db_execute", "READ");
}
