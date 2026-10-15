use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::ReplicatedEvent;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::create_index;
use shamir_query_builder::query::Query;
use shamir_query_builder::response::BatchResponseExt;
use shamir_query_builder::write::{doc, insert};
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

// The journal API has no acknowledged drain. Capture the real synchronous
// emission instead, retaining the wire decode and follower-loop apply path.
struct EmittedSource {
    leader: InProcessReplSource,
    event: ReplicatedEvent,
}

#[async_trait]
impl ReplSource for EmittedSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.leader.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        assert_eq!((db, repo), ("app", "main"));
        assert!(from_version <= self.event.event.commit_version);
        assert!(limit > 0);
        assert_eq!(wait_ms, Some(0));
        let ReplResponse::Hello { repos, .. } = self.leader.hello("metadata").await? else {
            return Err(ReplError::Transport("expected Hello".into()));
        };
        let identity = &repos[0];
        Ok(ReplResponse::Pull {
            schema_digest: None,
            repo_lineage_id: identity.repo_lineage_id,
            history_epoch: identity.history_epoch,
            gap_first: None,
            gap_through: None,
            leader_epoch: self.leader.epoch(),
            events: rmp_serde::to_vec_named(std::slice::from_ref(&self.event)).unwrap(),
            gap_at: None,
            current_version: self.event.event.commit_version,
        })
    }
}

fn owner() -> Actor {
    Actor::User(principal64_from_username("alice"))
}

async fn database() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db_as("app", owner()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("items"))
            .add_table(TableConfig::new("local").with_indexes()),
        owner(),
    )
    .await
    .unwrap();
    db
}

/// Without by-name translation, low leader ids decode as the local DDL names.
/// The index must be on another table: indexed replication targets are rejected.
#[tokio::test]
async fn separate_table_index_occupies_ids_but_named_select_converges() {
    let leader = database().await;
    let follower = database().await;
    let mut ddl = Batch::new();
    ddl.create_index(
        "index",
        create_index("zzz", "local").repo("main").field("zzz"),
    );
    let response = follower
        .execute_as(owner(), "app", &ddl.build())
        .await
        .unwrap();
    assert!(!response.results.contains_key("__error"), "{response:?}");

    let follower_repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    let manager = follower_repo.repo_interner().await.unwrap();
    let interner = manager.get().await.unwrap();
    let occupied = interner.get_ind("zzz").expect("DDL interned zzz").id();
    assert!(interner.get_ind("label").is_none());
    assert!(interner.get_ind("v").is_none());

    let mut emitted = leader
        .subscribe_changelog("app", "main")
        .await
        .unwrap()
        .unwrap();
    let expected = doc().set("label", "leader").set("v", 20_i64).build();
    let mut write = Batch::new();
    write.transactional();
    write.insert("write", insert("items").rows([expected.clone()]));
    let response = leader
        .execute_as(owner(), "app", &write.build())
        .await
        .unwrap();
    assert_eq!(response.transaction().unwrap().status, "committed");
    let leader_repo = leader.get_db("app").unwrap().get_repo("main").unwrap();
    let leader_manager = leader_repo.repo_interner().await.unwrap();
    let leader_interner = leader_manager.get().await.unwrap();
    // Assert a real foreign-name collision, not a particular allocation order.
    assert!(["label", "v"]
        .iter()
        .any(|name| leader_interner.get_ind(name).unwrap().id() == occupied));
    let target = leader.current_commit_version("app", "main").await.unwrap();
    let event = emitted.try_recv().expect("commit emits synchronously");
    assert_eq!(event.commit_version, target);
    let field_bindings = ["label", "v"]
        .into_iter()
        .map(|name| (leader_interner.get_ind(name).unwrap().id(), name.into()))
        .collect();
    let source = EmittedSource {
        leader: InProcessReplSource::new(leader),
        event: ReplicatedEvent {
            event: (*event).clone(),
            field_bindings,
        },
    };
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(source),
        FollowerLoopConfig::new("collision", "app", "main")
            .with_poll_wait_ms(0)
            .with_max_iterations(1),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(follower_repo.replication_bookmark().await.unwrap(), target);
    let mut read = Batch::new();
    read.query(
        "read",
        Query::with_repo("main", "items").select(["label", "v"]),
    );
    let response = follower
        .execute_as(owner(), "app", &read.build())
        .await
        .unwrap();
    assert!(!response.results.contains_key("__error"), "{response:?}");
    let result = response.result("read").unwrap();
    assert!(result.corrupt_records.is_empty());
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].as_value().into_owned(), expected);
    assert_eq!(interner.get_ind("zzz").unwrap().id(), occupied);
}
