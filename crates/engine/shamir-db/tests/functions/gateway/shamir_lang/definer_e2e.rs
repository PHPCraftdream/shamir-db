//! End-to-end checks that `Security::Definer` reaches the real shamir DB gateway.

use shamir_db::shamir_db::FunctionSource;
use shamir_db::{DbError, DbResult, ShamirDb};
use shamir_engine::function::{CreateFunctionOptions, Params, Security, Visibility};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::filter::FilterValue;
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

/// A guest-controlled message that LOOKS like a stable batch code.
const FORGED_CODE_SOURCE: &str = r#"@entry procedure p() -> i64 {
    if (true) { throw "server_busy"; }
    return 0;
}"#;

/// A transactional `db_execute` over the restricted table: authorization
/// refuses the read before the transaction runs (it is NOT an `Ok` aborted
/// response), so the escaping failure is a gateway refusal.
const TX_READ_REFUSED_SOURCE: &str = r#"@entry procedure p() -> i64 {
    const b = batch_read(batch_tx(), "rows", "secrets", { id: 1 });
    let resp = db_execute(request(b));
    let rows: [{ id: i64 }]? = response_rows(resp, "rows");
    if (is_null(rows)) { return -1; }
    return to_i64(len(unwrap(rows)));
}"#;

/// A transactional `db_execute` that fails DURING execution (unique
/// violation), so the gateway returns an `Ok` batch response whose
/// transaction is aborted. The abort is data; the guest wrapper raises a
/// plain kind-2 error, so no stable code escapes.
const TX_ABORTED_SOURCE: &str = r#"@entry procedure p() -> i64 {
    const b = batch_tx();
    const op = batch_insert(b, "dup", "secrets", { id: 9001, label: "dup" });
    let resp = db_execute(request(op));
    let rows: [{ id: i64 }]? = response_inserted(resp, "dup");
    if (is_null(rows)) { return -1; }
    return 1;
}"#;

/// Child procedure whose gateway INSERT is refused (CREATE denial on the
/// restricted table) - the refusal must survive the parent's `call`.
const NESTED_CHILD_INSERT_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let inserted: { id: i64, label: string } = db_insert("secrets", { id: 3, label: "nested" });
    return inserted.id;
}"#;

const NESTED_PARENT_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let child: i64 = call("nested_child_insert", {});
    return child;
}"#;

const BATCH_READ_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let b = batch_read(batch(), "rows", "secrets", null);
    let resp = db_execute(request(b));
    let rows: [{ id: i64 }]? = response_rows(resp, "rows");
    if (is_null(rows)) { return -1; }
    return to_i64(len(unwrap(rows)));
}"#;

const UPDATE_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let b = batch_update(batch(), "write", "secrets", { id: 10 }, { label: "updated" });
    let resp = db_execute(request(b));
    let affected: i64? = response_affected(resp, "write");
    if (is_null(affected)) { return -1; }
    return unwrap(affected);
}"#;

const DELETE_SOURCE: &str = r#"@entry procedure p() -> i64 {
    let b = batch_delete(batch(), "remove", "secrets", { id: 11 });
    let resp = db_execute(request(b));
    let affected: i64? = response_affected(resp, "remove");
    if (is_null(affected)) { return -1; }
    return unwrap(affected);
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
    for action in [Action::Read, Action::Write, Action::Delete] {
        let error = db
            .authorize_access(&restricted(), &table_path, action)
            .await
            .expect_err("restricted caller must have no direct table rights");
        let action_name = match action {
            Action::Read => "READ",
            Action::Write => "WRITE",
            Action::Delete => "DELETE",
            Action::Create | Action::Execute | Action::List | Action::Manage => {
                unreachable!("only READ, WRITE, and DELETE are checked here")
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

/// Make `secrets.label` unique so a transactional second insert of an existing
/// label aborts during execution (not during authorization).
async fn add_unique_label_rule(db: &ShamirDb) {
    let mut index = Batch::new();
    index.id("unique_label_index");
    index.create_index(
        "unique_label",
        ddl::create_index("unique_label", TABLE).field("label"),
    );
    db.execute(
        DB_NAME,
        &index
            .to_request_via_msgpack()
            .expect("unique index request"),
    )
    .await
    .expect("create unique index on secrets.label");

    let mut schema = Batch::new();
    schema.id("unique_label_schema");
    schema.set_table_schema(
        "unique_label_schema",
        ddl::set_table_schema(TABLE).rules([ddl::field(["label"])
            .string()
            .required()
            .unique()
            .build()]),
    );
    db.execute(
        DB_NAME,
        &schema
            .to_request_via_msgpack()
            .expect("unique schema request"),
    )
    .await
    .expect("set unique schema rule on secrets.label");
}

/// Run an outer batch `Call` as the restricted actor and return the batch
/// error's stable code plus its rendered message.
async fn call_code(db: &ShamirDb, name: &str) -> (Option<String>, String) {
    let mut batch = Batch::new();
    batch.id("code_probe");
    batch.call("result", name, [] as [FilterValue; 0]);
    let error = db
        .execute_as(
            restricted(),
            DB_NAME,
            &batch
                .to_request_via_msgpack()
                .expect("code probe request encoding"),
        )
        .await
        .expect_err("the probed outer Call must fail");
    (error.code().map(str::to_string), error.to_string())
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
    // A denied READ now carries the unified `unknown_table` code (the SAME
    // code an absent table answers — the read refusal is not an existence
    // oracle), which the gateway classifies as a generic execution failure.
    // CREATE/WRITE/DELETE keep `access_denied`, hence `Permission`.
    let gateway_class = if action == "READ" {
        "execution failed"
    } else {
        "permission denied"
    };
    assert!(
        message.contains(&format!("{operation}: db gateway: {gateway_class}:")),
        "expected the gateway {gateway_class} denial for {operation}, got: {message}"
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
    // Inserting a record is a CREATE action on the table in the platform's DAC.
    assert_gateway_denial(invoke(&db, "insert_invoker").await, "db_insert", "CREATE");
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

async fn assert_native_absent(db: &ShamirDb, id: i64) {
    let mut read = Batch::new();
    read.id("verify_absent");
    read.query(
        "rows",
        shamir_query_builder::query::Query::from(TABLE).where_eq("id", id),
    );
    let response = db
        .execute(
            DB_NAME,
            &read
                .to_request_via_msgpack()
                .expect("absence query request"),
        )
        .await
        .expect("native query for deleted row");
    assert!(
        response.results["rows"].records.is_empty(),
        "native query must confirm id={id} is absent"
    );
}

#[tokio::test]
async fn shamir_update_delete_invoker_is_denied_but_definer_uses_owner_rights() {
    let db = setup().await;
    seed_row(&db, 10, "before-update").await;
    seed_row(&db, 11, "before-delete").await;
    create_procedure(&db, "update_definer", UPDATE_SOURCE, Security::Definer).await;
    create_procedure(&db, "update_invoker", UPDATE_SOURCE, Security::Invoker).await;
    create_procedure(&db, "delete_definer", DELETE_SOURCE, Security::Definer).await;
    create_procedure(&db, "delete_invoker", DELETE_SOURCE, Security::Invoker).await;

    assert_gateway_denial(invoke(&db, "update_invoker").await, "db_execute", "WRITE");
    // An unauthorized write must not change the row before the Definer runs.
    assert_native_row(&db, 10, "before-update").await;
    assert_eq!(
        invoke(&db, "update_definer").await.expect("Definer update"),
        QueryValue::Int(1)
    );
    assert_native_row(&db, 10, "updated").await;

    assert_gateway_denial(invoke(&db, "delete_invoker").await, "db_execute", "DELETE");
    // The denied Invoker delete must not remove the still-existing seeded row.
    assert_native_row(&db, 11, "before-delete").await;
    assert_eq!(
        invoke(&db, "delete_definer").await.expect("Definer delete"),
        QueryValue::Int(1)
    );
    assert_native_absent(&db, 11).await;
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

/// The outer batch `Call` must report the SAME stable code the stored
/// procedure's gateway refusal carried - the code rides the escaping typed
/// error, never a server-side slot and never guest text.
///
/// The probe uses an INSERT (a CREATE denial, `access_denied`), one of the
/// two typed gateway refusal classes. A denied READ carries the unified
/// `unknown_table` batch code, which the gateway classifies as a generic
/// execution failure; that class exposes no code to the WASM host (it cannot
/// name the batch error type), so a read denial arrives without a code.
#[tokio::test]
async fn shamir_invoker_gateway_denial_code_reaches_the_outer_call() {
    let db = setup().await;
    create_procedure(&db, "insert_invoker", INSERT_SOURCE, Security::Invoker).await;

    let (code, message) = call_code(&db, "insert_invoker").await;
    assert_eq!(
        code.as_deref(),
        Some("access_denied"),
        "outer Call must report the gateway's stable code, got: {message}"
    );
}

/// A refusal in a nested stored call is the CHILD's escaping failure, so its
/// stable code must reach the outer `Call`. Before this change the nested
/// gateway was built without a code channel, so the child's code vanished.
#[tokio::test]
async fn shamir_nested_gateway_denial_code_reaches_the_outer_call() {
    let db = setup().await;
    create_procedure(
        &db,
        "nested_child_insert",
        NESTED_CHILD_INSERT_SOURCE,
        Security::Invoker,
    )
    .await;
    create_procedure(
        &db,
        "nested_parent",
        NESTED_PARENT_SOURCE,
        Security::Invoker,
    )
    .await;

    let (code, message) = call_code(&db, "nested_parent").await;
    assert_eq!(
        code.as_deref(),
        Some("access_denied"),
        "the child's gateway code must reach the outer Call, got: {message}"
    );
}

/// A transactional `db_execute` over the restricted table is refused during
/// authorization (before the transaction runs), so it escapes as a gateway
/// failure rather than an `Ok` aborted response. Nothing is stored outside
/// the escaping error: the outer `Call` must NOT retain the batch's
/// `unknown_table` code.
#[tokio::test]
async fn shamir_transactional_execute_refusal_sets_no_outer_code() {
    let db = setup().await;
    create_procedure(
        &db,
        "tx_read_refused",
        TX_READ_REFUSED_SOURCE,
        Security::Invoker,
    )
    .await;

    let (code, message) = call_code(&db, "tx_read_refused").await;
    assert_eq!(
        code, None,
        "a refused transactional read must leave no outer code, got: {message}"
    );
}

/// A transactional `db_execute` whose gateway call returns an `Ok` aborted
/// `BatchResponse` does NOT set a code: the abort is data, and the guest's
/// own kind-2 raise is not a typed gateway refusal.
#[tokio::test]
async fn shamir_transactional_abort_data_sets_no_outer_code() {
    let db = setup().await;
    add_unique_label_rule(&db).await;
    seed_row(&db, 9000, "dup").await;
    create_procedure(&db, "tx_aborted", TX_ABORTED_SOURCE, Security::Definer).await;

    let (code, message) = call_code(&db, "tx_aborted").await;
    assert_eq!(
        code, None,
        "an aborted transactional response is data, not a code, got: {message}"
    );
    assert!(
        message.contains("transactional batch aborted"),
        "expected the guest's stable transactional-abort message, got: {message}"
    );
}

/// A guest `throw` whose text looks like a stable code must NOT become the
/// outer `Call`'s code: only a server-minted refusal code can supply one.
#[tokio::test]
async fn shamir_guest_trap_text_cannot_forge_batch_code() {
    let db = setup().await;
    create_procedure(&db, "forged_code", FORGED_CODE_SOURCE, Security::Invoker).await;

    let mut batch = Batch::new();
    batch.id("forged_code_call");
    batch.call("result", "forged_code", [] as [FilterValue; 0]);
    let error = db
        .execute_as(
            restricted(),
            DB_NAME,
            &batch
                .to_request_via_msgpack()
                .expect("forged-code request encoding"),
        )
        .await
        .expect_err("a guest throw must fail the outer batch");
    assert!(
        error.to_string().contains("server_busy"),
        "the guest trap text must still be visible in the message, got: {error:?}"
    );
    assert_eq!(
        error.code(),
        None,
        "guest-controlled trap text must never become a stable code, got: {error:?}"
    );
}
