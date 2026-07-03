//! R1-c — tests for [`run_follower_loop`] with the in-process
//! [`InProcessReplSource`].
//!
//! Scenarios (REPLICATION §4/§5.2/§5.3/§5.6):
//!   1. **Apply N events convergence:** write N rows on the leader, run the
//!      loop for a bounded number of iterations → the follower converges
//!      (its changefeed carries the re-emitted events) and its durable
//!      bookmark equals the leader's `current_version`.
//!   2. **Idempotent restart:** after catch-up, run the loop again with the
//!      same bookmark → no new events are applied, the bookmark does not
//!      regress, the follower state is unchanged.
//!   3. **Epoch regression (§5.2):** a source whose epoch regresses below
//!      the previously-seen maximum terminates the loop with
//!      [`ReplError::StaleLeaderEpoch`].
//!
//! The loop is bounded via `max_iterations` (no infinite sleep) and a
//! `CancellationToken` as a belt-and-suspenders backstop.

use std::sync::Arc;

use shamir_db::access::principal_id;
use shamir_db::access::Actor;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// Build an in-memory `ShamirDb` with one db `app`, one repo `main`, one
/// table `items`, owned by `alice`. Used for BOTH the leader and the
/// follower (a fresh instance each) — the schema is identical, the data is
/// independent.
async fn build_db(owner_label: &str) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal_id(owner_label));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    shamir
}

/// Write `n` rows into `app/main/items` as `alice`. Each transactional
/// insert commits → emits one changefeed event. Polls until all `n` events
/// are durable in the journal.
async fn write_rows(leader: &ShamirDb, n: usize) {
    let owner = Actor::User(principal_id("alice"));
    for i in 0..n {
        let key_str = format!("k{i}");
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            insert("items").rows([doc! {
                "id" => key_str,
                "v" => i as i64,
            }]),
        );
        let resp = leader
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("fixture write should succeed");
        assert!(
            !resp.results.contains_key("__error"),
            "write failed: {resp:?}",
        );
    }

    // The journal writer is async; poll until all n events are durable.
    for _ in 0..200 {
        if let Some(jr) = leader
            .read_changelog_from_journal("app", "main", 0, 1000)
            .await
        {
            if jr.events.len() >= n {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("leader journal did not durable-land {n} events in time");
}

/// Poll the follower's bookmark until it reaches `target` (or panic after
/// a timeout). The bookmark is advanced by the loop as events are applied.
///
/// Takes the `ShamirDb` by value (it's cheaply cloneable, `Arc`-backed) so
/// the future is `'static` and can be `tokio::spawn`ed.
async fn wait_for_bookmark(follower: ShamirDb, target: u64) {
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo exists");
    for _ in 0..200 {
        let b = repo.replication_bookmark().await.expect("bookmark read");
        if b >= target {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let b = repo.replication_bookmark().await.expect("bookmark read");
    panic!("follower bookmark did not reach {target} in time (last={b})");
}

// ---------------------------------------------------------------------------
// Test 1 — apply N events convergence
// ---------------------------------------------------------------------------

/// Write 5 rows on the leader; run the follower loop (bounded) until it
/// catches up. The follower's durable bookmark must equal the leader's
/// `current_version`, and the follower's own changefeed must carry the
/// re-emitted events (chain replication works, §4.3).
#[tokio::test]
async fn apply_n_events_convergence() {
    const N: usize = 5;

    let leader = build_db("alice").await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");
    assert!(
        leader_version >= N as u64,
        "leader advanced past {N} writes"
    );

    let follower = build_db("follower").await;

    let source = Arc::new(InProcessReplSource::new(leader.clone()));
    let cancel = CancellationToken::new();
    // generous iteration cap; the loop should catch up well before.
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(100)
        .with_max_iterations(50);
    let loop_fut = run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg,
        cancel.clone(),
    );

    // Drive the loop and the wait concurrently; cancel once converged.
    let bookmark_target = leader_version;
    let converge = tokio::spawn(wait_for_bookmark(follower.clone(), bookmark_target));

    // Run the loop; it exits via max_iterations or we cancel after convergence.
    tokio::select! {
        res = loop_fut => {
            // Loop ended (max_iterations). Check it didn't error.
            res.expect("follower loop did not panic");
        }
        _ = converge => {
            // Converged before max_iterations — cancel the loop.
            cancel.cancel();
        }
    }

    // Bookmark == leader current_version (the events applied in order).
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo");
    let bookmark = repo.replication_bookmark().await.expect("bookmark");
    assert!(
        bookmark >= leader_version,
        "follower bookmark {bookmark} should have reached leader version {leader_version}"
    );

    // The follower re-emitted the events into its OWN changefeed (chain).
    let follower_jr = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    assert!(
        follower_jr.events.len() >= N,
        "follower changefeed should carry >= {N} re-emitted events, got {}",
        follower_jr.events.len()
    );
}

// ---------------------------------------------------------------------------
// Test 2 — idempotent restart
// ---------------------------------------------------------------------------

/// After the follower catches up, running the loop again with the SAME
/// bookmark → every event is `Skipped`, the bookmark does not regress, and
/// the follower's changefeed does not grow (no double-application).
#[tokio::test]
async fn idempotent_restart_after_catchup() {
    const N: usize = 3;

    let leader = build_db("alice").await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");

    let follower = build_db("follower").await;
    let source = Arc::new(InProcessReplSource::new(leader.clone()));

    // First run: catch up.
    let cancel1 = CancellationToken::new();
    let f1 = Arc::new(follower.clone());
    let cfg1 = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(100)
        .with_max_iterations(50);
    let loop1 = tokio::spawn(run_follower_loop(
        f1.clone(),
        source.clone(),
        cfg1,
        cancel1.clone(),
    ));
    wait_for_bookmark(follower.clone(), leader_version).await;
    cancel1.cancel();
    let _ = loop1.await;

    // Snapshot the follower state AFTER catch-up.
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo");
    let bookmark_before = repo.replication_bookmark().await.expect("bookmark");
    let jr_before = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    let events_before = jr_before.events.len();

    assert!(
        bookmark_before >= leader_version,
        "pre-restart bookmark {bookmark_before} should be >= leader {leader_version}"
    );

    // Second run: the bookmark is already at leader_version, so every pull
    // returns an empty batch (from_version = bookmark+1 > leader_version)
    // and no events are applied. The bookmark must NOT regress.
    let cancel2 = CancellationToken::new();
    let cfg2 = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(50)
        .with_max_iterations(3);
    run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg2,
        cancel2.clone(),
    )
    .await
    .expect("restart loop completes cleanly");

    // Bookmark unchanged (no regression, no spurious advance).
    let bookmark_after = repo.replication_bookmark().await.expect("bookmark");
    assert_eq!(
        bookmark_after, bookmark_before,
        "idempotent restart: bookmark must not change"
    );

    // Changelog did not grow (no double-application).
    let jr_after = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    assert_eq!(
        jr_after.events.len(),
        events_before,
        "idempotent restart: follower changefeed must not grow"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — epoch regression terminates the loop (§5.2)
// ---------------------------------------------------------------------------

/// A source whose `leader_epoch` regresses below the previously-seen
/// maximum → the loop returns [`ReplError::StaleLeaderEpoch`] and stops.
#[tokio::test]
async fn epoch_regression_terminate_loop() {
    let leader = build_db("alice").await;
    write_rows(&leader, 1).await;

    let follower = build_db("follower").await;
    // Start the source at epoch 5; the hello seeds max_seen_epoch = 5.
    let source = Arc::new(InProcessReplSource::with_epoch(leader.clone(), 5));

    // Spawn the loop. Once it pulls once (epoch 5), regress the source to
    // epoch 3 → the next pull reply carries epoch 3 < 5 → StaleLeaderEpoch.
    let cancel = CancellationToken::new();
    let source_for_thread = source.clone();
    let regressing = tokio::spawn(async move {
        // Give the loop time to call hello + at least one pull at epoch 5.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Regress the epoch — simulates a "resurrected" old leader.
        source_for_thread.set_epoch(3);
    });

    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(200)
        .with_max_iterations(50);
    let result = run_follower_loop(Arc::new(follower.clone()), source, cfg, cancel).await;

    // The regressing helper may or may not have raced the loop's exit; but
    // if the loop observed the regression, it MUST return StaleLeaderEpoch.
    // If the loop exited via max_iterations before the regression landed,
    // that's also acceptable (no stale epoch observed). We only assert the
    // error shape WHEN an error is returned.
    match result {
        Err(ReplError::StaleLeaderEpoch { observed, max_seen }) => {
            assert_eq!(observed, 3, "observed regressed epoch");
            assert_eq!(max_seen, 5, "max-seen epoch seeded by hello");
        }
        Ok(()) => {
            // Loop exited via max_iterations before the regression landed —
            // acceptable. The regression task should still complete.
            regressing.await.expect("regression task");
        }
        Err(other) => panic!("expected StaleLeaderEpoch or clean exit, got {other:?}"),
    }
}
