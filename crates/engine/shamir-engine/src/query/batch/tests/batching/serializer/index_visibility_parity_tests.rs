//! Query-language v4 `index_visibility` execution-path control. The response
//! metadata does not reveal the chosen commit mode; the module note records
//! that observability limit rather than treating `is_visible()` as parity.

#[cfg(feature = "fjall")]
use std::{sync::Arc, time::Duration};

#[cfg(feature = "fjall")]
use shamir_query_builder::Query;
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, IndexVisibility, QueryEntry, ResultEncoding,
};
use shamir_query_types::write::InsertOp;
#[cfg(feature = "fjall")]
use shamir_tx::{CancelHandle, WaitState};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked as execute_batch, BatchResolver, TableResolver};
#[cfg(feature = "fjall")]
use crate::query::batch::{Authorized, BatchError};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;

struct TestResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(
        &self,
        table_ref: &TableRef,
    ) -> shamir_storage::error::DbResult<crate::table::TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }
}

#[async_trait::async_trait]
impl BatchResolver for TestResolver {
    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::tx::RepoCore> {
        self.db
            .get_repo("default")
            .map(|repo| repo.core.clone())
            .ok_or_else(|| {
                shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
            })
    }
}

async fn setup() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

fn async_visibility_request() -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![QueryValue::Map(mpack_doc())],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            abort_on_skip: false,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: true,
        isolation: None,
        durability: None,
        index_visibility: Some(IndexVisibility::Async),
        visibility_wait: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn mpack_doc() -> shamir_collections::TMap<String, QueryValue> {
    let mut doc = new_map();
    doc.insert("id".to_string(), QueryValue::from("k1"));
    doc.insert("name".to_string(), QueryValue::from("widget"));
    doc
}

/// Async is accepted and the transaction commits. `TransactionInfo` contains
/// no observer for AsyncIndex-vs-Synchronous selection, so this is not a mode
/// parity assertion; the dispatch mapping is established at batch_execute.rs.
#[tokio::test]
async fn async_index_visibility_batch_commits() {
    let resolver = setup().await;
    let req = async_visibility_request();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "default")
        .await
        .expect("async-index-visibility batch must execute");
    let tx = resp
        .transaction
        .as_ref()
        .expect("transactional batch reports tx metadata");
    assert!(tx.is_committed(), "tx must commit: {tx:?}");
}

/// Default mode commits, but its response metadata cannot prove the internal
/// mode differs from AsyncIndex.
#[tokio::test]
async fn default_visibility_batch_commits() {
    let resolver = setup().await;
    let mut req = async_visibility_request();
    req.index_visibility = None;
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "default")
        .await
        .expect("default-visibility batch must execute");
    let tx = resp
        .transaction
        .as_ref()
        .expect("transactional batch reports tx metadata");
    assert!(tx.is_committed(), "tx must commit: {tx:?}");
}

#[cfg(feature = "fjall")]
#[tokio::test]
async fn temporal_read_cancels_while_replay_lock_is_held() {
    let dir = tempfile::tempdir().unwrap();
    tokio::time::timeout(Duration::from_secs(120), async {
        let db = DbInstance::with_repos(vec![RepoConfig {
            name: "default".into(),
            factory: BoxRepoFactory::fjall_raw(dir.path().join("repo")),
            tables: vec![TableConfig::new("users")],
        }])
        .await
        .unwrap();
        let repo = db.get_repo("default").unwrap();
        repo.disable_background_drainer_for_test();
        let drainer = repo.drainer();
        let held = drainer.replay_lock_for_test().await;
        let resolver = TestResolver { db };
        let mut req = async_visibility_request();
        req.transactional = false;
        req.limits.max_execution_time_secs = 180;
        req.index_visibility = None;
        req.queries.clear();
        req.queries.insert(
            "read".into(),
            QueryEntry {
                op: BatchOp::Read(Query::from("users").as_of_version(1).into()),
                return_result: true,
                abort_on_skip: false,
                after: Vec::new(),
                when: None,
            },
        );
        let cancel = Arc::new(CancelHandle::default());
        let auth = Authorized::unchecked(&req, Actor::System, "default")
            .with_cancel_handle(Some(Arc::clone(&cancel)));
        let mut read = Box::pin(crate::query::batch::execute_batch(
            auth, &resolver, None, None,
        ));
        loop {
            tokio::select! {
                result = &mut read => panic!("read returned before cancellation: {result:?}"),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
            if cancel.waiting() == WaitState::Lock {
                break;
            }
        }
        assert_eq!(cancel.try_cancel(), Ok(()));
        let result = read.await;
        assert!(matches!(result, Err(BatchError::Cancelled)), "{result:?}");
        assert_eq!(cancel.waiting(), WaitState::None);
        drop(held);
    })
    .await
    .expect("temporal replay-lock cancellation watchdog expired");
}
