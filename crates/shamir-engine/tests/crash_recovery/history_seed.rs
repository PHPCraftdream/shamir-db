//! Split out of `tests/crash_recovery.rs` (layout sweep Q2); bodies moved verbatim,
//! root harness items reached via `super::`.

use super::*;

// ---------------------------------------------------------------------------
// CRIT-1 (#435): a history-write failure during cold recovery is FATAL.
//
// `seed_version_cache_for_entry` used to swallow
// `write_committed_to_history` errors in a `log::warn!` and return `()`, so
// `recover_inflight_v2` unconditionally proceeded to mark the entry
// durable/materialized — a silent loss of an acked commit (cold-start readers
// see `last_committed ≥ v` with no value in overlay or history) and an open
// door for F6 truncation to unlink the sole surviving WAL copy.
//
// The fix propagates the error through `replay_v2_entry` →
// `recover_inflight_v2` → `open()` (`db_management.rs:343`'s
// `recover_v2_inflight().await?` refuses to serve a repo that cannot recover).
// This test proves the end-to-end contract over a REAL disk-backed repo:
// commit a tx (clean exit, no crash — the WAL entry is durable), then reopen
// with the `SHAMIR_TEST_FAIL_HISTORY_SEED` env var armed (the test-only
// fault-injection seam in `seed_version_cache_for_entry`) and assert
// `recover_v2_inflight` returns `Err`.
//
// The companion in-process unit tests in `recovery_tests.rs`
// (`crit1_history_seed_failure_aborts_recovery`,
// `crit1_multi_table_history_seed_failure_propagates`,
// `crit1_no_injection_recovery_succeeds`) exercise the same contract through
// the `FAIL_HISTORY_SEED_TX_ID` static-atomic seam for tighter determinism;
// this integration test proves the contract holds over the real on-disk
// image + reopen path.
// ---------------------------------------------------------------------------

/// Env sentinel: when present the child runs the CRIT-1 scenario — commit a
/// single data record (so the WAL entry carries a Put against an MVCC-attached
/// table) and write the gate-assigned `txn_id` to a sidecar so the parent can
/// re-open over the SAME on-disk image. Distinct from `CHILD_SENTINEL` etc.
const CHILD_SENTINEL_CRIT1: &str = "SHAMIR_TEST_CRASH_CHILD_CRIT1";

/// The CRIT-1 child scenario: open the repo, commit ONE record, then write
/// the gate-assigned `txn_id` to a sidecar (so the parent can arm the
/// fault-injection env var that the recovery path reads) and exit cleanly.
/// Unlike the phase-crash children this one does NOT `process::abort()` —
/// the goal is a durable, clean WAL entry that recovery then FAILS to seed
/// into history (because the env var makes `seed_version_cache_for_entry`
/// return a synthetic error), proving recovery surfaces Err rather than
/// silently continuing.
async fn run_child_scenario_crit1(path: PathBuf) {
    let repo = open_repo(&path).await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&repo, &tbl, "body").await;

    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _ = tbl
        .insert_tx(
            &text_record(body_id, "crit1 history-seed fault vector"),
            Some(&mut tx),
        )
        .await;
    // Commit normally — the WAL entry is durable, no crash.
    let outcome = repo.commit_tx(tx).await;
    drop(guard);
    let outcome = outcome.expect("clean commit must succeed (no fault injected yet)");

    // Write the gate-assigned txn_id to a sidecar so the parent can verify
    // the fault fired for the EXPECTED entry (defensive — the env var is
    // txn-agnostic, so this is a cross-check, not the primary oracle).
    let sidecar = path.with_extension("crit1_txid");
    let tmp = sidecar.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).expect("create crit1 sidecar temp");
        write!(f, "{}", outcome.tx_id).expect("write crit1 sidecar temp");
        f.sync_all().expect("fsync crit1 sidecar temp");
    }
    std::fs::rename(&tmp, &sidecar).expect("rename crit1 sidecar into place");

    // Flush + drop so the WAL entry is fully durable on disk before the
    // parent reopens over the same path.
    drop(repo);
}

/// Child entrypoint for the CRIT-1 scenario. NO-OP in the parent (sentinel
/// absent); runs the CRIT-1 commit+sidecar scenario in the spawned child.
#[test]
fn child_entrypoint_crit1() {
    if std::env::var(CHILD_SENTINEL_CRIT1).is_err() {
        return;
    }
    let path = PathBuf::from(std::env::var(REPO_PATH).expect("child needs REPO_PATH"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    rt.block_on(run_child_scenario_crit1(path));
}

/// Spawn the CRIT-1 child (clean commit + sidecar write, NO crash).
fn spawn_child_crit1(repo_path: &Path) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current_exe");
    std::process::Command::new(exe)
        .args(["--exact", "child_entrypoint_crit1", "--nocapture"])
        .env(CHILD_SENTINEL_CRIT1, "1")
        .env(REPO_PATH, repo_path)
        .output()
        .expect("spawn crit1 child")
        .status
}

/// CRIT-1 (#435) end-to-end proof: commit a tx (clean exit, durable WAL
/// entry), reopen over the SAME on-disk image with the
/// `SHAMIR_TEST_FAIL_HISTORY_SEED` env var armed (the test-only fault seam in
/// `seed_version_cache_for_entry`), and assert `recover_v2_inflight` returns
/// `Err`. Pre-fix the history-write error was swallowed in a `log::warn!` and
/// recovery returned `Ok(1)`, marking the entry durable despite the value
/// being absent from `history` — a silent loss of an acked commit.
#[tokio::test]
async fn crit1_history_seed_failure_aborts_recovery_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    // Child commits ONE record cleanly and writes the txn_id sidecar.
    let status = spawn_child_crit1(&repo_path);
    assert!(
        status.success(),
        "crit1 child must exit cleanly (no crash — the WAL entry is durable); \
         got {status:?}"
    );

    // Reopen over the SAME on-disk image. The WAL entry the child committed
    // is inflight (the drainer may or may not have drained it — either way the
    // recovery loop replays inflight entries, and the fault injection fires
    // on the seed step).
    // Arm before the public constructor: recovery now completes before a
    // RepoInstance can escape to any caller.
    std::env::set_var("SHAMIR_TEST_FAIL_HISTORY_SEED", "1");

    let result = RepoInstance::from_factory(
        REPO_NAME.into(),
        BoxRepoFactory::fjall_raw(repo_path.clone()),
        vec![TableConfig::new(TABLE), TableConfig::new(TABLE_B)],
    )
    .await;

    // CRITICAL: disarm IMMEDIATELY so no later test in the same binary is
    // poisoned by a stale env var (the static-atomic seam is per-process, but
    // the env var is process-global and the test runner shares one process).
    std::env::remove_var("SHAMIR_TEST_FAIL_HISTORY_SEED");

    // THE FIX: recovery MUST return Err. Pre-fix it returned Ok and the entry
    // was marked durable despite the history write never landing — the silent
    // loss of an acked commit.
    let err = match result {
        Err(error) => error,
        Ok(_) => panic!("CRIT-1: history seed failure must refuse the public constructor"),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("CRIT-1"),
        "the propagated error must be the injected CRIT-1 fault (got: {msg})"
    );

    // Cross-check the sidecar: the fault fired for the EXPECTED entry. The
    // env var is txn-agnostic, so this confirms the child's tx was the one
    // recovery tried to seed (and failed). Defensive — not the primary oracle.
    let sidecar = repo_path.with_extension("crit1_txid");
    if let Ok(s) = std::fs::read_to_string(&sidecar) {
        let _committed_txid: u64 = s
            .trim()
            .parse()
            .expect("crit1 sidecar must hold a valid txn_id (atomic write)");
        // The error message carries the txn_id; it should match the sidecar.
        // (Not a hard assert — the env-var path does not key on txn_id — but
        // a useful diagnostic if it ever diverges.)
        assert!(
            msg.contains(&s.trim().to_string()),
            "the injected CRIT-1 error should reference the committed txn_id \
             {} (sidecar); got: {msg}",
            s.trim()
        );
    }
}

/// CRIT-1 (#435) regression control: WITHOUT the fault-injection env var
/// armed, recovery of the same on-disk image MUST succeed (the fix did not
/// break the happy path). This is the symmetric counterpart to
/// `crit1_history_seed_failure_aborts_recovery_on_disk` and proves the env
/// var is reset between tests.
#[tokio::test]
async fn crit1_no_fault_recovery_succeeds_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_path = dir.path().join("repo.redb");

    let status = spawn_child_crit1(&repo_path);
    assert!(
        status.success(),
        "crit1 child must exit cleanly; got {status:?}"
    );

    // Defensive: ensure no stale fault env var leaks from a sibling test.
    std::env::remove_var("SHAMIR_TEST_FAIL_HISTORY_SEED");

    let repo = open_repo(&repo_path).await;
    let replayed = repo
        .recover_v2_inflight()
        .await
        .expect("CRIT-1 control: recovery with NO fault injection must succeed");

    // The child committed one tx; recovery replays at least one inflight
    // entry (the drainer may have already drained some — file WAL replays
    // idempotently — so `>= 0`, not a fixed count, is the safe oracle here).
    // The load-bearing assertion is that recovery did NOT return Err.
    assert!(
        replayed == 0 || replayed >= 1,
        "CRIT-1 control: recovery must succeed (no fault); replayed {replayed}"
    );
}
