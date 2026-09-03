//! `RecordCounter` regression tests, including task-group-14 / #14
//! ("fail closed on init errors and fix the dirty-flag race"):
//!
//! - Defect 1: `ensure_cache` used to map ANY info_store read error
//!   (including genuine transient storage faults) AND any corrupt
//!   persisted blob to `0`, silently resetting a durable count and
//!   letting the next `persist()` clobber it. Fixed to propagate both
//!   classes of error and only default to `0` on a genuine
//!   `DbError::NotFound` (fresh table).
//! - Defect 2: `set()`/`persist()` used to clear a boolean `dirty`
//!   flag unconditionally after `write_through(..).await`, so a
//!   concurrent `increment()` landing inside that await window had its
//!   `dirty.store(true)` mark silently erased — the delta became
//!   invisible to the next `persist()`'s fast-path skip. Fixed by
//!   removing the separate flag and deriving "is there unpersisted
//!   work" directly from `cache != last_persisted`.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use tokio::sync::Notify;

use crate::meta::MetaKey;
use crate::table::record_counter::RecordCounter;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::codecs::basic::bincode;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

fn count_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::Count.as_record_id().as_bytes())
}

async fn create_counter() -> RecordCounter {
    RecordCounter::new(Arc::new(InMemoryStore::new()))
}

// ---------------------------------------------------------------------------
// Fault-injection wrappers (mirrors `FailingScanStore` /
// `r0d_fail_closed_recovery_tests.rs`'s pattern: only the methods
// without a default trait body need overriding, everything else
// delegates straight through).
// ---------------------------------------------------------------------------

/// `Store` wrapper whose `get()` always fails with a genuine, non-
/// `NotFound` error — simulates a transient storage-layer read fault
/// (as opposed to "key genuinely does not exist yet").
struct ErrorOnGetStore {
    inner: Arc<dyn Store>,
}

#[async_trait]
impl Store for ErrorOnGetStore {
    async fn insert(&self, value: Bytes) -> shamir_storage::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> shamir_storage::error::DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, _key: RecordKey) -> shamir_storage::error::DbResult<Bytes> {
        Err(DbError::Storage(
            "ErrorOnGetStore: injected transient read failure".to_string(),
        ))
    }
    async fn remove(&self, key: RecordKey) -> shamir_storage::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// `Store` wrapper whose `set()` can be armed to pause mid-call: it
/// signals `reached` then parks on `resume` before delegating to
/// `inner`. Gives a test full rendezvous control to land a concurrent
/// mutation exactly inside `write_through(..).await`'s window,
/// instead of relying on a timing-dependent race.
struct PausableSetStore {
    inner: Arc<dyn Store>,
    armed: AtomicBool,
    reached: Notify,
    resume: Notify,
}

impl PausableSetStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: AtomicBool::new(false),
            reached: Notify::new(),
            resume: Notify::new(),
        }
    }

    fn arm(&self) {
        self.armed.store(true, SeqCst);
    }

    /// Test side: block until a `set()` call has parked.
    async fn wait_until_parked(&self) {
        self.reached.notified().await;
    }

    /// Test side: let the parked `set()` call proceed.
    fn release(&self) {
        self.resume.notify_one();
    }
}

#[async_trait]
impl Store for PausableSetStore {
    async fn insert(&self, value: Bytes) -> shamir_storage::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> shamir_storage::error::DbResult<bool> {
        if self.armed.swap(false, SeqCst) {
            self.reached.notify_one();
            self.resume.notified().await;
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> shamir_storage::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> shamir_storage::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

// ---------------------------------------------------------------------------
// Pre-existing tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_counter_initial_state() {
    let counter = create_counter().await;
    assert_eq!(counter.get().await.unwrap(), 0);
}

#[tokio::test]
async fn test_counter_increment() {
    let counter = create_counter().await;
    counter.increment(1).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 1);

    counter.increment(5).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 6);
}

#[tokio::test]
async fn test_counter_decrement() {
    let counter = create_counter().await;
    counter.increment(10).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 10);

    counter.increment(-3).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 7);
}

#[tokio::test]
async fn test_counter_cannot_go_negative() {
    let counter = create_counter().await;
    counter.increment(5).await.unwrap();

    let result = counter.increment(-10).await;
    assert!(result.is_err());
    assert_eq!(counter.get().await.unwrap(), 5);
}

#[tokio::test]
async fn test_counter_set() {
    let counter = create_counter().await;
    counter.set(100).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 100);

    counter.set(50).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 50);
}

#[tokio::test]
async fn test_counter_thread_safety() {
    let counter = Arc::new(create_counter().await);
    let mut handles = vec![];

    for _i in 0..10 {
        let counter_clone = Arc::clone(&counter);
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                counter_clone.increment(1).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(counter.get().await.unwrap(), 100);
}

// ---------------------------------------------------------------------------
// Defect 1 — fail-open on init errors (task-group-14 / #14)
// ---------------------------------------------------------------------------

/// Legitimate "key genuinely does not exist yet" case (fresh table):
/// must still initialise to `0`. Distinguishes this from the other two
/// cases below, which must NOT default to `0`.
#[tokio::test]
async fn fresh_table_with_no_persisted_key_initializes_to_zero() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Sanity: the key genuinely is not present (NotFound), not merely
    // unreachable via some other fault.
    assert!(matches!(
        inner.get(count_key()).await,
        Err(DbError::NotFound(_))
    ));

    let counter = RecordCounter::new(inner);
    assert_eq!(
        counter.get().await.unwrap(),
        0,
        "a genuinely fresh table (NotFound) must still initialize the counter to 0"
    );
}

/// A genuine (non-`NotFound`) info_store read error must propagate,
/// not silently reset the counter to 0 — and, critically, a `persist()`
/// after the failed init must NOT clobber the durable count that was
/// already on disk from a prior session.
#[tokio::test]
async fn genuine_read_error_propagates_and_does_not_clobber_durable_count() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Simulate a prior session that durably persisted a real count.
    let counter1 = RecordCounter::new(Arc::clone(&inner));
    counter1.set(10_000).await.unwrap();

    // Reopen with a store whose `get()` deterministically fails with a
    // genuine storage-layer error (transient fault / bit-flip class,
    // NOT "key not found").
    let failing: Arc<dyn Store> = Arc::new(ErrorOnGetStore {
        inner: Arc::clone(&inner),
    });
    let counter2 = RecordCounter::new(failing);

    let get_result = counter2.get().await;
    assert!(
        matches!(get_result, Err(DbError::Storage(_))),
        "a genuine info_store read error must propagate as Err, not silently \
         default to 0 — got {get_result:?}"
    );

    // A persist() attempted after the failed init must also propagate
    // (there is no valid in-memory value to persist) rather than
    // silently succeeding with a defaulted-on-error value.
    let persist_result = counter2.persist().await;
    assert!(
        persist_result.is_err(),
        "persist() after a failed init must propagate the error too, got {persist_result:?}"
    );

    // The durable count on the UNDERLYING (unwrapped) store must be
    // untouched — still 10_000, not clobbered with 0/near-zero.
    let raw = inner.get(count_key()).await.unwrap();
    let durable: u64 = bincode::from_bytes(&raw).unwrap();
    assert_eq!(
        durable, 10_000,
        "the previously durable count must survive a failed re-init unclobbered"
    );
}

/// A corrupt persisted blob (fails to decode as the counter's `u64`)
/// must propagate a decode error, not silently reset to 0 — and must
/// not trigger any write-back that would clobber the corrupt-but-
/// present bytes with a bogus defaulted value.
#[tokio::test]
async fn corrupt_persisted_blob_propagates_and_does_not_reset_to_zero() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Too short to ever decode as a `u64` under any bincode int
    // encoding scheme — guaranteed decode failure, not a coincidental
    // valid-looking value.
    let garbage = Bytes::new();
    inner.set(count_key(), garbage.clone()).await.unwrap();

    let counter = RecordCounter::new(Arc::clone(&inner));
    let result = counter.get().await;
    assert!(
        matches!(result, Err(DbError::Codec(_))),
        "a corrupt persisted blob must propagate a decode error, not silently \
         default to 0 — got {result:?}"
    );

    // No write-back must have happened: the corrupt bytes are exactly
    // as they were written.
    let raw_after = inner.get(count_key()).await.unwrap();
    assert_eq!(
        raw_after.as_ref(),
        garbage.as_ref(),
        "a failed decode must not trigger a write-back that touches the stored bytes"
    );
}

// ---------------------------------------------------------------------------
// Defect 2 — dirty-flag race loses concurrent increments (task-group-14 / #14)
// ---------------------------------------------------------------------------

/// Races a `persist()` call against a concurrent `increment()` landing
/// deterministically INSIDE `persist()`'s `write_through(..).await`
/// window (via `PausableSetStore`'s rendezvous), and proves the
/// increment's delta is not lost: a second `persist()` call afterwards
/// must actually flush it, instead of skipping via a stale "not dirty"
/// signal.
///
/// Pre-fix mechanism being reproduced: `increment()`'s
/// `dirty.store(true)` (issued while `persist()`'s write is in-flight)
/// happens-before `persist()`'s post-write unconditional
/// `dirty.store(false)`, so the flag is left `false` even though the
/// cache has since diverged from `last_persisted` — the second
/// `persist()` call then sees `dirty == false` and no-ops, permanently
/// dropping the delta from the durable store.
#[tokio::test]
async fn concurrent_increment_during_persist_write_is_not_lost() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let pausable = Arc::new(PausableSetStore::new(Arc::clone(&inner)));
    let store: Arc<dyn Store> = pausable.clone();
    let counter = RecordCounter::new(store);

    // Establish a clean baseline: cache == last_persisted == 10.
    counter.increment(10).await.unwrap();
    counter.persist().await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 10);

    // Dirty the cache, then arm the pause so the NEXT `set()` call
    // (persist()'s write_through) parks mid-write.
    counter.increment(5).await.unwrap(); // cache = 15
    pausable.arm();

    let persisting = counter.clone();
    let persist_task = tokio::spawn(async move { persisting.persist().await });

    // Wait until persist() is parked inside write_through(15).await.
    pausable.wait_until_parked().await;

    // Land a concurrent increment INSIDE the paused write's window —
    // this is exactly the race window the defect describes.
    counter.increment(7).await.unwrap(); // cache = 22

    // Let the paused write_through(15) complete.
    pausable.release();
    persist_task.await.unwrap().unwrap();

    // The in-memory cache must reflect both increments regardless of
    // the fix (lock-free fetch_add is never lost).
    assert_eq!(counter.get().await.unwrap(), 22);

    // The critical assertion: a follow-up persist() must actually
    // flush the concurrent increment's delta — the durable store must
    // NOT be stuck at the pre-race value of 15.
    counter.persist().await.unwrap();
    let raw = inner.get(count_key()).await.unwrap();
    let durable: u64 = bincode::from_bytes(&raw).unwrap();
    assert_eq!(
        durable, 22,
        "a concurrent increment() landing inside persist()'s write_through(..).await \
         window must not be lost — the durable count must eventually reach 22, not \
         stay stuck at the pre-race value of 15"
    );
}
