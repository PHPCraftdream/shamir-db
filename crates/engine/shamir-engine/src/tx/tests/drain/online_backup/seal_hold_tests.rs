//! Seal-hold store-level exclusion (brief item 8, test 3): data committed
//! AFTER `seal_point` returned stays OUT of the stores while the hold is
//! live — asserted on STORE state (durable watermark) and WAL evidence,
//! not on drain counters — and is applied once the hold drops.

use std::time::Duration;

use shamir_query_types::batch::VisibilityWait;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::instance::repo_instance::OnlineBackupError;
use crate::table::TableConfig;
use crate::tx::tests::drain::drainer_tests::support::make_repo;
use crate::tx::CommitError;

/// Harness cap; the handshakes are the hold lifecycle itself.
const APPLY_BUDGET: Duration = Duration::from_secs(30);

async fn commit_insert(repo: &crate::repo::RepoInstance, value: &str) {
    let table = repo.get_table("t").await.unwrap();
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str(value.into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
}

/// Commit under a live hold with a BOUNDED visibility wait: the default
/// (`VisibilityWait::Strict`, `RepoInstance::new` — repo_instance.rs:143)
/// would park forever on `visible_floor < 2` because `mark_durable` only
/// fires in the drainer's replay, which the hold pauses. `Bounded { 0 }`
/// skips the wait entirely (tail.rs:129) and acknowledges without waiting.
/// The ack shape is either `Ok(outcome)` with a Pending visibility (the
/// commit is durable and published inline, only drain-durability lags) or
/// `Err(CommitError::WalDurability { .. })` (WAL uncertainty). BOTH are
/// accepted: neither un-commits the write, whose WAL entry is asserted
/// directly below.
async fn commit_insert_under_hold(
    repo: &crate::repo::RepoInstance,
    value: &str,
) -> shamir_types::types::record_id::RecordId {
    let table = repo.get_table("t").await.unwrap();
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let id = table
        .insert_tx(&InnerValue::Str(value.into()), Some(&mut tx))
        .await
        .unwrap();
    match repo
        .commit_tx_with_options(tx, VisibilityWait::Bounded { timeout_ms: 0 })
        .await
    {
        Ok(_) | Err(CommitError::WalDurability { .. }) => {}
        Err(error) => panic!("under-hold commit failed unexpectedly: {error}"),
    }
    id
}

#[tokio::test]
async fn seal_hold_keeps_committed_after_v_star_data_out_of_stores_until_drop() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();

    // Pre-V* state: one REAL commit through the allocator (it allocates
    // version 1 and publishes inline), then drained to durable. Seeding via
    // `publish_committed_max` would bypass the allocator
    // (`RepoTxGate::new` seeds `version_counter` from `last_committed`,
    // gate/repo_tx_gate/core.rs:15; the seam settles the tracker and raises
    // the published marker but never touches the allocator,
    // core.rs:175-178), so the post-V* commit would collide on version 1.
    commit_insert(&repo, "before").await;
    repo.drainer().drain_all(&repo.core).await.unwrap();
    assert_eq!(gate.durable_watermark(), 1);
    assert_eq!(gate.last_committed(), 1);

    // Step E: the hold comes back owning the pause; V* = 1.
    let hold = repo.seal_point().await.expect("seal");
    assert_eq!(hold.position(), 1, "V* is the last committed version");
    assert!(hold.held_since() <= std::time::Instant::now());
    fn assert_send_static<T: Send + 'static>(_: &T) {}
    assert_send_static(&hold);
    assert!(!repo.core().commit_admission().is_closed());

    // Writers run again after `seal_point` returned: the post-V* commit is
    // assigned version 2 by the allocator and settles INLINE — `materialize`'s
    // Phase 5a → `settle_visible(visible=true)` → `publish_journaled` →
    // `VersionGuard::commit` (materialize.rs:235/264-280, publish_journaled.rs:56,
    // version_guard.rs:150-165) has no pause dependency, so `last_committed`
    // advances to 2 DURING the hold. What stays out of the stores is
    // DRAIN-durability: `mark_durable` fires only in the drainer's replay,
    // which the hold pauses.
    let after_id = commit_insert_under_hold(&repo, "after").await;

    // The commit is WAL-durable; its entry (version taken FROM THE WAL, not
    // from the early acknowledgement) survives for the post-hold replay.
    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.recover()
            .await
            .unwrap()
            .iter()
            .any(|e| e.commit_version == 2),
        "the post-V* commit's WAL entry must be present while the hold is live"
    );

    // STORE state, not counters: the drainer has applied NOTHING above V*.
    assert_eq!(
        gate.durable_watermark(),
        1,
        "durable watermark must stay at V* while the hold is live"
    );

    // The hold owns the pause exclusively: a second seal and a capture are
    // typed-refused.
    assert!(matches!(
        repo.seal_point().await,
        Err(OnlineBackupError::HoldActive)
    ));
    assert!(matches!(
        repo.capture_point().await,
        Err(OnlineBackupError::HoldActive)
    ));

    // Drop the hold: the pause releases and the drain applies the
    // committed-after-V* data. The background loop may have applied the
    // version-2 entry first; the explicit pass is idempotent. The store
    // state assertions below are the application proof.
    drop(hold);
    assert!(!repo.drainer().pause_held());
    let _ = tokio::time::timeout(APPLY_BUDGET, repo.drainer().drain_all(&repo.core))
        .await
        .expect("drain completes after the hold drops")
        .expect("drain succeeds");
    assert_eq!(gate.durable_watermark(), 2, "store state advanced past V*");
    assert_eq!(
        gate.last_committed(),
        2,
        "the post-V* commit stays published across the drain"
    );
    let got = table.get(after_id).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "after"),
        "post-V* data applied after the hold drops, got {got:?}"
    );
}
