//! Round 6 (F1/F2/F3) regression tests: a failed binding flush must be
//! compensated by a CHECKED and FLUSHED restore of the previous logical
//! envelope, and a compensation that cannot be flushed must leave the table
//! IN DOUBT (binding DDL refused, everything else unaffected) instead of
//! silently diverging from the backing record.
//!
//! Fault injection: [`FlushFaultStore`] delegates every store operation to an
//! in-memory store and fails `flush()` while armed, so these tests need no
//! sleep and no timing.
//!
//! Oracle, corrected in round 7: the tests distinguish a MISSING compensation
//! flush by the FLUSH CALL COUNT, not by reading the forward value back — the
//! in-memory store makes a `set` visible without any flush, so the backing
//! record cannot tell a flushed compensation from an unflushed one. Without the
//! round-6 fix tests 1 and 3 fail on that count (the old compensation was an
//! unchecked, unflushed `set`) and test 2 sees no in-doubt refusal at all. The
//! F3 oracle is the per-flush observation log: the FORWARD flush of an unbind
//! must run while the live binding is STILL present, and the whole slice for the
//! operation is asserted — a publish-before-flush regression (publish the
//! removal, then restore the old snapshot before compensating) keeps the final
//! snapshot and the compensation observation right and still fails.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use shamir_engine_validator::validator::{ValidatorBinding, WriteOp};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::smallvec;

use super::super::persistence::load_validators_metadata;
use crate::table_manager::TableManager;

/// Hang detector only — never part of the protocol.
const DEADLINE: Duration = Duration::from_secs(120);
const TABLE: &str = "bound_table";
const VALIDATOR: &str = "flush_fault_validator";
const PRIORITY: u16 = 1000;

async fn bounded<T>(step: impl Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, Box::pin(step))
        .await
        .expect("binding flush-fault step timed out")
}

fn binding(id: RecordId) -> ValidatorBinding {
    ValidatorBinding {
        validator_id: id,
        ops: smallvec![WriteOp::Insert],
        priority: PRIORITY,
    }
}

fn is_in_doubt(error: &DbError) -> bool {
    matches!(error, DbError::Conflict(message) if message.contains("binding state in doubt"))
}

/// `Store` wrapper whose `flush()` fails while `failing` is non-zero. It also
/// records, for every flush call, whether the watched validator was present in
/// the LIVE snapshot at that moment — the F3 oracle: no flush of an unbind may
/// run after the live removal was published.
struct FlushFaultStore {
    inner: InMemoryStore,
    failing: AtomicUsize,
    /// Number of `flush()` calls seen, so a test can assert that the
    /// compensation is FLUSHED (one forward flush + one per attempt).
    flushes: AtomicUsize,
    live_at_flush: Mutex<Vec<bool>>,
    /// Attached live-snapshot source for the F3 oracle. The fixture DETACHES it
    /// at the end of every test: the fault store is the table's own info store,
    /// so a retained manager clone closes a strong cycle
    /// (store -> manager -> store).
    table: Mutex<Option<TableManager>>,
    watched: OnceLock<RecordId>,
}

impl FlushFaultStore {
    fn new() -> Self {
        Self {
            inner: InMemoryStore::new(),
            failing: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
            live_at_flush: Mutex::new(Vec::new()),
            table: Mutex::new(None),
            watched: OnceLock::new(),
        }
    }

    /// Fail the next `count` flushes, then behave normally.
    fn fail_next_flushes(&self, count: usize) {
        self.failing.store(count, Ordering::SeqCst);
    }

    fn flush_calls(&self) -> usize {
        self.flushes.load(Ordering::SeqCst)
    }

    fn live_observations(&self) -> Vec<bool> {
        self.live_at_flush.lock().expect("observation log").clone()
    }

    /// Attach the live-snapshot source for the F3 oracle.
    fn attach(&self, table: TableManager) {
        *self.table.lock().expect("attached table") = Some(table);
    }

    /// Break the store -> manager -> store cycle at the end of a test.
    fn detach(&self) {
        self.table.lock().expect("attached table").take();
    }
}

#[async_trait]
impl Store for FlushFaultStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.inner.transact(ops).await
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }

    async fn flush(&self) -> DbResult<()> {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        let present = {
            let table = self.table.lock().expect("attached table");
            match (table.as_ref(), self.watched.get()) {
                (Some(table), Some(id)) => Some(
                    table
                        .validator_bindings()
                        .iter()
                        .any(|binding| binding.validator_id == *id),
                ),
                _ => None,
            }
        };
        if let Some(present) = present {
            self.live_at_flush
                .lock()
                .expect("observation log")
                .push(present);
        }
        // Decrement exactly once per call: `Err` means the counter was zero.
        if self
            .failing
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(DbError::Storage("injected binding flush fault".into()));
        }
        self.inner.flush().await
    }
}

/// A throwaway table whose info-twin is the fault store.
async fn fixture() -> (TableManager, Arc<FlushFaultStore>, RecordId, Arc<dyn Store>) {
    let fault = Arc::new(FlushFaultStore::new());
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = fault.clone();
    let table = bounded(TableManager::create(
        TABLE.to_string(),
        data,
        Arc::clone(&info),
    ))
    .await
    .expect("table manager");
    let id = RecordId::system(VALIDATOR);
    fault.attach(table.clone());
    let _ = fault.watched.set(id);
    (table, fault, id, info)
}

#[tokio::test]
async fn bind_flush_fault_restores_the_backing_record_and_retry_succeeds() {
    bounded(async {
        let (table, fault, id, info) = fixture().await;
        let flushes_before = fault.flush_calls();
        fault.fail_next_flushes(1);

        let error = table
            .add_validator_binding(binding(id))
            .await
            .expect_err("the failed flush must not be reported as a successful bind");
        assert!(matches!(error, DbError::Storage(_)), "{error}");
        assert!(
            table.validator_bindings().is_empty(),
            "an unflushed binding must never reach the live snapshot"
        );
        assert!(
            !table.validator_bindings_in_doubt(),
            "a flush failure whose compensation succeeds leaves a coherent table"
        );

        let stored = load_validators_metadata(&info)
            .await
            .unwrap()
            .expect("the compensation writes a present envelope");
        assert!(
            stored.bindings.is_empty(),
            "the backing record must equal the previous logical one, got {:?}",
            stored.bindings.len()
        );

        assert_eq!(
            fault.flush_calls(),
            flushes_before + 2,
            "F1: the failed forward flush must be followed by ONE flushed compensation"
        );

        table
            .add_validator_binding(binding(id))
            .await
            .expect("a retry after a recovered flush must succeed");
        assert_eq!(table.validator_bindings().len(), 1);
        let stored = load_validators_metadata(&info).await.unwrap().unwrap();
        assert_eq!(stored.bindings.len(), 1);
        assert_eq!(stored.bindings[0].validator_id, id);
        fault.detach();
    })
    .await;
}

#[tokio::test]
async fn bind_compensation_flush_fault_marks_the_table_in_doubt() {
    bounded(async {
        let (table, fault, id, _info) = fixture().await;
        let flushes_before = fault.flush_calls();
        // Every flush fails: the forward write and all compensation attempts.
        fault.fail_next_flushes(usize::MAX);

        let error = table
            .add_validator_binding(binding(id))
            .await
            .expect_err("an unreconciled binding write must fail");
        assert_eq!(
            fault.flush_calls(),
            flushes_before + 1 + 3,
            "F1: one forward flush plus COMPENSATION_ATTEMPTS flushed restores"
        );
        assert!(
            is_in_doubt(&error),
            "the compensation could not be flushed, so the table must report doubt: {error}"
        );
        assert!(
            table.validator_bindings_in_doubt(),
            "the in-doubt state must be visible to the holder scan"
        );
        assert!(table.validator_bindings().is_empty());

        let again = table
            .add_validator_binding(binding(id))
            .await
            .expect_err("bind must refuse while the table is in doubt");
        assert!(is_in_doubt(&again), "{again}");
        assert!(
            !again.to_string().contains(TABLE),
            "the in-doubt refusal must not disclose the table: {again}"
        );
        let unbind = table
            .remove_validator_binding(&id)
            .await
            .expect_err("unbind must refuse while the table is in doubt");
        assert!(is_in_doubt(&unbind), "{unbind}");

        table
            .insert(&InnerValue::Map(new_map()))
            .await
            .expect("ordinary writes are not gated by the in-doubt flag");
        fault.detach();
    })
    .await;
}

#[tokio::test]
async fn unbind_flush_fault_keeps_the_live_binding_and_restores_the_envelope() {
    bounded(async {
        let (table, fault, id, info) = fixture().await;
        table.add_validator_binding(binding(id)).await.unwrap();
        let before = load_validators_metadata(&info).await.unwrap().unwrap();
        assert_eq!(before.bindings.len(), 1);
        let flushes_before = fault.flush_calls();
        let observations_before = fault.live_observations().len();

        fault.fail_next_flushes(1);
        let error = table
            .remove_validator_binding(&id)
            .await
            .expect_err("the failed flush must not be reported as a successful unbind");
        assert_eq!(
            fault.flush_calls(),
            flushes_before + 2,
            "F1: the failed unbind flush must be followed by ONE flushed compensation"
        );
        assert!(matches!(error, DbError::Storage(_)), "{error}");
        assert!(
            table
                .validator_bindings()
                .iter()
                .any(|binding| binding.validator_id == id),
            "F3: the live removal must not be published before the flush succeeds"
        );
        assert_eq!(
            &fault.live_observations()[observations_before..],
            &[true, true],
            "F3: the FORWARD flush must run while the live binding is still present (publishing \
             the removal first is the regression this pins) and the compensation flush must \
             observe the same live binding; saw {:?}",
            fault.live_observations()
        );
        assert!(
            !table.validator_bindings_in_doubt(),
            "a compensated failure leaves a coherent table"
        );

        let after = load_validators_metadata(&info).await.unwrap().unwrap();
        assert_eq!(after.bindings.len(), 1);
        assert_eq!(after.bindings[0].validator_id, id);

        let removed = table
            .remove_validator_binding(&id)
            .await
            .expect("a retry after a recovered flush must finish the removal");
        assert!(removed);
        assert!(table.validator_bindings().is_empty());
        assert!(load_validators_metadata(&info)
            .await
            .unwrap()
            .unwrap()
            .bindings
            .is_empty());
        fault.detach();
    })
    .await;
}

#[tokio::test]
async fn unbind_compensation_flush_fault_marks_the_table_in_doubt() {
    bounded(async {
        let (table, fault, id, _info) = fixture().await;
        table.add_validator_binding(binding(id)).await.unwrap();
        fault.fail_next_flushes(usize::MAX);
        let observations_before = fault.live_observations().len();

        let error = table
            .remove_validator_binding(&id)
            .await
            .expect_err("an unreconciled unbind must fail");
        assert!(is_in_doubt(&error), "{error}");
        assert!(table.validator_bindings_in_doubt());

        assert_eq!(
            &fault.live_observations()[observations_before..],
            &[true, true, true, true],
            "F3: the forward flush and all three compensation attempts must run while the live \
             binding is still present; saw {:?}",
            fault.live_observations()
        );
        assert!(
            table
                .validator_bindings()
                .iter()
                .any(|binding| binding.validator_id == id),
            "F3: the live binding stays in place on an in-doubt unbind"
        );
        let again = table
            .add_validator_binding(binding(id))
            .await
            .expect_err("bind must refuse while the table is in doubt");
        assert!(is_in_doubt(&again), "{again}");
        fault.detach();
    })
    .await;
}
