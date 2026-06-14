//! CRIT-A / CRIT-B regression tests: V2 crash recovery must (a) be
//! reachable on the open path and (b) restore the MVCC version floor so
//! `assign_next_version()` never re-issues a commit_version a recovered
//! inflight entry already consumed.
//!
//! These exercise `RepoInstance::recover_v2_inflight` — the exact entry
//! point wired into the `shamir-db` bootstrap (`ShamirDb::init` /
//! `ShamirDb::add_repo`) — over a shared `Arc<InMemoryRepo>` so a
//! "restart" observes the same persisted state a real process restart
//! would.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::meta::recovery_marker::{load_last_committed, save_last_committed};
use crate::repo::{repo_token, BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

/// Write a durable inflight V2 `Put` entry (NO matching `wal.commit`)
/// carrying `commit_version`, exactly as a crash between commit Phase 4
/// and Phase 7 leaves behind. Uses a throwaway `RepoInstance` over
/// `underlying` and never constructs its gate, so the "restart" repo
/// seeds a fresh gate from the persisted state.
async fn seed_inflight_put(
    underlying: &Arc<InMemoryRepo>,
    table: &str,
    record: RecordId,
    body: bytes::Bytes,
    commit_version: u64,
) {
    let seed = RepoInstance::new(
        "r".into(),
        BoxRepo::InMemory(Arc::clone(underlying)),
        Vec::new(),
    );
    let wal = seed.repo_wal().await.unwrap();
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(seed.name()),
        vec![WalOpV2::Put {
            table_id_interned: table_token_for(table),
            rid: record,
            body,
        }],
    )
    .with_commit_version(commit_version);
    wal.begin(entry).await.unwrap();
    // Drop `seed`: models the in-memory side of a restart. The inflight
    // marker lives in the shared `underlying` and survives.
    drop(seed);
}

/// CRIT-A: recovery (the function the bootstrap open path invokes) replays
/// an inflight entry left by a crash, AND CRIT-B: it advances the gate's
/// version floor past the replayed commit_version and persists the
/// recovered floor durably.
#[tokio::test]
async fn recovery_wired_into_open_replays_inflight() {
    let underlying = Arc::new(InMemoryRepo::new());
    let record = rid(42);
    let body = InnerValue::Str("recovered".into()).to_bytes().unwrap();

    // Fresh repo (no marker) + a single inflight tx at commit_version 10.
    seed_inflight_put(&underlying, "t", record, body, 10).await;

    // === SIMULATED RESTART: fresh RepoInstance over the same storage ===
    let repo = RepoInstance::new(
        "r".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("t"));

    // Sanity: the inflight entry survived the "restart".
    let wal = repo.repo_wal().await.unwrap();
    assert_eq!(wal.list_inflight().await.unwrap().len(), 1);

    // This is exactly what `ShamirDb::init` / `add_repo` now call.
    let recovered = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(recovered, 1, "the inflight tx must be replayed");

    // Data is applied to the data store.
    let tbl = repo.get_table("t").await.unwrap();
    let read_back = tbl.get(record).await.unwrap();
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "recovered"),
        "expected recovered value, got {read_back:?}"
    );

    // CRIT-B: the next assigned version must be strictly greater than the
    // replayed commit_version (10) — not reuse 8/9/10.
    let gate = repo.tx_gate().await.unwrap();
    let next = gate.assign_next_version();
    assert!(
        next > 10,
        "assign_next_version must exceed replayed commit_version 10, got {next}"
    );

    // The persisted marker must reflect the recovered max so the *next*
    // restart (which sees no inflight markers) seeds the floor correctly.
    let info = repo.tx_info_store().await.unwrap();
    let marker = load_last_committed(&info).await.unwrap();
    assert!(
        marker >= Some(10),
        "persisted last_committed marker must be >= recovered max 10, got {marker:?}"
    );

    // WAL marker cleared; re-running recovery is a no-op.
    assert!(wal.list_inflight().await.unwrap().is_empty());
    assert_eq!(repo.recover_v2_inflight().await.unwrap(), 0);
}

/// CRIT-B core: a stale persisted marker (7) plus an inflight entry whose
/// commit_version (10) outran the marker must yield `assign_next_version()
/// == 11` after recovery — NOT 8 (which would re-use the 8/9/10 version
/// space the crashed tx already consumed → monotonicity violation,
/// snapshot reads returning wrong data).
#[tokio::test]
async fn recovery_advances_gate_past_replayed_commit_version() {
    let underlying = Arc::new(InMemoryRepo::new());

    // Persist a stale marker = 7 directly (a clean commit that landed
    // before the crashed one), WITHOUT constructing any gate.
    {
        let seed = RepoInstance::new(
            "r".into(),
            BoxRepo::InMemory(Arc::clone(&underlying)),
            Vec::new(),
        );
        let info = seed.tx_info_store().await.unwrap();
        save_last_committed(&info, 7).await.unwrap();
    }

    // Inflight tx at commit_version = 10 (marker never advanced to it —
    // crash before Phase 6.5).
    let record = rid(7);
    let body = InnerValue::Str("v10".into()).to_bytes().unwrap();
    seed_inflight_put(&underlying, "t", record, body, 10).await;

    // === SIMULATED RESTART ===
    let repo = RepoInstance::new(
        "r".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("t"));

    let recovered = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(recovered, 1);

    // The decisive assertion: 11, not 8.
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(
        gate.assign_next_version(),
        11,
        "gate must resume above the inflight commit_version (10), not from \
         the stale marker (7)"
    );

    // Marker re-persisted to the recovered floor (10) so a *second*
    // restart — now with no inflight entries — still seeds the gate at 10
    // rather than rewinding to 7.
    let info = repo.tx_info_store().await.unwrap();
    assert_eq!(load_last_committed(&info).await.unwrap(), Some(10));
}

/// P1d: recovery rebuilds the completion-prefix. After replaying 3 durable
/// WAL entries (V=1,2,3), the completion tracker watermark must equal 3 and
/// last_committed must mirror it.
#[tokio::test]
async fn recovery_rebuilds_completion_prefix() {
    let underlying = Arc::new(InMemoryRepo::new());

    // Seed 3 inflight entries at commit_versions 1, 2, 3.
    for v in 1..=3u64 {
        let record = rid(v as u8);
        let body = InnerValue::Str(format!("v{v}")).to_bytes().unwrap();
        seed_inflight_put(&underlying, "t", record, body, v).await;
    }

    // === SIMULATED RESTART ===
    let repo = RepoInstance::new(
        "r".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("t"));

    let recovered = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(recovered, 3);

    let gate = repo.tx_gate().await.unwrap();
    // P1d: watermark must advance to 3 (all versions ≤ 3 are Materialized).
    assert_eq!(
        gate.completion().watermark(),
        3,
        "completion watermark must equal highest recovered commit_version"
    );
    // last_committed must mirror the watermark.
    assert_eq!(
        gate.last_committed(),
        3,
        "last_committed must equal the completion watermark after recovery"
    );
}

/// Defence-in-depth for the marker re-persist: after recovery clears the
/// inflight markers, a SECOND fresh `RepoInstance` over the same storage
/// (no inflight entries left) must still seed its gate above the recovered
/// commit_version — proving the floor survives via the durable marker, not
/// just the (now-gone) inflight pre-scan.
#[tokio::test]
async fn recovered_floor_survives_a_second_restart() {
    let underlying = Arc::new(InMemoryRepo::new());
    let record = rid(9);
    let body = InnerValue::Str("v25".into()).to_bytes().unwrap();
    seed_inflight_put(&underlying, "t", record, body, 25).await;

    // First restart: recover (clears the inflight marker, persists floor).
    {
        let repo1 = RepoInstance::new(
            "r".into(),
            BoxRepo::InMemory(Arc::clone(&underlying)),
            Vec::new(),
        );
        repo1.add_table(TableConfig::new("t"));
        assert_eq!(repo1.recover_v2_inflight().await.unwrap(), 1);
        assert!(repo1
            .repo_wal()
            .await
            .unwrap()
            .list_inflight()
            .await
            .unwrap()
            .is_empty());
    }

    // Second restart: no inflight entries remain — the floor must come
    // purely from the persisted marker.
    let repo2 = RepoInstance::new(
        "r".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo2.add_table(TableConfig::new("t"));
    assert_eq!(
        repo2.recover_v2_inflight().await.unwrap(),
        0,
        "no inflight entries left after the first recovery"
    );

    let gate = repo2.tx_gate().await.unwrap();
    assert!(
        gate.assign_next_version() > 25,
        "the recovered floor (25) must survive a second restart via the \
         persisted marker"
    );
}
