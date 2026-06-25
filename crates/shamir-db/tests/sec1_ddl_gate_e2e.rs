//! SEC-1 regression test: admin/DDL ops must be gated by `authorize_access`.
//!
//! For each gated arm we prove that a non-owner actor with `Actor::User(OTHER)`
//! is denied (`access_denied` error code) when the target resource has been
//! chmod'ed to `0o700` (owner-only).  `Actor::System` always bypasses — that
//! behaviour is validated by all other tests that call `execute()` directly.
//!
//! The `authorize_access` implementation is POSIX-mode + open-by-default
//! (mode 0o777), so the gate only fires after an explicit `chmod`.  The test
//! thus proves two things:
//! 1. The gate exists at all (without it the non-owner would succeed).
//! 2. A restricted resource denies the non-owner (and not the owner).

use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

const OWNER: u64 = 7;
const OTHER: u64 = 99;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let mut b = Batch::new();
    b.id("s");
    b.op(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["items"]),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    // G.4c: new objects default to enforced (0o700, System). Open the db +
    // store ancestors so the per-resource `restrict_*` helpers below are the
    // sole gate — otherwise the owner (Actor::User(OWNER)) would be denied
    // traversal on the System-owned ancestors before reaching the target.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .unwrap();
    shamir
}

/// Restrict `testdb/main/items` table: chown to OWNER, chmod to 0o700.
async fn restrict_table(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id("acl");
    b.op(
        "chown",
        ddl::chown(ddl::res::table("testdb", "main", "items"), OWNER),
    );
    b.op(
        "chmod",
        ddl::chmod(ddl::res::table("testdb", "main", "items"), 0o700),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Restrict `testdb/main` store: chown to OWNER, chmod to 0o700.
async fn restrict_repo(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id("acl");
    b.op(
        "chown",
        ddl::chown(ddl::res::store("testdb", "main"), OWNER),
    );
    b.op(
        "chmod",
        ddl::chmod(ddl::res::store("testdb", "main"), 0o700),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Restrict `testdb` database: chown to OWNER, chmod to 0o700.
async fn restrict_db(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id("acl");
    b.op("chown", ddl::chown(ddl::res::database("testdb"), OWNER));
    b.op("chmod", ddl::chmod(ddl::res::database("testdb"), 0o700));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Execute a single-op batch as `actor` and return the `BatchError`, asserting
/// that it carries code `"access_denied"`.
macro_rules! assert_access_denied {
    ($shamir:expr, $actor:expr, $op_key:expr, $op:expr) => {{
        let mut b = Batch::new();
        b.id("t");
        b.op($op_key, $op);
        let err = $shamir
            .execute_as($actor, "testdb", &b.to_request_via_msgpack())
            .await
            .unwrap_err();
        assert_eq!(
            err.code(),
            Some("access_denied"),
            "expected access_denied, got: {:?} ({})",
            err.code(),
            err
        );
    }};
}

/// Execute a single-op batch as `actor` and assert it SUCCEEDS (no error).
macro_rules! assert_permitted {
    ($shamir:expr, $actor:expr, $op_key:expr, $op:expr) => {{
        let mut b = Batch::new();
        b.id("t");
        b.op($op_key, $op);
        let result = $shamir
            .execute_as($actor, "testdb", &b.to_request_via_msgpack())
            .await;
        assert!(
            result.is_ok(),
            "expected success for owner, got: {:?}",
            result
        );
    }};
}

// ============================================================================
// DropTable — ResourcePath::table(db, repo, table), Action::Delete
// ============================================================================

#[tokio::test]
async fn drop_table_gated_by_table_delete() {
    let shamir = setup().await;
    restrict_table(&shamir).await;

    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_table("items").repo("main")
    );

    // Owner succeeds (mode 0o700 → owner class → rwx → Delete allowed).
    assert_permitted!(
        shamir,
        Actor::User(OWNER),
        "op",
        ddl::drop_table("items").repo("main")
    );
}

// ============================================================================
// CreateIndex — ResourcePath::table(db, repo, table), Action::Write
// ============================================================================

#[tokio::test]
async fn create_index_gated_by_table_write() {
    let shamir = setup().await;
    restrict_table(&shamir).await;

    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::create_index("idx_x", "items")
            .repo("main")
            .fields(vec![vec!["name".to_string()]])
    );

    // Owner succeeds.
    assert_permitted!(
        shamir,
        Actor::User(OWNER),
        "op",
        ddl::create_index("idx_x", "items")
            .repo("main")
            .fields(vec![vec!["name".to_string()]])
    );
}

// ============================================================================
// DropIndex — ResourcePath::table(db, repo, table), Action::Write
// ============================================================================

#[tokio::test]
async fn drop_index_gated_by_table_write() {
    let shamir = setup().await;

    // Create index as System first.
    let mut b = Batch::new();
    b.id("pre");
    b.op(
        "idx",
        ddl::create_index("idx_x", "items")
            .repo("main")
            .fields(vec![vec!["name".to_string()]]),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    restrict_table(&shamir).await;

    assert_access_denied!(
        shamir,
        Actor::User(OTHER),
        "op",
        ddl::drop_index("idx_x", "items").repo("main")
    );
}

// ============================================================================
// DropRepo — ResourcePath::store(db, repo), Action::Delete
// ============================================================================

#[tokio::test]
async fn drop_repo_gated_by_store_delete() {
    let shamir = setup().await;
    restrict_repo(&shamir).await;

    assert_access_denied!(shamir, Actor::User(OTHER), "op", ddl::drop_repo("main"));

    // Owner passes the ACL gate (still_referenced is a business error, not access_denied).
    {
        let mut b = Batch::new();
        b.id("t");
        b.op("op", ddl::drop_repo("main").cascade());
        let result = shamir
            .execute_as(Actor::User(OWNER), "testdb", &b.to_request_via_msgpack())
            .await;
        assert!(
            result.is_ok(),
            "owner should succeed DropRepo with cascade: {:?}",
            result
        );
    }
}

// ============================================================================
// DropDb — ResourcePath::database(db), Action::Delete
// ============================================================================

#[tokio::test]
async fn drop_db_gated_by_database_delete() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("dropme").await;

    // Restrict database "dropme".
    let mut b = Batch::new();
    b.id("acl");
    b.op("chown", ddl::chown(ddl::res::database("dropme"), OWNER));
    b.op("chmod", ddl::chmod(ddl::res::database("dropme"), 0o700));
    shamir
        .execute("dropme", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Non-owner is denied.
    {
        let mut b2 = Batch::new();
        b2.id("t");
        b2.op("op", ddl::drop_db("dropme"));
        let err = shamir
            .execute_as(Actor::User(OTHER), "dropme", &b2.to_request_via_msgpack())
            .await
            .unwrap_err();
        assert_eq!(
            err.code(),
            Some("access_denied"),
            "expected access_denied for DropDb, got: {:?} ({})",
            err.code(),
            err
        );
    }

    // Owner succeeds.
    {
        let mut b3 = Batch::new();
        b3.id("t2");
        b3.op("op", ddl::drop_db("dropme"));
        let result = shamir
            .execute_as(Actor::User(OWNER), "dropme", &b3.to_request_via_msgpack())
            .await;
        assert!(result.is_ok(), "owner should succeed DropDb: {:?}", result);
    }
}

// ============================================================================
// List::Tables — ResourcePath::store(db, repo), Action::List
// ============================================================================

#[tokio::test]
async fn list_tables_gated_by_store_list() {
    let shamir = setup().await;
    restrict_repo(&shamir).await;

    let mut b = Batch::new();
    b.id("t");
    b.op("op", ddl::list_tables().repo("main"));
    let err = shamir
        .execute_as(Actor::User(OTHER), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "expected access_denied for List::Tables, got: {:?} ({})",
        err.code(),
        err
    );
}

// ============================================================================
// List::Indexes — ResourcePath::table(db, repo, table), Action::List
// ============================================================================

#[tokio::test]
async fn list_indexes_gated_by_table_list() {
    let shamir = setup().await;
    restrict_table(&shamir).await;

    let mut b = Batch::new();
    b.id("t");
    b.op("op", ddl::list_indexes("items").repo("main"));
    let err = shamir
        .execute_as(Actor::User(OTHER), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "expected access_denied for List::Indexes, got: {:?} ({})",
        err.code(),
        err
    );
}

// ============================================================================
// List::Repos — ResourcePath::database(db), Action::List
// ============================================================================

#[tokio::test]
async fn list_repos_gated_by_database_list() {
    let shamir = setup().await;
    restrict_db(&shamir).await;

    let mut b = Batch::new();
    b.id("t");
    b.op("op", ddl::list_repos());
    let err = shamir
        .execute_as(Actor::User(OTHER), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "expected access_denied for List::Repos, got: {:?} ({})",
        err.code(),
        err
    );
}
