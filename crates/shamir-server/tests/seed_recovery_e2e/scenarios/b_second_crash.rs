//! Scenario B — a SECOND crash after convergence, and the at-or-below
//! bookmark branch:
//!
//! 1. converge a seeded follower (offline seed → Resume → catch up);
//! 2. leader writes more, is SIGKILLed, reopen certifies a NEW horizon
//!    above the follower bookmark → the follower stops with
//!    `resync_required` again (no payload applied);
//! 3. a second offline seed into ANOTHER new repository converges;
//! 4. the opposite branch: a crash with NO new writes (new horizon ==
//!    follower bookmark) does NOT stop the follower — it keeps pulling
//!    from bookmark + 1 (deterministic here because convergence means
//!    bookmark == current version, so the certified horizon equals the
//!    bookmark; the `HorizonAboveBookmark` refusal then cannot fire).

use std::sync::Arc;
use std::time::Duration;

use shamir_client::Client;
use shamir_db::ShamirDb;
use shamir_server::replication_seed::Manifest;

use crate::harness::child;
use crate::harness::follower;
use crate::harness::ops;
use crate::harness::wire;

const WAIT: Duration = Duration::from_secs(90);

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

    // -- Leader v1: writes, unclean kill, export. --
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    wire::setup_leader(&admin).await;
    wire::put(&admin, "items", "k0", 0).await;
    wire::put(&admin, "audit", "a0", 1).await;
    drop(admin);
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
    wait_converged(&home1, &admin).await;
    let bookmark1 = follower::bookmark(&home1).await;

    // -- Leader writes MORE, then is killed: horizon rises above bookmark. --
    wire::put(&admin, "items", "k1", 10).await;
    wire::update_row(&admin, "audit", "a0", 77).await;
    child::kill(&mut leader).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    drop(child::connect_admin(&leader).await);
    let cert = wire::gap_certificate(addr).await;
    let horizon = cert.gap_through.expect("reopen certifies a new horizon");
    assert!(
        horizon > bookmark1,
        "new horizon {horizon} must sit ABOVE the follower bookmark {bookmark1}",
    );
    let before = ops::data_fingerprint(&home1).await;
    // The old loop's wire client is bound to the dead leader incarnation
    // (lazy connect caches the session); a follower would re-enter through
    // its supervisor after the leader returns. Recreate the supervisor —
    // its first pull starts at bookmark+1 and hits the new certified gap.
    loop1.stop_all().await;
    let loop1 = follower::supervisor(&home1);
    loop1.reconcile().await;
    follower::wait_until("home-1 resync_required after second crash", WAIT, || {
        let home1 = home1.clone();
        async move {
            follower::status_of(&home1, "sub")
                .await
                .is_some_and(|entry| follower::str_field(&entry, "state") == "resync_required")
        }
    })
    .await;
    assert_eq!(
        ops::data_fingerprint(&home1).await,
        before,
        "no payload may apply past a gap above the bookmark",
    );
    loop1.stop_all().await;

    // -- Second offline seed into ANOTHER new repository. --
    child::kill(&mut leader).await;
    let digest2 = child::cli_export(&leader.config_path, &seed2).await;
    let bytes = std::fs::read(seed2.join("manifest.mpack")).expect("manifest");
    let manifest2: Manifest = rmp_serde::from_slice(&bytes).expect("decode manifest");
    assert!(
        manifest2.version >= horizon,
        "second seed must cover the second crash horizon",
    );
    let home2_cfg = child::write_home_config(&home2, addr);
    child::cli_install(&home2_cfg, &seed2, &digest2, &upstream).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    let home2 = Arc::new(follower::open_home(&home2).await);
    let loop2 = follower::supervisor(&home2);
    let entry = follower::status_of(&home2, "sub").await.expect("status");
    assert_eq!(follower::str_field(&entry, "state"), "paused");
    assert_eq!(
        follower::u64_field(&entry, "seed_source_version"),
        Some(manifest2.version),
        "second seed carries the second-crash version",
    );
    ops::resume(&home2).await.expect("resume home-2");
    loop2.reconcile().await;
    wait_converged(&home2, &admin).await;
    assert_eq!(
        follower::bookmark(&home2).await,
        wire::current_version(&admin).await,
        "fully caught up before the equal-horizon crash",
    );

    // -- Opposite branch: crash with NO new writes (horizon == bookmark). --
    child::kill(&mut leader).await;
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    drop(child::connect_admin(&leader).await);
    let cert = wire::gap_certificate(addr).await;
    assert_eq!(
        cert.gap_through,
        Some(follower::bookmark(&home2).await),
        "no writes since convergence: the certified horizon equals the bookmark",
    );
    let before = ops::data_fingerprint(&home2).await;
    loop2.stop_all().await;
    let loop2 = follower::supervisor(&home2);
    loop2.reconcile().await;
    follower::wait_until("home-2 loop running at equal horizon", WAIT, || async {
        loop2.is_running("sub")
    })
    .await;
    // The loop pulls bookmark+1 (above the horizon), applies nothing, and
    // must NOT be stopped as resync_required.
    assert_eq!(ops::data_fingerprint(&home2).await, before);
    assert_ne!(
        follower::status_of(&home2, "sub")
            .await
            .map(|entry| follower::str_field(&entry, "state").to_string()),
        Some("resync_required".to_string()),
        "an at-or-below-bookmark horizon must not stop the follower",
    );
    loop2.stop_all().await;
    child::kill(&mut leader).await;
}

/// Poll until the follower's rows match the leader's on every table AND its
/// bookmark reached the leader's version (the bookmark is persisted after the
/// rows become visible, so sampling it right after the rows is racy).
async fn wait_converged(home: &Arc<ShamirDb>, admin: &Client) {
    for table in wire::TABLES {
        let expected = wire::read_rows(admin, table).await;
        follower::wait_until(&format!("convergence on {table}"), WAIT, {
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
    follower::wait_until("bookmark reaches the leader version", WAIT, {
        let home = home.clone();
        move || {
            let home = home.clone();
            async move { follower::bookmark(&home).await >= version }
        }
    })
    .await;
}
