//! Concurrent replacement sampling starts after at least 1,000 reader lookups.
//! During the old function's drop, a remove-then-insert gap permits 1,000 more
//! lookups; a held bucket instead stalls the reader until 500 ms without progress.
//! Drop also has an absolute 10-second limit. This overwhelmingly exposes a gap,
//! but is not deterministic: a 500 ms reader deschedule can hide it.

use crate::{
    FnBatch, FnCtx, FunctionError, FunctionMeta, FunctionRegistry, Params, ResolvedStoredCall,
    Security, ShamirFunction, StoredCallResolver, Visibility,
};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn checked_metadata_roundtrip_and_rejection() {
    let mut record = QueryValue::Map(Default::default());
    let QueryValue::Map(ref mut fields) = record else {
        unreachable!()
    };
    fields.insert("version".into(), QueryValue::Int(37));
    let legacy = FunctionMeta::try_from_record(&record).unwrap();
    assert_eq!(legacy.visibility, Visibility::Private);
    assert_eq!(legacy.security, Security::Invoker);
    assert!(legacy.secret_grants.is_empty() && legacy.net_grants.is_empty());
    let meta = FunctionMeta::new(
        Visibility::Public,
        Security::Definer,
        vec!["secret".into()],
        vec!["example.org".into()],
    );
    meta.inject_into(&mut record);
    let decoded = FunctionMeta::try_from_record(&record).unwrap();
    assert_eq!(decoded.visibility, meta.visibility);
    assert_eq!(decoded.security, meta.security);
    assert_eq!(decoded.secret_grants, meta.secret_grants);
    assert_eq!(decoded.net_grants, meta.net_grants);
    assert_eq!(record["version"], QueryValue::Int(37));
    assert!(FunctionMeta::try_from_record(&QueryValue::Null).is_err());
    for (field, value) in [
        ("metadata_version", QueryValue::Int(2)),
        ("visibility", QueryValue::Str("invalid".into())),
        ("security", QueryValue::Int(1)),
        ("secret_grants", QueryValue::List(vec![QueryValue::Int(1)])),
        ("net_grants", QueryValue::Bool(true)),
    ] {
        let mut bad = record.clone();
        let QueryValue::Map(ref mut fields) = bad else {
            unreachable!()
        };
        fields.insert(field.into(), value);
        assert!(FunctionMeta::try_from_record(&bad).is_err(), "{field}");
    }
    let QueryValue::Map(ref mut fields) = record else {
        unreachable!()
    };
    fields.shift_remove("security");
    assert!(FunctionMeta::try_from_record(&record).is_err());
}

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

struct ForwardTo(String);

#[async_trait]
impl ShamirFunction for ForwardTo {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        ctx.call_stored(&self.0, batch, params).await
    }
}

struct FailOnce(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for FailOnce {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(FunctionError::User("first attempt".into()))
        } else {
            Ok(QueryValue::Int(2))
        }
    }
}

struct PendingOnce(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for PendingOnce {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            std::future::pending().await
        } else {
            Ok(QueryValue::Int(3))
        }
    }
}

struct RetryAfterError(&'static str);

#[async_trait]
impl ShamirFunction for RetryAfterError {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        match ctx.call_stored(self.0, batch, params).await {
            Err(FunctionError::User(_)) => ctx.call_stored(self.0, batch, params).await,
            other => Err(FunctionError::Compute(format!(
                "expected first target call to fail, got {other:?}"
            ))),
        }
    }
}

#[tokio::test]
async fn stored_call_rejects_self_cycle_before_reentry() {
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register("self", Arc::new(ForwardTo("self".into())))
        .unwrap();
    let ctx = FnCtx::new().with_registry(registry.clone());
    let error = registry
        .invoke("self", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("cycle")));
}

#[tokio::test]
async fn stored_call_rejects_two_function_cycle_at_repeated_identity() {
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register("first", Arc::new(ForwardTo("second".into())))
        .unwrap();
    registry
        .register("second", Arc::new(ForwardTo("first".into())))
        .unwrap();
    let ctx = FnCtx::new().with_registry(registry.clone());
    let error = registry
        .invoke("first", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("cycle")));
}

fn call_chain(registry: &FunctionRegistry, count: usize) {
    for index in 0..count {
        let name = format!("node-{index}");
        let target = format!("node-{}", index + 1);
        let function: Arc<dyn ShamirFunction> = if index + 1 < count {
            Arc::new(ForwardTo(target))
        } else {
            Arc::new(Const(1))
        };
        registry.register(&name, function).unwrap();
    }
}

#[tokio::test]
async fn stored_call_path_accepts_32_functions_including_root() {
    let registry = Arc::new(FunctionRegistry::new());
    call_chain(&registry, 32);
    let ctx = FnCtx::new().with_registry(registry.clone());
    let result = registry
        .invoke("node-0", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(result, QueryValue::Int(1));
}

#[tokio::test]
async fn stored_call_path_rejects_33rd_function() {
    let registry = Arc::new(FunctionRegistry::new());
    call_chain(&registry, 33);
    let ctx = FnCtx::new().with_registry(registry.clone());
    let error = registry
        .invoke("node-0", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("path limit")));
}

#[tokio::test]
async fn stored_call_keeps_lower_depth_limit() {
    let registry = Arc::new(FunctionRegistry::new());
    call_chain(&registry, 5);
    let ctx = FnCtx::new()
        .with_registry(registry.clone())
        .with_depth_limit(3);
    let error = registry
        .invoke("node-0", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("depth limit")));
}

#[tokio::test]
async fn stored_call_identity_survives_replace_and_rename_and_rejects_repeat() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("entry", Arc::new(Const(1))).unwrap();
    let original = registry.capture("entry").unwrap();
    registry.replace("entry", Arc::new(Const(9)));
    let replacement = registry.capture("entry").unwrap();
    assert!(!original.same_as(&replacement));
    assert_eq!(original.identity(), replacement.identity());
    registry.rename("entry", "renamed").unwrap();
    let renamed = registry.capture("renamed").unwrap();
    assert!(replacement.same_as(&renamed));
    assert_eq!(original.identity(), renamed.identity());

    let context = FnCtx::new().with_registry(registry.clone());
    let result = registry
        .invoke_captured(&original, &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(result, QueryValue::Int(1));

    assert!(registry.remove("renamed"));
    registry.register("renamed", Arc::new(Const(10))).unwrap();
    let re_registered = registry.capture("renamed").unwrap();
    assert_ne!(original.identity(), re_registered.identity());
}

struct MutateOwnEntryAndRepeat(Weak<FunctionRegistry>);

#[async_trait]
impl ShamirFunction for MutateOwnEntryAndRepeat {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let registry = self.0.upgrade().expect("registry stays alive during call");
        registry.replace("entry", Arc::new(Const(9)));
        registry.rename("entry", "renamed")?;
        ctx.call_stored("renamed", batch, params).await
    }
}

#[tokio::test]
async fn stored_call_identity_rejects_repeat_after_replace_and_rename() {
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register(
            "entry",
            Arc::new(MutateOwnEntryAndRepeat(Arc::downgrade(&registry))),
        )
        .unwrap();
    let context = FnCtx::new().with_registry(registry.clone());
    let error = registry
        .invoke("entry", &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("cycle")));
}

struct CallTwice(&'static str);

#[async_trait]
impl ShamirFunction for CallTwice {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let first = ctx.call_stored(self.0, batch, params).await?;
        let second = ctx.call_stored(self.0, batch, params).await?;
        Ok(QueryValue::List(vec![first, second]))
    }
}

struct Increment(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for Increment {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Int(
            self.0.fetch_add(1, Ordering::SeqCst) as i64 + 1,
        ))
    }
}

#[tokio::test]
async fn stored_call_path_allows_sequential_successful_calls_in_one_parent() {
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register("target", Arc::new(Increment(Arc::new(AtomicUsize::new(0)))))
        .unwrap();
    registry
        .register("parent", Arc::new(CallTwice("target")))
        .unwrap();
    let context = FnCtx::new().with_registry(registry.clone());
    let result = registry
        .invoke("parent", &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(
        result,
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)])
    );
}

struct CancelThenRetry(&'static str);

#[async_trait]
impl ShamirFunction for CancelThenRetry {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        {
            let mut first_attempt = Box::pin(ctx.call_stored(self.0, batch, params));
            let mut task_context = Context::from_waker(Waker::noop());
            assert!(matches!(
                first_attempt.as_mut().poll(&mut task_context),
                Poll::Pending
            ));
            drop(first_attempt);
        }
        ctx.call_stored(self.0, batch, params).await
    }
}

#[tokio::test]
async fn stored_call_path_is_scoped_after_cancelled_child_in_same_parent() {
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register(
            "target",
            Arc::new(PendingOnce(Arc::new(AtomicUsize::new(0)))),
        )
        .unwrap();
    registry
        .register("parent", Arc::new(CancelThenRetry("target")))
        .unwrap();
    let context = FnCtx::new().with_registry(registry.clone());
    let result = registry
        .invoke("parent", &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(result, QueryValue::Int(3));
}

struct CapturedInvokeBypass {
    registry: Arc<FunctionRegistry>,
    binding: crate::RegistryBinding,
}

#[async_trait]
impl ShamirFunction for CapturedInvokeBypass {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.registry
            .invoke_captured(&self.binding, ctx, batch, params)
            .await
    }
}

#[tokio::test]
async fn invoke_captured_cannot_bypass_active_nested_call_path() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("target", Arc::new(Const(1))).unwrap();
    let binding = registry.capture("target").unwrap();
    registry
        .register(
            "parent",
            Arc::new(CapturedInvokeBypass {
                registry: registry.clone(),
                binding,
            }),
        )
        .unwrap();
    let context = FnCtx::new().with_registry(registry.clone());
    let error = registry
        .invoke("parent", &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("active")));
}

#[tokio::test]
async fn stored_call_path_is_scoped_after_error() {
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register("target", Arc::new(FailOnce(Arc::new(AtomicUsize::new(0)))))
        .unwrap();
    registry
        .register("parent", Arc::new(RetryAfterError("target")))
        .unwrap();
    let ctx = FnCtx::new().with_registry(registry.clone());
    let batch = FnBatch::new();
    let params = Params::new();
    let result = registry
        .invoke("parent", &ctx, &batch, &params)
        .await
        .unwrap();
    assert_eq!(result, QueryValue::Int(2));
}

#[tokio::test]
async fn artifact_ids_participate_in_binding_and_follow_registry_updates() {
    let registry = FunctionRegistry::new();
    let function: Arc<dyn ShamirFunction> = Arc::new(Const(1));
    registry
        .register_with_artifact_id("entry", Arc::clone(&function), Some("artifact-a".into()))
        .unwrap();
    let initial = registry.capture("entry").unwrap();
    assert_eq!(initial.artifact_id(), Some("artifact-a"));

    registry.replace_with_artifact_id("entry", Arc::clone(&function), Some("artifact-b".into()));
    let changed_id = registry.capture("entry").unwrap();
    assert_eq!(changed_id.artifact_id(), Some("artifact-b"));
    assert!(!initial.same_as(&changed_id));

    registry.rename("entry", "renamed").unwrap();
    let renamed = registry.capture("renamed").unwrap();
    assert_eq!(renamed.artifact_id(), Some("artifact-b"));
    assert!(changed_id.same_as(&renamed));

    let legacy_replacement: Arc<dyn ShamirFunction> = Arc::new(Const(3));
    registry.replace("renamed", Arc::clone(&legacy_replacement));
    let legacy = registry.capture("renamed").unwrap();
    assert_eq!(legacy.artifact_id(), Some("artifact-b"));
    assert_eq!(legacy.identity(), renamed.identity());
    assert!(!renamed.same_as(&legacy));
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(legacy.artifact()),
        Arc::as_ptr(&legacy_replacement)
    ));

    let context = FnCtx::new();
    let saved_result = registry
        .invoke_captured(&renamed, &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(saved_result, QueryValue::Int(1));
    let current_result = registry
        .invoke_captured(&legacy, &context, &FnBatch::new(), &Params::new())
        .await
        .unwrap();
    assert_eq!(current_result, QueryValue::Int(3));

    registry.replace_with_artifact_id("renamed", Arc::clone(&function), None);
    let cleared = registry.capture("renamed").unwrap();
    assert_eq!(cleared.artifact_id(), None);
    assert_eq!(legacy.identity(), cleared.identity());

    let vacant_replacement: Arc<dyn ShamirFunction> = Arc::new(Const(4));
    registry.replace("vacant", Arc::clone(&vacant_replacement));
    let vacant = registry.capture("vacant").unwrap();
    assert_eq!(vacant.artifact_id(), None);
    assert_ne!(vacant.identity(), cleared.identity());
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(vacant.artifact()),
        Arc::as_ptr(&vacant_replacement)
    ));

    registry
        .register("registered", Arc::clone(&function))
        .unwrap();
    assert_eq!(registry.capture("registered").unwrap().artifact_id(), None);
}

struct CountedCall(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for CountedCall {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(QueryValue::Int(1))
    }
}

struct RejectingResolver(Arc<AtomicUsize>);

#[async_trait]
impl StoredCallResolver for RejectingResolver {
    async fn resolve(&self, _name: &str, _parent: &FnCtx) -> crate::FnResult<ResolvedStoredCall> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(FunctionError::Compute("unexpected resolver call".into()))
    }
}

#[tokio::test]
async fn catalogue_bound_registry_rejects_unresolved_paths_without_running_functions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(FunctionRegistry::with_catalogue_builtins());
    registry
        .register("counted", Arc::new(CountedCall(Arc::clone(&calls))))
        .unwrap();
    let binding = registry.capture("counted").unwrap();
    let batch = FnBatch::new();
    let params = Params::new();

    let rootless = FnCtx::new().with_registry(Arc::clone(&registry));
    let invoke_error = registry
        .invoke("counted", &rootless, &batch, &params)
        .await
        .unwrap_err();
    assert!(
        matches!(invoke_error, FunctionError::Compute(message) if message.contains("resolver"))
    );
    let captured_error = registry
        .invoke_captured(&binding, &rootless, &batch, &params)
        .await
        .unwrap_err();
    assert!(
        matches!(captured_error, FunctionError::Compute(message) if message.contains("resolver"))
    );

    let call_error = rootless
        .call_stored("counted", &batch, &params)
        .await
        .unwrap_err();
    assert!(
        matches!(call_error, FunctionError::Compute(message) if message.contains("captured root"))
    );

    let active = rootless.with_call_runtime(
        None,
        crate::runtime::registry::ActiveCallPath::standalone_root(),
        None,
    );
    let active_error = active
        .call_stored("counted", &batch, &params)
        .await
        .unwrap_err();
    assert!(
        matches!(active_error, FunctionError::Compute(message) if message.contains("resolver"))
    );
    let active_invoke_error = registry
        .invoke("counted", &active, &batch, &params)
        .await
        .unwrap_err();
    assert!(
        matches!(active_invoke_error, FunctionError::Compute(message) if message.contains("resolver"))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rootless_calls_with_resolver_are_rejected_before_resolution() {
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("target", Arc::new(Const(1))).unwrap();
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let ctx = FnCtx::new()
        .with_registry(registry)
        .with_stored_call_resolver(Arc::new(RejectingResolver(Arc::clone(&resolver_calls))));
    let error = ctx
        .call_stored("target", &FnBatch::new(), &Params::new())
        .await
        .unwrap_err();
    assert!(matches!(error, FunctionError::Compute(message) if message.contains("captured root")));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn host_only_rootless_call_stored_and_invoke_remain_available() {
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(FunctionRegistry::new());
    registry
        .register("target", Arc::new(CountedCall(Arc::clone(&calls))))
        .unwrap();
    let ctx = FnCtx::new().with_registry(Arc::clone(&registry));
    let batch = FnBatch::new();
    let params = Params::new();

    let stored = ctx.call_stored("target", &batch, &params).await.unwrap();
    assert_eq!(stored, QueryValue::Int(1));
    let invoked = registry
        .invoke("target", &ctx, &batch, &params)
        .await
        .unwrap();
    assert_eq!(invoked, QueryValue::Int(1));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn database_builtin_provenance_is_private_and_only_set_during_catalogue_construction() {
    let function: Arc<dyn ShamirFunction> = Arc::new(Const(1));
    let catalogue = FunctionRegistry::with_catalogue_builtins();
    assert!(catalogue.capture("argon2id").unwrap().is_database_builtin());
    assert!(!FunctionRegistry::with_builtins()
        .capture("argon2id")
        .unwrap()
        .is_database_builtin());

    let user_registry = FunctionRegistry::new();
    user_registry
        .register("registered", Arc::clone(&function))
        .unwrap();
    assert!(!user_registry
        .capture("registered")
        .unwrap()
        .is_database_builtin());
    user_registry
        .register_with_artifact_id("explicit", Arc::clone(&function), Some("id".into()))
        .unwrap();
    assert!(!user_registry
        .capture("explicit")
        .unwrap()
        .is_database_builtin());
    user_registry.replace("vacant", Arc::clone(&function));
    assert!(!user_registry
        .capture("vacant")
        .unwrap()
        .is_database_builtin());

    let original_builtin = catalogue.capture("argon2id").unwrap();
    catalogue.replace("argon2id", catalogue.get("argon2id").unwrap());
    let preserved_builtin = catalogue.capture("argon2id").unwrap();
    assert!(preserved_builtin.is_database_builtin());
    assert!(original_builtin.same_as(&preserved_builtin));
    catalogue.rename("argon2id", "renamed_builtin").unwrap();
    assert!(catalogue
        .capture("renamed_builtin")
        .unwrap()
        .is_database_builtin());
    catalogue.replace_with_artifact_id(
        "renamed_builtin",
        Arc::clone(&function),
        Some("managed".into()),
    );
    assert!(!catalogue
        .capture("renamed_builtin")
        .unwrap()
        .is_database_builtin());

    let argon_registry = FunctionRegistry::with_catalogue_builtins();
    assert!(argon_registry.remove("argon2id"));
    argon_registry.register("argon2id", function).unwrap();
    assert!(!argon_registry
        .capture("argon2id")
        .unwrap()
        .is_database_builtin());
}

#[test]
fn builtins_include_argon2id() {
    let reg = FunctionRegistry::with_builtins();
    assert!(reg.list().iter().any(|n| n == "argon2id"));
}
