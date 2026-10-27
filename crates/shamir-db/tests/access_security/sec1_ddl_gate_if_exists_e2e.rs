//! SEC-1 #995 regression tests: `if_exists` / `if_not_exists` DDL must
//! authorize before probing existence (split from `sec1_ddl_gate_e2e.rs`).

use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::sec1_ddl_gate_e2e::{restrict_repo, restrict_table, setup, OTHER, OWNER};

// ============================================================================
// #995: broaden #989's auth-before-existence-probe fix to 8 more handlers.
// ============================================================================
//
// #989 fixed handle_drop_index / handle_rename_index (the block above). #995
// applies the identical reorder to the 8 remaining handlers that shared the
// same vulnerable shape (existence probe ran BEFORE authorize_access).
//
// Two classes of test:
//
// A. Auth-resource ≠ probe-resource — the actual leak IS observable:
//    create_table (auth: Store, probe: table), create_db (auth: Root,
//    probe: db), create_repo (auth: Database, probe: repo), drop_validator
//    (auth: FunctionNamespace, probe: validator). The resource
//    authorize_access checks EXISTS and is restricted, INDEPENDENTLY of
//    whether the thing being created/dropped exists. So reordering changes
//    the observable outcome: was a silent no-op (probe fired before auth),
//    now access_denied (auth fires first). For these we get a full pair:
//    the "actual fix" case (was a no-op) + a regression guard (was already
//    access_denied).
//
// B. Auth-resource == probe-resource — the fix is structural (defense in
//    depth) only: drop_table, drop_db, drop_repo, drop_function. The
//    resource authorize_access checks IS the thing being dropped; a missing
//    resource has open meta by design (resource_meta returns default = open
//    for an absent catalogue record), so auth passes for the missing case
//    either way and the reorder produces no observable change. We still
//    write a regression guard (existing + restricted + if_exists →
//    access_denied) to prove the handler denies correctly after the reorder.
//
// Root defaults to mode 0o751 (System-owned): Other keeps Execute (traverse)
// but loses Write/Create. This means create_db's handler-level
// authorize_access(Root, Create) already denies a non-System actor by
// default — the top-level context check (Database Read, which only needs
// Root Execute for traversal) still passes, so the handler is reachable.

// ---------------------------------------------------------------------------
// Helpers for #995
// ---------------------------------------------------------------------------

/// Minimal WASM module that accepts input and returns msgpack `null`.
/// Used to materialise function/validator catalogue entries without a
/// Rust toolchain (mirrors ddl_wire_e2e/helpers.rs `accept_wasm`).
fn accept_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 512) "\c0")
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.const 512) (i64.const 32)) (i64.const 1)))
)
"#,
    )
    .expect("WAT parse failed")
}

/// Restrict the FunctionNamespace singleton: chown to OWNER, chmod 0o700.
async fn restrict_function_namespace(shamir: &ShamirDb) {
    shamir
        .set_resource_meta(
            &ResourcePath::FunctionNamespace,
            &ResourceMeta {
                owner: Actor::User(OWNER),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();
}

/// Restrict a function by name: chown to OWNER, chmod 0o700.
async fn restrict_function(shamir: &ShamirDb, name: &str) {
    shamir
        .set_resource_meta(
            &ResourcePath::Function {
                name: name.to_string(),
            },
            &ResourceMeta {
                owner: Actor::User(OWNER),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();
}

/// Restrict "testdb" to OWNER / 0o744: Other keeps Read (top-level context
/// check passes) but loses Create/Write (handler's
/// authorize_access(Database, Create) denies). Used for create_repo whose
/// auth checks the database itself.
async fn restrict_db_create_only(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id("acl");
    b.op(
        "chown",
        ddl::chown(ddl::res::database("testdb"), OWNER)
            .hmac("dummy")
            .build()
            .expect("build Chown"),
    );
    b.op(
        "chmod",
        ddl::chmod(ddl::res::database("testdb"), 0o744)
            .hmac("dummy")
            .build()
            .expect("build Chmod"),
    );
    shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// CreateTable — auth: ResourcePath::store(db, repo), Action::Create
//                probe: table existence (db.has_table)
// Auth-resource (store) ≠ probe-resource (table): full pair.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_table_if_not_exists_denies_unauthorized_on_existing_table() {
    let shamir = setup().await;
    restrict_repo(&shamir).await; // restrict the STORE "testdb/main"

    // "items" EXISTS (created in setup). Before the fix the if_not_exists
    // probe returned a silent {"existed": true} no-op before authorize_access
    // ever ran — the oracle. After the fix auth fires first → access_denied.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_table("items").repo("main").if_not_exists()
    );
}

#[tokio::test]
async fn create_table_if_not_exists_denies_unauthorized_on_missing_table() {
    let shamir = setup().await;
    restrict_repo(&shamir).await;

    // "ghost" does NOT exist. Even before the fix this fell through the
    // if_not_exists guard (the guard only early-exits when the table EXISTS)
    // to authorize_access → access_denied. Regression guard.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_table("ghost").repo("main").if_not_exists()
    );
}

// ---------------------------------------------------------------------------
// CreateDb — auth: ResourcePath::Root, Action::Create
//             probe: db existence (has_db)
// Auth-resource (Root) ≠ probe-resource (db): full pair.
// Root defaults to 0o751 → Other has Execute but NOT Write/Create.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_db_if_not_exists_denies_unauthorized_on_existing_db() {
    let shamir = setup().await;

    // "testdb" EXISTS. Before the fix the if_not_exists probe returned a
    // silent {"existed": true} no-op before authorize_access(Root, Create)
    // ran — the oracle. After the fix auth fires first → access_denied.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_db("testdb").if_not_exists()
    );
}

#[tokio::test]
async fn create_db_if_not_exists_denies_unauthorized_on_missing_db() {
    let shamir = setup().await;

    // "ghostdb" does NOT exist. Even before the fix this fell through to
    // authorize_access(Root, Create) → access_denied (Root 0o751 denies
    // Create for non-System). Regression guard.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_db("ghostdb").if_not_exists()
    );
}

// ---------------------------------------------------------------------------
// CreateRepo — auth: ResourcePath::Database{db}, Action::Create
//              probe: repo existence (db.has_repo)
// Auth-resource (database) ≠ probe-resource (repo): full pair.
// The db is chmod 0o744 so Other retains Read (top-level context check
// passes) but loses Create.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_repo_if_not_exists_denies_unauthorized_on_existing_repo() {
    let shamir = setup().await;
    restrict_db_create_only(&shamir).await; // 0o744: Read yes, Create no

    // "main" EXISTS (created in setup). Before the fix the if_not_exists
    // probe returned a silent {"existed": true} no-op before
    // authorize_access(Database, Create) ran — the oracle.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_repo("main").engine("in_memory").if_not_exists()
    );
}

#[tokio::test]
async fn create_repo_if_not_exists_denies_unauthorized_on_missing_repo() {
    let shamir = setup().await;
    restrict_db_create_only(&shamir).await;

    // "ghostrepo" does NOT exist. Even before the fix this fell through to
    // authorize_access(Database, Create) → access_denied. Regression guard.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_repo("ghostrepo")
            .engine("in_memory")
            .if_not_exists()
    );
}

// ---------------------------------------------------------------------------
// DropTable — auth: ResourcePath::table(db, repo, table), Action::Delete
//             probe: table existence (db.has_table)
// Auth-resource == probe-resource: regression guard only.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_table_if_exists_denies_unauthorized_on_existing_table() {
    let shamir = setup().await;
    restrict_table(&shamir).await;

    // "items" EXISTS + restricted. The if_exists guard only early-exits when
    // MISSING, so even before the fix this fell through to authorize_access
    // → access_denied. Regression guard proving the reorder + the untouched
    // reverse-FK guard still deny correctly.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_table("items")
            .repo("main")
            .if_exists()
            .hmac("test")
            .build()
            .expect("build op")
    );
}

// ---------------------------------------------------------------------------
// DropDb — auth: ResourcePath::database(db), Action::Delete
//          probe: db existence (has_db)
// Auth-resource == probe-resource: regression guard only.
// Uses a separate context db ("testdb", open) and target db ("victim",
// restricted) so the handler-level auth is reachable.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_db_if_exists_denies_unauthorized_on_existing_db() {
    let shamir = setup().await;
    shamir.create_db("victim").await.expect("create_db");

    // Restrict "victim" to OWNER / 0o700.
    let mut b = Batch::new();
    b.id("acl");
    b.op(
        "chown",
        ddl::chown(ddl::res::database("victim"), OWNER)
            .hmac("dummy")
            .build()
            .expect("build Chown"),
    );
    b.op(
        "chmod",
        ddl::chmod(ddl::res::database("victim"), 0o700)
            .hmac("dummy")
            .build()
            .expect("build Chmod"),
    );
    shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();

    // "victim" EXISTS + restricted. The if_exists guard only early-exits when
    // MISSING, so even before the fix this fell through to authorize_access
    // → access_denied. Regression guard.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_db("victim")
            .if_exists()
            .hmac("test")
            .build()
            .expect("build op")
    );
}

// ---------------------------------------------------------------------------
// DropRepo — auth: ResourcePath::store(db, repo), Action::Delete
//            probe: repo existence (db.has_repo)
// Auth-resource == probe-resource: regression guard only.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_repo_if_exists_denies_unauthorized_on_existing_repo() {
    let shamir = setup().await;
    restrict_repo(&shamir).await;

    // "main" EXISTS + restricted. Regression guard — the if_exists guard
    // doesn't early-exit for an existing resource, so authorize_access ran
    // and denied both before and after the reorder.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_repo("main")
            .if_exists()
            .hmac("test")
            .build()
            .expect("build op")
    );
}

// ---------------------------------------------------------------------------
// DropFunction — auth: ResourcePath::Function{name}, Action::Delete
//                probe: function existence (functions().contains)
// Auth-resource == probe-resource: regression guard only.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_function_if_exists_denies_unauthorized_on_existing_function() {
    let shamir = setup().await;

    // Create a function as System so it has a catalogue record.
    shamir
        .create_function_from_wasm("myfunc", &accept_wasm(), false)
        .await
        .unwrap();
    restrict_function(&shamir, "myfunc").await;

    // "myfunc" EXISTS + restricted. Regression guard — the if_exists guard
    // doesn't early-exit for an existing function, so authorize_access ran
    // and denied both before and after the reorder.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_function("myfunc")
            .if_exists()
            .hmac("test")
            .build()
            .expect("build op")
    );
}

// ---------------------------------------------------------------------------
// DropValidator — auth: ResourcePath::FunctionNamespace, Action::Delete
//                 probe: validator existence (validators().id_for_name)
// Auth-resource (FunctionNamespace) ≠ probe-resource (validator): full pair.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_validator_if_exists_denies_unauthorized_on_missing_validator() {
    let shamir = setup().await;
    restrict_function_namespace(&shamir).await;

    // No validator "ghost" exists. Before the fix the if_exists probe
    // returned a silent {"existed": false} no-op before authorize_access
    // (FunctionNamespace, Delete) ran — the oracle. After the fix auth fires
    // first → access_denied.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_validator("ghost")
            .if_exists()
            .hmac("test")
            .build()
            .expect("build op")
    );
}

#[tokio::test]
async fn drop_validator_if_exists_denies_unauthorized_on_existing_validator() {
    let shamir = setup().await;

    // Create a validator as System so the probe sees it.
    shamir
        .create_validator_from_wasm("myval", &accept_wasm(), false)
        .await
        .unwrap();
    restrict_function_namespace(&shamir).await;

    // "myval" EXISTS. Even before the fix this fell through the if_exists
    // guard to authorize_access(FunctionNamespace, Delete) → access_denied.
    // Regression guard.
    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_validator("myval")
            .if_exists()
            .hmac("test")
            .build()
            .expect("build op")
    );
}
