use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_storage::error::{DbError, DbResult};
use tokio::task::JoinSet;

use crate::repo::group_commit::GroupCommit;

/// Correctness note: `GroupCommit::run` only returns `Ok` to a caller after a
/// flush that BEGAN after that caller registered its oneshot. This means any
/// writes buffered before the `run` call are guaranteed to be on disk when the
/// caller's future resolves — the structural durability invariant.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batches_concurrent_flushes() {
    let gc = Arc::new(GroupCommit::new());
    let flush_count = Arc::new(AtomicUsize::new(0));

    let n = 12usize;
    let mut js = JoinSet::new();

    for _ in 0..n {
        let gc = Arc::clone(&gc);
        let flush_count = Arc::clone(&flush_count);
        js.spawn(async move {
            gc.run(|| {
                let flush_count = Arc::clone(&flush_count);
                async move {
                    flush_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(())
                }
            })
            .await
        });
    }

    let mut results = Vec::new();
    while let Some(res) = js.join_next().await {
        results.push(res.expect("task panicked"));
    }

    // Every caller must succeed.
    for r in &results {
        assert!(r.is_ok(), "expected Ok, got {:?}", r);
    }

    let count = flush_count.load(Ordering::SeqCst);
    assert!(count >= 1, "at least one flush must run; got {}", count);
    assert!(
        count < n,
        "batching should reduce flush count below N={}; got {}",
        n,
        count
    );

    eprintln!("batches_concurrent_flushes: N={}, flush_count={}", n, count);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_caller_flushes_once() {
    let gc = GroupCommit::new();
    let flush_count = Arc::new(AtomicUsize::new(0));

    let fc = Arc::clone(&flush_count);
    let result = gc
        .run(|| {
            let fc = Arc::clone(&fc);
            async move {
                fc.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(flush_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn propagates_flush_error() {
    let gc = GroupCommit::new();

    let result: DbResult<()> = gc
        .run(|| async { Err(DbError::Internal("boom".into())) })
        .await;

    let err = result.expect_err("expected error");
    let msg = err.to_string();
    assert!(
        msg.contains("boom"),
        "error message should contain 'boom', got: {}",
        msg
    );
}
