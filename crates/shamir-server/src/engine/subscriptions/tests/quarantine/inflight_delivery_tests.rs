use std::sync::Arc;

use super::super::super::bridge::bridge_task;
use super::super::super::registry::SubscriptionRegistry;
use super::pausing_sink::PausingSink;
use super::quarantine_termination_tests::{bridge_database, quarantine, write_user};
use crate::subscriptions::tests::bridge::bridge_snapshot_cut_tests::{Hook, HOOK};
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::{PushRejected, PushSink};
use shamir_db::access::Actor;
use shamir_query_builder::batch::subscribe::SourceBuilder;
use shamir_query_types::batch::BatchLimits;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use tokio::sync::mpsc;

struct RecordingSink(mpsc::Sender<PushEnvelope>);

impl PushSink for RecordingSink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        self.0
            .try_send(rmp_serde::from_slice(&frame).unwrap())
            .map_err(|_| PushRejected)
    }
}

#[tokio::test]
async fn event_page_read_before_flip_is_discarded_after_resume() {
    let db = bridge_database().await;
    write_user(&db, "seed").await;
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(7);
    let (frames, mut observed) = mpsc::channel(32);
    let hook = Arc::new(Hook::new(3));
    let page_entered = hook.entered.notified();
    tokio::pin!(page_entered);
    page_entered.as_mut().enable();
    let db_task = db.clone();
    let hook_task = Arc::clone(&hook);
    let registry_task = Arc::clone(&registry);
    let bridge = tokio::spawn(async move {
        HOOK.scope(
            hook_task,
            bridge_task(
                7,
                db_task,
                "app".into(),
                vec![SourceBuilder::table("users").build()],
                DeliverMode::Records,
                Actor::System,
                Arc::new(RecordingSink(frames)),
                None,
                true,
                BatchLimits::default(),
                registry_task,
            ),
        )
        .await;
    });
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Event);
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Ready);
    write_user(&db, "paused-page").await;
    page_entered.await;
    quarantine(&db).await;
    hook.release.notify_one();
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Closed);
    assert!(observed.try_recv().is_err());
    bridge.await.unwrap();
    assert_eq!(registry.count(), 0);
}

#[tokio::test]
async fn bridge_is_revoked_while_event_retries_backpressure() {
    let db = bridge_database().await;
    write_user(&db, "seed").await;
    let (frames, mut observed) = mpsc::channel(32);
    let sink = PausingSink::new(frames);
    let entered = sink.entered.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(8);
    let bridge_registry = Arc::clone(&registry);
    let bridge = tokio::spawn(bridge_task(
        8,
        db.clone(),
        "app".into(),
        vec![SourceBuilder::table("users").build()],
        DeliverMode::Records,
        Actor::System,
        sink.clone(),
        None,
        true,
        BatchLimits::default(),
        bridge_registry,
    ));
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Event);
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Ready);
    write_user(&db, "blocked-event").await;
    entered.await;
    quarantine(&db).await;
    sink.allow_after_revoke
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Closed);
    assert!(observed.try_recv().is_err());
    bridge.await.unwrap();
    assert_eq!(registry.count(), 0);
}

#[tokio::test]
async fn materialized_snapshot_read_cannot_deliver_after_flip() {
    let db = bridge_database().await;
    write_user(&db, "snapshot-seed").await;
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(9);
    let (frames, mut observed) = mpsc::channel(16);
    let hook = Arc::new(Hook::new(4));
    let snapshot_entered = hook.entered.notified();
    tokio::pin!(snapshot_entered);
    snapshot_entered.as_mut().enable();
    let hook_task = Arc::clone(&hook);
    let db_task = db.clone();
    let registry_task = Arc::clone(&registry);
    let bridge = tokio::spawn(async move {
        HOOK.scope(
            hook_task,
            bridge_task(
                9,
                db_task,
                "app".into(),
                vec![SourceBuilder::table("users").build()],
                DeliverMode::Records,
                Actor::System,
                Arc::new(RecordingSink(frames)),
                None,
                true,
                BatchLimits::default(),
                registry_task,
            ),
        )
        .await;
    });
    snapshot_entered.await;
    quarantine(&db).await;
    hook.release.notify_one();
    assert_eq!(observed.recv().await.unwrap().push, PushKind::Closed);
    assert!(observed.try_recv().is_err());
    bridge.await.unwrap();
    assert_eq!(registry.count(), 0);
}
