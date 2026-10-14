//! Scenario B — a SECOND crash after convergence, and the at-or-below
//! bookmark branch:
//!
//! 1. converge a seeded follower (offline seed → Resume → catch up);
//! 2. after the follower loop is stopped, inject a certificate AFTER the
//!    leader is killed and BEFORE its first restart; the injection models
//!    unavailable recovery evidence, with no automatic-gap premise;
//! 3. a second offline seed into ANOTHER new repository converges;
//! 4. a crash with NO new writes (new horizon == follower bookmark) keeps the
//!    follower active and able to continue from bookmark + 1.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use shamir_client::Client;
use shamir_db::engine::tx::ReplicatedEvent;
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_server::replication_seed::Manifest;

use crate::harness::child;
use crate::harness::follower;
use crate::harness::ops;
use crate::harness::wire;

const WAIT: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_e2e_b_second_crash_reseeds_and_equal_horizon_continues() {
    let root = tempfile::tempdir().expect("temp root");
    let leader_dir = root.path().join("leader");
    let seed1 = root.path().join("seed-1");
    let seed2 = root.path().join("seed-2");
    let home1 = root.path().join("home-1");
    let home2 = root.path().join("home-2");
    let addr = child::free_addr();
    let upstream = format!("tcp://{addr}");

    // -- Leader v1: writes, unclean kill, WAL repair, explicit gap, export. --
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    wire::setup_leader(&admin).await;
    wire::put(&admin, "items", "k0", 0).await;
    wire::put(&admin, "audit", "a0", 1).await;
    drop(admin);
    child::kill(&mut leader).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let gap_cert = wire::gap_certificate(addr).await;
    let admin = child::connect_admin(&leader).await;
    let horizon = wire::current_version(&admin).await;
    drop(admin);
    assert_eq!(horizon, 2);
    assert_eq!(gap_cert.gap_first, None);
    assert_eq!(gap_cert.gap_through, None);
    child::kill(&mut leader).await;
    child::inject_recovery_gap(&leader_dir, 1, horizon).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let cert_before_second_crash = wire::gap_certificate(addr).await;
    assert_eq!(cert_before_second_crash.gap_first, Some(1));
    assert_eq!(cert_before_second_crash.gap_through, Some(horizon));
    drop(child::connect_admin(&leader).await);
    child::kill(&mut leader).await;
    let digest1 = child::cli_export(&leader.config_path, &seed1).await;

    // -- Seed #1 into home-1 and converge. --
    let home1_cfg = child::write_home_config(&home1, addr);
    child::cli_install(&home1_cfg, &seed1, &digest1, &upstream).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    let home1 = Arc::new(follower::open_home(&home1).await);
    let loop1 = follower::supervisor(&home1);
    ops::resume(&home1).await.expect("resume home-1");
    loop1.reconcile().await;
    wait_converged(&home1, &admin, addr).await;
    loop1.stop_all().await;
    let bookmark1 = follower::bookmark(&home1).await;
    assert_eq!(bookmark1, 2, "home-1 converged through leader version 2");
    assert_eq!(bookmark1, wire::current_version(&admin).await);
    let before = ops::data_fingerprint(&home1).await;

    // -- Leader writes MORE; simulate lost recovery evidence. --
    wire::put(&admin, "items", "k1", 10).await;
    wire::update_row(&admin, "audit", "a0", 77).await;
    let horizon = wire::current_version(&admin).await;
    assert_eq!(
        horizon,
        bookmark1 + 2,
        "exactly two commits follow convergence"
    );
    drop(admin);
    child::kill(&mut leader).await;
    // This certificate fixture models unavailable recovery evidence, not
    // physical disk corruption; inject it after kill and before first restart.
    child::inject_recovery_gap(&leader_dir, bookmark1 + 1, horizon).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    drop(child::connect_admin(&leader).await);
    let cert = wire::gap_certificate(addr).await;
    let mut expected_cert = cert_before_second_crash.clone();
    expected_cert.gap_through = Some(horizon);
    assert_eq!(
        cert, expected_cert,
        "extension preserves certificate lineage and schema"
    );
    let cert_after_extension = cert.clone();
    assert_eq!(follower::bookmark(&home1).await, bookmark1);
    assert_eq!(ops::data_fingerprint(&home1).await, before);
    // No old polling task may race the frozen bookmark or hide the injected
    // evidence-loss boundary; recreate the supervisor only after those checks.
    let loop1 = follower::supervisor(&home1);
    loop1.reconcile().await;
    follower::wait_until(
        "home-1 resync_required after second crash",
        WAIT,
        &home1,
        addr,
        || {
            let home1 = home1.clone();
            async move {
                follower::status_of(&home1, "sub")
                    .await
                    .is_some_and(|entry| follower::str_field(&entry, "state") == "resync_required")
            }
        },
    )
    .await;
    assert_eq!(
        ops::data_fingerprint(&home1).await,
        before,
        "no payload may apply past a gap above the bookmark",
    );
    assert_eq!(follower::bookmark(&home1).await, bookmark1);
    loop1.stop_all().await;

    // -- Second offline seed into ANOTHER new repository. --
    child::kill(&mut leader).await;
    let digest2 = child::cli_export(&leader.config_path, &seed2).await;
    let bytes = std::fs::read(seed2.join("manifest.mpack")).expect("manifest");
    let manifest2: Manifest = rmp_serde::from_slice(&bytes).expect("decode manifest");
    assert_eq!(
        manifest2.version, horizon,
        "seed captures the exact second-crash horizon"
    );
    assert_eq!(manifest2.certified_gap, Some((1, horizon)));
    let home2_cfg = child::write_home_config(&home2, addr);
    child::cli_install(&home2_cfg, &seed2, &digest2, &upstream).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    let home2 = Arc::new(follower::open_home(&home2).await);
    let loop2 = follower::supervisor(&home2);
    let entry = follower::status_of(&home2, "sub").await.expect("status");
    assert_eq!(follower::str_field(&entry, "state"), "paused");
    assert_eq!(
        follower::bookmark(&home2).await,
        manifest2.version,
        "installed home starts at the exact persisted seed version",
    );
    assert_eq!(
        follower::str_field(&entry, "installation_state"),
        "complete"
    );
    assert_eq!(
        follower::u64_field(&entry, "seed_source_version"),
        Some(manifest2.version),
        "second seed carries the second-crash version",
    );
    ops::resume(&home2).await.expect("resume home-2");
    loop2.reconcile().await;
    wait_converged(&home2, &admin, addr).await;
    assert_eq!(
        follower::bookmark(&home2).await,
        wire::current_version(&admin).await,
        "fully caught up before the equal-horizon crash",
    );

    // -- Opposite branch: crash with NO new writes (horizon == bookmark). --
    drop(admin);
    child::kill(&mut leader).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    let cert = wire::gap_certificate(addr).await;
    assert_eq!(
        cert, cert_after_extension,
        "the extended certificate remains immutable at equal horizon"
    );
    assert_eq!(
        cert.gap_through,
        Some(follower::bookmark(&home2).await),
        "no writes since convergence: the certified horizon equals the bookmark",
    );
    let before = ops::data_fingerprint(&home2).await;
    loop2.stop_all().await;
    let loop2 = follower::supervisor(&home2);
    loop2.reconcile().await;
    follower::wait_until(
        "home-2 loop running at equal horizon",
        WAIT,
        &home2,
        addr,
        || async { loop2.is_running("sub") },
    )
    .await;
    // The loop pulls bookmark+1 (above the horizon), applies nothing, and
    // must NOT be stopped as resync_required.
    assert_eq!(ops::data_fingerprint(&home2).await, before);
    assert_eq!(
        follower::str_field(
            &follower::status_of(&home2, "sub").await.expect("status"),
            "state",
        ),
        "active",
        "equal-horizon follower remains active",
    );
    // Prove the authenticated continuation explicitly from bookmark + 1.
    let continuation = child::connect(addr, child::REPLICATOR, child::REPLICATOR_PW)
        .await
        .repl(ReplRequest::Pull {
            db: wire::DB.to_string(),
            repo: wire::REPO.to_string(),
            from_version: follower::bookmark(&home2).await + 1,
            limit: 64,
            wait_ms: Some(0),
            capabilities: None,
        })
        .await
        .expect("equal-horizon continuation pull");
    let ReplResponse::Pull {
        gap_at,
        current_version,
        events,
        ..
    } = continuation
    else {
        panic!("expected continuation Pull, got {continuation:?}");
    };
    assert_eq!(gap_at, None);
    assert_eq!(current_version, horizon);
    let events: Vec<ReplicatedEvent> =
        rmp_serde::from_slice(&events).expect("decode empty Pull event bytes");
    assert!(events.is_empty(), "no events exist beyond equal horizon");
    assert_eq!(follower::bookmark(&home2).await, horizon);

    wire::put(&admin, "items", "k2", 22).await;
    for table in wire::TABLES {
        let expected = wire::read_rows(&admin, table).await;
        follower::wait_until(
            &format!("equal-horizon continuation converges on {table}"),
            WAIT,
            &home2,
            addr,
            {
                let home2 = home2.clone();
                let expected = expected.clone();
                move || {
                    let home2 = home2.clone();
                    let expected = expected.clone();
                    async move { follower::rows(&home2, table).await == expected }
                }
            },
        )
        .await;
    }
    let expected_bookmark = horizon + 1;
    follower::wait_until("equal-horizon continuation bookmark", WAIT, &home2, addr, {
        let home2 = home2.clone();
        move || {
            let home2 = home2.clone();
            async move { follower::bookmark(&home2).await == expected_bookmark }
        }
    })
    .await;
    assert_eq!(wire::current_version(&admin).await, expected_bookmark);
    loop2.stop_all().await;
    child::kill(&mut leader).await;
}

/// Poll until the follower's rows match the leader's on every table AND its
/// bookmark reached the leader's version (the bookmark is persisted after the
/// rows become visible, so sampling it right after the rows is racy).
async fn wait_converged(home: &Arc<ShamirDb>, admin: &Client, addr: SocketAddr) {
    for table in wire::TABLES {
        let expected = wire::read_rows(admin, table).await;
        follower::wait_until(&format!("convergence on {table}"), WAIT, home, addr, {
            let home = home.clone();
            move || {
                let home = home.clone();
                let expected = expected.clone();
                async move { follower::rows(&home, table).await == expected }
            }
        })
        .await;
    }
    let version = wire::current_version(admin).await;
    follower::wait_until("bookmark reaches the leader version", WAIT, home, addr, {
        let home = home.clone();
        move || {
            let home = home.clone();
            async move { follower::bookmark(&home).await >= version }
        }
    })
    .await;
}
