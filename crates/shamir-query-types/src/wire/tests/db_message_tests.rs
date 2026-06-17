use serde_json::json;

use crate::batch::TransactionInfo;
use crate::wire::db_message::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

#[test]
fn current_query_lang_version_is_two() {
    // v2: server now supports MessagePack id-keyed write/read pass-through.
    assert_eq!(CURRENT_QUERY_LANG_VERSION, 2);
}

#[test]
fn tx_begin_request_roundtrip_and_tag() {
    let req = DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        repo: "main".into(),
        isolation: Some("serializable".into()),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["op"], "tx_begin");
    assert_eq!(v["repo"], "main");
    assert_eq!(v["isolation"], "serializable");

    let back: DbRequest = serde_json::from_value(v).unwrap();
    assert!(matches!(back, DbRequest::TxBegin { repo, .. } if repo == "main"));
}

#[test]
fn tx_begin_isolation_optional_and_query_version_defaults() {
    // Minimal payload — no isolation, no query_version (older/min client).
    let v = json!({
        "op": "tx_begin",
        "db": "app",
        "repo": "main"
    });
    let req: DbRequest = serde_json::from_value(v).unwrap();
    match req {
        DbRequest::TxBegin {
            query_version,
            isolation,
            ..
        } => {
            assert_eq!(
                query_version, CURRENT_QUERY_LANG_VERSION,
                "absent query_version must default"
            );
            assert!(isolation.is_none(), "absent isolation decodes to None");
        }
        _ => panic!("expected TxBegin"),
    }
}

#[test]
fn tx_execute_request_roundtrip() {
    let v = json!({
        "op": "tx_execute",
        "db": "app",
        "tx_handle": 42,
        "batch": {
            "id": 1,
            "queries": {}
        }
    });
    let req: DbRequest = serde_json::from_value(v).unwrap();
    assert!(matches!(req, DbRequest::TxExecute { tx_handle, .. } if tx_handle == 42));
}

#[test]
fn tx_commit_and_rollback_request_tags() {
    let commit = serde_json::to_value(&DbRequest::TxCommit {
        db: "app".into(),
        tx_handle: 7,
    })
    .unwrap();
    assert_eq!(commit["op"], "tx_commit");
    assert_eq!(commit["tx_handle"], 7);

    let rollback = serde_json::to_value(&DbRequest::TxRollback {
        db: "app".into(),
        tx_handle: 7,
    })
    .unwrap();
    assert_eq!(rollback["op"], "tx_rollback");
    assert_eq!(rollback["tx_handle"], 7);
}

#[test]
fn tx_opened_response_roundtrip_and_tag() {
    let resp = DbResponse::TxOpened {
        tx_handle: 99,
        snapshot_version: 1234,
        isolation: "snapshot".into(),
    };
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["kind"], "tx_opened");
    assert_eq!(v["tx_handle"], 99);

    let back: DbResponse = serde_json::from_value(v).unwrap();
    assert!(matches!(
        back,
        DbResponse::TxOpened { snapshot_version, .. } if snapshot_version == 1234
    ));
}

#[test]
fn tx_committed_response_carries_transaction_info() {
    let info = TransactionInfo::committed(5, 100, 105, true);
    let resp = DbResponse::TxCommitted { transaction: info };
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["kind"], "tx_committed");
    assert_eq!(v["transaction"]["status"], "committed");

    let back: DbResponse = serde_json::from_value(v).unwrap();
    assert!(matches!(
        back,
        DbResponse::TxCommitted { transaction } if transaction.is_committed()
    ));
}

#[test]
fn tx_rolled_back_response_tag() {
    let v = serde_json::to_value(&DbResponse::TxRolledBack { tx_handle: 3 }).unwrap();
    assert_eq!(v["kind"], "tx_rolled_back");
    assert_eq!(v["tx_handle"], 3);
}
