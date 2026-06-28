//! Unit tests for `TableManager::bindings_len` AtomicUsize mirror (perf-③.289).
//!
//! Covers:
//! - Mirror == bindings.len() after add/remove (single thread).
//! - Fast-skip: `run_validators_qv` returns early on empty bindings
//!   without touching the ArcSwap (verified via mirror staying 0).
//! - Concurrent add: multiple tasks bind distinct validators → mirror
//!   converges to actual bindings count.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use smallvec::smallvec;

use crate::table::table_manager::TableManager;
use crate::validator::{
    RecordFields, RecordValidator, Validation, ValidatorBinding, ValidatorCtx, ValidatorRegistry,
    WriteOp,
};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;

// ── Stub ─────────────────────────────────────────────────────────────────

struct AcceptAll;

#[async_trait]
impl RecordValidator for AcceptAll {
    async fn validate(
        &self,
        _new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        Validation::accept()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

async fn empty_table_manager() -> TableManager {
    let data_store: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());
    let info_store: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());
    TableManager::create("test_table".to_string(), data_store, info_store)
        .await
        .unwrap()
}

fn make_binding(id: RecordId) -> ValidatorBinding {
    ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

/// Fresh TableManager (no persisted bindings) → mirror == 0.
#[tokio::test]
async fn initial_mirror_is_zero() {
    let tm = empty_table_manager().await;
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 0);
}

/// After one add_validator_binding → mirror == 1.
#[tokio::test]
async fn mirror_increments_after_add() {
    let tm = empty_table_manager().await;

    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("v1");
    reg.register(id, "v1", Arc::new(AcceptAll) as Arc<dyn RecordValidator>)
        .unwrap();

    tm.add_validator_binding(make_binding(id)).await.unwrap();

    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 1);
    // ArcSwap must also be consistent.
    assert_eq!(tm.validator_bindings().len(), 1);
}

/// add then remove → mirror back to 0.
#[tokio::test]
async fn mirror_decrements_after_remove() {
    let tm = empty_table_manager().await;

    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("v2");
    reg.register(id, "v2", Arc::new(AcceptAll) as Arc<dyn RecordValidator>)
        .unwrap();

    tm.add_validator_binding(make_binding(id)).await.unwrap();
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 1);

    let removed = tm.remove_validator_binding(&id).await.unwrap();
    assert!(removed);
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 0);
    assert_eq!(tm.validator_bindings().len(), 0);
}

/// Idempotent add (same validator_id twice) keeps mirror at 1.
#[tokio::test]
async fn idempotent_add_does_not_double_count() {
    let tm = empty_table_manager().await;

    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("v3");
    reg.register(id, "v3", Arc::new(AcceptAll) as Arc<dyn RecordValidator>)
        .unwrap();

    tm.add_validator_binding(make_binding(id)).await.unwrap();
    tm.add_validator_binding(make_binding(id)).await.unwrap();

    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 1);
    assert_eq!(tm.validator_bindings().len(), 1);
}

/// Remove of non-existent id leaves mirror unchanged.
#[tokio::test]
async fn remove_nonexistent_leaves_mirror_unchanged() {
    let tm = empty_table_manager().await;
    let phantom = RecordId::system("phantom");

    let removed = tm.remove_validator_binding(&phantom).await.unwrap();
    assert!(!removed);
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 0);
}

/// Mirror == bindings.len() for multiple sequential adds and removes.
#[tokio::test]
async fn mirror_tracks_bindings_length_through_mutations() {
    let tm = empty_table_manager().await;

    let ids: Vec<RecordId> = (0..5)
        .map(|i| RecordId::system(Box::leak(format!("seq_v{i}").into_boxed_str())))
        .collect();

    for &id in &ids {
        tm.add_validator_binding(make_binding(id)).await.unwrap();
    }
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 5);
    assert_eq!(tm.validator_bindings().len(), 5);

    // Remove two of them.
    tm.remove_validator_binding(&ids[1]).await.unwrap();
    tm.remove_validator_binding(&ids[3]).await.unwrap();

    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 3);
    assert_eq!(tm.validator_bindings().len(), 3);
}

/// Fast-skip: run_validators_qv on a table with a registry but 0 bindings
/// returns Ok immediately. Verified indirectly: mirror stays 0, which is
/// the guard that causes early return before any load_full() call.
#[tokio::test]
async fn fast_skip_fires_on_empty_bindings() {
    let tm_base = empty_table_manager().await;

    // Wire in a registry — without this the first check (None registry)
    // would short-circuit and we'd never reach the bindings_len check.
    let reg = Arc::new(ValidatorRegistry::new());
    let mut tm = tm_base;
    tm.set_validator_registry(reg);

    // bindings_len == 0 → fast-skip → Ok(()) without touching ArcSwap.
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 0);

    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System, None, None)
        .await;
    assert!(result.is_ok(), "empty bindings must fast-skip to Ok(())");

    // Mirror is still 0 — no mutation happened.
    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 0);
}

/// After add_validator_binding the fast-skip no longer fires.
#[tokio::test]
async fn fast_skip_disabled_after_add() {
    let tm_base = empty_table_manager().await;

    let reg = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("accept_fast");
    reg.register(
        id,
        "accept_fast",
        Arc::new(AcceptAll) as Arc<dyn RecordValidator>,
    )
    .unwrap();

    let mut tm = tm_base;
    tm.set_validator_registry(Arc::clone(&reg));
    tm.add_validator_binding(make_binding(id)).await.unwrap();

    assert_eq!(tm.bindings_len.load(Ordering::Acquire), 1);

    // With a live binding, run_validators_qv goes past the fast-skip.
    // AcceptAll → Ok(()).
    let result = tm
        .run_validators_qv(WriteOp::Insert, None, None, &Actor::System, None, None)
        .await;
    assert!(result.is_ok(), "accept validator should produce Ok(())");
}

/// Concurrent add: N tasks each bind a distinct validator_id on a shared
/// TableManager → mirror == N after all tasks complete.
#[tokio::test]
async fn concurrent_adds_mirror_converges_to_n() {
    const N: usize = 20;

    let tm = Arc::new(empty_table_manager().await);

    let handles: Vec<_> = (0..N)
        .map(|i| {
            let tm = Arc::clone(&tm);
            // SAFETY: string is 'static because we leak it deliberately for this
            // test — test process owns it for its lifetime.
            let id = RecordId::system(Box::leak(format!("conc_v{i}").into_boxed_str()));
            tokio::spawn(async move {
                tm.add_validator_binding(make_binding(id)).await.unwrap();
            })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }

    let mirror = tm.bindings_len.load(Ordering::Acquire);
    let actual = tm.validator_bindings().len();
    assert_eq!(
        mirror, actual,
        "mirror must match actual bindings count after concurrent adds"
    );
    assert_eq!(
        actual, N,
        "all N distinct bindings must have been registered"
    );
}
