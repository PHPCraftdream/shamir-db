#![allow(deprecated)]

use crate::error::{DbError, DbResult};
use crate::storage_in_memory::InMemoryStore;
use crate::storage_mirrored::{is_durable_table_config, MirroredStore};
use crate::types::{KvOp, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::common::classified_key;

/// Test-only mirror wrapper whose `set` / `remove` can be configured to
/// fail — when `fail_writes` is set, both return `Err` WITHOUT delegating
/// to the inner store. All other `Store` methods (including `transact`)
/// delegate to `inner` unchanged. Sibling of F-39's
/// `FailingTransactMirror` (which only fails `transact`); this one fails
/// `set`/`remove` specifically, to prove F-41's mirror-first ordering
/// leaves `primary` untouched on a mirror write failure.
struct FailingSetRemoveMirror {
    inner: Arc<dyn Store>,
    /// When `true`, `set` / `remove` return `Err` without delegating.
    fail_writes: AtomicBool,
}

#[async_trait]
impl Store for FailingSetRemoveMirror {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(DbError::Internal(
                "injected set failure (FailingSetRemoveMirror)".into(),
            ));
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(DbError::Internal(
                "injected remove failure (FailingSetRemoveMirror)".into(),
            ));
        }
        self.inner.remove(key).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.inner.transact(ops).await
    }
    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// **Test 1 — mirror-first ordering, happy path (no regression).**
///
/// After F-41's mirror-commit-before-primary-publish reordering, a
/// classified `set`/`remove` must STILL land in both `primary` and
/// `mirror` exactly as before — the reordering only changes WHICH store
/// is written first, not whether both are written on success. Exercises
/// both `set` and `remove` in one place (the pre-F-41 happy-path tests
/// above cover them separately).
#[tokio::test]
async fn f41_mirror_first_ordering_classified_set_and_remove_land_in_both() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    assert!(is_durable_table_config(&key));

    // set: both primary (via facade get) and mirror reflect the write.
    let created = store
        .set(key.clone(), Bytes::from_static(b"v1"))
        .await
        .unwrap();
    assert!(
        created,
        "fresh classified insert must report created=true (InMemoryStore::set semantics)"
    );
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v1")
    );
    assert_eq!(
        mirror.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v1")
    );

    // remove: both primary and mirror drop the key.
    let existed = store.remove(key.clone()).await.unwrap();
    assert!(
        existed,
        "remove of a present classified key must report existed=true (InMemoryStore::remove semantics)"
    );
    assert!(store.get(key.clone()).await.is_err());
    assert!(mirror.get(key.clone()).await.is_err());
}

/// **Test 2a — mirror `set` failure leaves `primary` untouched.**
///
/// The core F-41 proof. With a mirror that errors on `set`, a classified
/// `set` must (a) return `Err` to the caller AND (b) leave `primary`
/// holding its PRE-write value (here `"old"`, not the attempted `"new"`)
/// — "API says failed" and "live state" now agree, unlike before F-41
/// (where primary was mutated first and the live process behaved as if
/// the write had succeeded even as the caller saw `Err`).
#[tokio::test]
async fn f41_mirror_set_failure_leaves_primary_untouched() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let failing_mirror = Arc::new(FailingSetRemoveMirror {
        inner: mirror_inner.clone(),
        fail_writes: AtomicBool::new(false),
    });
    let mirror_dyn: Arc<dyn Store> = failing_mirror.clone();

    let store = MirroredStore::new(mirror_dyn, is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    assert!(is_durable_table_config(&key));

    // Seed an OLD value into BOTH primary and mirror (writes succeed:
    // the flag is off, so the wrapper delegates to inner).
    store
        .set(key.clone(), Bytes::from_static(b"old"))
        .await
        .unwrap();

    // Flip the mirror to fail writes, then attempt an overwrite.
    failing_mirror.fail_writes.store(true, Ordering::Release);
    let result = store.set(key.clone(), Bytes::from_static(b"new")).await;
    assert!(
        result.is_err(),
        "classified set against a failing mirror must return Err"
    );

    // Primary must STILL hold the OLD value — the overwrite aborted at
    // the mirror BEFORE primary was touched. This is the core proof.
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"old"),
        "primary must hold the pre-write value after a mirror set failure"
    );
    // And the mirror's own inner store is of course unchanged too.
    assert_eq!(
        mirror_inner.get(key).await.unwrap(),
        Bytes::from_static(b"old"),
        "mirror inner must be untouched after a failed set"
    );
}

/// **Test 2b — mirror `remove` failure leaves `primary` untouched.**
///
/// Symmetric to test 2a for `remove`: a classified `remove` against a
/// failing mirror returns `Err` and leaves `primary` (and the key's
/// presence there) unchanged.
#[tokio::test]
async fn f41_mirror_remove_failure_leaves_primary_untouched() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let failing_mirror = Arc::new(FailingSetRemoveMirror {
        inner: mirror_inner.clone(),
        fail_writes: AtomicBool::new(false),
    });
    let mirror_dyn: Arc<dyn Store> = failing_mirror.clone();

    let store = MirroredStore::new(mirror_dyn, is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    // Seed the key into BOTH primary and mirror first (writes succeed:
    // flag off), so the subsequent `remove` has something to remove —
    // proving the failure aborts BEFORE the primary removal.
    store
        .set(key.clone(), Bytes::from_static(b"seeded"))
        .await
        .unwrap();
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"seeded")
    );
    assert_eq!(
        mirror_inner.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"seeded")
    );

    // Flip the mirror to fail writes, then attempt the removal.
    failing_mirror.fail_writes.store(true, Ordering::Release);
    let result = store.remove(key.clone()).await;
    assert!(
        result.is_err(),
        "classified remove against a failing mirror must return Err"
    );

    // Primary must STILL hold the key — the removal aborted at the
    // mirror before primary was touched.
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"seeded"),
        "primary must still hold the key after a mirror remove failure"
    );
    assert_eq!(
        mirror_inner.get(key).await.unwrap(),
        Bytes::from_static(b"seeded"),
        "mirror inner must be untouched after a failed remove"
    );
}
