//! Real-wire configured-pin mismatch and durable terminal state.
use std::sync::Arc;
use std::time::Duration;

use shamir_server::config::UpstreamPins;
use shamir_server::replication::{
    build_prod_factory, ReplError, ReplicatorPassword, Subscription, SubscriptionSupervisor,
};

use super::super::harness::{child, follower, ops, wire};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_server_e8e1d11b_mismatch_stops_and_persists() {
    Box::pin(mismatch_body()).await;
}

async fn mismatch_body() {
    let root = tempfile::tempdir().unwrap();
    let addr = child::free_addr();
    let mut leader = child::spawn_leader(&root.path().join("leader"), addr).await;
    let admin = child::connect_admin(&leader).await;
    wire::setup_leader(&admin).await;
    let mut wrong = admin.server_pub_key_pin();
    wrong[0] ^= 1;
    let config = format!("{{\"{addr}\":[\"{}\"]}}", hex::encode(wrong));
    let pins: UpstreamPins = serde_json::from_str(&config).unwrap();
    let factory = build_prod_factory(
        Some(child::REPLICATOR.to_string()),
        Some(ReplicatorPassword::from(child::REPLICATOR_PW.to_string())),
        "localhost".to_string(),
        pins,
    )
    .unwrap();
    let upstream = format!("tcp://{addr}");
    let source = factory(&Subscription {
        name: "sub".into(),
        upstream: upstream.clone(),
        publication: wire::PUBLICATION.into(),
        profile: "seedprof".into(),
        state: "active".into(),
        binding_generation: 0,
        binding_destinations: vec![],
    });
    assert!(matches!(
        source.hello("pin-check").await,
        Err(ReplError::UpstreamKeyMismatch { .. })
    ));
    let home = Arc::new(follower::fresh_in_memory().await);
    ops::create_subscription(&home, &upstream, false).await;
    let supervisor = SubscriptionSupervisor::new(home.clone(), factory, "pin-test");
    supervisor.reconcile().await;
    follower::wait_until(
        "upstream key mismatch",
        Duration::from_secs(30),
        &home,
        addr,
        || {
            let home = home.clone();
            async move {
                follower::status_of(&home, "sub")
                    .await
                    .is_some_and(|entry| {
                        follower::str_field(&entry, "state") == "upstream_key_mismatch"
                    })
            }
        },
    )
    .await;
    supervisor.reconcile().await;
    assert!(!supervisor.is_running("sub"));
    assert_eq!(follower::bookmark(&home).await, 0);
    supervisor.stop_all().await;
    drop(admin);
    child::kill(&mut leader).await;
}
