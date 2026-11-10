//! Native database-route tests for stored-call cycle and lifecycle behavior.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::DbError;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, Security, ShamirFunction};
use shamir_types::types::value::QueryValue;

use super::support::{invoke_as, register_native, setup, user, Forward, CALLER};

struct Counted(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for Counted {
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

struct ForwardOwned(String);

#[async_trait]
impl ShamirFunction for ForwardOwned {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        ctx.call_stored(&self.0, batch, params).await
    }
}

async fn register_chain(db: &shamir_db::ShamirDb, count: usize, leaf_calls: Arc<AtomicUsize>) {
    for index in 0..count {
        let body: Arc<dyn ShamirFunction> = if index + 1 == count {
            Arc::new(Counted(leaf_calls.clone()))
        } else {
            Arc::new(ForwardOwned(format!("chain_{}", index + 1)))
        };
        register_native(
            db,
            &format!("chain_{index}"),
            body,
            user(CALLER),
            0o711,
            Security::Invoker,
            (Vec::new(), Vec::new()),
        )
        .await;
    }
}

fn assert_cycle_error(result: Result<QueryValue, DbError>) {
    let error = result.expect_err("repeated binding must fail");
    assert!(
        error.to_string().contains("stored call cycle detected"),
        "expected repeated identity rejection, got: {error}"
    );
}

#[tokio::test]
async fn nested_call_rejects_self_cycle_before_reentry() {
    let db = setup().await;
    register_native(
        &db,
        "self_cycle",
        Arc::new(Forward("self_cycle")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_cycle_error(invoke_as(&db, "self_cycle", user(CALLER)).await);
}

#[tokio::test]
async fn nested_call_rejects_two_function_cycle_at_repeat() {
    let db = setup().await;
    register_native(
        &db,
        "cycle_a",
        Arc::new(Forward("cycle_b")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "cycle_b",
        Arc::new(Forward("cycle_a")),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_cycle_error(invoke_as(&db, "cycle_a", user(CALLER)).await);
}

#[tokio::test]
async fn nested_call_accepts_32_distinct_bindings_including_root() {
    let db = setup().await;
    let leaf_calls = Arc::new(AtomicUsize::new(0));
    register_chain(&db, 32, leaf_calls.clone()).await;

    assert_eq!(
        invoke_as(&db, "chain_0", user(CALLER)).await.unwrap(),
        QueryValue::Int(1)
    );
    assert_eq!(leaf_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nested_call_rejects_33_distinct_bindings_including_root() {
    let db = setup().await;
    let leaf_calls = Arc::new(AtomicUsize::new(0));
    register_chain(&db, 33, leaf_calls.clone()).await;

    assert!(invoke_as(&db, "chain_0", user(CALLER)).await.is_err());
    assert_eq!(leaf_calls.load(Ordering::SeqCst), 0);
}

struct CallTwice;

#[async_trait]
impl ShamirFunction for CallTwice {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let first = ctx.call_stored("sequential_target", batch, params).await?;
        let second = ctx.call_stored("sequential_target", batch, params).await?;
        Ok(QueryValue::List(vec![first, second]))
    }
}

#[tokio::test]
async fn nested_call_path_is_restored_after_success_for_sequential_calls() {
    let db = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "sequential_target",
        Arc::new(Counted(calls.clone())),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "sequential_parent",
        Arc::new(CallTwice),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "sequential_parent", user(CALLER))
            .await
            .unwrap(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)])
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
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
            Err(FunctionError::User("first call fails".into()))
        } else {
            Ok(QueryValue::Int(2))
        }
    }
}

struct RetryAfterError;

#[async_trait]
impl ShamirFunction for RetryAfterError {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        match ctx.call_stored("error_target", batch, params).await {
            Err(FunctionError::User(_)) => ctx.call_stored("error_target", batch, params).await,
            other => Err(FunctionError::Compute(format!(
                "expected target error before retry, got {other:?}"
            ))),
        }
    }
}

#[tokio::test]
async fn nested_call_path_is_restored_after_error_for_retry() {
    let db = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "error_target",
        Arc::new(FailOnce(calls.clone())),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "error_parent",
        Arc::new(RetryAfterError),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "error_parent", user(CALLER)).await.unwrap(),
        QueryValue::Int(2)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

struct CancelThenRetry {
    target: &'static str,
    calls: Arc<AtomicUsize>,
    entered: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl ShamirFunction for CancelThenRetry {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let receiver = self.entered.lock().await.take().ok_or_else(|| {
            FunctionError::Compute("target body receiver already consumed".into())
        })?;
        let child_context = ctx.clone();
        let child_batch = batch.clone();
        let child_params = params.clone();
        let target = self.target;
        let first_attempt = {
            let child = async move {
                child_context
                    .call_stored(target, &child_batch, &child_params)
                    .await
            };
            tokio::pin!(child);
            tokio::select! {
                handshake = receiver => handshake.map_err(|_| {
                    FunctionError::Compute("target body handshake dropped".into())
                }),
                result = &mut child => Err(FunctionError::Compute(format!(
                    "first child unexpectedly completed: {result:?}"
                ))),
            }
        };
        first_attempt?;
        assert_eq!(
            self.calls.load(Ordering::SeqCst),
            1,
            "target body entered before its child future was dropped"
        );
        ctx.call_stored(self.target, batch, params).await
    }
}

struct PendingOnce {
    calls: Arc<AtomicUsize>,
    entered: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait]
impl ShamirFunction for PendingOnce {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            if let Some(sender) = self.entered.lock().await.take() {
                let _ = sender.send(());
            }
            std::future::pending().await
        } else {
            Ok(QueryValue::Int(2))
        }
    }
}

#[tokio::test]
async fn nested_call_path_is_restored_after_cancelled_child_retry() {
    let db = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    register_native(
        &db,
        "pending_target",
        Arc::new(PendingOnce {
            calls: calls.clone(),
            entered: tokio::sync::Mutex::new(Some(entered_tx)),
        }),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "cancel_retry_parent",
        Arc::new(CancelThenRetry {
            target: "pending_target",
            calls: calls.clone(),
            entered: tokio::sync::Mutex::new(Some(entered_rx)),
        }),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "cancel_retry_parent", user(CALLER))
            .await
            .expect("retry succeeds after cancelled child"),
        QueryValue::Int(2)
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "first pending body and retry body both ran"
    );
}

struct RenameAndRepeat {
    db: shamir_db::ShamirDb,
}

#[async_trait]
impl ShamirFunction for RenameAndRepeat {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.db.functions().replace(
            "rename_entry",
            Arc::new(Counted(Arc::new(AtomicUsize::new(0)))),
        );
        self.db
            .rename_function("rename_entry", "renamed_entry")
            .await
            .map_err(|error| FunctionError::Compute(error.to_string()))?;
        ctx.call_stored("renamed_entry", batch, params).await
    }
}

#[tokio::test]
async fn nested_call_repeat_detection_survives_native_replace_and_rename() {
    let db = setup().await;
    register_native(
        &db,
        "rename_entry",
        Arc::new(RenameAndRepeat { db: db.clone() }),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_cycle_error(invoke_as(&db, "rename_entry", user(CALLER)).await);
    assert!(db
        .system_store()
        .load_function("renamed_entry")
        .await
        .expect("catalogue lookup")
        .is_some());
}
