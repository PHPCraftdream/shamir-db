//! Scenario D — the follower home is reopened (process restart of the
//! seeded follower) at EVERY durable boundary:
//!   (i)   right after installation — pause + receipt + seeded rows survive;
//!   (ii)  right after the validated Resume — activation state persists;
//!   (iii) right after the first applied batch — bookmark + applied data
//!         survive, and the follower converges on the next batch.
//!
//! The reopen is a real cold open of the on-disk home (`init_seed`), i.e.
//! the same boot gate production uses. As everywhere in this target: this
//! is process-level restart, not power-loss simulation.

use std::sync::Arc;
use std::time::Duration;

use shamir_server::replication_seed::Manifest;
use shamir_types::types::value::QueryValue;

use crate::harness::child;
use crate::harness::follower;
use crate::harness::ops;
use crate::harness::wire;

const WAIT: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_e2e_d_reopen_at_install_resume_and_first_batch_boundaries() {
    let root = tempfile::tempdir().expect("temp root");
    let leader_dir = root.path().join("leader");
    let home_dir = root.path().join("home");
    let artifact = root.path().join("seed");
    let addr = child::free_addr();
    let upstream = format!("tcp://{addr}");

    // -- Leader writes, unclean kill, export, install (boundary 1). --
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    wire::setup_leader(&admin).await;
    wire::put(&admin, "items", "k0", 1).await;
    wire::put(&admin, "audit", "a0", 2).await;
    drop(admin);
    child::kill(&mut leader).await;
    let digest = child::cli_export(&leader.config_path, &artifact).await;
    let bytes = std::fs::read(artifact.join("manifest.mpack")).expect("manifest");
    let manifest: Manifest = rmp_serde::from_slice(&bytes).expect("decode manifest");
    let v = manifest.version;

    let cfg = child::write_home_config(&home_dir, addr);
    child::cli_install(&cfg, &artifact, &digest, &upstream).await;

    // (i) Reopen right after install.
    let home = Arc::new(follower::open_home(&home_dir).await);
    let entry = follower::status_of(&home, "sub").await.expect("status");
    assert_eq!(follower::str_field(&entry, "state"), "paused");
    assert_eq!(
        follower::str_field(&entry, "installation_state"),
        "complete"
    );
    assert_eq!(follower::str_field(&entry, "artifact_digest"), digest);
    assert_eq!(follower::bookmark(&home).await, v);
    drop(home);
    let home = Arc::new(follower::open_home(&home_dir).await);
    let entry = follower::status_of(&home, "sub").await.expect("status");
    assert_eq!(follower::str_field(&entry, "state"), "paused");
    assert_eq!(follower::bookmark(&home).await, v);

    // -- Leader back up; (ii) Resume, then reopen. --
    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    let supervisor = follower::supervisor(&home);
    ops::resume(&home).await.expect("validated resume");
    assert_eq!(
        follower::str_field(
            &follower::status_of(&home, "sub").await.expect("status"),
            "state",
        ),
        "active",
    );
    supervisor.stop_all().await;
    drop(supervisor);
    drop(home);
    let home = Arc::new(follower::open_home(&home_dir).await);
    assert_eq!(
        follower::str_field(
            &follower::status_of(&home, "sub").await.expect("status"),
            "state",
        ),
        "active",
        "activation must survive a restart",
    );
    assert_eq!(follower::bookmark(&home).await, v);

    // -- (iii) first applied batch, then reopen. --
    let supervisor = follower::supervisor(&home);
    supervisor.reconcile().await;
    wire::put(&admin, "items", "k1", 10).await;
    follower::wait_until("first applied batch lands", WAIT, &home, addr, {
        let home = home.clone();
        move || {
            let home = home.clone();
            async move {
                follower::rows(&home, "items")
                    .await
                    .iter()
                    .any(|row| row.get("id").and_then(QueryValue::as_str) == Some("k1"))
            }
        }
    })
    .await;
    // The follower persists the bookmark only after the applied rows are
    // durable, so the rows can be visible a moment before the bookmark moves:
    // wait for the bookmark itself instead of sampling it right after the rows.
    follower::wait_until("first batch advances the bookmark", WAIT, &home, addr, {
        let home = home.clone();
        move || {
            let home = home.clone();
            async move { follower::bookmark(&home).await > v }
        }
    })
    .await;
    supervisor.stop_all().await;
    let bookmark = follower::bookmark(&home).await;
    assert!(bookmark > v, "first batch advanced the bookmark");
    drop(supervisor);
    drop(home);
    let home = Arc::new(follower::open_home(&home_dir).await);
    assert_eq!(
        follower::bookmark(&home).await,
        bookmark,
        "bookmark survives restart after the first applied batch",
    );
    assert!(
        follower::rows(&home, "items")
            .await
            .iter()
            .any(|row| row.get("id").and_then(QueryValue::as_str) == Some("k1")),
        "applied data survives restart",
    );

    // -- Convergence continues on the next batch. --
    let supervisor = follower::supervisor(&home);
    supervisor.reconcile().await;
    wire::put(&admin, "audit", "a1", 20).await;
    follower::wait_until("second batch converges", WAIT, &home, addr, {
        let home = home.clone();
        move || {
            let home = home.clone();
            async move {
                follower::rows(&home, "audit")
                    .await
                    .iter()
                    .any(|row| row.get("id").and_then(QueryValue::as_str) == Some("a1"))
            }
        }
    })
    .await;
    supervisor.stop_all().await;
    child::kill(&mut leader).await;
}
