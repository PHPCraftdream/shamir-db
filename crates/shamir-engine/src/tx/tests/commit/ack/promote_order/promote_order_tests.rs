//! P0 stage L3b — p0-index-promote-order regression tests.
//!
//! Vector promotes of DIFFERENT transactions touching the same rid are
//! ordered by the per-backend promote turnstile (`promote_turn` +
//! `turn_finished` + `turn_notify`, tickets created in the Phase 5d DELTA
//! half, awaited before `enter_write`, released before the next wait or on
//! `Drop`). Without it, transaction B (published later, chunk idx+1) can
//! promote BEFORE transaction A (chunk idx, parked between publish and
//! promote) and A's stale mutation overwrites B's.
//!
//! Determinism: transaction A is parked at the pre-broadcast test hook
//! (fires AFTER publish, BEFORE `promote_vectors`); transaction B's promote
//! must WAIT at the turnstile — observed via the backend's one-shot
//! `arm_promote_turn_wait_hook_for_test` — before A is resumed. The hooks
//! are armed only AFTER the seed transaction has committed (arming the
//! journal hook earlier would park the seed forever). No sleeps; every DIRECT
//! await on the test's own task is bounded by `within`; awaits inside the
//! spawned tasks are bounded by the outer `within(join)`.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use shamir_query_types::admin::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Store;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::oneshot;

use crate::index2::backend::{IndexBackend, IndexQuery, IndexResult};
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::tx::tests::journal_publish_order_tests::JournalPublishHook;
use crate::tx::tx_outcome::MaterializationState;
use shamir_index::vector::vector_backend::VectorBackend;

/// Deadlines are deadlock guards, never scheduling or performance oracles.
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(45), future)
        .await
        .expect("promote-order test stalled — the turnstile leaked a turn")
}

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("promote_order".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn field_id(tbl: &crate::table::TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn vector_index_op(name: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: "vecs".into(),
        fields: vec![vec!["embedding".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(3),
        vector_metric: Some("cosine".into()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

fn vec_record(emb_key_id: u64, vec: &[f64]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(emb_key_id),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f)).collect()),
    );
    InnerValue::Map(m)
}

/// The table's `VectorBackend`, with the registry Vec kept alive by the caller.
fn as_vector(backends: &[Arc<dyn IndexBackend>]) -> &VectorBackend {
    backends
        .iter()
        .find_map(|b| b.as_any().downcast_ref::<VectorBackend>())
        .expect("the table must own a VectorBackend")
}

/// Which embedding the graph currently holds for the single row: the match
/// score of the live vector against `q`. The stale/new vectors are mutually
/// orthogonal, so the larger score identifies which promote won.
async fn match_score(backend: &VectorBackend, q: &[f64]) -> f32 {
    let q: Vec<f32> = q.iter().map(|f| *f as f32).collect();
    match IndexBackend::lookup(
        backend,
        IndexQuery::Vector {
            vec: q,
            k: 5,
            opts: crate::index2::vector::SearchOpts::default(),
        },
    )
    .await
    .unwrap()
    {
        IndexResult::Ranked(hits) => {
            assert!(!hits.is_empty(), "the rid must be in the live graph");
            hits[0].1
        }
        _ => panic!("expected Ranked result"),
    }
}

async fn hits_contain(backend: &VectorBackend, q: &[f64], rid: &RecordId) -> bool {
    let q: Vec<f32> = q.iter().map(|f| *f as f32).collect();
    match IndexBackend::lookup(
        backend,
        IndexQuery::Vector {
            vec: q,
            k: 10,
            opts: crate::index2::vector::SearchOpts::default(),
        },
    )
    .await
    .unwrap()
    {
        IndexResult::Ranked(hits) => hits.iter().any(|(r, _)| r == rid),
        _ => panic!("expected Ranked result"),
    }
}

fn park_hook() -> Arc<JournalPublishHook> {
    let hook = Arc::new(JournalPublishHook::default());
    hook.broadcast_armed.store(true, Ordering::Release);
    hook
}

/// Reopen a fresh backend over the same stores (the restart oracle).
async fn restarted_backend(
    backend: &VectorBackend,
    emb_id: u64,
    info_store: &Arc<dyn Store>,
    data_store: &Arc<dyn Store>,
) -> VectorBackend {
    let desc = IndexBackend::descriptor(backend).clone();
    let adapter: Arc<dyn shamir_index::vector::adapter::VectorAdapter> =
        Arc::new(shamir_index::vector::hnsw_adapter::HnswAdapter::new(
            3,
            shamir_index::kind::VectorMetric::Cosine,
            shamir_index::vector::hnsw_adapter::HnswConfig {
                max_elements: 100_000,
                m: 16,
                ef_construction: 200,
                ef_search: 50,
                ..Default::default()
            },
        ));
    let restarted = VectorBackend::new(desc, vec![emb_id], adapter);
    IndexBackend::restore_on_open(&restarted, Arc::clone(info_store), Arc::clone(data_store))
        .await
        .unwrap();
    assert_eq!(
        IndexBackend::rebuild_count(&restarted),
        0,
        "restart must take the snapshot+delta path"
    );
    restarted
}

async fn join_tail(outcome: &mut crate::tx::tx_outcome::TxOutcome) {
    if let Some(tail) = outcome.take_background() {
        within(tail.join()).await;
    }
}

/// Every vector backend on a table, DOWNCAST — for per-backend oracles the
/// two-index scenario (round-2 finding 2) asserts on BOTH backends.
fn vector_backends(backends: &[Arc<dyn IndexBackend>]) -> Vec<&VectorBackend> {
    backends
        .iter()
        .filter_map(|b| b.as_any().downcast_ref::<VectorBackend>())
        .collect()
}

/// The VectorBackend with the LOWEST descriptor id — the FIRST ticket in
/// the global (token, descriptor id) order. Index ids auto-increment, so
/// the first-created index is always lowest.
fn as_vector_lowest(backends: &[Arc<dyn IndexBackend>]) -> &VectorBackend {
    let low = backends
        .iter()
        .min_by_key(|b| shamir_index::IndexBackend::descriptor(b.as_ref()).id)
        .expect("the table must own at least one index backend");
    low.as_any()
        .downcast_ref::<VectorBackend>()
        .expect("the lowest-id backend must be the vector backend")
}

/// (a) main scenario: A (stale vector, chunk idx) is parked between publish
/// and promote; B (newer vector, chunk idx+1, same rid) promotes — and MUST
/// wait at the turnstile; A resumes LAST yet must NOT overwrite B. The
/// snapshot + restart oracle still holds B's vector.
///
/// Fails on old code because: reverting the turnstile in
/// `apply_vector_graph_batch` (wait_turn/promote via tickets) back to the
/// unordered `apply_staged_vector_deletes` + `apply_staged_vectors` loop lets
/// B promote without ever waiting — the turn-wait one-shot never fires,
/// `within(turn_rx)` times out, and A's stale promote lands last.
///
/// Awaits inside the spawned tasks are bounded by the outer `within(join)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_index_l3b_promote_order_stale_promote_loses_to_newer_vector() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = within(repo.get_table("vecs")).await.unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx")))
        .await
        .unwrap();
    let emb = within(field_id(&tbl, "embedding")).await;
    let backends = within(tbl.index2_registry().all_backends()).await;
    let backend = as_vector(&backends);

    let park = park_hook();
    repo.journal_publish_hook.store(Some(park.clone()));
    let (turn_tx, turn_rx) = oneshot::channel();
    backend.arm_promote_turn_wait_hook_for_test(turn_tx);

    // Transaction A: stale vector; parked pre-promote (chunk idx reserved).
    let (mut tx_a, pin_a) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx_a)))
        .await
        .unwrap();
    let repo_a = repo.clone();
    let task_a = tokio::spawn(async move { repo_a.commit_tx(tx_a).await });
    within(park.reached.notified()).await;

    // Transaction B: newer vector, same rid. Its promote must WAIT.
    let (mut tx_b, pin_b) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 1.0, 0.0]), Some(&mut tx_b)))
        .await
        .unwrap();
    let repo_b = repo.clone();
    let task_b = tokio::spawn(async move { repo_b.commit_tx(tx_b).await });
    let _ = within(turn_rx).await;

    // Resume A — its stale promote runs, but B's chunk still promotes AFTER.
    park.resume.notify_one();
    let mut outcome_a = within(task_a).await.unwrap().unwrap();
    join_tail(&mut outcome_a).await;
    let mut outcome_b = within(task_b).await.unwrap().unwrap();
    join_tail(&mut outcome_b).await;

    // The live graph holds B's vector: the NEW vector outranks the stale one.
    let s_new = within(match_score(backend, &[0.0, 1.0, 0.0])).await;
    let s_stale = within(match_score(backend, &[1.0, 0.0, 0.0])).await;
    assert!(
        s_new > s_stale,
        "the live graph must hold B's newer vector (s_new {s_new} <= s_stale {s_stale} — \
         A's stale promote landed after B's)"
    );

    // Snapshot + restart: the persisted graph is still B's vector.
    let info_store = tbl.info_store_for_test();
    within(backend.snapshot_now(&info_store)).await.unwrap();
    let reloaded = within(restarted_backend(
        backend,
        emb,
        &info_store,
        tbl.data_store_for_test(),
    ))
    .await;
    let s_new = within(match_score(&reloaded, &[0.0, 1.0, 0.0])).await;
    let s_stale = within(match_score(&reloaded, &[1.0, 0.0, 0.0])).await;
    assert!(
        s_new > s_stale,
        "the restarted graph must still hold B's newer vector"
    );
    drop((pin_a, pin_b));
}

/// (b) upsert-then-delete: A (insert, chunk idx) parked; B deletes the rid
/// (chunk idx+1) and must wait at the turnstile; after A's insert and B's
/// delete both promote IN ORDER the rid is ABSENT from the graph.
///
/// Fails on old code because: reverting the ordered ticket path lets B's
/// delete promote immediately (A's chunk idx is parked mid-promote), then A's
/// resume inserts the rid afterwards — `hits_contain` finds the deleted rid
/// live in the graph and the assertion fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_index_l3b_promote_order_upsert_then_delete_stays_absent() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = within(repo.get_table("vecs")).await.unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx")))
        .await
        .unwrap();
    let emb = within(field_id(&tbl, "embedding")).await;
    let backends = within(tbl.index2_registry().all_backends()).await;
    let backend = as_vector(&backends);

    // Seed BEFORE arming the journal hook (RV finding 6): an armed hook parks
    // EVERY commit at the pre-broadcast point, so the seed would hang forever.
    let (mut tx0, pin0) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx0)))
        .await
        .unwrap();
    let mut outcome0 = within(repo.commit_tx(tx0)).await.unwrap();
    join_tail(&mut outcome0).await;
    drop(pin0);

    // NOW arm: A parks pre-promote, B waits at the turnstile behind A.
    let park = park_hook();
    repo.journal_publish_hook.store(Some(park.clone()));
    let (turn_tx, turn_rx) = oneshot::channel();
    backend.arm_promote_turn_wait_hook_for_test(turn_tx);

    // A: upsert (stale mutation), parked pre-promote.
    let (mut tx_a, pin_a) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 1.0, 0.0]), Some(&mut tx_a)))
        .await
        .unwrap();
    let repo_a = repo.clone();
    let task_a = tokio::spawn(async move { repo_a.commit_tx(tx_a).await });
    within(park.reached.notified()).await;

    // B: delete the rid; its promote must wait behind A's chunk.
    let (mut tx_b, pin_b) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.delete_tx(rid, Some(&mut tx_b))).await.unwrap();
    let repo_b = repo.clone();
    let task_b = tokio::spawn(async move { repo_b.commit_tx(tx_b).await });
    let _ = within(turn_rx).await;

    park.resume.notify_one();
    let mut outcome_a = within(task_a).await.unwrap().unwrap();
    join_tail(&mut outcome_a).await;
    let mut outcome_b = within(task_b).await.unwrap().unwrap();
    join_tail(&mut outcome_b).await;

    // The rid is ABSENT from the live graph despite A's later-promoting insert.
    assert!(
        !within(hits_contain(backend, &[0.0, 1.0, 0.0], &rid)).await,
        "deleted rid must be absent from the live graph"
    );
    assert!(
        !within(hits_contain(backend, &[1.0, 0.0, 0.0], &rid)).await,
        "deleted rid must be absent from the live graph"
    );
    drop((pin_a, pin_b));
}

/// (c) delete-then-reinsert: A (delete of the rid, chunk idx) parked; B
/// re-inserts the same record (chunk idx+1) and must wait; after both
/// promote IN ORDER the rid is present with the newest vector.
///
/// Fails on old code because: on the unordered path B's insert promotes
/// first and A's parked delete promotes after it, tombstoning the freshly
/// re-inserted rid — `hits_contain` finds nothing and the test fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_index_l3b_promote_order_delete_then_reinsert_present_newest() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = within(repo.get_table("vecs")).await.unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx")))
        .await
        .unwrap();
    let emb = within(field_id(&tbl, "embedding")).await;
    let backends = within(tbl.index2_registry().all_backends()).await;
    let backend = as_vector(&backends);

    // Seed a committed row BEFORE arming the journal hook (RV finding 6); the
    // re-insert below uses the SAME value so the content-derived rid lands on
    // the same record.
    let (mut tx0, pin0) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx0)))
        .await
        .unwrap();
    let mut outcome0 = within(repo.commit_tx(tx0)).await.unwrap();
    join_tail(&mut outcome0).await;
    drop(pin0);

    // NOW arm: A parks pre-promote, B waits at the turnstile behind A.
    let park = park_hook();
    repo.journal_publish_hook.store(Some(park.clone()));
    let (turn_tx, turn_rx) = oneshot::channel();
    backend.arm_promote_turn_wait_hook_for_test(turn_tx);

    // A: delete the rid, parked pre-promote.
    let (mut tx_a, pin_a) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.delete_tx(rid, Some(&mut tx_a))).await.unwrap();
    let repo_a = repo.clone();
    let task_a = tokio::spawn(async move { repo_a.commit_tx(tx_a).await });
    within(park.reached.notified()).await;

    // B: re-insert the SAME value (same rid, newest vector).
    let (mut tx_b, pin_b) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid_b = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx_b)))
        .await
        .unwrap();
    assert_eq!(
        rid_b, rid,
        "re-insert must land on the same content-derived rid"
    );
    let repo_b = repo.clone();
    let task_b = tokio::spawn(async move { repo_b.commit_tx(tx_b).await });
    let _ = within(turn_rx).await;

    park.resume.notify_one();
    let mut outcome_a = within(task_a).await.unwrap().unwrap();
    join_tail(&mut outcome_a).await;
    let mut outcome_b = within(task_b).await.unwrap().unwrap();
    join_tail(&mut outcome_b).await;

    assert!(
        within(hits_contain(backend, &[1.0, 0.0, 0.0], &rid)).await,
        "re-inserted rid must be present in the live graph after the \
         delete-then-reinsert pair promotes in chunk order"
    );
    drop((pin_a, pin_b));
}

/// (d) a FAILED promote (the `FAIL_VECTOR_PROMOTE_TX_ID` failpoint, which
/// fires BEFORE the ticket loop, so the tickets Drop unentered at the end of
/// `promote_vectors`' per-table call) releases the turn: B (parked behind
/// A's failed chunk) still promotes — no hang — and A's failed chunk stays
/// unacknowledged (prune prefix stays at 1).
///
/// Fails on old code because: without the Drop-based release (turnstile
/// kept, release dropped) B's waiter is never woken after A's failure and
/// `within(task_b)` stalls out; reverting the turnstile entirely makes B
/// promote while A is parked, the turn-wait one-shot never fires, and
/// `within(turn_rx)` fails.
///
/// Awaits inside the spawned tasks are bounded by the outer `within(join)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn rv_index_l3b_promote_order_failed_promote_releases_turn() {
    use crate::tx::commit_phases::FAIL_VECTOR_PROMOTE_TX_ID;

    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = within(repo.get_table("vecs")).await.unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx")))
        .await
        .unwrap();
    let emb = within(field_id(&tbl, "embedding")).await;
    let backends = within(tbl.index2_registry().all_backends()).await;
    let backend = as_vector(&backends);

    // Row 1 commits cleanly (BEFORE arming any hook): the graph gains its
    // entry point, chunk 0 acked.
    let (mut tx0, pin0) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx0)))
        .await
        .unwrap();
    let mut outcome0 = within(repo.commit_tx(tx0)).await.unwrap();
    assert_eq!(outcome0.materialization, MaterializationState::Complete);
    join_tail(&mut outcome0).await;
    drop(pin0);
    assert_eq!(backend.applied_prefix(), 1, "chunk 0 must be acknowledged");

    let park = park_hook();
    repo.journal_publish_hook.store(Some(park.clone()));
    let (turn_tx, turn_rx) = oneshot::channel();
    backend.arm_promote_turn_wait_hook_for_test(turn_tx);

    // A: its promote will FAIL (failpoint), but it is parked pre-promote.
    let (mut tx_a, pin_a) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 1.0, 0.0]), Some(&mut tx_a)))
        .await
        .unwrap();
    let inject_tx_id = tx_a.tx_id.0;
    FAIL_VECTOR_PROMOTE_TX_ID.store(inject_tx_id, Ordering::SeqCst);
    let repo_a = repo.clone();
    let task_a = tokio::spawn(async move { repo_a.commit_tx(tx_a).await });
    within(park.reached.notified()).await;

    // B: newer vector; its promote must WAIT behind A's doomed chunk.
    let (mut tx_b, pin_b) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 0.0, 1.0]), Some(&mut tx_b)))
        .await
        .unwrap();
    let repo_b = repo.clone();
    let task_b = tokio::spawn(async move { repo_b.commit_tx(tx_b).await });
    let _ = within(turn_rx).await;

    // Resume A: its promote fails; the ticket Drop must release the turn.
    park.resume.notify_one();
    let mut outcome_a = within(task_a).await.unwrap().unwrap();
    assert_eq!(
        outcome_a.materialization,
        MaterializationState::Complete,
        "a failed post-lock promote must NOT defer the tx (III.5)"
    );
    join_tail(&mut outcome_a).await;
    FAIL_VECTOR_PROMOTE_TX_ID.store(0, Ordering::SeqCst);

    // No hang: B promotes through the released turn.
    let mut outcome_b = within(task_b).await.unwrap().unwrap();
    join_tail(&mut outcome_b).await;

    let s_new = within(match_score(backend, &[0.0, 0.0, 1.0])).await;
    let s_stale = within(match_score(backend, &[1.0, 0.0, 0.0])).await;
    assert!(
        s_new > s_stale,
        "B's vector must be live after A's failed promote"
    );

    // A's failed chunk stays unacknowledged (never acked by its failure);
    // the prune prefix therefore cannot pass it even though B acked later.
    assert_eq!(
        backend.applied_prefix(),
        1,
        "the failed chunk must stay unacknowledged behind the prefix"
    );
    drop((pin_a, pin_b));
}

/// (g) WAIT-CYCLE ACROSS BACKENDS with the round-2 GLOBAL ORDER (finding 2):
/// ONE table ("vecs") with TWO vector indexes (`create_index_v2` twice —
/// index ids auto-increment, so index A sorts before index B), i.e. TWO
/// backends inside ONE `apply_vector_graph_batch` call. Because
/// `apply_vector_delta_phase` and `promote_vectors` both merge their tokens
/// into ONE sorted Vec and `apply_vector_delta_batch` mints tickets by
/// ascending descriptor id, tx A and tx B reserve AND promote their
/// (token, backend id) pairs in the SAME global total order: B's promote
/// must WAIT at the LOW-id backend FIRST (the turn-wait seam is armed ONLY
/// there, so the handshake is deterministic — if B ever promoted low-out-of
///-order, `within(turn_rx)` would time out before anything else), and only
/// after A finishes BOTH backends does B proceed.
///
/// Revert honesty (stated as required): with the round-1
/// wait-all-then-promote-all-no-release shape, B cannot START on backend-low
/// until A finished BOTH backends — the seam still fires (the wait exists);
/// what the sort guarantees is the ORDERED-RELEASE property. The true
/// inversion DEADLOCK requires two txs processing the two tickets in
/// OPPOSITE orders: that is impossible BY CONSTRUCTION under the sorted
/// order, and makes the test NONDETERMINISTIC under the unsorted revert
/// (registry iteration order) — this test therefore asserts the strongest
/// deterministic oracle available (B waits at backend-low FIRST; both txs
/// Complete; both backends hold B's newer embedding; both prefixes exactly
/// 3) and NOT the nondeterministic inversion itself.
///
/// Awaits inside the spawned tasks are bounded by the outer `within(join)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_index_l3b_promote_order_no_wait_cycle_across_backends_one_table() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = within(repo.get_table("vecs")).await.unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx_a")))
        .await
        .unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx_b")))
        .await
        .unwrap();
    let emb = within(field_id(&tbl, "embedding")).await;
    let backends = within(tbl.index2_registry().all_backends()).await;
    assert_eq!(
        vector_backends(&backends).len(),
        2,
        "the table must own TWO vector backends (one per index)"
    );
    let low = as_vector_lowest(&backends);

    // Seed BEFORE arming the journal hook: one committed row on BOTH
    // backends (chunk 0 acked on each → applied_prefix == 1 on each).
    let (mut tx0, pin0) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx0)))
        .await
        .unwrap();
    let mut outcome0 = within(repo.commit_tx(tx0)).await.unwrap();
    join_tail(&mut outcome0).await;
    drop(pin0);
    for b in vector_backends(&backends) {
        assert_eq!(b.applied_prefix(), 1, "seed chunk 0 acked on every backend");
    }

    // NOW arm: A parks pre-promote (chunk 1 reserved on BOTH backends); B
    // (chunk 2 on BOTH) must wait at the LOW-id backend.
    let park = park_hook();
    repo.journal_publish_hook.store(Some(park.clone()));
    let (turn_tx, turn_rx) = oneshot::channel();
    low.arm_promote_turn_wait_hook_for_test(turn_tx);

    let (mut tx_a, pin_a) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 1.0, 0.0]), Some(&mut tx_a)))
        .await
        .unwrap();
    let repo_a = repo.clone();
    let task_a = tokio::spawn(async move { repo_a.commit_tx(tx_a).await });
    within(park.reached.notified()).await;

    let (mut tx_b, pin_b) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 0.0, 1.0]), Some(&mut tx_b)))
        .await
        .unwrap();
    let repo_b = repo.clone();
    let task_b = tokio::spawn(async move { repo_b.commit_tx(tx_b).await });
    // THE handshake: B must park at the LOW-id backend's turnstile (the
    // seam is armed only there).
    let _ = within(turn_rx).await;

    park.resume.notify_one();
    let mut out_a = within(task_a).await.unwrap().unwrap();
    join_tail(&mut out_a).await;
    let mut out_b = within(task_b).await.unwrap().unwrap();
    join_tail(&mut out_b).await;
    assert_eq!(
        out_a.materialization,
        MaterializationState::Complete,
        "tx A must commit Complete (deadlock-free promote order)"
    );
    assert_eq!(
        out_b.materialization,
        MaterializationState::Complete,
        "tx B must commit Complete (deadlock-free promote order)"
    );

    // BOTH backends hold B's NEWER embedding (match_score comparison, not
    // just rid presence): B's vector outranks A's on each backend.
    for b in vector_backends(&backends) {
        let s_new = within(match_score(b, &[0.0, 0.0, 1.0])).await;
        let s_mid = within(match_score(b, &[0.0, 1.0, 0.0])).await;
        assert!(
            s_new > s_mid,
            "each backend's live graph must hold B's newer vector \
             (s_new {s_new} <= s_mid {s_mid})"
        );
        // Contiguous acked prefix: seed chunk 0 + A's chunk 1 + B's chunk 2
        // acked on EACH backend ⇒ exactly 3 (computed from the seed hwm).
        assert_eq!(
            b.applied_prefix(),
            3,
            "A's and B's chunks must each be acked exactly once on every backend"
        );
    }
    drop((pin_a, pin_b));
}

/// (h) RETRY-INSIDE-THE-TURN (round-2 finding 1): a promote that fails on
/// the FIRST attempt and succeeds on the retry, injected at the APPLY level
/// INSIDE the ticket loop (`FAIL_VECTOR_PROMOTE_ONCE_*`, after `wait_turn`,
/// before `t.promote`), so the FIRST failure happens while A's turn is
/// still HELD and the in-loop bounded retry re-runs with the ticket ACTIVE.
/// The old round-1 shape released/consumed the ticket BEFORE propagating
/// the failure, so `retry_materialize` re-invoked the apply on a CONSUMED
/// ticket whose `promote` answered a phantom `Ok(())` — a real adapter
/// failure "succeeded" on retry and the chunk got acknowledged without its
/// mutations ever reaching the adapter.
///
/// Fails on old code because: with the ticket released before `res?`, the
/// retried apply errors with "promote ticket already released" (or, before
/// that fix, silently Ok()s without applying) — A's chunk 1 is never truly
/// re-promoted, so it is never acknowledged and `applied_prefix()` stops at
/// 2 instead of the asserted 3 (and B's waiter races A's phantom retry).
///
/// Awaits inside the spawned tasks are bounded by the outer `within(join)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn rv_index_l3b_promote_order_transient_promote_failure_retries_in_turn() {
    use crate::tx::commit_phases::{
        FAIL_VECTOR_PROMOTE_ONCE_REMAINING, FAIL_VECTOR_PROMOTE_ONCE_TX_ID,
    };

    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = within(repo.get_table("vecs")).await.unwrap();
    within(tbl.create_index_v2(&vector_index_op("vec_idx")))
        .await
        .unwrap();
    let emb = within(field_id(&tbl, "embedding")).await;
    let backends = within(tbl.index2_registry().all_backends()).await;
    let backend = as_vector(&backends);

    // Row commits cleanly (BEFORE arming any hook): chunk 0 acked.
    let (mut tx0, pin0) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    let rid = within(tbl.insert_tx(&vec_record(emb, &[1.0, 0.0, 0.0]), Some(&mut tx0)))
        .await
        .unwrap();
    let mut outcome0 = within(repo.commit_tx(tx0)).await.unwrap();
    join_tail(&mut outcome0).await;
    drop(pin0);
    assert_eq!(backend.applied_prefix(), 1, "chunk 0 must be acknowledged");

    let park = park_hook();
    repo.journal_publish_hook.store(Some(park.clone()));
    let (turn_tx, turn_rx) = oneshot::channel();
    backend.arm_promote_turn_wait_hook_for_test(turn_tx);

    // A: its FIRST promote attempt will fail (fail-once, in-loop), but it is
    // parked pre-promote.
    let (mut tx_a, pin_a) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 1.0, 0.0]), Some(&mut tx_a)))
        .await
        .unwrap();
    let inject_tx_id = tx_a.tx_id.0;
    FAIL_VECTOR_PROMOTE_ONCE_TX_ID.store(inject_tx_id, Ordering::SeqCst);
    FAIL_VECTOR_PROMOTE_ONCE_REMAINING.store(1, Ordering::SeqCst);
    let repo_a = repo.clone();
    let task_a = tokio::spawn(async move { repo_a.commit_tx(tx_a).await });
    within(park.reached.notified()).await;

    // B: newer vector; its promote must WAIT behind A's ACTIVE ticket (A's
    // turn stays held across the failed first attempt).
    let (mut tx_b, pin_b) = within(repo.begin_tx(IsolationLevel::Snapshot))
        .await
        .unwrap();
    within(tbl.update_tx(rid, &vec_record(emb, &[0.0, 0.0, 1.0]), Some(&mut tx_b)))
        .await
        .unwrap();
    let repo_b = repo.clone();
    let task_b = tokio::spawn(async move { repo_b.commit_tx(tx_b).await });
    let _ = within(turn_rx).await;

    // Resume A: attempt 1 fails IN-TURN, attempt 2 (the in-loop retry)
    // succeeds for real; the ticket is released only AFTER the loop.
    park.resume.notify_one();
    let mut outcome_a = within(task_a).await.unwrap().unwrap();
    assert_eq!(
        outcome_a.materialization,
        MaterializationState::Complete,
        "a transient promote failure must recover via the in-loop retry"
    );
    join_tail(&mut outcome_a).await;
    FAIL_VECTOR_PROMOTE_ONCE_TX_ID.store(0, Ordering::SeqCst);
    FAIL_VECTOR_PROMOTE_ONCE_REMAINING.store(0, Ordering::SeqCst);

    // No hang: B promotes through the released turn.
    let mut outcome_b = within(task_b).await.unwrap().unwrap();
    join_tail(&mut outcome_b).await;
    assert_eq!(
        outcome_b.materialization,
        MaterializationState::Complete,
        "tx B must commit Complete"
    );

    // The live graph holds B's NEWER embedding (not A's mid vector).
    let s_new = within(match_score(backend, &[0.0, 0.0, 1.0])).await;
    let s_mid = within(match_score(backend, &[0.0, 1.0, 0.0])).await;
    let s_stale = within(match_score(backend, &[1.0, 0.0, 0.0])).await;
    assert!(
        s_new > s_mid && s_new > s_stale,
        "B's newer vector must be live after A's transient promote failure"
    );

    // EXACT ack accounting: chunk 0 (seed) + chunk 1 (A, retried and then
    // acked EXACTLY ONCE) + chunk 2 (B) ⇒ applied_prefix == 3, not more.
    // A false-success retry (consumed ticket answering Ok) acks a chunk
    // whose apply never ran — with the fixed code the retry really applies,
    // so the prefix is exactly start + 2.
    assert_eq!(
        backend.applied_prefix(),
        3,
        "A's chunk must be acked EXACTLY ONCE after the successful in-turn retry \
         (prefix start+2 == 3)"
    );
    drop((pin_a, pin_b));
}
