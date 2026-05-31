//! Phase B Stage 5 — wire-level end-to-end tests for interactive
//! (multi-call) transactions.  Drives `ShamirDbHandler::handle()` with
//! msgpack-encoded `DbRequest` variants and decodes the `DbResponse`.

use std::sync::Arc;

use serde_json::json;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

// ---------- fixtures (mirror tests/db_handler.rs) ----------

fn make_session_with_sid(sid: [u8; 32], roles: Vec<String>) -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(roles),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = sid;
    s
}

fn user_session_a() -> Session {
    make_session_with_sid([0x11; 32], vec!["read_write".into()])
}
fn user_session_b() -> Session {
    make_session_with_sid([0x22; 32], vec!["read_write".into()])
}

async fn make_db_with_table(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}
fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}
fn parse_batch(v: serde_json::Value) -> BatchRequest {
    serde_json::from_value(v).expect("parse batch")
}

fn tx_begin(db: &str, repo: &str) -> DbRequest {
    DbRequest::TxBegin {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        repo: repo.into(),
        isolation: None,
    }
}

fn tx_execute(db: &str, tx_handle: u64, body: serde_json::Value) -> DbRequest {
    DbRequest::TxExecute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        tx_handle,
        batch: parse_batch(body),
    }
}

fn execute(db: &str, body: serde_json::Value) -> DbRequest {
    let batch: BatchRequest = serde_json::from_value(body).expect("parse batch");
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

// ---------- 1. happy path: BEGIN → EXECUTE(write) → EXECUTE(read RYOW) → COMMIT → fresh Execute ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_tx_happy_path_wire() {
    let shamir = make_db_with_table("app", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session_a();

    // BEGIN
    let res = decode(
        &handler
            .handle(&s, &encode(&tx_begin("app", "main")))
            .unwrap(),
    );
    let tx_handle = match res {
        DbResponse::TxOpened {
            tx_handle,
            isolation,
            ..
        } => {
            assert_eq!(isolation, "snapshot");
            tx_handle
        }
        other => panic!("expected TxOpened, got {:?}", other),
    };

    // EXECUTE(write) — two rows via `set`.
    let wres = decode(
        &handler
            .handle(
                &s,
                &encode(&tx_execute(
                    "app",
                    tx_handle,
                    json!({
                        "id": "w",
                        "queries": {
                            "s1": {"set": "items", "key": {"id": "a"}, "value": {"id": "a", "qty": 3}},
                            "s2": {"set": "items", "key": {"id": "b"}, "value": {"id": "b", "qty": 5}}
                        },
                        "return_all": false
                    }),
                )),
            )
            .unwrap(),
    );
    let wresp = match wres {
        DbResponse::TxBatch { response } => response,
        other => panic!("expected TxBatch, got {:?}", other),
    };
    assert!(
        wresp.transaction.is_none(),
        "open tx → no TransactionInfo per call"
    );

    // EXECUTE(read inside open tx) — streaming scans do NOT overlay the
    // tx's write_set (KNOWN LIMITATION C8, see stream_tx_tests.rs:95-100).
    // Only point reads (`read_one_tx`) do RYOW.  Assert current behaviour:
    // scan returns 0 rows while the tx is still open.
    let rres = decode(
        &handler
            .handle(
                &s,
                &encode(&tx_execute(
                    "app",
                    tx_handle,
                    json!({
                        "id": "r",
                        "queries": {
                            "top": {
                                "from": "items",
                                "order_by": {"items": [{"field": ["qty"], "direction": "desc"}]}
                            }
                        }
                    }),
                )),
            )
            .unwrap(),
    );
    let rresp = match rres {
        DbResponse::TxBatch { response } => response,
        other => panic!("expected TxBatch on read, got {:?}", other),
    };
    let rows = &rresp.results.get("top").expect("top result").records;
    assert_eq!(
        rows.len(),
        0,
        "C8: streaming scan does not overlay write_set — staged rows invisible until commit"
    );

    // COMMIT
    let cres = decode(
        &handler
            .handle(
                &s,
                &encode(&DbRequest::TxCommit {
                    db: "app".into(),
                    tx_handle,
                }),
            )
            .unwrap(),
    );
    match cres {
        DbResponse::TxCommitted { transaction } => {
            assert!(transaction.is_committed(), "commit must report committed");
        }
        other => panic!("expected TxCommitted, got {:?}", other),
    }

    // FRESH non-tx Execute — committed rows must be visible.
    let vres = decode(
        &handler
            .handle(
                &s,
                &encode(&execute(
                    "app",
                    json!({
                        "id": "v",
                        "queries": {"all": {"from": "items"}}
                    }),
                )),
            )
            .unwrap(),
    );
    let vresp = match vres {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch from fresh Execute, got {:?}", other),
    };
    let all = &vresp.results.get("all").expect("all result").records;
    assert_eq!(all.len(), 2, "committed rows visible from non-tx Execute");
}

// ---------- 2. ROLLBACK path: writes discarded ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_tx_rollback_discards_writes() {
    let shamir = make_db_with_table("app", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let s = user_session_a();

    let tx_handle = match decode(
        &handler
            .handle(&s, &encode(&tx_begin("app", "main")))
            .unwrap(),
    ) {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened, got {:?}", other),
    };

    let _ = handler
        .handle(
            &s,
            &encode(&tx_execute(
                "app",
                tx_handle,
                json!({
                    "id": "w",
                    "queries": {
                        "s1": {"set": "items", "key": {"id": "x"}, "value": {"id": "x", "qty": 9}}
                    },
                    "return_all": false
                }),
            )),
        )
        .unwrap();

    let rb = decode(
        &handler
            .handle(
                &s,
                &encode(&DbRequest::TxRollback {
                    db: "app".into(),
                    tx_handle,
                }),
            )
            .unwrap(),
    );
    match rb {
        DbResponse::TxRolledBack { tx_handle: h } => assert_eq!(h, tx_handle),
        other => panic!("expected TxRolledBack, got {:?}", other),
    }

    let vres = decode(
        &handler
            .handle(
                &s,
                &encode(&execute(
                    "app",
                    json!({"id": "v", "queries": {"all": {"from": "items"}}}),
                )),
            )
            .unwrap(),
    );
    match vres {
        DbResponse::Batch { response } => {
            assert_eq!(
                response.results.get("all").unwrap().records.len(),
                0,
                "rollback discarded"
            );
        }
        other => panic!("expected Batch, got {:?}", other),
    }
}

// ---------- 3. Ownership rejection: foreign session is denied ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_tx_foreign_session_rejected_wire() {
    let shamir = make_db_with_table("app", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let sa = user_session_a();
    let sb = user_session_b();

    // Session A opens the tx.
    let tx_handle = match decode(
        &handler
            .handle(&sa, &encode(&tx_begin("app", "main")))
            .unwrap(),
    ) {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened, got {:?}", other),
    };

    // Session B attempts TxExecute on A's handle → tx_forbidden.
    let res = decode(
        &handler
            .handle(
                &sb,
                &encode(&tx_execute(
                    "app",
                    tx_handle,
                    json!({
                        "id": "w",
                        "queries": {
                            "s": {"set": "items", "key": {"id": "a"}, "value": {"id": "a", "qty": 1}}
                        },
                        "return_all": false
                    }),
                )),
            )
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, .. } => assert_eq!(code, "tx_forbidden"),
        other => panic!("expected tx_forbidden Error, got {:?}", other),
    }

    // Session B attempts TxCommit on A's handle → tx_forbidden.
    let res2 = decode(
        &handler
            .handle(
                &sb,
                &encode(&DbRequest::TxCommit {
                    db: "app".into(),
                    tx_handle,
                }),
            )
            .unwrap(),
    );
    match res2 {
        DbResponse::Error { code, .. } => assert_eq!(code, "tx_forbidden"),
        other => panic!("expected tx_forbidden on TxCommit, got {:?}", other),
    }

    // Unknown handle from A → tx_not_found.
    let res3 = decode(
        &handler
            .handle(
                &sa,
                &encode(&tx_execute(
                    "app",
                    999_999_999,
                    json!({
                        "id": "x",
                        "queries": {"q": {"from": "items"}}
                    }),
                )),
            )
            .unwrap(),
    );
    match res3 {
        DbResponse::Error { code, .. } => assert_eq!(code, "tx_not_found"),
        other => panic!("expected tx_not_found, got {:?}", other),
    }

    // Tidy: roll back A's handle so the registry is empty at test exit.
    let _ = handler
        .handle(
            &sa,
            &encode(&DbRequest::TxRollback {
                db: "app".into(),
                tx_handle,
            }),
        )
        .unwrap();
}
