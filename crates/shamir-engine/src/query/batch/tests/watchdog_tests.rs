//! Watchdog tests (#1094).
//!
//! Tests for the OS-thread-based watchdog that logs stuck batch ops while
//! they're still running.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use shamir_query_types::batch::{BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use shamir_query_types::call::CallOp;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::executor_traits::FunctionInvoker;
use crate::query::batch::{
    execute_batch_unchecked as execute_batch, register_op_watchdog, BatchError, IN_ITER_SYNC,
    REGISTRY, THREAD_SPAWN_COUNT,
};
use crate::query::read::QueryResult;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;

// ============================================================================
// Log capture helper
// ============================================================================

/// Simple in-memory log sink for test assertions.
#[derive(Clone)]
struct TestLogSink {
    logs: Arc<Mutex<Vec<String>>>,
}

impl TestLogSink {
    fn new() -> Self {
        Self {
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn clear(&self) {
        self.logs.lock().unwrap().clear();
    }

    fn contains(&self, pattern: &str) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|log| log.contains(pattern))
    }

    fn count_matching(&self, pattern: &str) -> usize {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|log| log.contains(pattern))
            .count()
    }
}

impl log::Log for TestLogSink {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let msg = format!("[{}] {}", record.level(), record.args());
        self.logs.lock().unwrap().push(msg);
    }

    fn flush(&self) {}
}

// ============================================================================
// Mock FunctionInvoker that can inject delays
// ============================================================================

#[derive(Clone)]
struct SlowFunctionInvoker {
    delay: Duration,
}

impl SlowFunctionInvoker {
    fn new(delay: Duration) -> Self {
        Self { delay }
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for SlowFunctionInvoker {
    async fn invoke_call(
        &self,
        op: &CallOp,
        _actor: &Actor,
        _resolved_refs: &shamir_types::types::common::TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        // Simulate a slow function by sleeping before returning.
        tokio::time::sleep(self.delay).await;
        // Return a dummy result.
        Ok(QueryResult {
            value: Some(shamir_types::types::value::QueryValue::Bin(
                format!("called {}", op.call).into_bytes(),
            )),
            records: vec![],
            stats: None,
            pagination: None,
            explain: None,
            skipped: false,
            versions: None,
            corrupt_records: vec![],
            op_id: None,
            ddl_status: None,
        })
    }
}

// ============================================================================
// Test resolver setup
// ============================================================================

struct TestResolver {
    db: DbInstance,
    repo: String,
}

#[async_trait::async_trait]
impl crate::query::batch::TableResolver for TestResolver {
    async fn resolve(
        &self,
        table_ref: &TableRef,
    ) -> shamir_storage::error::DbResult<crate::table::TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }
}

async fn setup_resolver() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver {
        db,
        repo: "default".to_string(),
    }
}

// ============================================================================
// Thread leak regression tests (#1105)
// ============================================================================

/// Regression test for #1105: verify that multiple `register_op_watchdog`
/// calls within the same process spawn exactly ONE watchdog thread, not N.
///
/// The buggy code (before fix) spawned a new OS thread on EVERY call to
/// `init_watchdog()` because `std::thread::spawn` ran before the
/// `OnceLock::set` dedup. The fixed code uses `OnceLock::get_or_init` to
/// guarantee the spawn closure runs at most once.
///
/// This test MUST fail on the buggy code (counter > 1) and pass after the
/// fix (counter == 1).
#[test]
fn watchdog_spawn_count_regression_test() {
    // Reset the counter for clean test state.
    // Note: we don't have a public reset API for the static counters,
    // but since this test runs in its own process (nextest isolation),
    // we start from zero.

    // Simulate the production usage pattern: register multiple ops in rapid succession.
    // Each call triggers `init_watchdog()`, but only the first should spawn a thread.
    let guards: Vec<_> = (0..100)
        .map(|i| register_op_watchdog(&format!("op_{}", i)))
        .collect();

    // Drop the guards to clean up registry entries.
    drop(guards);

    // Wait a moment for any cleanup to complete.
    std::thread::sleep(Duration::from_millis(100));

    // Verify: exactly ONE thread was spawned, not 100.
    // The counter is incremented inside the `get_or_init` closure,
    // which is guaranteed by `OnceLock` to run at most once.
    let spawn_count = THREAD_SPAWN_COUNT.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        spawn_count, 1,
        "Expected exactly ONE watchdog thread to be spawned after 100 registration calls, got {}. \
         If this fails, the thread leak bug (#1105) is NOT fixed.",
        spawn_count
    );
}

// ============================================================================
// Bookkeeping tests (from round 1)
// ============================================================================

#[test]
fn test_watchdog_initialization() {
    // Test that the watchdog can be initialized and that registration works.
    let _guard = register_op_watchdog("test_op");

    // The registry should be initialized.
    assert!(REGISTRY.get().is_some());

    // Wait a bit for the guard to drop.
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn test_multiple_registrations() {
    // Test that multiple ops can be registered concurrently.
    let guard1 = register_op_watchdog("op1");
    let guard2 = register_op_watchdog("op2");
    let guard3 = register_op_watchdog("op3");

    // The registry should be initialized.
    let registry = REGISTRY.get().unwrap();

    // Count entries.
    let mut count = 0;
    registry.iter_sync(|_, _| {
        count += 1;
        true
    });

    // We should have at least 3 entries (there might be other ops from other tests).
    assert!(count >= 3, "Expected at least 3 entries, got {}", count);

    // Drop the guards.
    drop(guard1);
    drop(guard2);
    drop(guard3);

    // Wait for cleanup.
    std::thread::sleep(Duration::from_millis(100));

    // Count entries again.
    let mut count_after = 0;
    registry.iter_sync(|_, _| {
        count_after += 1;
        true
    });

    // The count should be reduced by 3.
    assert!(
        count_after <= count - 3 + 1,
        // +1 to allow for potential new registrations from concurrent tests
        "Expected count to be reduced by 3, was {} -> {}",
        count,
        count_after
    );
}

// ============================================================================
// Behavioral tests (round 2 additions)
// ============================================================================

/// Core test: verify watchdog warning fires WHILE a slow op is still running.
///
/// This test drives a batch through the real `execute_batch` path with a
/// deliberately slow function call (2.5s, well over the 1s threshold).
/// The watchdog's warning must appear BEFORE the op completes.
#[tokio::test]
async fn watchdog_warns_while_slow_op_still_running() {
    let resolver = setup_resolver().await;
    let log_sink = TestLogSink::new();

    // Set up a custom logger that captures to our in-memory sink.
    let _ = log::set_boxed_logger(Box::new(log_sink.clone()));
    log::set_max_level(log::LevelFilter::Warn);

    // Clear any pre-existing logs.
    log_sink.clear();

    // Create a batch with a single slow function call.
    // Use 2.5s delay: > 1s threshold, < 180s nextest kill.
    let invoker = SlowFunctionInvoker::new(Duration::from_millis(2500));

    let mut queries = new_map();
    queries.insert(
        "slow_call".to_string(),
        QueryEntry {
            op: BatchOp::Call(CallOp {
                call: "slow_function".to_string(),
                params: vec![],
                repo: "default".to_string(),
            }),
            return_result: true,
            after: vec![],
            when: None,
        },
    );

    let req = BatchRequest {
        id: shamir_types::types::value::QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: shamir_query_types::batch::BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Spawn a task that will signal when the op completes.
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let resolver = Arc::new(resolver);
    let invoker = Arc::new(invoker);

    tokio::spawn(async move {
        let result = execute_batch(
            &req,
            resolver.as_ref() as &dyn crate::query::batch::TableResolver,
            None,
            Some(invoker.as_ref()),
            Actor::System,
            "test",
        )
        .await;
        let _ = completion_tx.send(result);
    });

    // Wait for the watchdog to fire (it polls every 1s, threshold is 1s).
    // After 1.2s, the watchdog should have warned.
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Assert: the warning must have appeared by now, BEFORE the op completes.
    assert!(
        log_sink.contains("slow_call"),
        "Watchdog warning should appear before slow op completes. Logs: {:?}",
        log_sink.logs.lock().unwrap()
    );

    // Wait for the op to actually complete (it takes 2.5s total).
    let result = tokio::time::timeout(Duration::from_secs(5), completion_rx)
        .await
        .expect("Slow op should complete within 5s")
        .expect("Batch execution should succeed");

    // Verify the batch actually succeeded.
    assert!(result.is_ok(), "Batch should succeed: {:?}", result);

    // Final verification: the warning should contain "still running".
    assert!(
        log_sink.contains("still running"),
        "Warning should contain 'still running'. Logs: {:?}",
        log_sink.logs.lock().unwrap()
    );
}

/// False-positive test: verify the watchdog does NOT fire for fast ops.
///
/// This test runs a normal fast operation (well under the 1s threshold)
/// through the real `execute_batch` path and asserts NO watchdog warning.
#[tokio::test]
async fn watchdog_no_false_positive_for_fast_ops() {
    let resolver = setup_resolver().await;
    let log_sink = TestLogSink::new();

    // Set up log capture.
    let _ = log::set_boxed_logger(Box::new(log_sink.clone()));
    log::set_max_level(log::LevelFilter::Warn);

    log_sink.clear();

    // Create a batch with a very fast function call (10ms).
    let invoker = SlowFunctionInvoker::new(Duration::from_millis(10));

    let mut queries = new_map();
    queries.insert(
        "fast_call".to_string(),
        QueryEntry {
            op: BatchOp::Call(CallOp {
                call: "fast_function".to_string(),
                params: vec![],
                repo: "default".to_string(),
            }),
            return_result: true,
            after: vec![],
            when: None,
        },
    );

    let req = BatchRequest {
        id: shamir_types::types::value::QueryValue::Int(2),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: shamir_query_types::batch::BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Execute the batch.
    let result = execute_batch(&req, &resolver, None, Some(&invoker), Actor::System, "test").await;

    assert!(result.is_ok(), "Fast batch should succeed: {:?}", result);

    // Wait a bit to ensure the watchdog thread has had time to poll.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Assert: NO warning should appear for fast ops.
    assert!(
        !log_sink.contains("fast_call"),
        "Watchdog should NOT warn about fast ops. Logs: {:?}",
        log_sink.logs.lock().unwrap()
    );

    assert!(
        !log_sink.contains("still running"),
        "No 'still running' warning should appear. Logs: {:?}",
        log_sink.logs.lock().unwrap()
    );
}

/// Once-only test: verify a single stuck op produces exactly ONE warning.
///
/// This test runs an op that takes 3.5s (spanning multiple poll intervals
/// of 1s) and asserts the watchdog warns exactly ONCE, not 3-4 times.
#[tokio::test]
async fn watchdog_warns_exactly_once_per_stuck_op() {
    let resolver = setup_resolver().await;
    let log_sink = TestLogSink::new();

    // Set up log capture.
    let _ = log::set_boxed_logger(Box::new(log_sink.clone()));
    log::set_max_level(log::LevelFilter::Warn);

    log_sink.clear();

    // Create a batch with a slow function call (3.5s).
    // This spans multiple 1s poll intervals but should trigger only ONE warning.
    let invoker = SlowFunctionInvoker::new(Duration::from_millis(3500));

    let mut queries = new_map();
    queries.insert(
        "stuck_call".to_string(),
        QueryEntry {
            op: BatchOp::Call(CallOp {
                call: "stuck_function".to_string(),
                params: vec![],
                repo: "default".to_string(),
            }),
            return_result: true,
            after: vec![],
            when: None,
        },
    );

    let req = BatchRequest {
        id: shamir_types::types::value::QueryValue::Int(3),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: shamir_query_types::batch::BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Execute the batch.
    let result = execute_batch(&req, &resolver, None, Some(&invoker), Actor::System, "test").await;

    assert!(result.is_ok(), "Slow batch should succeed: {:?}", result);

    // Wait a bit after completion to ensure no late warnings arrive.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Assert: exactly ONE watchdog warning for the stuck op.
    // Note: we filter to only count "still running" warnings from the watchdog,
    // not the post-completion "took Xs to execute" warning from warn_if_op_slow.
    let warning_count = log_sink.count_matching("still running");
    assert_eq!(
        warning_count,
        1,
        "Expected exactly ONE watchdog warning for stuck op, got {}. Logs: {:?}",
        warning_count,
        log_sink.logs.lock().unwrap()
    );

    assert!(
        log_sink.contains("still running"),
        "Warning should contain 'still running'. Logs: {:?}",
        log_sink.logs.lock().unwrap()
    );
}

/// Regression test (#1200): the watchdog never does `log::warn!` I/O
/// while `iter_sync` holds `scc`'s per-bucket lock.
///
/// `IN_ITER_SYNC` (test-only) is bracketed EXACTLY around the
/// `iter_sync` call in the watchdog thread — `true` while the scan is
/// in progress (bucket lock held), `false` once it returns. This spy
/// `log::Log` impl checks that flag at the instant it receives a "still
/// running" record: if the fix regresses back to calling `log::warn!`
/// FROM INSIDE the `iter_sync` closure, this observes `IN_ITER_SYNC ==
/// true` and fails. Deterministic, not timing-flaky: both the flag
/// flips and the log calls happen sequentially on the SAME watchdog OS
/// thread, so there is no cross-thread race to land in.
#[test]
fn watchdog_log_emitted_outside_iter_sync_lock_scope() {
    struct LockScopeSpy {
        violation: Arc<std::sync::atomic::AtomicBool>,
        observed_any: Arc<std::sync::atomic::AtomicBool>,
    }

    impl log::Log for LockScopeSpy {
        fn enabled(&self, _metadata: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            let msg = format!("{}", record.args());
            if msg.contains("still running") {
                self.observed_any
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                if IN_ITER_SYNC.load(std::sync::atomic::Ordering::SeqCst) {
                    self.violation
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }

        fn flush(&self) {}
    }

    let violation = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed_any = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let _ = log::set_boxed_logger(Box::new(LockScopeSpy {
        violation: Arc::clone(&violation),
        observed_any: Arc::clone(&observed_any),
    }));
    log::set_max_level(log::LevelFilter::Warn);

    // Register a stuck op and let it sit past the 1s watchdog poll +
    // warn threshold so the watchdog thread scans and warns about it.
    let guard = register_op_watchdog("lock_scope_probe");

    std::thread::sleep(Duration::from_millis(1500));

    assert!(
        observed_any.load(std::sync::atomic::Ordering::SeqCst),
        "expected the watchdog to have warned about the stuck op by now"
    );
    assert!(
        !violation.load(std::sync::atomic::Ordering::SeqCst),
        "watchdog emitted a log record while IN_ITER_SYNC was true — \
         log I/O ran inside the iter_sync bucket-lock scope"
    );

    drop(guard);
}

/// Integration test: watchdog with normal read/write ops (non-Call).
///
/// This test verifies the watchdog works correctly with ordinary table
/// operations, not just function calls. It runs a mix of fast ops and
/// should produce no warnings.
#[tokio::test]
async fn watchdog_no_warnings_for_normal_ops() {
    use shamir_query_builder::batch::Batch;
    use shamir_query_builder::query::Query;
    use shamir_query_builder::write::doc;

    let resolver = setup_resolver().await;
    let log_sink = TestLogSink::new();

    // Set up log capture.
    let _ = log::set_boxed_logger(Box::new(log_sink.clone()));
    log::set_max_level(log::LevelFilter::Warn);

    log_sink.clear();

    // Seed data.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "insert",
        shamir_query_builder::write::insert("users")
            .row(doc().set("name", "Alice").set("age", 30))
            .row(doc().set("name", "Bob").set("age", 25)),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Read back.
    let mut b = Batch::new();
    b.id(2);
    b.query("users", Query::from("users"));
    let req = b.build();

    let result = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(result.results["users"].records.len(), 2);

    // Wait for any potential watchdog polling.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Assert: NO warnings for fast normal ops.
    assert!(
        !log_sink.contains("still running"),
        "No 'still running' warnings should appear for fast normal ops. Logs: {:?}",
        log_sink.logs.lock().unwrap()
    );
}
