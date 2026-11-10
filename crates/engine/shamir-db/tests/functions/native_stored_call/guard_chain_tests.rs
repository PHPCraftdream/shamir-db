//! Regression tests for owner-guard propagation across stored-call hops.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, Security, ShamirFunction};
use shamir_types::types::value::QueryValue;

use super::support::{invoke_as, register_native, setup, user, ActorProbe, Forward, CALLER, OWNER};

struct Counted(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for Counted {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(QueryValue::Null)
    }
}

#[tokio::test]
async fn definer_owner_guard_survives_invoker_hops() {
    let db = setup().await;
    let target_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "guard_chain_target_o2",
        Arc::new(Counted(target_calls.clone())),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "guard_chain_invoker_o1",
        Arc::new(Forward("guard_chain_target_o2")),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "guard_chain_definer_o1",
        Arc::new(Forward("guard_chain_invoker_o1")),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;

    let error = invoke_as(&db, "guard_chain_definer_o1", user(CALLER))
        .await
        .expect_err("the third hop crosses the active Definer owner guard");
    assert!(error
        .to_string()
        .contains("Definer cannot call Invoker function owned by a different actor"));
    assert_eq!(target_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn definer_owner_guard_allows_same_owner_after_invoker_hops() {
    let db = setup().await;
    register_native(
        &db,
        "guard_chain_target_o1",
        Arc::new(ActorProbe),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "guard_chain_middle_o1",
        Arc::new(Forward("guard_chain_target_o1")),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "guard_chain_root_o1",
        Arc::new(Forward("guard_chain_middle_o1")),
        user(OWNER),
        0o711,
        Security::Definer,
        (Vec::new(), Vec::new()),
    )
    .await;

    assert_eq!(
        invoke_as(&db, "guard_chain_root_o1", user(CALLER))
            .await
            .unwrap(),
        QueryValue::Int(OWNER as i64)
    );
}

#[tokio::test]
async fn setuid_invoker_root_seeds_owner_guard_even_when_caller_is_owner() {
    let db = setup().await;
    let other_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "setuid_root_other_owner_target",
        Arc::new(Counted(other_calls.clone())),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "setuid_root_other_owner",
        Arc::new(Forward("setuid_root_other_owner_target")),
        user(OWNER),
        0o4751,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "setuid_root_same_owner_target",
        Arc::new(ActorProbe),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "setuid_root_same_owner",
        Arc::new(Forward("setuid_root_same_owner_target")),
        user(OWNER),
        0o4751,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    let error = invoke_as(&db, "setuid_root_other_owner", user(OWNER))
        .await
        .expect_err("setuid Invoker root must guard even when actor already equals owner");
    assert!(error
        .to_string()
        .contains("Definer cannot call Invoker function owned by a different actor"));
    assert_eq!(other_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        invoke_as(&db, "setuid_root_same_owner", user(OWNER))
            .await
            .unwrap(),
        QueryValue::Int(OWNER as i64)
    );
}

#[tokio::test]
async fn nested_setuid_invoker_seeds_owner_guard() {
    let db = setup().await;
    let other_calls = Arc::new(AtomicUsize::new(0));
    register_native(
        &db,
        "nested_setuid_other_owner_target",
        Arc::new(Counted(other_calls.clone())),
        user(CALLER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "nested_setuid_target",
        Arc::new(Forward("nested_setuid_other_owner_target")),
        user(OWNER),
        0o4751,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;
    register_native(
        &db,
        "nested_setuid_parent",
        Arc::new(Forward("nested_setuid_target")),
        user(OWNER),
        0o711,
        Security::Invoker,
        (Vec::new(), Vec::new()),
    )
    .await;

    let error = invoke_as(&db, "nested_setuid_parent", user(OWNER))
        .await
        .expect_err("nested setuid Invoker must establish a guard even without actor change");
    assert!(error
        .to_string()
        .contains("Definer cannot call Invoker function owned by a different actor"));
    assert_eq!(other_calls.load(Ordering::SeqCst), 0);
}
