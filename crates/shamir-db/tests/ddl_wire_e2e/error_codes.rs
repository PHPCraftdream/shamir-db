//! Structured error codes tests.

use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::Actor;

use super::helpers::*;

// =====================================================================
// DDL S5: structured error codes
// =====================================================================

/// Create existing DB without if_not_exists -> code == "exists".
#[tokio::test]
async fn error_code_exists_create_db_duplicate() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("bootstrap").await;
    shamir.create_db("dup_db").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_db("op", ddl::create_db("dup_db"));
    let req = b.to_request_via_msgpack();
    let err = shamir.execute("bootstrap", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("exists"),
        "expected code 'exists', got: {:?} ({})",
        err.code(),
        err
    );
}

/// Create existing table without if_not_exists -> code == "exists".
#[tokio::test]
async fn error_code_exists_create_table_duplicate() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_table("op", ddl::create_table("users").repo("main"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("exists"),
        "expected code 'exists', got: {:?} ({})",
        err.code(),
        err
    );
}

/// Create existing repo without if_not_exists -> code == "exists".
#[tokio::test]
async fn error_code_exists_create_repo_duplicate() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("op", ddl::create_repo("main").engine("in_memory"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("exists"),
        "expected code 'exists', got: {:?} ({})",
        err.code(),
        err
    );
}

/// Drop non-empty DB without cascade -> code == "still_referenced".
#[tokio::test]
async fn error_code_still_referenced_drop_db() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id(1);
    b.drop_db("op", ddl::drop_db("testdb"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("still_referenced"),
        "expected code 'still_referenced', got: {:?} ({})",
        err.code(),
        err
    );
}

/// Drop non-empty repo without cascade -> code == "still_referenced".
#[tokio::test]
async fn error_code_still_referenced_drop_repo() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id(1);
    b.drop_repo("op", ddl::drop_repo("main"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("still_referenced"),
        "expected code 'still_referenced', got: {:?} ({})",
        err.code(),
        err
    );
}

/// DDL op by unprivileged user -> code == "access_denied".
#[tokio::test]
async fn error_code_access_denied_ddl() {
    let db = setup_db().await;

    // Restrict the database to owner-only.
    let mut b = Batch::new();
    b.id(1);
    let chown_h = b.chown("chown", ddl::chown(ddl::res::database("testdb"), 1));
    let chmod_h = b.chmod("chmod", ddl::chmod(ddl::res::database("testdb"), 0o700));
    b.after(&chmod_h, &chown_h);
    let chmod_req = b.to_request_via_msgpack();
    db.execute("testdb", &chmod_req).await.unwrap();

    // Non-owner user tries to create a table (needs traversal through db).
    let user_actor = Actor::User(999);
    let mut b = Batch::new();
    b.id(2);
    b.create_table("op", ddl::create_table("forbidden_table").repo("main"));
    let req = b.to_request_via_msgpack();
    let err = db.execute_as(user_actor, "testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "expected code 'access_denied', got: {:?} ({})",
        err.code(),
        err
    );
}

/// GrantRole for non-existent user -> code == "not_found".
#[tokio::test]
async fn error_code_not_found_grant_role_user() {
    let db = setup_db().await;

    // Create a role first.
    let mut b = Batch::new();
    b.id(1);
    b.create_role("op", ddl::create_role("testrole", vec![]));
    let create_role = b.to_request_via_msgpack();
    db.execute("testdb", &create_role).await.unwrap();

    // Grant to a non-existent user.
    let mut b = Batch::new();
    b.id(2);
    b.grant_role("op", ddl::grant_role("testrole", "ghost_user"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("not_found"),
        "expected code 'not_found', got: {:?} ({})",
        err.code(),
        err
    );
}

/// Create existing index without if_not_exists -> code == "exists".
#[tokio::test]
async fn error_code_exists_create_index_duplicate() {
    let db = setup_db().await;

    // Create an index.
    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "op",
        ddl::create_index("idx_name", "users")
            .repo("main")
            .fields(vec![vec!["name".to_string()]]),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // Try to create the same index again.
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("exists"),
        "expected code 'exists', got: {:?} ({})",
        err.code(),
        err
    );
}
