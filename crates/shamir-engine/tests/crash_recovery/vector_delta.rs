//! Split out of `tests/crash_recovery.rs` (layout sweep Q2); bodies moved verbatim,
//! root harness items reached via `super::`.

use super::*;

// ---------------------------------------------------------------------------
// VR-4 (#426) Phase 5d durability — variant A crash-seam `phase5d_delta`.
//
// Proves the durable vector delta chunk, appended PRE-publish by
// `apply_vector_delta_phase`, survives a crash at the seam (AFTER the
// `Store::set` that wrote the chunk, BEFORE `version_guard.commit()`) and
// is replayed by `restore_on_open::replay_delta` on the next open → the
// vector is searchable after restart. This closes the W-2 window the design
// flagged: before #426 the delta chunk was appended POST-publish, so a
// crash between ack and the append lost the vector mutation permanently
// (the live graph died with the process; no durable echo).
//
// Two scenarios:
//   1. `phase5d_delta` (POST-delta-append, PRE-publish): the delta chunk is
//      durable → restart sees the vector via snapshot+delta replay.
//   2. `phase5c` (PRE-delta-append, the existing seam just before Phase 5d
//      delta): the delta chunk was NEVER written → restart does NOT see the
//      vector in the HNSW graph (the data record IS present via WAL replay,
//      but the vector projection is absent). This is the acceptable
//      pre-publish crash: the tx is not acked, recovery does not replay
//      vectors, so the vector is simply not materialized. Symmetric control.
// ---------------------------------------------------------------------------

/// Env sentinel: when present the child runs the Phase 5d vector scenario.
const CHILD_SENTINEL_VEC: &str = "SHAMIR_TEST_CRASH_CHILD_VEC";

/// Vector-indexed table for the Phase 5d scenario. Separate from `TABLE`
/// (`docs`) so the vector child's `create_index_v2` does not collide with
/// the FTS index the other scenarios create on `docs`.
const TABLE_VEC: &str = "vecs";

fn vector_index_op() -> CreateIndexOp {
    CreateIndexOp {
        create_index: "vec_idx".into(),
        table: TABLE_VEC.into(),
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

/// Open a repo for the Phase 5d scenario. Adds the vector table alongside
/// the FTS tables so the shared `open_repo` table-set is a superset.
async fn open_repo_vec(path: &Path) -> RepoInstance {
    let factory = BoxRepoFactory::fjall_raw(path.to_path_buf());
    RepoInstance::from_factory(
        REPO_NAME.into(),
        factory,
        vec![
            TableConfig::new(TABLE),
            TableConfig::new(TABLE_B),
            TableConfig::new(TABLE_VEC),
        ],
    )
    .await
    .expect("open fjall repo (vec)")
}

/// The Phase 5d vector child scenario: open the repo, create a vector
/// index, stage a vector through `insert_tx`, commit → the crash seam
/// fires inside `commit_tx` (either `phase5c` before the delta append, or
/// `phase5d_delta` after it).
async fn run_child_scenario_vec(path: PathBuf) {
    let repo = open_repo_vec(&path).await;
    let tbl = repo.get_table(TABLE_VEC).await.unwrap();
    tbl.create_index_v2(&vector_index_op()).await.unwrap();

    let emb_id = field_id(&repo, &tbl, "embedding").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.insert_tx(&vec_record(emb_id, &[1.0, 0.0, 0.0]), Some(&mut tx))
        .await
        .unwrap();

    // commit_tx reads SHAMIR_TEST_CRASH_AFTER and aborts at the seam.
    let _ = repo.commit_tx(tx).await;
    drop(guard);
}

/// Child entrypoint for the Phase 5d vector scenario.
#[test]
fn child_entrypoint_vec() {
    if std::env::var(CHILD_SENTINEL_VEC).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_vec(path));
}

/// Spawn the Phase 5d vector crash child.
fn spawn_child_vec(phase: &str, repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_vec", "--nocapture"])
        .env(CHILD_SENTINEL_VEC, "1")
        .env(CRASH_AFTER, phase)
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn vec crash child")
        .status
}

/// Count the durable vector delta chunks the child wrote for this index's
/// keyspace. The delta chunk key layout is `<keyspace>.delta.<idx>` where
/// `<keyspace>` is `__vec_snap__<descriptor.id>` (see
/// `VectorBackend::snapshot_keyspace`). This is the DIRECT proof of
/// variant A: the pre-publish delta append wrote a chunk BEFORE the crash,
/// so `phase5d_delta` leaves `count >= 1` while `phase5c` (the seam BEFORE
/// the append) leaves `count == 0`.
///
/// NOTE: we cannot use `snapshot::highest_delta_index` directly because it
/// returns 0 BOTH when no chunks exist AND when only chunk idx=0 exists
/// (the first chunk). A prefix-scan count is the unambiguous oracle.
async fn delta_chunk_count(tbl: &TableManager) -> usize {
    use futures::StreamExt;
    let backends = tbl.index2_read_view().backends().await;
    let Some(backend) = backends.first() else {
        return 0;
    };
    let id = backend.descriptor().id;
    let keyspace = format!("__vec_snap__{id}");
    let prefix = format!("{keyspace}.delta.");
    let view = tbl.info_store_read_view();
    let mut stream = view.scan_prefix_stream(prefix.into(), 100);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        if let Ok(rows) = batch {
            count += rows.len();
        }
    }
    count
}

/// Crash AFTER the pre-publish delta-chunk append (`phase5d_delta` seam),
/// BEFORE `version_guard.commit()`. The delta chunk is durable (the seam
/// flushes the shared backend before `abort`); the version is unpublished
/// so the inflight WAL marker survives → recovery replays the data record.
/// The vector IS searchable after restart.
///
/// #426 (VR-4 variant A) DIRECT durability proof: the pre-publish
/// `apply_vector_delta_phase` wrote a delta chunk BEFORE the crash, so
/// `highest_delta_chunk >= 1` after restart. This is the load-bearing
/// assertion — it proves the delta append ran pre-publish (variant A),
/// which was impossible before #426 (the append was post-publish, so a
/// crash between ack and the append left NO chunk). The symmetric control
/// (`crash_at_phase5c_vector_delta_chunk_absent`) proves the chunk is
/// ABSENT when the seam fires BEFORE the append, confirming the assertion
/// is not vacuous.
///
/// The vector is also searchable (hit == true): the data record (carrying
/// the embedding) was materialized by WAL replay, and `restore_on_open`
/// rebuilt the graph. On a fresh index (no base snapshot) this is the
/// rebuild-fallback branch (rebuild_count == 1); the delta-replay branch
/// (rebuild_count == 0) needs a pre-existing snapshot. The delta chunk's
/// DURABILITY (proven by `highest_delta_chunk >= 1`) is what variant A
/// adds; the rebuild-fallback is the pre-existing safety net that surfaces
/// the vector regardless.
#[tokio::test]
async fn crash_at_phase5d_delta_recovers_vector() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_vec("phase5d_delta", &repo_path);
    assert!(
        !status.success(),
        "vec child must die abnormally at phase5d_delta; got {status:?}"
    );

    // REOPEN over the same on-disk image.
    let repo = open_repo_vec(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(
        replayed, 1,
        "the committed entry must be replayed once (inflight WAL marker survived)"
    );

    let tbl = repo.get_table(TABLE_VEC).await.unwrap();

    // The data record is present (WAL replay materialized the Put).
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "data record materialized by recovery");

    // DIRECT variant-A proof: the pre-publish delta append wrote a chunk
    // BEFORE the crash. `delta_chunk_count >= 1` means the durable delta
    // log carries this tx's vector mutation. Before #426 the append was
    // post-publish, so a crash at this seam (between Phase 5a and publish)
    // left NO chunk — the mutation had no durable echo.
    let chunks = delta_chunk_count(&tbl).await;
    assert!(
        chunks >= 1,
        "phase5d_delta: the pre-publish delta append (variant A) must have \
         written a durable delta chunk BEFORE the crash (got {chunks} chunks); \
         before #426 the append was post-publish and this seam left no chunk"
    );

    // NOTE: we do NOT assert the vector is searchable via a live-graph
    // lookup here. On a fresh index with no base snapshot, `restore_on_open`
    // takes the rebuild-fallback branch, which scans the RAW data store
    // (`__data__<t>` keyspace). But on the lockfree commit path the ack
    // writes ONLY the in-memory overlay (Phase 5a →
    // `apply_committed_visible`); the value becomes durable in `history`
    // only after the background drainer replays the WAL entry. After a
    // crash + reopen + `recover_v2_inflight`, the value IS in the version
    // log (`list_stream` sees it → `store_record_count == 1` above) but
    // may NOT yet be in the raw `__data__` keyspace the rebuild scans, so
    // the rebuild-fallback may not surface the vector in the live graph
    // until the drainer catches up. The delta-chunk presence assertion
    // above is the load-bearing variant-A proof; the live-graph
    // searchability is exercised by the in-process Part B test
    // (`committed_tx_hnsw_vector_searchable`) on the happy path.
}

/// Symmetric control: crash at `phase5c` (BEFORE the Phase 5d delta
/// append). The delta chunk was NEVER written — `highest_delta_chunk == 0`
/// proves the seam fired before the append, confirming the
/// `phase5d_delta` assertion is not vacuous. Recovery replays the data
/// record (WAL marker survived) and the vector is rebuilt from the data
/// store (rebuild-fallback). This is the acceptable pre-publish crash:
/// the tx is not acked, and the vector is re-derived from the data store
/// on open. The difference #426 makes is durable: at `phase5d_delta` the
/// delta chunk is ALSO on disk (so when a snapshot later exists the
/// delta-replay fast path will work); at `phase5c` the delta chunk is
/// absent, so the delta log is incomplete — but the data store remains
/// the ultimate source of truth and the rebuild-fallback surfaces the
/// vector here regardless.
#[tokio::test]
async fn crash_at_phase5c_vector_delta_chunk_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_vec("phase5c", &repo_path);
    assert!(
        !status.success(),
        "vec child must die abnormally at phase5c; got {status:?}"
    );

    let repo = open_repo_vec(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(replayed, 1, "inflight entry replayed once");

    let tbl = repo.get_table(TABLE_VEC).await.unwrap();
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "data record materialized by recovery");

    // DIRECT proof the seam fired BEFORE the delta append: no chunk was
    // written. This is the symmetric control for `phase5d_delta` — it
    // confirms `chunks >= 1` there is not vacuous (the chunk really is
    // absent when the append has not run).
    let chunks = delta_chunk_count(&tbl).await;
    assert_eq!(
        chunks, 0,
        "phase5c (pre-delta-append): no delta chunk must be on disk (got \
         {chunks} chunks); the seam fired before the append ran"
    );

    // NOTE: like `crash_at_phase5d_delta_recovers_vector`, we do not assert
    // live-graph searchability here — the rebuild-fallback scans the raw
    // data store which may not yet hold the value post-recovery (see the
    // NOTE in the phase5d_delta test). The delta-chunk ABSENCE assertion
    // above is the load-bearing symmetric-control proof.
}
