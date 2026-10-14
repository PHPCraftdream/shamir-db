use std::sync::Arc;

use shamir_collections::new_map;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_query_types::admin::AlterRepoOp;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse, QueryEntry, SubBatchOp};
use shamir_query_types::read::QueryResult;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::db_handler::handler::ShamirDbHandler;

#[derive(Default)]
struct MemoryAppender(parking_lot::Mutex<Vec<AuditEntry>>);

impl AuditAppender for MemoryAppender {
    fn append_entry(&self, entry: &AuditEntry) {
        self.0.lock().push(entry.clone());
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) {}
}

async fn setup() -> (ShamirDbHandler, Arc<MemoryAppender>, Session) {
    let chain = Arc::new(AuditChain::new([7; 32]));
    let appender = Arc::new(MemoryAppender::default());
    let writer = Arc::new(AuditChainWriter::new_with_shared(chain, appender.clone()));
    let handler = ShamirDbHandler::new(Arc::new(shamir_db::ShamirDb::init_memory().await.unwrap()))
        .with_audit_writer(writer);
    let session = Session::new(
        [3; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        UnixNanos::now().as_u64(),
    );
    (handler, appender, session)
}

fn alter_entry(repo: &str) -> QueryEntry {
    QueryEntry {
        op: BatchOp::AlterRepo(AlterRepoOp {
            alter_repo: repo.into(),
            durability: None,
        }),
        after: Vec::new(),
        when: None,
        return_result: true,
    }
}

fn request(queries: shamir_collections::TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        visibility_wait: None,
        queries,
        return_all: true,
        return_only: None,
        limits: Default::default(),
        interner_epochs: new_map(),
        result_encoding: Default::default(),
    }
}

fn response(results: shamir_collections::TMap<String, QueryResult>) -> BatchResponse {
    BatchResponse {
        id: QueryValue::Null,
        results,
        execution_plan: Vec::new(),
        edge_provenance: new_map(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: new_map(),
    }
}

fn successful_result(repo: &str, durability: QueryValue) -> QueryResult {
    QueryResult::with_value(mpack!({
        "altered": true,
        "altered_repo": repo,
        "durability": @durability,
    }))
}

#[test]
fn audit_requires_exact_success_marker_and_repo_identity() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (handler, appender, session) = runtime.block_on(setup());
    let mut queries = new_map();
    queries.insert("alter".into(), alter_entry("main"));
    let batch = request(queries);

    let mut good_results = new_map();
    good_results.insert(
        "alter".into(),
        successful_result("main", mpack!({"wal_commit": "synced"})),
    );
    handler.audit_successful_alter_repo(&session, "db", &batch, &response(good_results));
    assert_eq!(appender.0.lock().len(), 1);

    for result in [
        QueryResult::default(),
        QueryResult::with_value(mpack!({"altered": false, "altered_repo": "main"})),
        QueryResult::with_value(mpack!({"altered": true, "altered_repo": "other"})),
        QueryResult::skipped(),
    ] {
        let mut results = new_map();
        results.insert("alter".into(), result);
        handler.audit_successful_alter_repo(&session, "db", &batch, &response(results));
    }
    assert_eq!(appender.0.lock().len(), 1);
}

#[test]
fn nested_alter_is_not_claimed_from_an_unrelated_local_alias_result() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (handler, appender, session) = runtime.block_on(setup());
    let mut inner_queries = new_map();
    inner_queries.insert("same".into(), alter_entry("main"));
    let mut outer_queries = new_map();
    outer_queries.insert(
        "same".into(),
        QueryEntry {
            op: BatchOp::Batch(SubBatchOp {
                batch: request(inner_queries),
                bind: new_map(),
            }),
            after: Vec::new(),
            when: None,
            return_result: true,
        },
    );
    let batch = request(outer_queries);
    let mut results = new_map();
    results.insert(
        "same".into(),
        successful_result("main", mpack!({"wal_commit": "synced"})),
    );
    handler.audit_successful_alter_repo(&session, "db", &batch, &response(results));
    assert!(appender.0.lock().is_empty());
}
