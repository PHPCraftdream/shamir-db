//! Unit tests for `TableManager::run_validators` (S3).
//!
//! Tests the priority ordering, stop-the-loop, collect-all, missing id
//! (fail-closed), and empty bindings fast-path using stub validators.

use std::sync::Arc;

use async_trait::async_trait;
use smallvec::smallvec;

use crate::function::{FnBatch, FnCtx, FnResult, FunctionError, Params, ShamirFunction};
use crate::validator::{ValidatorBinding, ValidatorFailure, ValidatorRegistry, WriteOp};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

// ── Stub validators ─────────────────────────────────────────────────────

/// A validator that returns an empty list (accept).
struct AcceptValidator;

#[async_trait]
impl ShamirFunction for AcceptValidator {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, _params: &Params) -> FnResult<QueryValue> {
        // null = valid
        Ok(QueryValue::Null)
    }
}

/// A validator that returns a single error with the given code.
struct RejectValidator {
    code: String,
}

#[async_trait]
impl ShamirFunction for RejectValidator {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, _params: &Params) -> FnResult<QueryValue> {
        Ok(QueryValue::List(vec![QueryValue::Str(self.code.clone())]))
    }
}

/// A validator that returns errors + `stop: true`.
struct StopValidator {
    code: String,
}

#[async_trait]
impl ShamirFunction for StopValidator {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, _params: &Params) -> FnResult<QueryValue> {
        let mut map = shamir_types::types::common::new_map();
        map.insert(
            "errors".to_string(),
            QueryValue::List(vec![QueryValue::Str(self.code.clone())]),
        );
        map.insert("stop".to_string(), QueryValue::Bool(true));
        Ok(QueryValue::Map(map))
    }
}

/// A validator that traps (returns Err).
struct TrapValidator;

#[async_trait]
impl ShamirFunction for TrapValidator {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, _params: &Params) -> FnResult<QueryValue> {
        Err(FunctionError::Compute("wasm trap".into()))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a minimal `TableManager` with a `ValidatorRegistry` and the
/// given bindings pre-loaded.
async fn build_table_with_validators(
    bindings: Vec<ValidatorBinding>,
    registry: Arc<ValidatorRegistry>,
) -> crate::table::TableManager {
    use crate::table::table_manager::TableManager;
    use shamir_storage::storage_in_memory::InMemoryStore;

    let data_store: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());
    let info_store: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());

    // Persist bindings in the info-twin so TableManager loads them.
    crate::validator::persistence::save_validators_metadata(&bindings, &info_store)
        .await
        .unwrap();

    let mut tm = TableManager::create("test_table".to_string(), data_store, info_store)
        .await
        .unwrap();
    tm.set_validator_registry(registry);
    tm
}

// ── Tests ───────────────────────────────────────────────────────────────

/// Empty bindings (no validators) → `Ok(())`.
#[tokio::test]
async fn no_bindings_returns_ok() {
    let reg = Arc::new(ValidatorRegistry::new());
    let tm = build_table_with_validators(vec![], reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    assert!(result.is_ok());
}

/// No bindings matching the op → `Ok(())`.
#[tokio::test]
async fn no_matching_op_returns_ok() {
    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("val_a");
    reg.register(
        id,
        "val_a",
        Arc::new(RejectValidator { code: "bad".into() }) as Arc<dyn ShamirFunction>,
    )
    .unwrap();

    let bindings = vec![ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Delete], // not Insert
        priority: 1000,
    }];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    assert!(result.is_ok());
}

/// An accepting validator → `Ok(())`.
#[tokio::test]
async fn accept_validator_returns_ok() {
    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("val_accept");
    reg.register(
        id,
        "val_accept",
        Arc::new(AcceptValidator) as Arc<dyn ShamirFunction>,
    )
    .unwrap();

    let bindings = vec![ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    assert!(result.is_ok());
}

/// A rejecting validator → `Err(Failed(...))`.
#[tokio::test]
async fn reject_validator_returns_failed() {
    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("val_reject");
    reg.register(
        id,
        "val_reject",
        Arc::new(RejectValidator {
            code: "invalid_email".into(),
        }) as Arc<dyn ShamirFunction>,
    )
    .unwrap();

    let bindings = vec![ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    match result {
        Err(ValidatorFailure::Failed(errors)) => {
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].code, "invalid_email");
        }
        other => panic!("expected Failed, got: {other:?}"),
    }
}

/// Priority order: lower priority fires first. Both reject — errors
/// from both are collected.
#[tokio::test]
async fn priority_order_and_collect_all() {
    let reg = Arc::new(ValidatorRegistry::new());

    let id_a = RecordId::system("val_a");
    let id_b = RecordId::system("val_b");

    reg.register(
        id_a,
        "val_a",
        Arc::new(RejectValidator {
            code: "first".into(),
        }) as Arc<dyn ShamirFunction>,
    )
    .unwrap();
    reg.register(
        id_b,
        "val_b",
        Arc::new(RejectValidator {
            code: "second".into(),
        }) as Arc<dyn ShamirFunction>,
    )
    .unwrap();

    // val_b has lower priority (1000) → fires first; val_a has 2000.
    let bindings = vec![
        ValidatorBinding {
            validator_id: id_a,
            ops: smallvec![WriteOp::Insert],
            priority: 2000,
        },
        ValidatorBinding {
            validator_id: id_b,
            ops: smallvec![WriteOp::Insert],
            priority: 1000,
        },
    ];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    match result {
        Err(ValidatorFailure::Failed(errors)) => {
            assert_eq!(errors.len(), 2, "both validators should contribute errors");
            // Priority 1000 (val_b = "second") fires first.
            assert_eq!(errors[0].code, "second");
            assert_eq!(errors[1].code, "first");
        }
        other => panic!("expected Failed with 2 errors, got: {other:?}"),
    }
}

/// `stop: true` halts remaining validators; the stopping validator's
/// own errors are still reported.
#[tokio::test]
async fn stop_halts_remaining_validators() {
    let reg = Arc::new(ValidatorRegistry::new());

    let id_stop = RecordId::system("val_stop");
    let id_after = RecordId::system("val_after");

    reg.register(
        id_stop,
        "val_stop",
        Arc::new(StopValidator {
            code: "stopped".into(),
        }) as Arc<dyn ShamirFunction>,
    )
    .unwrap();
    reg.register(
        id_after,
        "val_after",
        Arc::new(RejectValidator {
            code: "should_not_appear".into(),
        }) as Arc<dyn ShamirFunction>,
    )
    .unwrap();

    let bindings = vec![
        ValidatorBinding {
            validator_id: id_stop,
            ops: smallvec![WriteOp::Insert],
            priority: 1000, // fires first
        },
        ValidatorBinding {
            validator_id: id_after,
            ops: smallvec![WriteOp::Insert],
            priority: 2000, // would fire second, but skipped by stop
        },
    ];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    match result {
        Err(ValidatorFailure::Failed(errors)) => {
            assert_eq!(errors.len(), 1, "only the stop-validator's error");
            assert_eq!(errors[0].code, "stopped");
        }
        other => panic!("expected Failed with 1 error, got: {other:?}"),
    }
}

/// Missing validator id → `Err(Missing { id })`.
#[tokio::test]
async fn missing_validator_fails_closed() {
    let reg = Arc::new(ValidatorRegistry::new());

    let missing_id = RecordId::system("nonexistent");

    let bindings = vec![ValidatorBinding {
        validator_id: missing_id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    match result {
        Err(ValidatorFailure::Missing { id }) => {
            assert_eq!(id, missing_id);
        }
        other => panic!("expected Missing, got: {other:?}"),
    }
}

/// Invocation failure (trap) → `Err(Invocation { ... })`.
#[tokio::test]
async fn trap_validator_fails_closed() {
    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("val_trap");
    reg.register(
        id,
        "val_trap",
        Arc::new(TrapValidator) as Arc<dyn ShamirFunction>,
    )
    .unwrap();

    let bindings = vec![ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    let tm = build_table_with_validators(bindings, reg).await;

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    match result {
        Err(ValidatorFailure::Invocation { id: err_id, .. }) => {
            assert_eq!(err_id, id);
        }
        other => panic!("expected Invocation, got: {other:?}"),
    }
}

/// No registry set (None) → `Ok(())` regardless of bindings.
#[tokio::test]
async fn no_registry_returns_ok() {
    use crate::table::table_manager::TableManager;
    use shamir_storage::storage_in_memory::InMemoryStore;

    let data_store: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());
    let info_store: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());

    // Persist a binding (which would normally trigger a validator lookup).
    let bindings = vec![ValidatorBinding {
        validator_id: RecordId::system("whatever"),
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    crate::validator::persistence::save_validators_metadata(&bindings, &info_store)
        .await
        .unwrap();

    // Create without setting validator_registry — it stays None.
    let tm = TableManager::create("test_no_reg".to_string(), data_store, info_store)
        .await
        .unwrap();

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System)
        .await;
    assert!(result.is_ok(), "no registry = validators disabled");
}
