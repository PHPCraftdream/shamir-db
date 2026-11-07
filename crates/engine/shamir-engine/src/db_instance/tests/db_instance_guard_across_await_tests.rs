//! P0 group 3 regression: `DbInstance::get_table` (and, by the same code
//! shape, all seven index-routing methods) held the `dashmap::Ref` shard
//! guard on `self.repos` across the delegated `.await` into
//! `RepoInstance::get_table`. Same structural class as the already-fixed
//! `RepoInstance::get_table` hazard (commit 91be98b6) and the H4+H5
//! `per_table_mvcc`/`token_names` class
//! (`repo/tests/repo_instance_tests.rs`) — a synchronous shard
//! `std::sync::RwLock` read guard held over a long-running async init lets
//! a concurrent EXCLUSIVE writer on the SAME shard
//! (`add_repo`/`remove_repo`/`rename_repo`) block its tokio worker thread
//! synchronously; enough such blocked workers wedge the runtime.
//!
//! **Reproduction shape.** `remove_repo`/`add_repo` on the exact SAME repo
//! name as a concurrent `get_table` call are GUARANTEED to hash to the same
//! DashMap shard (shard selection is a pure function of the key), so this
//! is not a "maybe the hammer gets lucky" hazard on the shard-selection
//! axis — the only remaining question is whether the reader's critical
//! section (from `self.repos.get()` to the return of the delegated
//! `.await`) actually yields control back to the scheduler at least once
//! before completing, giving the writer a window to observe the guard
//! still held. Mirrors the `per_table_mvcc_concurrent_ddl_gc_no_deadlock`
//! good-faith hammer pattern (`repo/tests/repo_instance_tests.rs`):
//! `worker_threads = 2` (smallest oversubscribed runtime), many iterations,
//! `tokio::time::timeout` turning a real regression into a fast, NAMED
//! failure instead of an anonymous nextest TIMEOUT.
//!
//! Pre-fix: a `remove_repo`/`add_repo` writer landing on the same shard
//! while a `get_table` reader's guard is alive blocks its worker thread
//! synchronously until the reader's ENTIRE cold-open (`store_get` x3 +
//! `tx_gate` + `repo_interner` + `TableManager::create`) completes; with
//! only 2 workers and 6 concurrent hammers, this reliably starves the
//! runtime within the timeout window.
//! Post-fix: `get_table` clones the `RepoInstance` out and drops the guard
//! before the delegated `.await`, so a concurrent writer on the same shard
//! is never blocked by a reader's cold-open — the hammer completes almost
//! immediately.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::db_instance::db_instance::DbInstance;
use crate::repo::{BoxRepoFactory, RepoConfig};
use crate::table::TableConfig;

const HAMMERS: usize = 6;
const ITERS: usize = 300;
const REPOS: &[&str] = &["r_ddl_0", "r_ddl_1", "r_ddl_2"];

fn repo_config(name: &str) -> RepoConfig {
    RepoConfig::new(name, BoxRepoFactory::in_memory()).add_table(TableConfig::new("t"))
}

async fn make_db() -> DbInstance {
    let db = DbInstance::new();
    for name in REPOS {
        db.add_repo(repo_config(name)).await.unwrap();
    }
    db
}

/// Reader hammer: `get_table` cold-opens/re-opens tables on the shared repo
/// names — exercises `DbInstance::get_table`'s `self.repos.get(repo_name)`
/// guard on exactly the keys the DDL hammer targets for write access.
async fn get_table_reader_hammer(db: Arc<DbInstance>, stop: Arc<AtomicBool>) {
    for i in 0..ITERS {
        let name = REPOS[i % REPOS.len()];
        // NotFound is expected when a concurrent remove_repo wins the race;
        // the DDL hammer re-adds it immediately after.
        let _ = db.get_table(name, "t").await;
        tokio::task::yield_now().await;
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// DDL hammer: `remove_repo` (EXCLUSIVE DashMap write on the SAME shard as
/// the reader's key — shard is a pure function of the key, so this is a
/// guaranteed collision, not a probabilistic one) immediately followed by
/// `add_repo` re-registering it so the reader hammer can keep making cold
/// opens across iterations instead of hitting a permanently-missing repo.
async fn repo_ddl_hammer(db: Arc<DbInstance>, stop: Arc<AtomicBool>) {
    for i in 0..ITERS {
        let name = REPOS[i % REPOS.len()];
        let _ = db.remove_repo(name).await;
        db.add_repo(repo_config(name)).await.unwrap();
        tokio::task::yield_now().await;
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Regression: hammer `get_table` cold-opens concurrently with
/// `remove_repo`/`add_repo` DDL on the SAME repo names, on a two-worker
/// runtime. See module doc for the full mechanism and confidence caveat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_table_guard_does_not_block_concurrent_repo_ddl() {
    let db = Arc::new(make_db().await);
    let stop = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::with_capacity(HAMMERS);
    for i in 0..HAMMERS {
        let d = Arc::clone(&db);
        let s = Arc::clone(&stop);
        handles.push(tokio::spawn(async move {
            if i % 2 == 0 {
                get_table_reader_hammer(d, s).await;
            } else {
                repo_ddl_hammer(d, s).await;
            }
        }));
    }

    tokio::time::timeout(Duration::from_secs(15), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "DbInstance::get_table vs concurrent add_repo/remove_repo deadlocked \
         — this is the DashMap shard-guard-held-across-await hazard \
         (`self.repos.get(repo_name)` held across the delegated \
         `repo_manager.get_table(..).await`). get_table MUST clone the \
         RepoInstance out and drop the guard BEFORE the awaited call, \
         exactly like RepoInstance::get_table already does \
         (repo/repo_instance.rs) and DbInstance::get_repo already does \
         (db_instance/db_instance.rs).",
    );

    stop.store(true, Ordering::Relaxed);
}
