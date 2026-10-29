//! Concurrent replacement sampling starts after at least 1,000 reader lookups.
//! During the old function's drop, a remove-then-insert gap permits 1,000 more
//! lookups; a held bucket instead stalls the reader until 500 ms without progress.
//! Drop also has an absolute 10-second limit. This overwhelmingly exposes a gap,
//! but is not deterministic: a 500 ms reader deschedule can hide it.

use crate::{FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// A trivial function that ignores its inputs and returns a constant.
struct Const(i64);

#[async_trait]
impl ShamirFunction for Const {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Int(self.0))
    }
}

// The reader never owns the registered function, so replacement drops Gate
// while a remove-then-insert implementation would expose the missing name.
struct Gate {
    lookups: Arc<AtomicU64>,
}

#[async_trait]
impl ShamirFunction for Gate {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Null)
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        let before = self.lookups.load(Ordering::SeqCst);
        let start = Instant::now();
        let mut last_progress = start;
        let mut last = before;
        loop {
            let current = self.lookups.load(Ordering::SeqCst);
            let now = Instant::now();
            if current != last {
                last = current;
                last_progress = now;
            }
            if current.wrapping_sub(before) >= 1_000
                || now.duration_since(last_progress) >= Duration::from_millis(500)
                || now.duration_since(start) >= Duration::from_secs(10)
            {
                break;
            }
            thread::yield_now();
        }
    }
}

#[test]
fn rv_wasm_host_c51052df_replace_never_exposes_missing_name() {
    let registry = Arc::new(FunctionRegistry::new());
    let lookups = Arc::new(AtomicU64::new(0));
    let saw_missing = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    registry
        .register(
            "key",
            Arc::new(Gate {
                lookups: Arc::clone(&lookups),
            }),
        )
        .unwrap();
    let reader_registry = Arc::clone(&registry);
    let reader_lookups = Arc::clone(&lookups);
    let reader_missing = Arc::clone(&saw_missing);
    let reader_stop = Arc::clone(&stop);
    let reader = thread::spawn(move || {
        while !reader_stop.load(Ordering::SeqCst) {
            if !reader_registry.contains("key") {
                reader_missing.store(true, Ordering::SeqCst);
            }
            reader_lookups.fetch_add(1, Ordering::SeqCst);
        }
        let _ = done_tx.send(());
    });
    let wait_for = |threshold: u64| {
        let start = Instant::now();
        while lookups.load(Ordering::SeqCst) < threshold {
            if start.elapsed() >= Duration::from_secs(5) {
                return false;
            }
            thread::yield_now();
        }
        true
    };
    let ready = wait_for(1_000);
    let replacement: Arc<dyn ShamirFunction> = Arc::new(Const(1));
    let progressed = if ready {
        registry.replace("key", Arc::clone(&replacement));
        let after_return = lookups.load(Ordering::SeqCst);
        wait_for(after_return.saturating_add(1))
    } else {
        false
    };
    stop.store(true, Ordering::SeqCst);
    let done = done_rx.recv_timeout(Duration::from_secs(5));
    let joined = if done.is_ok() {
        Some(reader.join())
    } else {
        None
    };
    assert!(
        ready,
        "reader did not complete 1000 lookups before replacement"
    );
    assert!(
        progressed,
        "reader did not advance after replacement returned"
    );
    assert!(done.is_ok(), "reader did not finish: {done:?}");
    assert!(
        joined.is_some_and(|result| result.is_ok()),
        "reader panicked"
    );
    let found = registry.get("key").expect("replacement is missing");
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&found),
        Arc::as_ptr(&replacement)
    ));
    assert!(
        !saw_missing.load(Ordering::SeqCst),
        "reader saw a missing name"
    );
}

#[test]
fn rv_wasm_host_eae6abe8_rename_never_drops_function() {
    let registry = Arc::new(FunctionRegistry::new());
    let original: Arc<dyn ShamirFunction> = Arc::new(Const(1));
    registry.register("a", original.clone()).unwrap();
    let observed = Arc::new(OnceLock::new());
    let observed_hook = Arc::clone(&observed);
    let original_hook = original.clone();
    assert!(registry
        .interleave
        .set(Box::new(move |registry| {
            // Mid-rename both names resolve to the original; neither is free.
            let same = |name: &str| {
                registry.get(name).is_some_and(|f| {
                    std::ptr::addr_eq(Arc::as_ptr(&f), Arc::as_ptr(&original_hook))
                })
            };
            let a_taken = matches!(
                registry.register("a", Arc::new(Const(2))),
                Err(FunctionError::AlreadyExists(_))
            );
            let b_taken = matches!(
                registry.register("b", Arc::new(Const(3))),
                Err(FunctionError::AlreadyExists(_))
            );
            let _ = observed_hook.set(same("a") && same("b") && a_taken && b_taken);
        }))
        .is_ok());

    assert!(registry.rename("a", "b").is_ok());
    assert_eq!(
        observed.get(),
        Some(&true),
        "function absent or replaceable mid-rename"
    );
    let target = registry.get("b").expect("renamed target remains");
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&target),
        Arc::as_ptr(&original)
    ));
    assert!(registry.get("a").is_none());
}

#[test]
fn rv_wasm_host_eae6abe8_rename_rollback_preserves_concurrent_replace() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("a", Arc::new(Const(1))).unwrap();
    let replacement_a: Arc<dyn ShamirFunction> = Arc::new(Const(4));
    let replacement_b: Arc<dyn ShamirFunction> = Arc::new(Const(5));
    let replacement_a_hook = replacement_a.clone();
    let replacement_b_hook = replacement_b.clone();
    assert!(registry
        .interleave
        .set(Box::new(move |registry| {
            registry.replace("a", replacement_a_hook.clone());
            registry.replace("b", replacement_b_hook.clone());
        }))
        .is_ok());

    let result = registry.rename("a", "b");
    assert!(matches!(result, Err(FunctionError::NotFound(name)) if name == "a"));
    let found_a = registry
        .get("a")
        .expect("concurrent source replacement remains");
    let found_b = registry
        .get("b")
        .expect("concurrent target replacement remains");
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&found_a),
        Arc::as_ptr(&replacement_a)
    ));
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&found_b),
        Arc::as_ptr(&replacement_b)
    ));
}

#[test]
fn register_get_remove() {
    let reg = FunctionRegistry::new();
    assert!(reg.is_empty());
    reg.register("a", Arc::new(Const(1))).unwrap();
    assert!(reg.contains("a"));
    assert!(reg.get("a").is_some());
    assert_eq!(reg.len(), 1);
    assert!(reg.remove("a"));
    assert!(!reg.contains("a"));
    assert!(!reg.remove("a"));
}

#[test]
fn register_duplicate_errors() {
    let reg = FunctionRegistry::new();
    reg.register("a", Arc::new(Const(1))).unwrap();
    let err = reg.register("a", Arc::new(Const(2))).unwrap_err();
    assert!(matches!(err, FunctionError::AlreadyExists(ref n) if n == "a"));
}

#[tokio::test]
async fn replace_changes_behaviour() {
    let reg = FunctionRegistry::new();
    reg.register("a", Arc::new(Const(1))).unwrap();
    let before = reg
        .invoke("a", &FnCtx::new(), &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(before, QueryValue::Int(1));

    reg.replace("a", Arc::new(Const(2)));
    assert_eq!(reg.len(), 1);
    let after = reg
        .invoke("a", &FnCtx::new(), &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(after, QueryValue::Int(2));
}

#[test]
fn rename_rekeys() {
    let reg = FunctionRegistry::new();
    reg.register("a", Arc::new(Const(7))).unwrap();
    reg.rename("a", "b").unwrap();
    assert!(!reg.contains("a"));
    assert!(reg.contains("b"));
}

#[test]
fn rename_missing_errors() {
    let reg = FunctionRegistry::new();
    let err = reg.rename("nope", "b").unwrap_err();
    assert!(matches!(err, FunctionError::NotFound(_)));
}

#[test]
fn rename_to_taken_errors_and_keeps_source() {
    let reg = FunctionRegistry::new();
    reg.register("a", Arc::new(Const(1))).unwrap();
    reg.register("b", Arc::new(Const(2))).unwrap();
    let err = reg.rename("a", "b").unwrap_err();
    assert!(matches!(err, FunctionError::AlreadyExists(_)));
    assert!(reg.contains("a"), "source must be untouched on collision");
    assert!(reg.contains("b"));
}

#[tokio::test]
async fn invoke_unknown_errors() {
    let reg = FunctionRegistry::new();
    let err = reg
        .invoke("ghost", &FnCtx::new(), &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(err, FunctionError::NotFound(_)));
}

#[test]
fn builtins_include_argon2id() {
    let reg = FunctionRegistry::with_builtins();
    assert!(reg.list().iter().any(|n| n == "argon2id"));
}
