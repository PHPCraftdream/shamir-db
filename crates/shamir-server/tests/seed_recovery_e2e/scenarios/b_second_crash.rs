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
    Box::pin(seed_e2e_b_second_crash_reseeds_and_equal_horizon_continues_body()).await;
}

async fn seed_e2e_b_second_crash_reseeds_and_equal_horizon_continues_body() {
    let root = tempfile::tempdir().expect("temp root");
    let leader_dir = root.path().join("leader");
    let seed1 = root.path().join("seed-1");
    let seed2 = root.path().join("seed-2");
    let home1 = root.path().join("home-1");
    let home2 = root.path().join("home-2");
    let addr = child::free_addr();
    let upstream = format!("tcp://{addr}");

    // -- Leader v1: writes, unclean kill, WAL repair, explicit gap, export. --
    eprintln!("ZZDIAG step 1: line 45");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    eprintln!("ZZDIAG step 2: line 46");
    let admin = child::connect_admin(&leader).await;
    eprintln!("ZZDIAG step 3: line 47");
    wire::setup_leader(&admin).await;
    eprintln!("ZZDIAG step 4: line 48");
    wire::put(&admin, "items", "k0", 0).await;
    eprintln!("ZZDIAG step 5: line 49");
    wire::put(&admin, "audit", "a0", 1).await;
    drop(admin);
    eprintln!("ZZDIAG step 6: line 51");
    child::kill(&mut leader).await;
    eprintln!("ZZDIAG step 7: line 52");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    eprintln!("ZZDIAG step 8: line 53");
    let gap_cert = wire::gap_certificate(addr).await;
    eprintln!("ZZDIAG step 9: line 54");
    let admin = child::connect_admin(&leader).await;
    eprintln!("ZZDIAG step 10: line 55");
    let horizon = wire::current_version(&admin).await;
    drop(admin);
    assert_eq!(horizon, 2);
    assert_eq!(gap_cert.gap_first, None);
    assert_eq!(gap_cert.gap_through, None);
    eprintln!("ZZDIAG step 11: line 60");
    child::kill(&mut leader).await;
    eprintln!("ZZDIAG step 12: line 61");
    child::inject_recovery_gap(&leader_dir, 1, horizon).await;
    eprintln!("ZZDIAG step 13: line 62");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    eprintln!("ZZDIAG step 14: line 63");
    let cert_before_second_crash = wire::gap_certificate(addr).await;
    assert_eq!(cert_before_second_crash.gap_first, Some(1));
    assert_eq!(cert_before_second_crash.gap_through, Some(horizon));
    drop(child::connect_admin(&leader).await);
    eprintln!("ZZDIAG step 15: line 67");
    child::kill(&mut leader).await;
    eprintln!("ZZDIAG step 16: line 68");
    let digest1 = child::cli_export(&leader.config_path, &seed1).await;

    // -- Seed #1 into home-1 and converge. --
    let home1_cfg = child::write_home_config(&home1, addr);
    eprintln!("ZZDIAG step 17: line 72");
    child::cli_install(&home1_cfg, &seed1, &digest1, &upstream).await;
    eprintln!("ZZDIAG step 18: line 73");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    eprintln!("ZZDIAG step 19: line 74");
    let admin = child::connect_admin(&leader).await;
    eprintln!("ZZDIAG step 20: line 75");
    let home1 = Arc::new(follower::open_home(&home1).await);
    let loop1 = follower::supervisor(&home1);
    eprintln!("ZZDIAG step 21: line 77");
    ops::resume(&home1).await.expect("resume home-1");
    eprintln!("ZZDIAG step 22: line 78");
    loop1.reconcile().await;
    wait_converged(&home1, &admin, addr).await;
    eprintln!("ZZDIAG step 23: line 80");
    loop1.stop_all().await;
    eprintln!("ZZDIAG step 24: line 81");
    let bookmark1 = follower::bookmark(&home1).await;
    assert_eq!(bookmark1, 2, "home-1 converged through leader version 2");
    assert_eq!(bookmark1, wire::current_version(&admin).await);
    eprintln!("ZZDIAG step 25: line 84");
    let before = ops::data_fingerprint(&home1).await;

    // -- Leader writes MORE; simulate lost recovery evidence. --
    eprintln!("ZZDIAG step 26: line 87");
    wire::put(&admin, "items", "k1", 10).await;
    eprintln!("ZZDIAG step 27: line 88");
    wire::update_row(&admin, "audit", "a0", 77).await;
    eprintln!("ZZDIAG step 28: line 89");
    let horizon = wire::current_version(&admin).await;
    assert_eq!(
        horizon,
        bookmark1 + 2,
        "exactly two commits follow convergence"
    );
    drop(admin);
    eprintln!("ZZDIAG step 29: line 96");
    child::kill(&mut leader).await;
    // This certificate fixture models unavailable recovery evidence, not
    // physical disk corruption; inject it after kill and before first restart.
    eprintln!("ZZDIAG step 30: line 99");
    child::inject_recovery_gap(&leader_dir, bookmark1 + 1, horizon).await;
    eprintln!("ZZDIAG step 31: line 100");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    drop(child::connect_admin(&leader).await);
    eprintln!("ZZDIAG step 32: line 102");
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
    eprintln!("ZZDIAG step 33: line 115");
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
    // PR8b: a quarantined destination refuses ordinary reads, so the frozen
    // payload can no longer be re-observed row-wise (same semantics scenario
    // A asserts); the no-mutation guarantee is carried by the unchanged
    // bookmark plus the typed refusal on every table read.
    for table in wire::TABLES {
        assert!(
            follower::read_refused_resync(&home1, table).await,
            "{table} must be refused while resync_required",
        );
    }
    assert_eq!(follower::bookmark(&home1).await, bookmark1);
    eprintln!("ZZDIAG step 34: line 142");
    loop1.stop_all().await;

    // -- Second offline seed into ANOTHER new repository. --
    eprintln!("ZZDIAG step 35: line 145");
    child::kill(&mut leader).await;
    eprintln!("ZZDIAG step 36: line 146");
    let digest2 = child::cli_export(&leader.config_path, &seed2).await;
    let bytes = std::fs::read(seed2.join("manifest.mpack")).expect("manifest");
    let manifest2: Manifest = rmp_serde::from_slice(&bytes).expect("decode manifest");
    assert_eq!(
        manifest2.version, horizon,
        "seed captures the exact second-crash horizon"
    );
    assert_eq!(manifest2.certified_gap, Some((1, horizon)));
    let home2_cfg = child::write_home_config(&home2, addr);
    eprintln!("ZZDIAG step 37: line 155");
    child::cli_install(&home2_cfg, &seed2, &digest2, &upstream).await;
    eprintln!("ZZDIAG step 38: line 156");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    eprintln!("ZZDIAG step 39: line 157");
    let admin = child::connect_admin(&leader).await;
    eprintln!("ZZDIAG step 40: line 158");
    let home2 = Arc::new(follower::open_home(&home2).await);
    let loop2 = follower::supervisor(&home2);
    eprintln!("ZZDIAG step 41: line 160");
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
    eprintln!("ZZDIAG step 42: line 176");
    ops::resume(&home2).await.expect("resume home-2");
    eprintln!("ZZDIAG step 43: line 177");
    loop2.reconcile().await;
    wait_converged(&home2, &admin, addr).await;
    assert_eq!(
        follower::bookmark(&home2).await,
        wire::current_version(&admin).await,
        "fully caught up before the equal-horizon crash",
    );

    // -- Opposite branch: crash with NO new writes (horizon == bookmark). --
    drop(admin);
    eprintln!("ZZDIAG step 44: line 187");
    child::kill(&mut leader).await;
    eprintln!("ZZDIAG step 45: line 188");
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    eprintln!("ZZDIAG step 46: line 189");
    let admin = child::connect_admin(&leader).await;
    eprintln!("ZZDIAG step 47: line 190");
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
    eprintln!("ZZDIAG step 48: line 200");
    let before = ops::data_fingerprint(&home2).await;
    eprintln!("ZZDIAG step 49: line 201");
    loop2.stop_all().await;
    let loop2 = follower::supervisor(&home2);
    eprintln!("ZZDIAG step 50: line 203");
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

    eprintln!("ZZDIAG step 51: line 252");
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
    eprintln!("ZZDIAG step 52: line 282");
    loop2.stop_all().await;
    eprintln!("ZZDIAG step 53: line 283");
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
    eprintln!("ZZDIAG step 54: line 302");
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
