//! SNAP-3a: explicit DDL pins, independent of transaction snapshot policy.
//!
//! The lagging published marker below is a SYNTHETIC fixture, not a
//! reproduction of a production commit: apply_committed_visible eagerly
//! raises last_committed to 11 today. Parking v10 alone would NOT make the
//! old online build fail. publish_committed(W) deliberately separates the
//! published marker from the allocation high water after materializing v11.

use std::sync::Arc;

use futures::FutureExt;
use shamir_query_builder::ddl::create_index;
use shamir_query_types::batch::BatchOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate, VersionGuard};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::index2::backend::{IndexQuery, IndexResult};
use crate::index2::functional_backend::FunctionalBackend;
use crate::index2::state::IndexState;
use crate::table::TableManager;

async fn table_at_gap() -> (
    TableManager,
    Arc<MvccStore>,
    Arc<RepoTxGate>,
    VersionGuard,
    RecordId,
) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gate = Arc::new(RepoTxGate::new(9, 1));
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let tbl = TableManager::create("target".into(), data, info)
        .await
        .unwrap()
        .with_mvcc_store(Arc::clone(&mvcc))
        .with_changefeed(Arc::clone(&gate));
    let field = key_id(&tbl, "name").await;
    let mut record = new_map();
    record.insert(InternerKey::new(field), InnerValue::Str("Alice".into()));
    let bytes = InnerValue::Map(record).to_bytes().unwrap();
    let rid = RecordId::new();

    // v10 belongs to another table; keep its owner alive without taking
    // target's writer-drain guard or write lock. No spawned commit/ack wait.
    let parked = gate.assign_next_version_guarded();
    assert_eq!(parked.version(), 10);
    let completed = gate.assign_next_version_guarded();
    assert_eq!(completed.version(), 11);
    mvcc.apply_committed_visible(&[KvOp::Set(RecordKey::from(rid.to_bytes()), bytes)], 11);
    completed.commit();
    assert_eq!(gate.completion().watermark(), 9);
    assert_eq!(
        gate.last_committed(),
        11,
        "materialization eagerly publishes"
    );
    gate.publish_committed(gate.completion().watermark());
    assert_eq!(gate.last_committed(), 9);
    assert_eq!(gate.version_allocation_high_water_mark(), 11);
    (tbl, mvcc, gate, parked, rid)
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    tbl.interner()
        .get()
        .await
        .unwrap()
        .touch_ind(name)
        .unwrap()
        .key()
        .id()
}

/// Red counterfactual: old open_snapshot pins 9, so Phase A skips the v11
/// row. It predates dirty-set registration, so C/D cannot recover it.
#[tokio::test]
async fn explicit_pin_online_build_keeps_completed_row_above_published_marker() {
    let (tbl, mvcc, gate, parked, rid) = table_at_gap().await;
    let name = key_id(&tbl, "idx_name").await;
    let field = key_id(&tbl, "name").await;
    let def = IndexDefinition {
        name_interned: name,
        paths: vec![IndexInfoItem::new(vec![field])],
        state: IndexState::Building,
        instance_epoch: 0,
    };
    assert_eq!(mvcc.get_at(&rid.to_bytes(), 9).await.unwrap(), None);
    assert!(mvcc.get_at(&rid.to_bytes(), 11).await.unwrap().is_some());
    let permit = tbl.write_domain.admit().await.unwrap();
    let phase = tbl
        .phase_b_a_backfill("idx_name", def, 1, &permit)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(phase.pin, 11, "DDL must not inherit the transaction pin");
    assert_eq!(phase.guard.version(), phase.pin);
    assert!(!gate.active_snapshots_empty());
    tbl.phase_c_d_catchup_and_publish(name, phase, &permit)
        .await
        .unwrap();
    assert!(gate.active_snapshots_empty(), "C/D releases the build pin");
    let ready = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|def| def.name_interned == name)
        .unwrap();
    assert_eq!(ready.state, IndexState::Ready);
    let owners = tbl
        .index_manager_ref()
        .lookup_by_index(name, &[InnerValue::Str("Alice".into())])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owners.as_ref(), &[rid]);
    assert_eq!(gate.completion().watermark(), 9, "DDL does not settle v10");
    drop(parked);
    assert_eq!(gate.completion().watermark(), 11);
}

/// Detect reading high water before drain: the writer allocates v12 only
/// after the build has been polled into its drain wait.
#[tokio::test]
async fn explicit_pin_is_read_after_writer_drain() {
    let (tbl, _mvcc, gate, parked, _rid) = table_at_gap().await;
    let writer = tbl.enter_writer_drain();
    let def = IndexDefinition {
        name_interned: key_id(&tbl, "idx_drain").await,
        paths: vec![IndexInfoItem::new(vec![key_id(&tbl, "name").await])],
        state: IndexState::Building,
        instance_epoch: 0,
    };
    let permit = tbl.write_domain.admit().await.unwrap();
    let build = tbl.phase_b_a_backfill("idx_drain", def, 1, &permit);
    futures::pin_mut!(build);
    assert!(build.as_mut().now_or_never().is_none());
    let later = gate.assign_next_version_guarded();
    assert_eq!(later.version(), 12);
    later.commit();
    drop(writer);
    let phase = build.await.unwrap().unwrap();
    assert_eq!(phase.pin, 12);
    drop(phase);
    assert!(gate.active_snapshots_empty());
    drop(parked);
}

/// Exact means neither clamp to W/last_committed nor chase a newer marker.
/// Duplicate guards must independently own their registration refcounts.
#[tokio::test]
async fn explicit_pin_exact_versions_and_duplicate_guard_release() {
    let gate = RepoTxGate::new(9, 1);
    let first = gate.open_snapshot_at(11).await;
    let second = gate.open_snapshot_at(11).await;
    let older = gate.open_snapshot_at(5).await;
    assert_eq!(first.version(), 11);
    assert_eq!(second.version(), 11);
    assert_eq!(older.version(), 5);
    gate.publish_committed(20);
    assert_eq!(first.version(), 11);
    assert_eq!(older.version(), 5);
    assert!(!gate.snapshots_opening());
    assert_eq!(gate.active_serializable_count(), 0);
    drop(older);
    drop(first);
    assert!(!gate.active_snapshots_empty(), "second still owns v11");
    drop(second);
    assert!(gate.active_snapshots_empty(), "all registrations released");
    let zero = gate.open_snapshot_at(0).await;
    assert_eq!(zero.version(), 0);
    assert!(!gate.active_snapshots_empty());
    drop(zero);
    assert!(gate.active_snapshots_empty());
}

/// Compatibility control, NOT red-before-fix evidence: index2 and sorted
/// use current_stream under a whole-build barrier, not the online pin API.
#[tokio::test]
async fn explicit_pin_index2_and_sorted_current_build_controls() {
    let (tbl, _mvcc, gate, parked, rid) = table_at_gap().await;
    // These families intentionally read the eager latest-state marker.
    gate.publish_committed(11);
    let op = create_index("idx_lower", "target")
        .functional(vec!["name".into()], "lower")
        .unwrap();
    let BatchOp::CreateIndex(op) = op else {
        panic!("functional builder must produce CreateIndex");
    };
    tbl.create_index_v2(&op).await.unwrap();
    let backend = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "idx_lower").await)
        .await
        .unwrap();
    let ready = tbl
        .index2_registry()
        .all_descriptors()
        .await
        .into_iter()
        .find(|desc| desc.name_interned == backend.descriptor().name_interned)
        .unwrap();
    assert_eq!(ready.state, IndexState::Ready);
    let mut keys = smallvec::SmallVec::new();
    keys.push(FunctionalBackend::hash_value(&InnerValue::Str("alice".into())).to_vec());
    match backend.lookup(IndexQuery::Point { keys }).await.unwrap() {
        IndexResult::Set(owners) => assert_eq!(owners.into_iter().collect::<Vec<_>>(), vec![rid]),
        IndexResult::Ranked(owners) => panic!("unexpected ranked functional result: {owners:?}"),
    }
    tbl.create_sorted_index("idx_sorted", &["name"])
        .await
        .unwrap();
    let name = key_id(&tbl, "idx_sorted").await;
    assert_eq!(
        tbl.sorted_indexes
            .find_by_name_interned(name)
            .unwrap()
            .state,
        IndexState::Ready,
    );
    assert_eq!(
        tbl.sorted_indexes.lookup_first_k(name, 2).await.unwrap(),
        vec![rid],
    );
    assert_eq!(gate.completion().watermark(), 9);
    assert!(gate.active_snapshots_empty());
    drop(parked);
}
