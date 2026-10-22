//! Deterministic regressions for binding DDL on cloned table managers.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{FutureExt, Stream};
use shamir_query_builder::write::{self, doc};
use shamir_storage::error::DbResult;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;
use tokio::sync::Notify;

use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::{
    insert_via_tx, setup_empty_table,
};
use crate::table::TableManager;
use crate::validator::persistence::load_validators_metadata;
use crate::validator::{
    RecordFields, RecordValidator, Validation, ValidatorBinding, ValidatorCtx, ValidatorRegistry,
    WriteOp,
};

type StoreStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

/// Only the next binding persist pauses; other writes remain immediately runnable.
struct PausingInfoStore {
    inner: Arc<dyn Store>,
    armed: AtomicBool,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl Store for PausingInfoStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let validator_key = crate::meta::MetaKey::Validators.as_record_id();
        if key == RecordKey::from_slice(validator_key.as_bytes())
            && self.armed.swap(false, Ordering::SeqCst)
        {
            self.entered.notify_one();
            // Single waiter: notify_one retains a permit even before this await.
            self.release.notified().await;
        }
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn flush(&self) -> DbResult<()> {
        self.inner.flush().await
    }

    fn iter_stream(&self, batch_size: usize) -> StoreStream {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> StoreStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

struct CountingValidator(Arc<AtomicUsize>);

#[async_trait]
impl RecordValidator for CountingValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        assert!(new.is_some(), "INSERT must supply the record being written");
        self.0.fetch_add(1, Ordering::Relaxed);
        Validation::accept()
    }
}

fn binding(id: RecordId) -> ValidatorBinding {
    ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }
}

fn install_validators(table: &mut TableManager) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let registry = Arc::new(ValidatorRegistry::new());
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    for (name, count) in [("binding_a", &a), ("binding_b", &b)] {
        registry
            .register(
                RecordId::system(name),
                name,
                Arc::new(CountingValidator(Arc::clone(count))),
            )
            .unwrap();
    }
    table.set_validator_registry(registry);
    (a, b)
}

fn pause_next_persist(table: &mut TableManager) -> Arc<PausingInfoStore> {
    let store = Arc::new(PausingInfoStore {
        inner: Arc::clone(&table.info_store),
        armed: AtomicBool::new(true),
        entered: Notify::new(),
        release: Notify::new(),
    });
    // Binding DDL uses this handle; sub-managers still use the same inner store.
    table.info_store = store.clone();
    store
}

async fn assert_binding_sets(table: &TableManager, expected: &[RecordId]) {
    let mut expected = expected.to_vec();
    expected.sort();
    let live = table.validator_bindings();
    let persisted = load_validators_metadata(&table.info_store)
        .await
        .unwrap()
        .expect("binding metadata must have been persisted");
    for bindings in [live.as_slice(), persisted.bindings.as_slice()] {
        let mut ids: Vec<_> = bindings.iter().map(|b| b.validator_id).collect();
        ids.sort();
        assert_eq!(ids, expected, "exact live and persisted binding sets");
        for b in bindings {
            assert_eq!(b.ops.as_slice(), &[WriteOp::Insert]);
            assert_eq!(b.priority, 1000);
        }
    }
}

/// Red without serialization: B completes while A is parked, then A overwrites B.
#[tokio::test]
async fn concurrent_bind_bind_preserves_both_and_invokes_both() {
    let (mut table, repo) = setup_empty_table().await;
    let (count_a, count_b) = install_validators(&mut table);
    let store = pause_next_persist(&mut table);
    let manager_a = table.clone();
    let manager_b = table.clone();
    let id_a = RecordId::system("binding_a");
    let id_b = RecordId::system("binding_b");
    let mut a = Box::pin(manager_a.add_validator_binding(binding(id_a)));
    let mut b = Box::pin(manager_b.add_validator_binding(binding(id_b)));

    assert!(a.as_mut().now_or_never().is_none());
    assert!(store.entered.notified().now_or_never().is_some());
    // Borrow the pinned future: the pending operation is retained, not cancelled.
    assert!(b.as_mut().now_or_never().is_none(), "B must wait for A");
    store.release.notify_one();
    let (a_result, b_result) = tokio::join!(a, b);
    a_result.unwrap();
    b_result.unwrap();

    assert_binding_sets(&table, &[id_a, id_b]).await;
    let op = write::insert("users")
        .row(doc().set("name", "Alice"))
        .build();
    let result = insert_via_tx(&repo, &table.clone(), &op, false)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(count_a.load(Ordering::Relaxed), 1);
    assert_eq!(count_b.load(Ordering::Relaxed), 1);
}

/// Red without serialization: unbind B completes against the old set; A restores B.
#[tokio::test]
async fn concurrent_bind_unbind_preserves_add_and_removal_effects() {
    let (mut table, repo) = setup_empty_table().await;
    let (count_a, count_b) = install_validators(&mut table);
    let id_a = RecordId::system("binding_a");
    let id_b = RecordId::system("binding_b");
    table.add_validator_binding(binding(id_b)).await.unwrap();
    let before = write::insert("users")
        .row(doc().set("name", "Before"))
        .build();
    let result = insert_via_tx(&repo, &table, &before, false).await.unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(count_a.load(Ordering::Relaxed), 0);
    assert_eq!(count_b.load(Ordering::Relaxed), 1);

    let store = pause_next_persist(&mut table);
    let manager_a = table.clone();
    let manager_b = table.clone();
    let mut a = Box::pin(manager_a.add_validator_binding(binding(id_a)));
    let mut b = Box::pin(manager_b.remove_validator_binding(&id_b));
    assert!(a.as_mut().now_or_never().is_none());
    assert!(store.entered.notified().now_or_never().is_some());
    assert!(
        b.as_mut().now_or_never().is_none(),
        "unbind must wait for bind"
    );
    store.release.notify_one();
    let (a_result, b_result) = tokio::join!(a, b);
    a_result.unwrap();
    assert!(b_result.unwrap(), "the existing binding must be removed");

    assert_binding_sets(&table, &[id_a]).await;
    let after = write::insert("users")
        .row(doc().set("name", "After"))
        .build();
    let result = insert_via_tx(&repo, &table.clone(), &after, false)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(count_a.load(Ordering::Relaxed), 1);
    assert_eq!(
        count_b.load(Ordering::Relaxed),
        1,
        "removed validator must not fire again"
    );
}
