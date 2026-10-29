//! Setter coverage: concurrent sampling and a bounded, timing-based drop probe.

use crate::{FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

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

// Assignment drops the old Arc while the occupied entry guard is held. A
// remove-then-insert implementation instead lets the reader see the gap;
// scheduling delays can make this timing-based negative control miss it.
struct Gate {
    ready: Mutex<mpsc::Receiver<()>>,
    release: mpsc::SyncSender<()>,
    done: Mutex<mpsc::Receiver<bool>>,
    observed: mpsc::SyncSender<(
        Result<(), mpsc::RecvTimeoutError>,
        Result<bool, mpsc::RecvTimeoutError>,
    )>,
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
        let ready = self
            .ready
            .get_mut()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            .and_then(|receiver| receiver.recv_timeout(Duration::from_secs(5)));
        let _ = self.release.send(());
        let result = self
            .done
            .get_mut()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            .and_then(|receiver| receiver.recv_timeout(Duration::from_millis(300)));
        let _ = self.observed.send((ready, result));
    }
}

#[test]
fn rv_wasm_host_c51052df_replace_never_exposes_missing_name() {
    let registry = Arc::new(FunctionRegistry::new());
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let (observed_tx, observed_rx) = mpsc::sync_channel(1);
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    registry
        .register(
            "key",
            Arc::new(Gate {
                ready: Mutex::new(ready_rx),
                release: release_tx,
                done: Mutex::new(done_rx),
                observed: observed_tx,
            }),
        )
        .unwrap();
    let reader_registry = Arc::clone(&registry);
    let reader = thread::spawn(move || {
        let _ = ready_tx.send(());
        if release_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
            let present = reader_registry.get("key").is_some();
            let _ = done_tx.send(present);
            let _ = finished_tx.send(Some(present));
        } else {
            let _ = finished_tx.send(None);
        }
    });
    registry.replace("key", Arc::new(Const(1)));
    let observed = observed_rx.recv_timeout(Duration::from_secs(5));
    let finished = finished_rx.recv_timeout(Duration::from_secs(5));
    let joined = if finished.is_ok() {
        Some(reader.join())
    } else {
        None
    };
    let (ready, done) = observed.expect("gate did not drop");
    assert!(ready.is_ok(), "reader was not ready: {ready:?}");
    assert!(
        matches!(done, Err(mpsc::RecvTimeoutError::Timeout)),
        "reader completed or disconnected during replacement: {done:?}"
    );
    assert_eq!(
        finished.expect("reader did not finish"),
        Some(true),
        "reader saw a missing name or was not released"
    );
    assert!(
        joined.expect("reader was not joined").is_ok(),
        "reader panicked"
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
