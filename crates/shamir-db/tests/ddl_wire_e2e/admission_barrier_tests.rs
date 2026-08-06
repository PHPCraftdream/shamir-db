//! F-3 (#1030) — `DROP TABLE ... CASCADE` must acquire `ddl_admission`
//! (via `TableManager::begin_write_barrier`) for its entire
//! index-mutation block, exactly like the barrier-taking `TableManager`
//! wrapper methods (`drop_index`/`drop_unique_index`/`drop_sorted_index`/
//! `drop_index2`) already do.
//!
//! See `docs/dev-artifacts/prompts/ddl-lifecycle/08-f3-cascade-repair-admission.md`
//! for the full defect/fix writeup, and
//! `crates/shamir-engine/src/table/tests/r0a_registry_watermark_admission_tests.rs`'s
//! `drop_sorted_index_acquires_write_barrier`/`drop_index2_acquires_write_barrier`
//! for the pattern this test mirrors (hold `unique_write_lock` externally,
//! spawn the op, assert it blocks until released).

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use super::helpers::*;

/// `DROP TABLE ... CASCADE`'s index-mutation block must acquire
/// `ddl_admission` for its whole duration. Hold `unique_write_lock`
/// externally (the same lock `begin_write_barrier` takes) and confirm the
/// cascade drop blocks until released — pre-fix, the cascade block took no
/// lock at all and this would finish immediately (never block).
#[tokio::test]
async fn drop_table_cascade_acquires_write_barrier() {
    let db = setup_db().await;

    // Create a regular index on "users" so the cascade block has real
    // index-mutation work to do.
    let mut b = Batch::new();
    b.id("ci");
    b.create_index(
        "op",
        ddl::create_index("idx_name", "users").fields([vec!["name".to_string()]]),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // Grab the TableManager BEFORE issuing the cascade drop, so the test can
    // hold `unique_write_lock` externally — the same lock
    // `begin_write_barrier` takes as Step 3 (after admission + drain).
    let db_inst = db.get_db("testdb").unwrap();
    let tbl = db_inst.get_table("main", "users").await.unwrap();
    let guard = tbl.unique_write_lock().lock_owned().await;

    let mut b = Batch::new();
    b.id("dt");
    b.drop_table("op", ddl::drop_table("users").repo("main").cascade());
    let req = b.to_request_via_msgpack();

    let db_drop = db.clone();
    let drop_task = tokio::spawn(async move { db_drop.execute("testdb", &req).await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !drop_task.is_finished(),
        "F-3 (#1030): DROP TABLE ... CASCADE's index-mutation block must \
         block on the unique_write_lock held here (pre-fix it acquired no \
         admission/lock at all and this would finish immediately)"
    );

    drop(guard);
    let resp = drop_task
        .await
        .unwrap()
        .expect("cascade drop must complete once the lock is released");
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(true),
    );

    // Sanity: the table is genuinely gone.
    let tables = db_inst.list_tables("main").unwrap_or_default();
    assert!(
        !tables.contains(&"users".to_string()),
        "table should be removed after cascade drop completes"
    );
}

/// Sanity companion: without an externally-held `unique_write_lock`, a
/// cascade drop with an index present completes promptly (the barrier is
/// acquired and released within the call, not held open-endedly).
#[tokio::test]
async fn drop_table_cascade_completes_without_external_contention() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id("ci");
    b.create_index(
        "op",
        ddl::create_index("idx_name", "users").fields([vec!["name".to_string()]]),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("dt");
    b.drop_table("op", ddl::drop_table("users").repo("main").cascade());
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(true),
    );
}
