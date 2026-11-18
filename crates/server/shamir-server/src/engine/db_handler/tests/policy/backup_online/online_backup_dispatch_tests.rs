//! Wire/dispatch round-trip for the node-local online-backup admin ops
//! (R2a step 3a, design `online-backup.md` §4.7 decision D6): each op is
//! built through `shamir-query-builder`, sent through the normal batch
//! entry (`ShamirDbHandler::execute`), and asserted to reach the registry
//! boundary — a stub `OnlineBackupAdminPort` installed on the engine — and
//! to return the standard single-record admin response.
//!
//! The superuser bridge (`entry.op.is_admin()`, `handler.rs`) must admit a
//! superuser session for all three ops; a non-superuser session is refused
//! before dispatch. The follower refusal for `StartBackup`/`CancelBackup`
//! lives in the read-only-replica gate (`online_backup_follower_read_only`);
//! `BackupStatus` is allowed anywhere.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::ShamirDb;
use shamir_db::{OnlineBackupAdminError, OnlineBackupAdminPort};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{backup_status, cancel_backup, start_backup};
use shamir_query_types::read::QueryRecord;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_handler::config::NodeMode;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

/// What the stub port observed at the registry boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Start(String),
    Status,
    Cancel,
}

/// Records the dispatch; answers with fixed records so the response shape
/// can be asserted without a real backup run.
#[derive(Default)]
struct StubPort {
    calls: std::sync::Mutex<Vec<Call>>,
}

impl StubPort {
    fn record(&self, call: Call) {
        self.calls.lock().expect("stub lock").push(call);
    }
}

#[async_trait]
impl OnlineBackupAdminPort for StubPort {
    async fn start(&self, to: String) -> Result<QueryValue, OnlineBackupAdminError> {
        self.record(Call::Start(to.clone()));
        let mut value = new_map();
        value.insert("backup_id".to_string(), QueryValue::Str("ob-1".into()));
        value.insert("destination".to_string(), QueryValue::Str(to));
        Ok(QueryValue::Map(value))
    }

    async fn status(&self) -> Result<QueryValue, OnlineBackupAdminError> {
        self.record(Call::Status);
        let mut value = new_map();
        value.insert("backup_id".to_string(), QueryValue::Str(String::new()));
        value.insert("running".to_string(), QueryValue::Bool(false));
        Ok(QueryValue::Map(value))
    }

    async fn cancel(&self) -> Result<QueryValue, OnlineBackupAdminError> {
        self.record(Call::Cancel);
        let mut value = new_map();
        value.insert("cancelled".to_string(), QueryValue::Bool(false));
        Ok(QueryValue::Map(value))
    }
}

fn superuser_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(true, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

fn plain_session() -> Session {
    Session::new(
        [0xCD; 16],
        "bob".into(),
        SessionPermissions::new(false, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// In-memory engine with the stub port installed (no journal/catalogue is
/// touched, so no persistent system store is needed).
async fn build_handler() -> (ShamirDbHandler, Arc<StubPort>) {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let stub = Arc::new(StubPort::default());
    shamir.set_online_backup_admin_port(Arc::clone(&stub) as Arc<dyn OnlineBackupAdminPort>);
    (ShamirDbHandler::new(Arc::new(shamir)), stub)
}

fn single_op_batch(
    alias: &str,
    op: shamir_query_types::batch::BatchOp,
) -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.op(alias, op);
    b.build()
}

/// Pull the single direct record out of a successful single-alias response.
fn first_record(resp: DbResponse, alias: &str) -> QueryValue {
    match resp {
        DbResponse::Batch { mut response } => {
            let mut result = response.results.shift_remove(alias).expect("alias present");
            match result.records.pop() {
                Some(QueryRecord::Direct(value)) => value,
                other => panic!("expected one direct record, got: {other:?}"),
            }
        }
        other => panic!("expected success, got: {other:?}"),
    }
}

#[tokio::test]
async fn start_backup_round_trips_to_registry_boundary() {
    let (handler, stub) = build_handler().await;
    let conn = ConnectionServices::without_push(0);
    let batch = single_op_batch(
        "start",
        start_backup(Path::new("D:/backup-root").display().to_string()),
    );

    let resp = handler
        .execute(
            &superuser_session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            batch,
            &conn,
        )
        .await;

    let value = first_record(resp, "start");
    assert_eq!(
        stub.calls.lock().expect("stub lock").as_slice(),
        [Call::Start("D:/backup-root".to_string())]
    );
    let QueryValue::Map(map) = value else {
        panic!("start must return a map record");
    };
    assert_eq!(
        map.get("backup_id"),
        Some(&QueryValue::Str("ob-1".to_string()))
    );
    assert_eq!(
        map.get("destination"),
        Some(&QueryValue::Str("D:/backup-root".to_string()))
    );
}

#[tokio::test]
async fn backup_status_round_trips_to_registry_boundary() {
    let (handler, stub) = build_handler().await;
    let conn = ConnectionServices::without_push(0);
    let batch = single_op_batch("status", backup_status());

    let resp = handler
        .execute(
            &superuser_session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            batch,
            &conn,
        )
        .await;

    let value = first_record(resp, "status");
    assert_eq!(
        stub.calls.lock().expect("stub lock").as_slice(),
        [Call::Status]
    );
    let QueryValue::Map(map) = value else {
        panic!("status must return a map record");
    };
    assert!(map.contains_key("backup_id"));
    assert_eq!(map.get("running"), Some(&QueryValue::Bool(false)));
}

#[tokio::test]
async fn cancel_backup_round_trips_to_registry_boundary() {
    let (handler, stub) = build_handler().await;
    let conn = ConnectionServices::without_push(0);
    let batch = single_op_batch("cancel", cancel_backup());

    let resp = handler
        .execute(
            &superuser_session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            batch,
            &conn,
        )
        .await;

    let value = first_record(resp, "cancel");
    assert_eq!(
        stub.calls.lock().expect("stub lock").as_slice(),
        [Call::Cancel]
    );
    let QueryValue::Map(map) = value else {
        panic!("cancel must return a map record");
    };
    assert_eq!(map.get("cancelled"), Some(&QueryValue::Bool(false)));
}

/// The superuser bridge covers all three ops: a non-superuser session is
/// refused before dispatch (the stub sees nothing).
#[tokio::test]
async fn non_superuser_is_refused_before_dispatch() {
    let (handler, stub) = build_handler().await;
    let conn = ConnectionServices::without_push(0);
    let batch = single_op_batch("status", backup_status());

    let resp = handler
        .execute(
            &plain_session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            batch,
            &conn,
        )
        .await;

    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "permission_denied"),
        other => panic!("non-superuser must be refused, got: {other:?}"),
    }
    assert!(stub.calls.lock().expect("stub lock").is_empty());
}

/// Follower refusal: `BackupStatus` is allowed on a read-only replica, and
/// the write arms carry the stable `online_backup_follower_read_only` code.
#[tokio::test]
async fn follower_allows_status_but_refuses_start_and_cancel() {
    let (handler, _stub) = build_handler().await;
    let follower = handler.with_node_mode(NodeMode::ReadOnly);
    let conn = ConnectionServices::without_push(0);
    let session = superuser_session();

    let status = follower
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            single_op_batch("status", backup_status()),
            &conn,
        )
        .await;
    assert!(
        matches!(status, DbResponse::Batch { .. }),
        "status must pass on a follower"
    );

    for (alias, op) in [
        ("start", start_backup("D:/backup-root")),
        ("cancel", cancel_backup()),
    ] {
        let resp = follower
            .execute(
                &session,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                single_op_batch(alias, op),
                &conn,
            )
            .await;
        match resp {
            DbResponse::Error { code, message } => {
                assert_eq!(
                    code, "online_backup_follower_read_only",
                    "message: {message}"
                );
            }
            other => panic!("follower must refuse {alias}, got: {other:?}"),
        }
    }
}
