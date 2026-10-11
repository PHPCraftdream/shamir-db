use async_trait::async_trait;
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::collections::BTreeMap;
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

async fn assert_shadow_replay(ops: Vec<ShadowOp>, ids: &[RecordId], final_drain: bool) {
    let (info, src, dst) = make_stores();
    let shadow = Arc::new(MigrationShadowLog::new("migorder".into(), info));
    let state = MigrationState::new(
        "migorder".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "memory".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone(), None);
    let mut model = BTreeMap::new();
    for id in ids {
        src.set(
            RecordKey::from(id.as_bytes().to_vec()),
            Bytes::from_static(b"snapshot"),
        )
        .await
        .unwrap();
        model.insert(*id, b"snapshot".to_vec());
    }
    assert_eq!(coord.run_snapshot().await.unwrap(), ids.len() as u64);
    for op in &ops {
        match op {
            ShadowOp::Put { record_id, value } => {
                model.insert(*record_id, value.clone());
            }
            ShadowOp::Delete { record_id } => {
                model.remove(record_id);
            }
        }
    }
    let expected_applied = ops.len() as u64;
    if final_drain {
        coord.mark_cutover_ready().await.unwrap();
    }
    let lsns = shadow.append_batch(ops).await.unwrap();
    let applied = if final_drain {
        coord.final_drain_and_commit().await.unwrap()
    } else {
        coord.drain_shadow_log().await.unwrap()
    };
    assert_eq!(applied, expected_applied);
    assert_eq!(coord.state().await.last_lsn_applied, *lsns.last().unwrap());
    assert_eq!(coord.shadow_lag().await, 0);
    assert_eq!(
        coord.phase().await,
        if final_drain {
            MigrationPhase::Committed
        } else {
            MigrationPhase::Draining
        },
    );
    for id in ids {
        let actual = dst
            .get_many(vec![RecordKey::from(id.as_bytes().to_vec())])
            .await
            .unwrap();
        assert_eq!(actual, vec![model.get(id).cloned().map(Bytes::from)]);
    }
    if final_drain {
        assert!(shadow.read_from(1).await.unwrap().is_empty());
    } else {
        assert_eq!(
            shadow.read_from(1).await.unwrap().len() as u64,
            expected_applied,
        );
        assert_eq!(coord.drain_shadow_log().await.unwrap(), 0);
    }
}

async fn assert_same_key_sequence(kinds: &[bool]) {
    let id = RecordId([7; 16]);
    for final_drain in [false, true] {
        let ops = kinds
            .iter()
            .enumerate()
            .map(|(i, put)| {
                if *put {
                    ShadowOp::Put {
                        record_id: id,
                        value: format!("value-{i}").into_bytes(),
                    }
                } else {
                    ShadowOp::Delete { record_id: id }
                }
            })
            .collect();
        assert_shadow_replay(ops, &[id], final_drain).await;
    }
}

#[tokio::test]
async fn shadow_delete_then_put_preserves_lsn_order() {
    assert_same_key_sequence(&[false, true]).await;
}

#[tokio::test]
async fn shadow_put_then_delete_preserves_lsn_order() {
    assert_same_key_sequence(&[true, false]).await;
}

#[tokio::test]
async fn shadow_put_delete_put_preserves_lsn_order() {
    assert_same_key_sequence(&[true, false, true]).await;
}

#[tokio::test]
async fn shadow_delete_put_delete_preserves_lsn_order() {
    assert_same_key_sequence(&[false, true, false]).await;
}

#[tokio::test]
async fn shadow_duplicate_put_run_is_last_writer_wins() {
    let ids = [RecordId([8; 16]), RecordId([9; 16])];
    for final_drain in [false, true] {
        let ops = [0, 1, 0, 1, 0]
            .iter()
            .enumerate()
            .map(|(i, key)| ShadowOp::Put {
                record_id: ids[*key],
                value: format!("overwrite-{i}").into_bytes(),
            })
            .collect();
        assert_shadow_replay(ops, &ids, final_drain).await;
    }
}

fn lcg_next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state
}

async fn assert_lcg_shadow_model(ids: &[RecordId]) {
    for seed in [1u64, 0x5eed, 0xdead_beef] {
        let mut rng = seed;
        for iteration in 0..128 {
            let mut ops = Vec::with_capacity(66);
            for step in 0..64 {
                let key = usize::try_from((lcg_next(&mut rng) >> 32) % ids.len() as u64).unwrap();
                let id = ids[key];
                if (lcg_next(&mut rng) >> 32) & 1 == 0 {
                    ops.push(ShadowOp::Put {
                        record_id: id,
                        value: format!("{seed}-{iteration}-{step}").into_bytes(),
                    });
                } else {
                    ops.push(ShadowOp::Delete { record_id: id });
                }
            }
            if iteration % 4 == 0 {
                ops.push(ShadowOp::Delete { record_id: ids[0] });
                ops.push(ShadowOp::Put {
                    record_id: ids[0],
                    value: b"resurrected".to_vec(),
                });
            }
            assert_shadow_replay(ops, ids, iteration % 2 == 0).await;
        }
    }
}

#[tokio::test]
async fn shadow_lcg_one_key_matches_sequential_model() {
    assert_lcg_shadow_model(&[RecordId([10; 16])]).await;
}

#[tokio::test]
async fn shadow_lcg_multiple_keys_match_sequential_model() {
    assert_lcg_shadow_model(&[
        RecordId([10; 16]),
        RecordId([11; 16]),
        RecordId([12; 16]),
        RecordId([13; 16]),
    ])
    .await;
}

/// Tallies calls and records ordered batches to distinguish adjacent runs
/// from global partitions and per-entry round trips.
#[derive(Default)]
struct CallCounts {
    set: AtomicUsize,
    set_many: AtomicUsize,
    set_many_items: AtomicUsize,
    remove: AtomicUsize,
    remove_many: AtomicUsize,
    remove_many_items: AtomicUsize,
    batches: tokio::sync::Mutex<Vec<BatchCall>>,
}

#[derive(Debug, PartialEq, Eq)]
enum BatchCall {
    Put(Vec<(RecordKey, Bytes)>),
    Delete(Vec<RecordKey>),
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
        self.counts
            .batches
            .lock()
            .await
            .push(BatchCall::Put(items.clone()));
        self.inner.set_many(items).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.counts.remove_many.fetch_add(1, SeqCst);
        self.counts.remove_many_items.fetch_add(keys.len(), SeqCst);
        self.counts
            .batches
            .lock()
            .await
            .push(BatchCall::Delete(keys.clone()));
        self.inner.remove_many(keys).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// Adjacent runs must be batched without moving puts across deletes.
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

    let put_ids = [RecordId([1; 16]), RecordId([2; 16]), RecordId([3; 16])];
    let del_ids = [RecordId([4; 16]), RecordId([5; 16])];

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

    for (i, id) in put_ids[..2].iter().enumerate() {
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

    shadow
        .append(ShadowOp::Put {
            record_id: put_ids[2],
            value: b"v-2".to_vec(),
        })
        .await
        .unwrap();

    let applied = coord.drain_shadow_log().await.unwrap();
    assert_eq!(applied, 5);

    assert_eq!(counts.set.load(SeqCst), 0);
    assert_eq!(counts.set_many.load(SeqCst), 2);
    assert_eq!(counts.set_many_items.load(SeqCst), 3);
    assert_eq!(counts.remove.load(SeqCst), 0);
    assert_eq!(counts.remove_many.load(SeqCst), 1);
    assert_eq!(counts.remove_many_items.load(SeqCst), 2);
    assert_eq!(
        *counts.batches.lock().await,
        vec![
            BatchCall::Put(vec![
                (
                    RecordKey::from(put_ids[0].as_bytes().to_vec()),
                    Bytes::from_static(b"v-0"),
                ),
                (
                    RecordKey::from(put_ids[1].as_bytes().to_vec()),
                    Bytes::from_static(b"v-1"),
                ),
            ]),
            BatchCall::Delete(
                del_ids
                    .iter()
                    .map(|id| RecordKey::from(id.as_bytes().to_vec()))
                    .collect(),
            ),
            BatchCall::Put(vec![(
                RecordKey::from(put_ids[2].as_bytes().to_vec()),
                Bytes::from_static(b"v-2"),
            )]),
        ],
    );

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
    coord.mark_cutover_ready().await.unwrap();
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"late".to_vec(),
        })
        .await
        .unwrap();
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
