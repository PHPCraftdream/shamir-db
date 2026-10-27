#![cfg(unix)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::engine::meta::repo_identity::RepoIdentity;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig, SeedImageWriter, SeedReceipt};
use shamir_db::{SeedRefusal, ShamirDb};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{paused_subscription, repl_scope, replication_profile};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};
use shamir_query_types::{DurabilityOverride, ReplicationServe};
use tokio_util::sync::CancellationToken;

use crate::replication::{
    run_follower_loop, FollowerLoopConfig, ReplError, ReplSource, SubscriptionSupervisor,
};

struct Source {
    receipt: SeedReceipt,
    wrong: SeedRefusal,
    pulls: AtomicU64,
    from: AtomicU64,
    hello_wrong: bool,
    cancel: CancellationToken,
}

#[async_trait]
impl ReplSource for Source {
    async fn hello(&self, _: &str) -> Result<ReplResponse, ReplError> {
        let r = &self.receipt;
        Ok(ReplResponse::Hello {
            leader_epoch: 1,
            repos: vec![ReplRepoInfo {
                db: r.source_db.clone(),
                repo: r.source_repo.clone(),
                current_version: 10,
                journal_floor: 0,
                repo_lineage_id: Some(if self.hello_wrong {
                    [9; 16]
                } else {
                    r.source_identity.lineage_id
                }),
                history_epoch: Some(r.source_identity.history_epoch),
                gap_first: None,
                gap_through: None,
                schema_digest: Some(r.schema_digest.clone()),
                settled_version: Some(10),
                publications: Vec::new(),
            }],
        })
    }
    async fn pull(
        &self,
        _: &str,
        _: &str,
        from: u64,
        _: u32,
        _: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        self.pulls.fetch_add(1, Ordering::SeqCst);
        self.from.store(from, Ordering::SeqCst);
        if self.wrong == SeedRefusal::UpstreamUnavailable {
            self.cancel.cancel();
        }
        Ok(ReplResponse::Pull {
            leader_epoch: 1,
            events: if self.wrong == SeedRefusal::UpstreamUnavailable {
                rmp_serde::to_vec_named(&Vec::<shamir_db::engine::tx::ReplicatedEvent>::new())
                    .unwrap()
            } else {
                vec![0xc1]
            },
            gap_at: None,
            current_version: 11,
            repo_lineage_id: match self.wrong {
                SeedRefusal::MissingIdentity => None,
                SeedRefusal::WrongIdentity => Some([9; 16]),
                _ => Some(self.receipt.source_identity.lineage_id),
            },
            history_epoch: Some(self.receipt.source_identity.history_epoch),
            gap_first: None,
            gap_through: None,
            schema_digest: Some(if self.wrong == SeedRefusal::WrongSchema {
                "c".repeat(64)
            } else {
                self.receipt.schema_digest.clone()
            }),
        })
    }
}

async fn home() -> (Arc<ShamirDb>, SeedReceipt) {
    let home = ShamirDb::init_memory().await.unwrap();
    home.create_db("app").await.expect("create_db");
    home.add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let receipt = SeedReceipt {
        artifact_digest: "a".repeat(64),
        source_db: "app".into(),
        source_repo: "main".into(),
        source_identity: RepoIdentity {
            lineage_id: [1; 16],
            history_epoch: 2,
        },
        schema_digest: "b".repeat(64),
        source_version: 10,
        local_baseline: 1,
    };
    let repo = home.get_db("app").unwrap().get_repo("main").unwrap();
    SeedImageWriter::begin(repo, receipt.clone(), Vec::new())
        .await
        .unwrap()
        .finish(&[])
        .await
        .unwrap();
    (Arc::new(home), receipt)
}

/// Invalid MessagePack makes fence-before-decode observable: without fencing
/// the old loop retries Decode instead of returning the typed refusal.
#[tokio::test]
async fn pull_identity_schema_and_missing_identity_stop_before_decode_and_apply() {
    for wrong in [
        SeedRefusal::WrongIdentity,
        SeedRefusal::WrongSchema,
        SeedRefusal::MissingIdentity,
    ] {
        let (home, receipt) = home().await;
        let source = Arc::new(Source {
            hello_wrong: false,
            cancel: CancellationToken::new(),
            receipt,
            wrong,
            pulls: AtomicU64::new(0),
            from: AtomicU64::new(0),
        });
        let error = run_follower_loop(
            home.clone(),
            source.clone(),
            FollowerLoopConfig::new("node", "app", "main")
                .with_max_iterations(1)
                .with_poll_wait_ms(0),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ReplError::Seed(reason) if reason == wrong));
        assert_eq!(source.from.load(Ordering::SeqCst), 11);
        let repo = home.get_db("app").unwrap().get_repo("main").unwrap();
        assert_eq!(repo.replication_bookmark().await.unwrap(), 10);
        assert_eq!(repo.consistent_version().await.unwrap(), 1);
    }
}

#[tokio::test]
async fn hello_fence_prevents_any_pull_and_matching_peer_first_pull_is_v_plus_one() {
    for hello_wrong in [true, false] {
        let (home, receipt) = home().await;
        let cancel = CancellationToken::new();
        let source = Arc::new(Source {
            receipt,
            wrong: SeedRefusal::UpstreamUnavailable,
            pulls: AtomicU64::new(0),
            from: AtomicU64::new(0),
            hello_wrong,
            cancel: cancel.clone(),
        });
        let result = run_follower_loop(
            home,
            source.clone(),
            FollowerLoopConfig::new("node", "app", "main").with_max_iterations(1),
            cancel,
        )
        .await;
        if hello_wrong {
            assert!(matches!(
                result,
                Err(ReplError::Seed(SeedRefusal::WrongIdentity))
            ));
            assert_eq!(source.pulls.load(Ordering::SeqCst), 0);
        } else {
            assert!(result.is_ok());
            assert_eq!(source.pulls.load(Ordering::SeqCst), 1);
            assert_eq!(source.from.load(Ordering::SeqCst), 11);
        }
    }
}

#[tokio::test]
async fn paused_subscription_never_constructs_source_or_pulls() {
    let (home, receipt) = home().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op("sub", paused_subscription("sub", "peer", "pub", "prof"));
    home.execute(
        "app",
        &batch
            .to_request_via_msgpack()
            .expect("batch request encoding"),
    )
    .await
    .unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let observed = calls.clone();
    let factory = Arc::new(move |_: &crate::replication::Subscription| {
        observed.fetch_add(1, Ordering::SeqCst);
        Arc::new(Source {
            hello_wrong: false,
            cancel: CancellationToken::new(),
            receipt: receipt.clone(),
            wrong: SeedRefusal::WrongIdentity,
            pulls: AtomicU64::new(0),
            from: AtomicU64::new(0),
        }) as Arc<dyn ReplSource>
    });
    let supervisor = SubscriptionSupervisor::new(home, factory, "node");
    supervisor.reconcile().await;
    assert_eq!(supervisor.active_count(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    supervisor.stop_all().await;
}

#[tokio::test]
async fn real_in_process_hello_advertises_schema_settled_position_and_named_publication() {
    let source = ShamirDb::init_memory().await.unwrap();
    source.create_db("app").await.expect("create_db");
    // The repo is in-memory (no persistent WAL), so publication-time
    // enforcement only allows publishing it under an explicit
    // `completed_reseed` durability override.
    source
        .add_repo_as_with_durability(
            "app",
            RepoConfig::new("main", BoxRepoFactory::in_memory()),
            shamir_db::access::Actor::System,
            Some(&DurabilityOverride {
                replication_serve: Some(ReplicationServe::CompletedReseed),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    source
        .add_table("app", "main", "rows", false)
        .await
        .unwrap();
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "pub",
        shamir_query_builder::ddl::publication("pub").scope(repl_scope("app").repo("main").build()),
    );
    source
        .execute(
            "app",
            &batch
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .unwrap();
    let visible = super::super::advertisement::authorized_publications(
        &source,
        &shamir_db::access::Actor::System,
    )
    .await
    .unwrap();
    assert_eq!(visible.len(), 1);
    let mut wider = Batch::new();
    wider.id(2);
    wider.op(
        "wide",
        shamir_query_builder::ddl::publication("wide")
            .scope(repl_scope("app").repo("main").build())
            .scope(repl_scope("app").repo("main").table("rows").build()),
    );
    source
        .execute(
            "app",
            &wider
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .unwrap();
    let visible = super::super::advertisement::authorized_publications(
        &source,
        &shamir_db::access::Actor::System,
    )
    .await
    .unwrap();
    assert!(visible
        .iter()
        .all(|publication| publication.create_publication != "wide"));
    let expected = super::super::fence::live_schema(&source, "app", "main")
        .await
        .unwrap();
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let identity = repo.repo_identity().await.unwrap();
    let peer = crate::replication::in_process::InProcessReplSource::new(source);
    let response = peer.hello("node").await.unwrap();
    let ReplResponse::Hello { repos, .. } = response else {
        panic!("Hello expected");
    };
    let advertisement = repos
        .iter()
        .find(|repo| repo.db == "app" && repo.repo == "main")
        .unwrap();
    assert_eq!(advertisement.repo_lineage_id, Some(identity.lineage_id));
    assert_eq!(
        advertisement.schema_digest.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(
        advertisement.settled_version,
        Some(repo.consistent_version().await.unwrap())
    );
    assert!(advertisement
        .publications
        .iter()
        .any(|publication| publication.create_publication == "pub"
            && publication.scopes[0].repo.as_deref() == Some("main")));
}

#[tokio::test]
async fn supervisor_preserves_typed_terminal_reason_without_advancing_bookmark() {
    let (home, receipt) = home().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op("sub", paused_subscription("sub", "peer", "pub", "prof"));
    home.execute(
        "app",
        &batch
            .to_request_via_msgpack()
            .expect("batch request encoding"),
    )
    .await
    .unwrap();
    home.mark_subscription_terminal("sub", "active", "test admission")
        .await
        .unwrap();
    // Test the terminal persistence seam directly: the same path invoked by
    // the supervised loop, with no scheduling or timer-dependent polling.
    // Durable-sequence generations are monotonic but not small predictable
    // integers, so the row's generation is read from the catalogue and fed
    // to the callback (its generation guard must match).
    let table = home.system_store().subscriptions_table().await.unwrap();
    let refs = shamir_collections::TMap::default();
    let ctx = shamir_db::query::filter::FilterContext::new(
        table.interner_for_test().get().await.unwrap(),
        &refs,
    );
    let generation = {
        let result = table
            .read(
                &shamir_db::query::read::ReadQuery::new("subscriptions"),
                &ctx,
            )
            .await
            .unwrap();
        result.records[0].as_value()["binding_generation"]
            .as_u64()
            .expect("binding_generation must be present")
    };
    super::super::terminal::persist_terminal(
        &home,
        "sub",
        &ReplError::Seed(SeedRefusal::WrongSchema),
        generation,
    )
    .await;
    let result = table
        .read(
            &shamir_db::query::read::ReadQuery::new("subscriptions"),
            &ctx,
        )
        .await
        .unwrap();
    let row = result.records[0].as_value();
    assert_eq!(row["state"].as_str(), Some("seed_wrong_schema"));
    assert_eq!(
        row["last_terminal_reason"].as_str(),
        Some("seed_wrong_schema")
    );
    assert_eq!(
        home.get_db("app")
            .unwrap()
            .get_repo("main")
            .unwrap()
            .replication_bookmark()
            .await
            .unwrap(),
        receipt.source_version
    );
}
