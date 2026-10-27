use crate::{FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::{Arc, Barrier};
use std::thread;

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

#[test]
fn rv_wasm_host_c51052df_replace_never_exposes_missing_name() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("key", Arc::new(Const(0))).unwrap();
    let start = Arc::new(Barrier::new(2));
    let writer_registry = Arc::clone(&registry);
    let writer_start = Arc::clone(&start);
    let writer = thread::spawn(move || {
        writer_start.wait();
        for value in 1..=100_000 {
            writer_registry.replace("key", Arc::new(Const(value)));
        }
    });
    start.wait();
    for _ in 0..100_000 {
        assert!(
            registry.get("key").is_some(),
            "replace exposed a missing name"
        );
    }
    writer.join().expect("writer thread panicked");
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
