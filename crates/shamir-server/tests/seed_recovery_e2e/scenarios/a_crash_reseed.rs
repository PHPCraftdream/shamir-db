//! Scenario A — the main recovery path (design §6 PR-5 acceptance row):
//!
//! real leader child writes rows (inserts, updates, deletes, two tables) →
//! SIGKILL → reopen reconciles the surviving WAL with no gap; a genuinely
//! residual durable gap is then fixture-recorded so the fresh bookmark-0
//! follower pulls and stops with `resync_required`, NO payload applied →
//! leader stopped → real `replication-seed-export` CLI → real
//! `replication-seed-verify` CLI → real `replication-seed-install` CLI into
//! a NEW follower data dir (paused subscription) → follower boots with no
//! loop running, status shows installation complete + both version domains
//! → leader restarted → validated Resume → first pull is `V + 1` →
//! subsequent puts/updates/deletes converge per table → the SOURCE gap
//! certificate observed over Hello is identical before and after the whole
//! procedure.
//!
//! PROCESS-crash only (see `main.rs`): the kill proves crash-recovery
//! certification and offline reseeding, not power-loss durability.

use std::sync::Arc;
use std::time::Duration;

use shamir_server::replication_seed::Manifest;

use crate::harness::child;
use crate::harness::follower;
use crate::harness::ops;
use crate::harness::wire;

const WAIT: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_e2e_a_kill_resync_offline_seed_resume_converges() {
    Box::pin(seed_e2e_a_kill_resync_offline_seed_resume_converges_body()).await;
}

async fn seed_e2e_a_kill_resync_offline_seed_resume_converges_body() {
    let root = tempfile::tempdir().expect("temp root");
    let leader_dir = root.path().join("leader");
    let artifact_dir = root.path().join("seed-artifact");
    let home_dir = root.path().join("follower-home");
    let addr = child::free_addr();
    let upstream = format!("tcp://{addr}");

    // -- Leader: schema + replicator + publication + mixed writes. --
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    wire::setup_leader(&admin).await;
    for i in 0..3u64 {
        wire::put(&admin, "items", &format!("k{i}"), i as i64).await;
    }
    for i in 0..2u64 {
        wire::put(&admin, "audit", &format!("a{i}"), 100 + i as i64).await;
    }
    wire::update_row(&admin, "items", "k1", 42).await;
    wire::delete_row(&admin, "audit", "a0").await;
    drop(admin);

    // -- Unclean kill, reopen: durable wire-created app/main reconciles WAL. --
    child::kill(&mut leader).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    drop(child::connect_admin(&leader).await);
    let cert = wire::gap_certificate(addr).await;
    assert_eq!(
        cert.gap_through, None,
        "surviving WAL must not force reseed"
    );

    // Retain the historical reseed proof with an explicit residual certificate.
    let admin = child::connect_admin(&leader).await;
    let gap_through = wire::current_version(&admin).await;
    drop(admin);
    assert!(
        gap_through >= 1,
        "surviving WAL must include the committed writes",
    );
    child::kill(&mut leader).await;
    child::inject_recovery_gap(&leader_dir, 1, gap_through).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    drop(child::connect_admin(&leader).await);
    let cert = wire::gap_certificate(addr).await;
    assert_eq!(cert.gap_first, Some(1));
    assert_eq!(cert.gap_through, Some(gap_through));

    // -- Fresh bookmark-0 follower: resync_required, no payload applied. --
    let fresh = Arc::new(follower::fresh_in_memory().await);
    ops::create_subscription(&fresh, &upstream, false).await;
    let fresh_loop = follower::supervisor(&fresh);
    fresh_loop.reconcile().await;
    follower::wait_until("fresh follower resync_required", WAIT, &fresh, addr, || {
        let fresh = fresh.clone();
        async move {
            follower::status_of(&fresh, "sub")
                .await
                .is_some_and(|entry| follower::str_field(&entry, "state") == "resync_required")
        }
    })
    .await;
    assert_eq!(follower::bookmark(&fresh).await, 0, "bookmark unchanged");
    for table in wire::TABLES {
        // PR8b: a quarantined destination refuses ordinary reads, so the
        // gap-batch payload can never be observed.
        assert!(
            follower::read_refused_resync(&fresh, table).await,
            "{table} must be refused while resync_required",
        );
    }
    fresh_loop.stop_all().await;

    // -- Stopped-source export through the real CLI. --
    child::kill(&mut leader).await;
    let digest = child::cli_export(&leader.config_path, &artifact_dir).await;
    let home_config = child::write_home_config(&home_dir, addr);
    child::cli_verify(&home_config, &artifact_dir, &digest).await;

    let bytes = std::fs::read(artifact_dir.join("manifest.mpack")).expect("manifest on disk");
    let manifest: Manifest = rmp_serde::from_slice(&bytes).expect("decode manifest");
    assert_eq!(
        manifest.certified_gap,
        cert.gap_first.zip(cert.gap_through),
        "manifest binds exactly the certified interval",
    );
    let v = manifest.version;
    assert!(
        v >= gap_through,
        "seed V {v} must cover horizon {gap_through}"
    );

    // -- Install into a NEW follower data dir (paused subscription). --
    child::cli_install(&home_config, &artifact_dir, &digest, &upstream).await;
    let home = Arc::new(follower::open_home(&home_dir).await);

    // No loop runs before the operator's explicit Resume.
    let seeded_loop = follower::supervisor(&home);
    assert_eq!(
        seeded_loop.active_count(),
        0,
        "no replication loop may run for a paused installation",
    );
    assert!(!seeded_loop.is_running("sub"));

    // Status: installation complete, both version domains visible.
    let entry = follower::status_of(&home, "sub")
        .await
        .expect("seed status");
    assert_eq!(follower::str_field(&entry, "state"), "paused");
    assert_eq!(
        follower::str_field(&entry, "installation_state"),
        "complete"
    );
    assert_eq!(follower::str_field(&entry, "artifact_digest"), digest);
    assert_eq!(follower::u64_field(&entry, "seed_source_version"), Some(v));
    assert_eq!(follower::u64_field(&entry, "upstream_bookmark"), Some(v));
    assert_eq!(
        follower::u64_field(&entry, "next_pull_version"),
        Some(v + 1)
    );
    assert_eq!(
        follower::u64_field(&entry, "local_baseline_version"),
        Some(1)
    );
    assert_eq!(
        follower::u64_field(&entry, "local_consistent_version"),
        Some(1),
        "seeded state sits at local baseline 1",
    );
    for table in wire::TABLES {
        assert!(
            !follower::rows(&home, table).await.is_empty(),
            "{table} seeded with live rows",
        );
    }

    // -- Leader back up; operator resumes; first pull is V+1. --
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    ops::resume(&home).await.expect("validated resume");
    seeded_loop.reconcile().await;
    follower::wait_until(
        "seeded loop running after resume",
        WAIT,
        &home,
        addr,
        || async { seeded_loop.is_running("sub") },
    )
    .await;
    assert_eq!(
        follower::bookmark(&home).await,
        v,
        "empty tail pull must not move the bookmark off V",
    );
    assert_eq!(
        follower::u64_field(
            &follower::status_of(&home, "sub").await.expect("status"),
            "next_pull_version",
        ),
        Some(v + 1),
        "first pull after resume starts at V+1",
    );

    // -- Subsequent leader writes converge row-for-row, both tables. --
    wire::put(&admin, "items", "k10", 7).await;
    wire::put(&admin, "items", "k11", 8).await;
    wire::update_row(&admin, "items", "k0", 99).await;
    wire::delete_row(&admin, "items", "k2").await;
    wire::put(&admin, "audit", "a9", 555).await;
    wire::delete_row(&admin, "audit", "a1").await;
    for table in wire::TABLES {
        let expected = wire::read_rows(&admin, table).await;
        follower::wait_until(&format!("convergence on {table}"), WAIT, &home, addr, {
            let home = home.clone();
            let expected = expected.clone();
            move || {
                let home = home.clone();
                let expected = expected.clone();
                async move { follower::rows(&home, table).await == expected }
            }
        })
        .await;
    }
    // The bookmark is persisted after the rows become visible: wait for it.
    follower::wait_until(
        "bookmark advances past the seed version",
        WAIT,
        &home,
        addr,
        {
            let home = home.clone();
            move || {
                let home = home.clone();
                async move { follower::bookmark(&home).await > v }
            }
        },
    )
    .await;
    let post = follower::bookmark(&home).await;
    assert!(post > v, "bookmark advanced past seed V ({post} > {v})");
    seeded_loop.stop_all().await;

    // -- The SOURCE gap certificate is unchanged by the whole procedure. --
    let cert_after = wire::gap_certificate(addr).await;
    assert_eq!(
        cert, cert_after,
        "the source certificate remains unchanged after reseed and later writes",
    );

    child::kill(&mut leader).await;
}
