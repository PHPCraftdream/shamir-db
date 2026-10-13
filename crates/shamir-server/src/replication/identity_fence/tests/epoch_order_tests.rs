//! Identity refusal wins over a simultaneous stale leader-epoch refusal.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::types::value::QueryValue;
use shamir_db::SeedRefusal;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    repl_scope, replication_profile, replication_status, subscription,
};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;

use super::identity_fence_tests::{build_memory, repo, write_rows};
use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::identity_fence::{checks::hex_lineage, recorded};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

struct EpochSource {
    hello: ReplResponse,
    pull: ReplResponse,
    pulls: AtomicUsize,
}

#[async_trait]
impl ReplSource for EpochSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Ok(self.hello.clone())
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        self.pulls.fetch_add(1, Ordering::AcqRel);
        Ok(self.pull.clone())
    }
}

// A synchronous current-thread runtime keeps the local recorder installed
// during every future poll; constructing an async future inside its closure
// alone would NOT install the recorder while the future actually executes.
#[test]
fn changed_identity_and_stale_epoch_records_refusal_and_counter_first() {
    epoch_order(Some(SeedRefusal::WrongIdentity));
}

#[test]
fn changed_history_and_stale_epoch_records_refusal_and_counter_first() {
    epoch_order(Some(SeedRefusal::WrongHistoryEpoch));
}

#[test]
fn unchanged_identity_and_stale_epoch_preserves_stale_leader_error() {
    epoch_order(None);
}

fn epoch_order(reason: Option<SeedRefusal>) {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    metrics::with_local_recorder(&recorder, || {
        runtime.block_on(async {
            let leader = build_memory().await;
            write_rows(&leader, 0, 2).await;
            let follower = build_memory().await;
            let expected = repo(&leader).repo_identity().await.unwrap();
            let mut observed = expected;
            match reason {
                Some(SeedRefusal::WrongIdentity) => observed.lineage_id[0] ^= 1,
                Some(SeedRefusal::WrongHistoryEpoch) => observed.history_epoch += 1,
                None => {}
                other => panic!("unsupported test refusal: {other:?}"),
            }
            let inner = InProcessReplSource::new(leader);
            let mut hello = inner.hello("probe").await.unwrap();
            let mut pull = inner.pull("app", "main", 1, 1000, Some(0)).await.unwrap();
            if let ReplResponse::Hello { leader_epoch, .. } = &mut hello {
                *leader_epoch = 9;
            }
            if let ReplResponse::Pull {
                leader_epoch,
                repo_lineage_id,
                history_epoch,
                ..
            } = &mut pull
            {
                *leader_epoch = 8;
                *repo_lineage_id = Some(observed.lineage_id);
                *history_epoch = Some(observed.history_epoch);
            }
            let source = Arc::new(EpochSource {
                hello,
                pull,
                pulls: AtomicUsize::new(0),
            });
            let before = repo(&follower).current_commit_version().await.unwrap();
            let error = run_follower_loop(
                Arc::new(follower.clone()),
                source.clone(),
                FollowerLoopConfig::new("probe", "app", "main")
                    .with_poll_wait_ms(0)
                    .with_max_iterations(1),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
            if let Some(reason) = reason {
                let ReplError::IdentityMismatch {
                    code,
                    recorded,
                    recorded_epoch,
                    observed: seen,
                    observed_epoch,
                } = &error
                else {
                    panic!("identity must win over stale epoch: {error:?}");
                };
                assert_eq!(*code, reason);
                assert_eq!(recorded, &hex_lineage(&expected.lineage_id));
                assert_eq!(*recorded_epoch, expected.history_epoch);
                assert_eq!(seen, &hex_lineage(&observed.lineage_id));
                assert_eq!(*observed_epoch, observed.history_epoch);
            } else {
                assert!(matches!(
                    error,
                    ReplError::StaleLeaderEpoch {
                        observed: 8,
                        max_seen: 9
                    }
                ));
            }
            let info = repo(&follower).tx_info_store().await.unwrap();
            let record = recorded::load(&info).await.unwrap().unwrap();
            assert_eq!(record.lineage_id, expected.lineage_id);
            assert_eq!(record.history_epoch, expected.history_epoch);
            assert_eq!(record.observed_identity, Some(observed));
            assert_eq!(record.bookmark, 0);
            assert_eq!(source.pulls.load(Ordering::Acquire), 1);
            assert_eq!(
                repo(&follower).current_commit_version().await.unwrap(),
                before
            );
            assert!(repo(&follower)
                .read_changelog_bounded(0, 1000)
                .await
                .unwrap()
                .read
                .events
                .is_empty());
        });
    });
    let rendered = handle.render();
    if let Some(reason) = reason {
        assert!(
            rendered.lines().any(|line| {
                line.starts_with("shamir_repl_identity_mismatch_total{")
                    && line.contains(&format!("reason=\"{}\"", reason.code()))
                    && line.ends_with(" 1")
            }),
            "missing exactly one mismatch: {rendered}"
        );
    } else {
        assert!(
            !rendered.contains("shamir_repl_identity_mismatch_total"),
            "{rendered}"
        );
    }
}

#[tokio::test]
async fn supervisor_persists_identity_reason_despite_stale_leader_epoch() {
    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let expected = repo(&leader).repo_identity().await.unwrap();
    let inner = InProcessReplSource::new(leader);
    let mut hello = inner.hello("probe").await.unwrap();
    let mut pull = inner.pull("app", "main", 1, 1000, Some(0)).await.unwrap();
    let mut observed = expected;
    observed.lineage_id[0] ^= 1;
    if let ReplResponse::Hello { leader_epoch, .. } = &mut hello {
        *leader_epoch = 9;
    }
    if let ReplResponse::Pull {
        leader_epoch,
        repo_lineage_id,
        ..
    } = &mut pull
    {
        *leader_epoch = 8;
        *repo_lineage_id = Some(observed.lineage_id);
    }
    let source = Arc::new(EpochSource {
        hello,
        pull,
        pulls: AtomicUsize::new(0),
    });
    let follower = Arc::new(build_memory().await);
    let mut batch = Batch::new();
    batch.op(
        "profile",
        replication_profile("prof_order").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        subscription("sub_order", "tcp://leader:9000", "pub", "prof_order"),
    );
    let created = follower.execute("app", &batch.build()).await.unwrap();
    assert!(!created.results.contains_key("__error"), "{created:?}");
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let supervisor = SubscriptionSupervisor::new(follower.clone(), factory, "probe")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);
    supervisor.reconcile().await;
    supervisor.wait_for_subscription("sub_order").await;
    let mut batch = Batch::new();
    batch.op("status", replication_status());
    let status = follower.execute("app", &batch.build()).await.unwrap();
    let value = status.results["status"].records[0].as_value();
    let QueryValue::List(entries) = &value["subscriptions"] else {
        panic!("expected subscription list: {value:?}");
    };
    assert_eq!(entries.len(), 1);
    let row = &entries[0];
    assert_eq!(
        row["state"].as_str(),
        Some(SeedRefusal::WrongIdentity.code())
    );
    assert_ne!(
        row["recorded_identity"]["lineage_id"],
        row["observed_identity"]["lineage_id"]
    );
    let message = row["status_message"].as_str().unwrap();
    assert!(
        message.contains(&hex_lineage(&expected.lineage_id)),
        "{message}"
    );
    assert!(
        message.contains(&hex_lineage(&observed.lineage_id)),
        "{message}"
    );
    let info = repo(&follower).tx_info_store().await.unwrap();
    let record = recorded::load(&info).await.unwrap().unwrap();
    assert_eq!(record.lineage_id, expected.lineage_id);
    assert_eq!(record.observed_identity, Some(observed));
    assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
    supervisor.reconcile().await;
    assert_eq!(supervisor.active_count(), 0);
    assert_eq!(source.pulls.load(Ordering::Acquire), 1);
    supervisor.stop_all().await;
}
