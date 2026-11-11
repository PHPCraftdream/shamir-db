use shamir_db::shamir_db::FunctionSource;
use shamir_db::{DbError, DbResult, ShamirDb};
use shamir_engine::function::{CreateFunctionOptions, Params};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;

pub(super) const DB_NAME: &str = "scalar_ref_db";
pub(super) const OWNER_ID: u64 = 1001;
pub(super) const RESTRICTED_ID: u64 = 2002;

pub(super) async fn setup(tables: &[&str]) -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("in-memory database");
    db.create_db(DB_NAME).await.expect("create database");
    let mut batch = Batch::new();
    batch.id("create_repo");
    batch.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(tables.iter().copied()),
    );
    db.execute(
        DB_NAME,
        &batch.to_request_via_msgpack().expect("create-repo request"),
    )
    .await
    .expect("create repository and tables");
    db
}

pub(super) async fn create(db: &ShamirDb, name: &str, source: &str) {
    db.create_function_with_opts(
        name,
        FunctionSource::Shamir(source),
        CreateFunctionOptions::default(),
    )
    .await
    .unwrap_or_else(|error| panic!("create {name}: {error}"));
}

pub(super) async fn create_as(
    db: &ShamirDb,
    name: &str,
    source: &str,
    security: shamir_engine::function::Security,
) {
    db.create_function_with_opts_as(
        name,
        FunctionSource::Shamir(source),
        CreateFunctionOptions {
            security,
            ..CreateFunctionOptions::default()
        },
        owner(),
    )
    .await
    .unwrap_or_else(|error| panic!("create {name}: {error}"));
    db.set_resource_meta(
        &ResourcePath::function(name),
        &ResourceMeta {
            owner: owner(),
            group: None,
            mode: 0o711,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("grant Execute on {name}: {error}"));
}

pub(super) async fn invoke(db: &ShamirDb, name: &str) -> DbResult<QueryValue> {
    db.invoke_function_in_db(DB_NAME, "main", name, Params::new())
        .await
}

pub(super) async fn invoke_as(db: &ShamirDb, name: &str, actor: Actor) -> DbResult<QueryValue> {
    db.invoke_function_in_db_as(DB_NAME, "main", name, Params::new(), actor)
        .await
}

pub(super) async fn invoke_with_id(db: &ShamirDb, name: &str, id: i64) -> DbResult<QueryValue> {
    let mut params = Params::new();
    params.set("id", QueryValue::Int(id));
    db.invoke_function_in_db(DB_NAME, "main", name, params)
        .await
}

pub(super) fn owner() -> Actor {
    Actor::User(OWNER_ID)
}
pub(super) fn restricted() -> Actor {
    Actor::User(RESTRICTED_ID)
}

pub(super) async fn seed_text(db: &ShamirDb, table: &str, id: i64, value: &str) {
    let mut batch = Batch::new();
    batch.id("seed");
    batch.insert(
        "insert",
        insert(table).rows([doc! { "id" => id, "value" => value }]),
    );
    db.execute(
        DB_NAME,
        &batch.to_request_via_msgpack().expect("seed request"),
    )
    .await
    .expect("seed source row");
}

pub(super) async fn seed_number(db: &ShamirDb, table: &str, id: i64, value: QueryValue) {
    let mut batch = Batch::new();
    batch.id("seed");
    batch.insert(
        "insert",
        insert(table).rows([doc! { "id" => id, "value" => value }]),
    );
    db.execute(
        DB_NAME,
        &batch.to_request_via_msgpack().expect("seed request"),
    )
    .await
    .expect("seed source row");
}

pub(super) async fn native_text(db: &ShamirDb, table: &str, id: i64) -> Option<String> {
    let mut batch = Batch::new();
    batch.id("verify_text");
    batch.query("rows", Query::from(table).where_eq("id", id));
    let response = db
        .execute(
            DB_NAME,
            &batch.to_request_via_msgpack().expect("verify request"),
        )
        .await
        .expect("native builder verification");
    let rows = &response.results["rows"].records;
    assert!(
        rows.len() <= 1,
        "unique fixture id should have at most one row"
    );
    rows.first()
        .and_then(|row| row.get_value_str("value"))
        .map(str::to_owned)
}

pub(super) async fn native_i64(db: &ShamirDb, table: &str, id: i64) -> Option<i64> {
    let mut batch = Batch::new();
    batch.id("verify_i64");
    batch.query("rows", Query::from(table).where_eq("id", id));
    let response = db
        .execute(
            DB_NAME,
            &batch.to_request_via_msgpack().expect("verify request"),
        )
        .await
        .expect("native builder verification");
    let rows = &response.results["rows"].records;
    assert!(
        rows.len() <= 1,
        "unique fixture id should have at most one row"
    );
    rows.first().and_then(|row| row.get_value_i64("value"))
}

pub(super) fn assert_reference_failure(result: DbResult<QueryValue>) {
    let error = result.expect_err("unresolved or invalid scalar reference must fail the procedure");
    let DbError::Function(message) = error else {
        panic!("expected DbError::Function, got {error:?}");
    };
    assert!(
        message.contains(
            "guest invalid argument: batch reference did not resolve to the expected scalar"
        ),
        "expected kind-2 guest invalid argument, got: {message}"
    );
}

pub(super) fn assert_gateway_read_denial(result: DbResult<QueryValue>) {
    let error = result.expect_err("Invoker without READ access must be denied at the gateway");
    let DbError::Function(message) = error else {
        panic!("expected gateway DbError::Function, got {error:?}");
    };
    assert!(
        message.contains("db_execute: db gateway: permission denied:"),
        "expected normal db_execute gateway denial, got: {message}"
    );
    assert!(
        message.contains(&format!("access denied: User({RESTRICTED_ID}) cannot READ")),
        "expected source-table READ denial, got: {message}"
    );
}

pub(super) async fn native_exists(db: &ShamirDb, table: &str, id: i64) -> bool {
    let mut batch = Batch::new();
    batch.id("verify_exists");
    batch.query("rows", Query::from(table).where_eq("id", id));
    let response = db
        .execute(
            DB_NAME,
            &batch.to_request_via_msgpack().expect("verify request"),
        )
        .await
        .expect("native builder verification");
    let rows = &response.results["rows"].records;
    assert!(
        rows.len() <= 1,
        "unique fixture id should have at most one row"
    );
    !rows.is_empty()
}

pub(super) async fn open_acl_ancestors(db: &ShamirDb) {
    let open = ResourceMeta::open();
    db.set_resource_meta(&ResourcePath::database(DB_NAME), &open)
        .await
        .expect("open database ancestor");
    db.set_resource_meta(&ResourcePath::store(DB_NAME, "main"), &open)
        .await
        .expect("open store ancestor");
}

pub(super) async fn restrict_table_to_owner(db: &ShamirDb, table: &str) {
    db.set_resource_meta(
        &ResourcePath::table(DB_NAME, "main", table),
        &ResourceMeta {
            owner: owner(),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("restrict source table to owner");
}
