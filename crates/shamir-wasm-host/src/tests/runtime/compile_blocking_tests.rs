use super::compile_tests::DOUBLE_SOURCE;
use crate::wasm::run_compile_blocking;
use crate::{compile_rust_source_blocking, FunctionError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ============================================================================
// RV-DB-B40771A1 — compile_rust_source_blocking (spawn_blocking wrapper)
// ============================================================================

/// Shared ticker helper: increments `counter` every 1 ms while `run` is set.
/// Spawned on the current-thread runtime BEFORE the blocking call is awaited;
/// a frozen counter during the await proves the runtime was starved.
async fn ticker_task(run: Arc<AtomicBool>, counter: Arc<AtomicU64>) {
    while run.load(Ordering::Relaxed) {
        counter.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

/// `compile_rust_source_blocking` must offload the (up to 120 s) cargo build
/// via `spawn_blocking` instead of running it inline on the async caller's
/// thread. On a `current_thread` runtime, an inline call would freeze the
/// ticker task at its pre-compile snapshot; the spawn_blocking wrapper lets
/// it keep ticking.
#[tokio::test(flavor = "current_thread")]
async fn rv_db_b40771a1_blocking_compile_does_not_starve_current_thread_runtime() {
    let run = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn(ticker_task(Arc::clone(&run), Arc::clone(&counter)));

    let snapshot = counter.load(Ordering::Relaxed);
    let res = compile_rust_source_blocking(DOUBLE_SOURCE).await;

    match res {
        Ok(_wasm) => {
            assert!(
                counter.load(Ordering::Relaxed) > snapshot,
                "ticker must advance during the blocking compile — the \
                 compile ran inline on the async thread (no spawn_blocking)"
            );
        }
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            run.store(false, Ordering::Relaxed);
            ticker.await.expect("ticker task should terminate");
            eprintln!(
                "SKIP rv_db_b40771a1_blocking_compile_does_not_starve_current_thread_runtime: {msg}"
            );
            return;
        }
        Err(e) => panic!("blocking compile failed: {e}"),
    }

    run.store(false, Ordering::Relaxed);
    ticker.await.expect("ticker task should terminate");
}

/// Toolchain-independent variant: `run_compile_blocking` must execute its
/// closure on the blocking thread pool, not inline on the async caller's
/// thread. A 50 ms `std::thread::sleep` inside the closure would freeze the
/// current-thread runtime if inlined; the ticker must still advance.
#[tokio::test(flavor = "current_thread")]
async fn rv_db_b40771a1_run_compile_blocking_does_not_starve_current_thread_runtime() {
    let run = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn(ticker_task(Arc::clone(&run), Arc::clone(&counter)));

    let snapshot = counter.load(Ordering::Relaxed);
    let res: Result<u64, FunctionError> = run_compile_blocking(|| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        Ok(42u64)
    })
    .await;

    assert_eq!(res.expect("run_compile_blocking should succeed"), 42);
    assert!(
        counter.load(Ordering::Relaxed) > snapshot,
        "ticker must advance during the blocking closure — the closure ran \
         inline on the async thread (no spawn_blocking)"
    );

    run.store(false, Ordering::Relaxed);
    ticker.await.expect("ticker task should terminate");
}
