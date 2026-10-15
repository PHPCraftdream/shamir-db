//! S5c-X — counter flushes (`set`/`set_to`/`persist`) acquire repository
//! admission BEFORE sampling the cache, and write through exactly once.
//!
//! Synthetic (d): a counter sharing a repo domain; with the permit held the
//! flush cannot proceed; once released the durable store holds the new
//! value exactly once. Two ADMITTED flushes are also shown to serialize via
//! explicit permit handoff (Notify ordering) — the admitted twins are
//! `pub(crate)`, so they cannot be reached permit-free from outside the
//! crate (compile boundary; asserted by construction here).
//!
//! Fails on pre-S5c-X code: the public flushes never consulted a domain, so
//! the "blocked" assertion trips immediately.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::codecs::basic::bincode;

use crate::meta::MetaKey;
use crate::repo::RepoWriteDomain;
use crate::table::record_counter::RecordCounter;
use crate::table::tests::write_domain::shared::{spin, wait, SPIN_BUDGET};
use tokio::sync::Notify;

fn count_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::Count.as_record_id().as_bytes())
}

/// Read the DURABLE count straight from the info store (no cache).
async fn durable_count(info: &Arc<dyn Store>) -> Option<u64> {
    match info.get(count_key()).await {
        Ok(bytes) => Some(bincode::from_bytes(&bytes).expect("decode count")),
        Err(_) => None,
    }
}

fn counter_with(info: &Arc<dyn Store>, domain: &Arc<RepoWriteDomain>) -> RecordCounter {
    RecordCounter::new_with_domain(Arc::clone(info), Arc::clone(domain))
}

#[tokio::test]
async fn set_blocks_while_permit_held_then_writes_durable_value_once() {
    let domain = RepoWriteDomain::new("counter-set");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);

    let permit = domain.admit().await.expect("admit");
    let task = tokio::spawn({
        let counter = counter.clone();
        async move { counter.set(7).await }
    });
    spin(SPIN_BUDGET).await;
    assert!(
        !task.is_finished(),
        "set() completed while repo admission was held"
    );
    assert_eq!(
        durable_count(&info).await,
        None,
        "durable store must be untouched while the flush is blocked"
    );
    drop(permit);
    wait(task).await.expect("set after release");

    // Exactly-once: the durable value equals the new value (not a stale or
    // partially-applied write).
    assert_eq!(durable_count(&info).await, Some(7));
    assert_eq!(counter.get().await.unwrap(), 7);
}

#[tokio::test]
async fn set_to_and_persist_block_while_permit_held() {
    let domain = RepoWriteDomain::new("counter-set-to");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);

    let permit = domain.admit().await.expect("admit");
    let task = tokio::spawn({
        let counter = counter.clone();
        async move { counter.set_to(42).await }
    });
    spin(SPIN_BUDGET).await;
    assert!(
        !task.is_finished(),
        "set_to() completed while repo admission was held"
    );

    let task2 = tokio::spawn({
        let counter = counter.clone();
        async move { counter.persist().await }
    });
    spin(SPIN_BUDGET).await;
    assert!(
        !task2.is_finished(),
        "persist() completed while repo admission was held"
    );

    drop(permit);
    wait(task).await.expect("set_to after release");
    wait(task2).await.expect("persist after release");
    assert_eq!(durable_count(&info).await, Some(42));
}

/// Two ADMITTED flushes serialize through the domain: permit 1 is handed
/// to flush 1, released, then handed to flush 2 — the durable value after
/// both is exactly the second write (no lost/duplicated write).
#[tokio::test]
async fn admitted_persists_serialize_in_order() {
    let domain = RepoWriteDomain::new("counter-serialize");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);

    // Flush 1 under permit 1.
    let permit1 = domain.admit().await.expect("admit 1");
    counter.increment(3).await.unwrap();
    let done1 = Arc::new(Notify::new());
    let task1 = tokio::spawn({
        let counter = counter.clone();
        let done1 = Arc::clone(&done1);
        async move {
            counter
                .persist_admitted(&permit1)
                .await
                .expect("persist_admitted 1");
            done1.notify_one();
        }
    });
    done1.notified().await;
    assert_eq!(durable_count(&info).await, Some(3));
    // Join flush 1: its task owns permit 1 and releases repo admission when
    // it ends — so permit 2 below is only acquired after flush 1 finished.
    wait(task1).await;

    // Flush 2 under permit 2 — acquired only AFTER permit 1 dropped.
    let permit2 = domain.admit().await.expect("admit 2");
    counter.increment(4).await.unwrap();
    let done2 = Arc::new(Notify::new());
    let task2 = tokio::spawn({
        let counter = counter.clone();
        let done2 = Arc::clone(&done2);
        async move {
            counter
                .persist_admitted(&permit2)
                .await
                .expect("persist_admitted 2");
            done2.notify_one();
        }
    });
    done2.notified().await;
    drop(permit2);
    wait(task2).await;

    assert_eq!(
        durable_count(&info).await,
        Some(7),
        "second admitted flush must land last, exactly once"
    );
    assert_eq!(counter.get().await.unwrap(), 7);
}

/// `increment` stays lock-free: it completes while a permit is held (the
/// leader write path gains NO admission).
#[tokio::test]
async fn increment_gains_no_admission() {
    let domain = RepoWriteDomain::new("counter-increment");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);
    let _permit = domain.admit().await.expect("admit");
    counter.increment(5).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 5);
}
