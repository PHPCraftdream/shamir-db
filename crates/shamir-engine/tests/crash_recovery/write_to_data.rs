//! Split out of `tests/crash_recovery.rs` (layout sweep Q2); bodies moved verbatim,
//! root harness items reached via `super::`.

use super::*;

// ---------------------------------------------------------------------------
// W2d write-cutover crash-seam: prove the lens-driven insert path
// (execute_insert_tx → insert_tx_many_bytes → RecordView) survives a crash
// at phase4 and recovery produces the correct data. Two variants:
//
// 1. IMPLICIT-tx: tx.implicit=true → base-intern → zero InnerValue on insert,
//    zero overlay remap. Exercises the full tree-free hot path.
//
// 2. INTERACTIVE-tx with a NEW field name: tx.implicit=false → overlay
//    intern → commit-time remap_inner_value_bytes on the staged bytes.
//    Exercises the cold remap-on-bytes path.
// ---------------------------------------------------------------------------

/// Env sentinel: when present the child runs the W2d write-cutover scenario.
const CHILD_SENTINEL_W2D: &str = "SHAMIR_TEST_CRASH_CHILD_W2D";

/// Env sentinel: when present + "1" the child uses implicit-tx; when "0"
/// interactive-tx with overlay remap.
const W2D_IMPLICIT_FLAG: &str = "SHAMIR_TEST_CRASH_W2D_IMPLICIT";

/// The W2d child scenario: insert a diverse record (int/f64/str/bin/nested-map/
/// list) via `execute_insert_tx` — the W2d cutover path — then commit. The
/// crash seam fires inside `commit_tx`. On the implicit path (flag=1) the
/// insert builds ZERO InnerValue; on the interactive path (flag=0) it uses
/// the overlay for a brand-new field name, exercising remap-on-bytes at commit.
async fn run_child_scenario_w2d(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();

    let implicit = std::env::var(W2D_IMPLICIT_FLAG).unwrap_or("1".into()) == "1";

    // Build an InsertOp with diverse field types. On the interactive path
    // we use a brand-new field name ("w2d_new_field") that has never been
    // interned before — this triggers the overlay-id → remap-on-bytes path
    // at commit. On the implicit path field names go straight to base.
    let record = if implicit {
        mpack!({
            "int_field": 42,
            "float_field": 99.5,
            "str_field": "hello w2d",
            "bin_field": [1, 2, 3, 4, 5],
            "nested": { "inner_key": "inner_val", "inner_num": 7 },
            "list_field": [10, 20, 30]
        })
    } else {
        mpack!({
            "int_field": 99,
            "float_field": 88.8,
            "str_field": "interactive w2d",
            "w2d_new_field": "newness",
            "nested": { "deep": "value" },
            "list_field": [1, 2, 3]
        })
    };

    let op = shamir_query_types::write::InsertOp {
        insert_into: shamir_query_types::TableRef::new(TABLE),
        values: vec![record],
        records_idmsgpack: Vec::new(),
        select: None,
    };

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tx.set_implicit(implicit);
    tbl.execute_insert_tx(
        &op,
        &mut tx,
        false,
        None,
        &shamir_types::access::Actor::System,
    )
    .await
    .unwrap();

    // commit_tx reads SHAMIR_TEST_CRASH_AFTER and aborts at the seam.
    let _ = repo.commit_tx(tx).await;
    drop(guard);
}

/// Child entrypoint for the W2d scenario.
#[test]
fn child_entrypoint_w2d() {
    if std::env::var(CHILD_SENTINEL_W2D).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_w2d(path));
}

/// Spawn the W2d crash child.
fn spawn_child_w2d(phase: &str, repo_path: &Path, implicit: bool) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_w2d", "--nocapture"])
        .env(CHILD_SENTINEL_W2D, "1")
        .env(CRASH_AFTER, phase)
        .env(W2D_IMPLICIT_FLAG, if implicit { "1" } else { "0" })
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn w2d crash child")
        .status
}

/// Read back all records from a table after recovery and return them as
/// QueryValues for comparison.
async fn read_all_records(tbl: &TableManager) -> Vec<shamir_types::types::value::QueryValue> {
    let stream = tbl.list_stream(256);
    futures::pin_mut!(stream);
    let mut records = Vec::new();
    let view = tbl.info_store_read_view();
    let interner = view.interner_read_view().await.unwrap();
    while let Some(batch) = stream.next().await {
        for (_id, cow) in batch.expect("list_stream batch") {
            let record = cow.into_inner().expect("decode record");
            records.push(interner.inner_to_query_value(&record).expect("to_qv"));
        }
    }
    records
}

/// W2d IMPLICIT-tx crash at phase4: the lens-driven insert path (zero
/// InnerValue, zero overlay remap) must survive a crash at the commit point
/// and recovery must materialize the correct data with all field types intact.
#[tokio::test]
async fn w2d_implicit_crash_phase4_recovers_diverse_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_w2d("phase4", &repo_path, true);
    assert!(
        !status.success(),
        "w2d implicit child must die abnormally at phase4; got {status:?}"
    );

    // Reopen + recover.
    let repo = open_repo(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(replayed, 1, "one inflight entry must be replayed");

    let tbl = repo.get_table(TABLE).await.unwrap();

    // Data must be present.
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "one record materialized by recovery");

    // Verify the field types survived the encode→lens→stage→crash→recover
    // round-trip with all values intact.
    let records = read_all_records(&tbl).await;
    assert_eq!(records.len(), 1, "exactly one record after recovery");
    let rec = &records[0];
    assert_eq!(rec["int_field"], 42i64);
    assert_eq!(rec["str_field"], "hello w2d");
    // Floats survive as f64.
    let expected_float: f64 = 99.5;
    assert!(
        (rec["float_field"].as_f64().unwrap() - expected_float).abs() < 1e-9,
        "float field preserved"
    );
    // Nested map + list survive.
    assert_eq!(rec["nested"]["inner_key"], "inner_val");
    assert_eq!(rec["nested"]["inner_num"], 7i64);
    assert_eq!(rec["list_field"], mpack!([10, 20, 30]));
}

/// W2d INTERACTIVE-tx crash at phase4 with a NEW field name: the overlay-id →
/// remap-on-bytes path at commit must survive a crash and recovery must
/// materialize the data with the remapped ids correct.
#[tokio::test]
async fn w2d_interactive_new_field_crash_phase4_recovers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_w2d("phase4", &repo_path, false);
    assert!(
        !status.success(),
        "w2d interactive child must die abnormally at phase4; got {status:?}"
    );

    // Reopen + recover.
    let repo = open_repo(&repo_path).await;
    let replayed = repo.recover_v2_inflight().await.expect("recovery");
    assert_eq!(replayed, 1, "one inflight entry must be replayed");

    let tbl = repo.get_table(TABLE).await.unwrap();
    let data = store_record_count(&tbl).await;
    assert_eq!(data, 1, "one record materialized by recovery");

    // The new field name must be present after recovery (the interner delta
    // + remap-on-bytes must have produced correct base-id-keyed bytes).
    let records = read_all_records(&tbl).await;
    assert_eq!(records.len(), 1, "exactly one record after recovery");
    let rec = &records[0];
    assert_eq!(rec["str_field"], "interactive w2d");
    // The brand-new field name survived the overlay→remap→crash→recover cycle.
    assert_eq!(
        rec["w2d_new_field"], "newness",
        "new field name must be readable after overlay remap + recovery"
    );
}

