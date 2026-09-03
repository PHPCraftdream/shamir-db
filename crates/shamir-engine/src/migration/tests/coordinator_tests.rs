use async_trait::async_trait;
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;

use crate::migration::coordinator::{MigrationCoordinator, MigrationPhase, MigrationState};
use crate::migration::shadow_log::{MigrationShadowLog, ShadowOp, READ_FROM_PAGE_CAP};

type TestStream = Pin<
    Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, shamir_storage::error::DbError>> + Send>,
>;

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>, Arc<dyn Store>) {
    let info = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let src = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let dst = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    (info, src, dst)
}

async fn seed_src(store: &Arc<dyn Store>, n: usize) -> Vec<RecordKey> {
    let mut keys = Vec::new();
    for i in 0..n {
        let k = store.insert(Bytes::from(format!("val_{i}"))).await.unwrap();
        keys.push(k);
    }
    keys
}

#[tokio::test]
async fn full_migration_lifecycle() {
    let (info, src, dst) = make_stores();
    let _keys = seed_src(&src, 10).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig1".into(), info));
    let state = MigrationState::new(
        "mig1".into(),
        "users".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone(), None);

    assert_eq!(coord.phase().await, MigrationPhase::ShadowStarted);

    let copied = coord.run_snapshot().await.unwrap();

    // Simulate a write that arrives after snapshot cut
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"concurrent_write".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(copied, 10);
    assert_eq!(coord.phase().await, MigrationPhase::Draining);

    let drained = coord.drain_until_caught_up(0).await.unwrap();
    assert_eq!(drained.total_applied, 1);
    assert_eq!(drained.residual_lag, 0);

    coord.mark_cutover_ready().await.unwrap();
    assert_eq!(coord.phase().await, MigrationPhase::CutoverReady);

    // One more write during cutover prep
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"late_write".to_vec(),
        })
        .await
        .unwrap();

    let final_drained = coord.final_drain_and_commit().await.unwrap();
    assert_eq!(final_drained, 1);
    assert_eq!(coord.phase().await, MigrationPhase::Committed);

    let (src_count, dst_count) = coord.verify_record_count().await.unwrap();
    assert_eq!(src_count, 10);
    // dst has 10 snapshot + 2 concurrent writes
    assert_eq!(dst_count, 12);
}

#[tokio::test]
async fn rollback_before_commit() {
    let (info, src, dst) = make_stores();
    seed_src(&src, 5).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig2".into(), info));
    let state = MigrationState::new(
        "mig2".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src, dst, None);

    coord.run_snapshot().await.unwrap();
    coord.rollback().await.unwrap();
    assert_eq!(coord.phase().await, MigrationPhase::RolledBack);
}

#[tokio::test]
async fn cannot_rollback_after_commit() {
    let (info, src, dst) = make_stores();
    seed_src(&src, 3).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig3".into(), info));
    let state = MigrationState::new(
        "mig3".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow, src, dst, None);

    coord.run_snapshot().await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    coord.final_drain_and_commit().await.unwrap();

    let err = coord.rollback().await.unwrap_err();
    assert!(err.to_string().contains("committed"));
}

#[tokio::test]
async fn phase_transitions_enforced() {
    let (info, src, dst) = make_stores();

    let shadow = Arc::new(MigrationShadowLog::new("mig4".into(), info));
    let state = MigrationState::new(
        "mig4".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow, src, dst, None);

    // Can't drain before snapshot
    assert!(coord.drain_shadow_log().await.is_err());
    // Can't mark cutover_ready before draining
    assert!(coord.mark_cutover_ready().await.is_err());
    // Can't final_drain before cutover_ready
    assert!(coord.final_drain_and_commit().await.is_err());
}

#[tokio::test]
async fn deletes_propagated_to_dst() {
    let (info, src, dst) = make_stores();
    let keys = seed_src(&src, 5).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig5".into(), info));
    let state = MigrationState::new(
        "mig5".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone(), None);

    coord.run_snapshot().await.unwrap();

    // Delete record from src (shadow captures it)
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&keys[0].as_ref()[..16]);
    let rid = RecordId(arr);
    shadow
        .append(ShadowOp::Delete { record_id: rid })
        .await
        .unwrap();

    coord.drain_until_caught_up(0).await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    coord.final_drain_and_commit().await.unwrap();

    let (_, dst_count) = coord.verify_record_count().await.unwrap();
    assert_eq!(dst_count, 4);
}

/// Tallies `set`/`set_many`/`remove`/`remove_many` calls against a wrapped
/// `Arc<dyn Store>` — used to prove a drain batch applies via ONE batched
/// call per op kind, not one round-trip per entry (defect 2).
#[derive(Default)]
struct CallCounts {
    set: AtomicUsize,
    set_many: AtomicUsize,
    set_many_items: AtomicUsize,
    remove: AtomicUsize,
    remove_many: AtomicUsize,
    remove_many_items: AtomicUsize,
}

struct CountingStore {
    inner: Arc<dyn Store>,
    counts: Arc<CallCounts>,
}

#[async_trait]
impl Store for CountingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.counts.set.fetch_add(1, SeqCst);
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.counts.remove.fetch_add(1, SeqCst);
        self.inner.remove(key).await
    }
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.counts.set_many.fetch_add(1, SeqCst);
        self.counts.set_many_items.fetch_add(items.len(), SeqCst);
        self.inner.set_many(items).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.counts.remove_many.fetch_add(1, SeqCst);
        self.counts.remove_many_items.fetch_add(keys.len(), SeqCst);
        self.inner.remove_many(keys).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// Defect 2 regression: a drain batch with a mix of `Put` and `Delete`
/// entries must apply all of them correctly via ONE `set_many` call
/// (all puts) and ONE `remove_many` call (all deletes) — not a
/// per-entry `set`/`remove` round trip each.
#[tokio::test]
async fn drain_applies_mixed_put_delete_via_batched_calls() {
    let (info, src, _unused_dst) = make_stores();
    let raw_dst = Arc::new(InMemoryStore::new());
    let counts = Arc::new(CallCounts::default());
    let dst: Arc<dyn Store> = Arc::new(CountingStore {
        inner: raw_dst.clone(),
        counts: Arc::clone(&counts),
    });

    let shadow = Arc::new(MigrationShadowLog::new("migbatch".into(), info));
    let state = MigrationState::new(
        "migbatch".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src, dst, None);

    coord.run_snapshot().await.unwrap(); // -> Draining, 0 src rows

    let put_ids: Vec<RecordId> = (0..3).map(|_| RecordId::new()).collect();
    let del_ids: Vec<RecordId> = (0..2).map(|_| RecordId::new()).collect();

    // Pre-seed the delete targets directly on the raw (uncounted) store so
    // the counters below reflect only the drain's own calls.
    for id in &del_ids {
        raw_dst
            .set(
                RecordKey::from(id.as_bytes().to_vec()),
                Bytes::from_static(b"stale"),
            )
            .await
            .unwrap();
    }

    for (i, id) in put_ids.iter().enumerate() {
        shadow
            .append(ShadowOp::Put {
                record_id: *id,
                value: format!("v-{i}").into_bytes(),
            })
            .await
            .unwrap();
    }
    for id in &del_ids {
        shadow
            .append(ShadowOp::Delete { record_id: *id })
            .await
            .unwrap();
    }

    let applied = coord.drain_shadow_log().await.unwrap();
    assert_eq!(applied, 5);

    // Batched, not per-entry: exactly one set_many covering all 3 puts,
    // one remove_many covering both deletes, zero singular set/remove.
    assert_eq!(counts.set.load(SeqCst), 0);
    assert_eq!(counts.set_many.load(SeqCst), 1);
    assert_eq!(counts.set_many_items.load(SeqCst), 3);
    assert_eq!(counts.remove.load(SeqCst), 0);
    assert_eq!(counts.remove_many.load(SeqCst), 1);
    assert_eq!(counts.remove_many_items.load(SeqCst), 2);

    // Correctness: puts landed with the right values, deletes are gone.
    for (i, id) in put_ids.iter().enumerate() {
        let v = raw_dst
            .get(RecordKey::from(id.as_bytes().to_vec()))
            .await
            .unwrap();
        assert_eq!(v.as_ref(), format!("v-{i}").into_bytes().as_slice());
    }
    for id in &del_ids {
        assert!(raw_dst
            .get(RecordKey::from(id.as_bytes().to_vec()))
            .await
            .is_err());
    }
}

/// Defect 3 regression: a COMMITTED migration's shadow log must actually
/// be gone from the backing store afterward — not just that commit
/// succeeded. Only the rollback path purged before this fix; a committed
/// migration leaked its full shadow log forever.
#[tokio::test]
async fn committed_migration_purges_shadow_log() {
    let (info, src, dst) = make_stores();
    seed_src(&src, 3).await;

    let shadow = Arc::new(MigrationShadowLog::new("migpurge".into(), info));
    let state = MigrationState::new(
        "migpurge".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src, dst, None);

    coord.run_snapshot().await.unwrap();
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"late".to_vec(),
        })
        .await
        .unwrap();
    coord.mark_cutover_ready().await.unwrap();
    coord.final_drain_and_commit().await.unwrap();

    assert_eq!(coord.phase().await, MigrationPhase::Committed);
    // The shadow log itself must be empty on disk now — not merely that
    // the phase transitioned.
    assert!(shadow.read_from(1).await.unwrap().is_empty());
}

/// Defect 4 regression (the most important test in this group):
/// `drain_until_caught_up` must TERMINATE under sustained source writes
/// — writes arriving faster than a bounded drain pass can catch up —
/// instead of looping forever on `while applied > 0`. Wrapped in a
/// bounded `tokio::time::timeout` so a real regression (infinite loop)
/// fails this test fast instead of hanging the suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_until_caught_up_terminates_under_sustained_writes() {
    let (info, src, dst) = make_stores();

    let shadow = Arc::new(MigrationShadowLog::new("migsustained".into(), info));
    let state = MigrationState::new(
        "migsustained".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = Arc::new(MigrationCoordinator::new(
        state,
        shadow.clone(),
        src,
        dst,
        None,
    ));

    coord.run_snapshot().await.unwrap(); // -> Draining, 0 src rows

    // Deterministic backlog: bigger than what DRAIN_PASS_CAP passes at
    // READ_FROM_PAGE_CAP entries/pass could possibly drain, so the pass
    // budget MUST be exhausted (and residual_lag > 0) regardless of how
    // the concurrent writer below happens to be scheduled.
    let guaranteed_undrainable =
        (MigrationCoordinator::DRAIN_PASS_CAP as u64) * (READ_FROM_PAGE_CAP as u64);
    let backlog = guaranteed_undrainable * 2;
    let seed_ops: Vec<ShadowOp> = (0..backlog)
        .map(|_| ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .collect();
    shadow.append_batch(seed_ops).await.unwrap();

    // Sustained concurrent writer: keeps appending for the duration of
    // the drain call, simulating source writes that never let up.
    let stop = Arc::new(AtomicBool::new(false));
    let writer_shadow = Arc::clone(&shadow);
    let writer_stop = Arc::clone(&stop);
    let writer = tokio::spawn(async move {
        while !writer_stop.load(SeqCst) {
            writer_shadow
                .append(ShadowOp::Delete {
                    record_id: RecordId::new(),
                })
                .await
                .unwrap();
        }
    });

    let result = tokio::time::timeout(Duration::from_secs(30), coord.drain_until_caught_up(0))
        .await
        .expect("drain_until_caught_up must terminate within its pass budget, not livelock")
        .unwrap();

    stop.store(true, SeqCst);
    writer.await.unwrap();

    assert_eq!(result.passes, MigrationCoordinator::DRAIN_PASS_CAP);
    assert!(
        result.residual_lag > 0,
        "expected residual lag after the pass budget was spent under \
         sustained writes, got {result:?}"
    );
}
