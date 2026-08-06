//! F-3 (#1030) — `doctor::repair()`'s self-heal (drop -> recreate) block
//! must acquire `ddl_admission` (via `TableManager::begin_write_barrier`)
//! for its entire duration, exactly like the barrier-taking `TableManager`
//! wrapper methods (`drop_index`/`drop_unique_index`/`drop_sorted_index`)
//! already do.
//!
//! See `docs/dev-artifacts/prompts/ddl-lifecycle/08-f3-cascade-repair-admission.md`
//! for the full defect/fix writeup, and this same directory's
//! `r0a_registry_watermark_admission_tests.rs`'s
//! `drop_sorted_index_acquires_write_barrier`/`drop_index2_acquires_write_barrier`
//! for the pattern this test mirrors (hold `unique_write_lock` externally,
//! spawn the op, assert it blocks until released).

use std::time::Duration;

use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::table::tests::test_helpers::query_value_to_inner_tracked;

/// Fresh in-memory DB with a `users` table carrying one regular index (on
/// `city`) and a handful of seeded rows — enough for `repair()`'s
/// drop -> recreate self-heal loop to have real work to do.
async fn seeded_with_regular_index(n: usize) -> crate::table::TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    table.create_index("by_city", &["city"]).await.unwrap();

    let interner = table.interner().get().await.unwrap();
    for i in 0..n {
        let mut m = new_map();
        m.insert(
            "city".to_string(),
            QueryValue::Str(format!("city_{}", i % 4)),
        );
        let row = QueryValue::Map(m);
        let (inner_val, new_keys) = query_value_to_inner_tracked(&row, interner).unwrap();
        if !new_keys.is_empty() {
            table.interner().save_new_keys(&new_keys).await.unwrap();
        }
        table.insert(&inner_val).await.unwrap();
    }
    table
}

/// `doctor::repair()`'s self-heal (drop -> recreate) block must acquire the
/// write barrier for its full critical section. Hold `unique_write_lock`
/// externally (the same lock `begin_write_barrier` takes) and confirm
/// `repair()` blocks until released — pre-fix, the self-heal loop's direct
/// `index_manager_ref().drop_index`/`create_index_from_records` calls took
/// no lock at all and this would finish immediately (never block).
#[tokio::test]
async fn doctor_repair_acquires_write_barrier() {
    let table = seeded_with_regular_index(20).await;

    let guard = table.unique_write_lock().lock_owned().await;

    let table_repair = table.clone();
    let repair_task = tokio::spawn(async move { table_repair.repair().await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !repair_task.is_finished(),
        "F-3 (#1030): doctor::repair()'s self-heal block must block on the \
         unique_write_lock held here (pre-fix it acquired no admission/lock \
         at all and this would finish immediately)"
    );

    drop(guard);
    let report = repair_task
        .await
        .unwrap()
        .expect("repair must complete once the lock is released");
    assert_eq!(report.records_scanned, 20);

    // Sanity: the index survived the repair and is still queryable.
    assert!(
        table
            .index_manager_ref()
            .iter_indexes()
            .any(|d| d.name_interned != 0),
        "the regular index must still be registered after repair"
    );
}
