//! Seal-hold store-level exclusion (brief item 8, test 3): data committed
//! AFTER `seal_point` returned stays OUT of the stores while the hold is
//! live — asserted on STORE state (durable watermark + history reads),
//! not on drain counters — and is applied once the hold drops.

use std::time::Duration;

use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::instance::repo_instance::OnlineBackupError;
use crate::table::TableConfig;
use crate::tx::tests::drain::drainer_tests::support::{make_repo, rid, seed_inflight_put};
use crate::tx::{MaterializationState, TxOutcome};

/// Harness cap; the handshakes are the hold lifecycle itself.
const APPLY_BUDGET: Duration = Duration::from_secs(30);

async fn commit_insert(repo: &crate::repo::RepoInstance, value: &str) -> TxOutcome {
    let table = repo.get_table("t").await.unwrap();
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str(value.into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap()
}

#[tokio::test]
async fn seal_hold_keeps_committed_after_v_star_data_out_of_stores_until_drop() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();

    // Pre-V* state: one visible commit, drained to durable.
    seed_inflight_put(&repo, "t", rid(1), "before", 1).await;
    gate.publish_committed_max(1);
    repo.drainer().drain_all(&repo.core).await.unwrap();
    assert_eq!(gate.durable_watermark(), 1);

    // Step E: the hold comes back owning the pause; V* = 1.
    let hold = repo.seal_point().await.expect("seal");
    assert_eq!(hold.position(), 1, "V* is the last committed version");
    assert!(hold.held_since() <= std::time::Instant::now());
    fn assert_send_static<T: Send + 'static>(_: &T) {}
    assert_send_static(&hold);
    assert!(!repo.core().commit_admission().is_closed());

    // Writers run again after `seal_point` returned: a commit AFTER V* is
    // assigned version 2. The commit is acknowledged WITHOUT publishing:
    // under the live pause the drainer cannot replay, so the post-WAL
    // settlement is DEFERRED to the drainer's orphan replay (`settle_visible`
    // drops the version guard unpublished; the armed guard's Drop registers
    // the Written ticket as an orphan, `VersionGuard::drop`). Publication
    // (`complete_orphaned_post_wal` → `sync_last_committed_from_watermark`)
    // happens only on the drain pass after the hold drops — asserted below.
    let outcome = commit_insert(&repo, "after").await;
    assert_eq!(
        outcome.commit_version, 2,
        "post-V* commit assigned version 2"
    );
    assert_eq!(
        outcome.materialization,
        MaterializationState::Deferred,
        "commit under a live hold is deferred to the drainer, not published inline"
    );
    // The commit is nevertheless WAL-durable: the entry survives for the
    // post-hold orphan replay.
    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.recover()
            .await
            .unwrap()
            .iter()
            .any(|e| e.commit_version == 2),
        "the deferred commit's WAL entry must be present while the hold is live"
    );

    // STORE state, not counters: the drainer has applied NOTHING above V*.
    assert_eq!(
        gate.durable_watermark(),
        1,
        "durable watermark must stay at V* while the hold is live"
    );
    assert!(
        table.get(rid(2)).await.is_err(),
        "history store must not contain post-V* data while the hold is live"
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

    // Drop the hold: the pause releases and the drainer applies the
    // committed-after-V* data.
    drop(hold);
    assert!(!repo.drainer().pause_held());
    let drained = tokio::time::timeout(APPLY_BUDGET, repo.drainer().drain_all(&repo.core))
        .await
        .expect("drain completes after the hold drops")
        .expect("drain succeeds");
    assert_eq!(drained, 1, "exactly the post-V* entry was applied");
    assert_eq!(gate.durable_watermark(), 2, "store state advanced past V*");
    assert_eq!(
        gate.last_committed(),
        2,
        "the deferred commit publishes on the post-hold orphan replay \
         (drain_step `complete_orphaned_post_wal` → \
         `sync_last_committed_from_watermark`)"
    );
    let got = table.get(rid(2)).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "after"),
        "post-V* data applied after the hold drops, got {got:?}"
    );
}
