//! Regression tests for task group 10 (panic-safety audit): a panic inside
//! the background `verify()` pass must not strand `verify_running = true`
//! forever. Before the fix, `TableManager::bump_write_counter`'s spawned
//! task only cleared `verify_running` on its normal (non-panicking) exit —
//! a panic inside `verify()` aborted the detached task mid-flight and
//! skipped that reset, permanently disabling the background consistency
//! watchdog with no signal to anyone (the identical shape as the
//! group-commit leader bug fixed alongside this test in
//! `repo/group_commit/tests/panicking_flush_tests.rs`).

use std::time::Duration;

use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::table_manager::AUTO_VERIFY_EVERY_N_WRITES;
use crate::table::{TableConfig, TableManager};

/// Build a fresh in-memory DB with a single empty `t` table. No indexes or
/// records needed — the injected panic fires before `verify()` touches any
/// index/data state.
async fn empty_table() -> TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("t")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    db.get_table("default", "t").await.unwrap()
}

/// Poll `is_background_verify_running()` until it reports `false`, bounded
/// by a timeout so a stranded `verify_running` flag fails the test instead
/// of hanging it forever.
async fn wait_until_verify_settles(table: &TableManager) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !table.is_background_verify_running() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "background verify did not settle within 5s — verify_running is stranded \
             (task group 10 regression)"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_verify_does_not_strand_the_watchdog() {
    let table = empty_table().await;
    table.install_verify_panic_hook();

    assert!(
        !table.is_background_verify_running(),
        "sanity: no verify should be running before the first bump"
    );

    // Cross the AUTO_VERIFY_EVERY_N_WRITES watermark — spawns a background
    // verify() that panics immediately (the injected hook fires first,
    // before any real audit work).
    table.bump_write_counter(AUTO_VERIFY_EVERY_N_WRITES);

    // Wait for the panicking background task to finish and release the
    // single-flight flag. Bounded so a real regression (stuck `true`
    // forever) fails the test instead of hanging it.
    wait_until_verify_settles(&table).await;

    assert_eq!(
        table.verify_call_count(),
        1,
        "the first (panicking) verify() must have been entered exactly once"
    );

    // Disarm the panic and cross the watermark again — a SUBSEQUENT
    // bump_write_counter call must still trigger a real verify(). Before
    // the fix, `verify_running` was stuck `true` from the panic above, so
    // this bump's `compare_exchange` would fail and no new verify() would
    // ever run again — the watchdog would be permanently disabled.
    table.clear_verify_panic_hook();
    table.bump_write_counter(AUTO_VERIFY_EVERY_N_WRITES);

    wait_until_verify_settles(&table).await;

    assert_eq!(
        table.verify_call_count(),
        2,
        "a subsequent bump_write_counter call must still trigger a real verify() \
         (task group 10: a panicking verify must not permanently disable the watchdog)"
    );
}
