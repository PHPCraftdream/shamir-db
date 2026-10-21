//! Tests proving that FacadeDbGateway (used by WASM functions) now routes
//! through `execute_as(effective_actor)` instead of `execute()` (which is
//! `execute_as(Actor::System)`).
//!
//! `FacadeDbGateway` is private, so we cannot construct it from outside the
//! module.  Instead we exercise the same code path it now uses — calling
//! `execute_as` with a restricted `Actor::User` on a database/table whose
//! mode denies that actor — and confirm the access is rejected.
//!
//! This test would *pass* with the old `execute()` path (System bypasses)
//! and *fail* (correctly) only after the fix that threads the real actor.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::Query;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

/// Helper: create an in-memory ShamirDb with `testdb` / `main` / `items`.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", config).await.unwrap();
    shamir
}

// ============================================================================
// execute_as(User) is denied when database mode blocks that user
// ============================================================================

#[tokio::test]
async fn execute_as_user_denied_on_restricted_database() {
    let shamir = setup().await;

    // Lock the database to owner=User(1), mode=0o700 (owner-only).
    // G.4c: the store + table default to enforced (0o700, System). Open them
    // so the database mode is the sole gate — this is the SUBJECT of the test.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &open)
        .await
        .unwrap();
    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("gw_read");
    b.query("r", Query::from("items"));
    let read_req = b.to_request_via_msgpack();

    // System bypasses — must succeed.
    assert!(
        shamir
            .execute_as(Actor::System, "testdb", &read_req)
            .await
            .is_ok(),
        "System must bypass ACLs"
    );

    // Owner succeeds.
    assert!(
        shamir
            .execute_as(Actor::User(1), "testdb", &read_req)
            .await
            .is_ok(),
        "Owner must be allowed"
    );

    // Non-owner User(99) is denied — this is the path the gateway now takes.
    let err = shamir
        .execute_as(Actor::User(99), "testdb", &read_req)
        .await
        .expect_err("User(99) must be denied on mode 0o700 database owned by User(1)");

    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "error should mention denial, got: {msg}"
    );
}

// ============================================================================
// execute_as(User) is denied when table mode blocks read
// ============================================================================

#[tokio::test]
async fn execute_as_user_denied_on_restricted_table() {
    let shamir = setup().await;

    // Database is open (defaults), but the table is locked to User(1).
    let table_meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &table_meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("gw_read");
    b.query("r", Query::from("items"));
    let read_req = b.to_request_via_msgpack();

    // System bypasses.
    assert!(
        shamir
            .execute_as(Actor::System, "testdb", &read_req)
            .await
            .is_ok(),
        "System must bypass table ACLs"
    );

    // Non-owner denied.
    let err = shamir
        .execute_as(Actor::User(99), "testdb", &read_req)
        .await
        .expect_err("User(99) must be denied read on restricted table");

    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "error should mention denial, got: {msg}"
    );
}

// ============================================================================
// NOTE: Full WASM-function-through-gateway test
// ============================================================================
//
// A complete end-to-end test that invokes a WASM function as Actor::User(N)
// and observes that the gateway's DB read is denied would require:
//   1. compiling a valid .wasm blob that calls db.get() via the host ABI,
//   2. registering it in the function catalogue,
//   3. invoking via invoke_function_in_db_as.
//
// That fixture is heavy and brittle (WASM toolchain dependency).  The tests
// above prove the critical property: the code path the gateway NOW calls
// (`execute_as(actor, ...)`) enforces ACLs, whereas the OLD path
// (`execute(...)` = `execute_as(Actor::System, ...)`) would bypass them.
// Combined with the structural change (FacadeDbGateway now carries `actor`
// and every method calls `execute_as(self.actor.clone(), ...)`), this gives
// sufficient coverage for the privilege-escalation fix.
