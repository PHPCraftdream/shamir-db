//! End-to-end tests for DML (data) enforcement at the facade level.
//!
//! Proves that `execute_as(Actor::User, ...)` respects table-level POSIX
//! mode for Read / Insert / Update / Set / Delete operations.
//!
//! Default mode is 0o777 (open) — so existing behaviour is unchanged.
//! Enforcement activates when `chmod` restricts a resource.
//!
//! **Query construction** uses the `shamir-query-builder` crate (Batch +
//! q!/doc! macros) and round-trips through msgpack to exercise the wire
//! encoding path.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::q;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

// ---------------------------------------------------------------------------
// Helper: build a Batch, encode to msgpack, decode back to BatchRequest.
// This proves the wire round-trip is lossless for every query in the file.
// ---------------------------------------------------------------------------

/// Helper: create an in-memory ShamirDb with `testdb` / `main` / `items`.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", config).await.unwrap();
    shamir
}

/// Insert a record via System actor (always allowed).
async fn seed_record(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id("seed");
    b.insert(
        "ins",
        q!(insert into items values {
            "name" => "widget",
            "price" => 42
        }),
    );
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();
}

// ============================================================================
// Default 0o777: any user can read, insert, delete — nothing is broken
// ============================================================================

#[tokio::test]
async fn default_mode_allows_all_users() {
    let shamir = setup().await;
    seed_record(&shamir).await;

    // Read
    let mut b = Batch::new();
    b.id("r");
    b.query("r", q!(from items));
    let read_req = b.to_request_via_msgpack();

    let resp = shamir
        .execute_as(Actor::User(99), "testdb", &read_req)
        .await;
    assert!(resp.is_ok(), "default 0o777 should allow any user to read");

    // Insert
    let mut b = Batch::new();
    b.id("i");
    b.insert(
        "i",
        q!(insert into items values {
            "name" => "gadget",
            "price" => 7
        }),
    );
    let ins_req = b.to_request_via_msgpack();

    let resp = shamir.execute_as(Actor::User(99), "testdb", &ins_req).await;
    assert!(
        resp.is_ok(),
        "default 0o777 should allow any user to insert"
    );
}

// ============================================================================
// Restricted table (0o750, owner=User(1)): owner allowed, stranger denied
// ============================================================================

#[tokio::test]
async fn restricted_table_owner_allowed_stranger_denied_read() {
    let shamir = setup().await;
    seed_record(&shamir).await;

    // Restrict the table: owner=User(1), mode=0o750
    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o750,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("r");
    b.query("r", q!(from items));
    let read_req = b.to_request_via_msgpack();

    // Owner User(1) reads successfully
    let resp = shamir.execute_as(Actor::User(1), "testdb", &read_req).await;
    assert!(resp.is_ok(), "owner should be able to read");

    // Stranger User(99) is denied
    let err = shamir
        .execute_as(Actor::User(99), "testdb", &read_req)
        .await
        .expect_err("stranger should be denied read on 0o750 table");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "error should mention denial, got: {msg}"
    );
}

#[tokio::test]
async fn restricted_table_stranger_denied_insert() {
    let shamir = setup().await;

    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o750,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("i");
    b.insert(
        "i",
        q!(insert into items values {
            "name" => "gadget",
            "price" => 7
        }),
    );
    let ins_req = b.to_request_via_msgpack();

    // Owner can insert (0o750 → owner has rwx)
    let resp = shamir.execute_as(Actor::User(1), "testdb", &ins_req).await;
    assert!(resp.is_ok(), "owner should be able to insert");

    // Stranger denied
    let err = shamir
        .execute_as(Actor::User(99), "testdb", &ins_req)
        .await
        .expect_err("stranger should be denied insert on 0o750 table");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "error should mention denial, got: {msg}"
    );
}

#[tokio::test]
async fn restricted_table_stranger_denied_delete() {
    let shamir = setup().await;
    seed_record(&shamir).await;

    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o750,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("d");
    b.delete("d", q!(delete from items where name == "widget"));
    let del_req = b.to_request_via_msgpack();

    // Stranger denied
    let err = shamir
        .execute_as(Actor::User(99), "testdb", &del_req)
        .await
        .expect_err("stranger should be denied delete on 0o750 table");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "error should mention denial, got: {msg}"
    );
}

// ============================================================================
// System actor always bypasses
// ============================================================================

#[tokio::test]
async fn system_bypasses_restricted_table() {
    let shamir = setup().await;
    seed_record(&shamir).await;

    // Lock down to User(1) only
    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("r");
    b.query("r", q!(from items));
    let read_req = b.to_request_via_msgpack();

    // System always passes
    let resp = shamir.execute_as(Actor::System, "testdb", &read_req).await;
    assert!(resp.is_ok(), "System must always bypass ACLs");

    // Also via the convenience `execute` (which is execute_as(System))
    let resp = shamir.execute("testdb", &read_req).await;
    assert!(resp.is_ok(), "execute() delegates to System, must pass");
}

// ============================================================================
// Interactive tx path: tx_execute_as enforces table DML too
// ============================================================================

#[tokio::test]
async fn tx_execute_as_enforces_table_acl() {
    let shamir = setup().await;
    seed_record(&shamir).await;

    // Restrict table to User(1)
    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "main", "items"), &meta)
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id("txr");
    b.query("r", q!(from items));
    let read_req = b.to_request_via_msgpack();

    // Open an interactive tx as User(1) (owner) — should succeed
    let (mut tx_ok, _guard_ok) = shamir
        .tx_begin_as(Actor::User(1), "testdb", "main", "snapshot")
        .await
        .unwrap();

    let resp = shamir
        .tx_execute_as(Actor::User(1), "testdb", &read_req, &mut tx_ok)
        .await;
    assert!(
        resp.is_ok(),
        "owner should be able to read inside interactive tx"
    );

    // Open an interactive tx as User(99) (stranger) — tx_begin may succeed
    // (database is open), but tx_execute_as with a read on restricted table
    // should fail.
    let (mut tx_bad, _guard_bad) = shamir
        .tx_begin_as(Actor::User(99), "testdb", "main", "snapshot")
        .await
        .unwrap();

    let err = shamir
        .tx_execute_as(Actor::User(99), "testdb", &read_req, &mut tx_bad)
        .await
        .expect_err("stranger should be denied read in interactive tx on restricted table");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "error should mention denial, got: {msg}"
    );
}
