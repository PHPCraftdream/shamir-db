//! R0-C (#1010) — DROP INDEX / RENAME INDEX must detect and refuse a
//! PRE-EXISTING cross-family name collision instead of silently resolving
//! to (DROP) the first matching family or (RENAME) every matching family.
//!
//! `CREATE INDEX`'s cross-family preflight (added by this same task) means a
//! collision can no longer be created through the normal DDL surface, so
//! these tests seed the collision directly through `TableManager` internals
//! (bypassing the CREATE preflight, exactly mirroring how a table that
//! predates the #1010 fix could have acquired one) and then drive DROP/RENAME
//! through the real `ShamirDb::execute` batch path to prove the handler-level
//! (DROP) / `TableManager::rename_index` (RENAME) refuse-on-collision guard
//! fires.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::core::interner::TouchInd;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

/// In-memory `ShamirDb` with `testdb` / `main` / `items`, plus a regular
/// index and a sorted index BOTH named `"shared_name"` — seeded directly
/// through `TableManager` (the sorted branch bypasses `create_sorted_index_with_include`'s
/// #1010 preflight by calling `SortedIndexManager::register` directly),
/// simulating a pre-existing cross-family collision.
async fn setup_with_collision() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let interner = table.interner().get().await.unwrap();
    let city_key = match interner.touch_ind("city").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };

    table.create_index("shared_name", &["city"]).await.unwrap();

    let name_interned = match interner.touch_ind("shared_name").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    let sorted_def = crate::engine::index::sorted_index_manager::SortedIndexDefinition::new(
        name_interned,
        vec![city_key],
    );
    table.sorted_indexes().register(sorted_def).await.unwrap();

    shamir
}

/// `DROP INDEX "shared_name"` on a table where BOTH a regular and a sorted
/// index carry that name must be REFUSED with a clear diagnostic, not
/// silently resolve to dropping only the first match (the regular index,
/// per the handler's existing resolution order) and leaving the sorted
/// sibling behind.
///
/// This test fails against the pre-fix handler: the short-circuit `||` chain
/// (`drop_index(...) || drop_sorted_index(...) || drop_index2(...)`) would
/// drop ONLY the regular index and return `Ok` with `existed: true`,
/// silently leaving the colliding sorted index intact — the `expect_err`
/// below would panic on an `Ok` response.
#[tokio::test]
async fn drop_index_refuses_pre_existing_cross_family_collision() {
    let shamir = setup_with_collision().await;

    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("shared_name", "items").repo("main"));
    let req = b.to_request_via_msgpack();

    let err = shamir
        .execute("testdb", &req)
        .await
        .expect_err("DROP INDEX must refuse when the name collides across families");
    assert_eq!(err.code(), Some("cross_family_collision"));

    // Neither index was touched — both families must still carry the name.
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();
    assert!(
        table.index_exists("shared_name").await,
        "the regular index must be untouched after a refused DROP"
    );
    assert!(
        table.sorted_index_exists("shared_name").await,
        "the sorted index must be untouched after a refused DROP"
    );
}

/// `RENAME INDEX "shared_name" TO "..."` must be refused the same way, via
/// `TableManager::rename_index`'s own collision guard (added by this task) —
/// exercised directly at the engine layer (RENAME's wire op has no dedicated
/// admin-handler collision-refuse check of its own; it delegates entirely to
/// `TableManager::rename_index`, which is where the guard lives).
#[tokio::test]
async fn rename_index_refuses_pre_existing_cross_family_collision() {
    let shamir = setup_with_collision().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let err = table
        .rename_index("shared_name", "renamed", None)
        .await
        .expect_err("RENAME INDEX must refuse when the source name collides across families");
    let msg = err.to_string();
    assert!(
        msg.contains("shared_name"),
        "the error must name the colliding index: {msg}"
    );

    // Neither index was touched — both families must still carry the
    // ORIGINAL name, and "renamed" must not exist anywhere.
    assert!(table.index_exists("shared_name").await);
    assert!(table.sorted_index_exists("shared_name").await);
    assert!(!table.index_exists("renamed").await);
    assert!(!table.sorted_index_exists("renamed").await);
}
