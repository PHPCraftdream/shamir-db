use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::TransactionInfo;
use crate::wire::db_message::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

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
    let v = to_qv(&req);
    assert_eq!(v.get("op").and_then(QueryValue::as_str), Some("tx_begin"));
    assert_eq!(v.get("repo").and_then(QueryValue::as_str), Some("main"));
    assert_eq!(
        v.get("isolation").and_then(QueryValue::as_str),
        Some("serializable")
    );

    let back: DbRequest = from_qv(v);
    assert!(matches!(back, DbRequest::TxBegin { repo, .. } if repo == "main"));
}

#[test]
fn tx_begin_isolation_optional_and_query_version_defaults() {
    // Minimal payload — no isolation, no query_version (older/min client).
    let v = mpack!({
        "op": "tx_begin",
        "db": "app",
        "repo": "main"
    });
    let req: DbRequest = from_qv(v);
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
    let v = mpack!({
        "op": "tx_execute",
        "db": "app",
        "tx_handle": 42_i64,
        "batch": {
            "id": 1_i64,
            "queries": {}
        }
    });
    let req: DbRequest = from_qv(v);
    assert!(matches!(req, DbRequest::TxExecute { tx_handle, .. } if tx_handle == 42));
}

#[test]
fn tx_commit_and_rollback_request_tags() {
    let commit = to_qv(&DbRequest::TxCommit {
        db: "app".into(),
        tx_handle: 7,
    });
    assert_eq!(
        commit.get("op").and_then(QueryValue::as_str),
        Some("tx_commit")
    );
    assert_eq!(
        commit.get("tx_handle").and_then(QueryValue::as_i64),
        Some(7)
    );

    let rollback = to_qv(&DbRequest::TxRollback {
        db: "app".into(),
        tx_handle: 7,
    });
    assert_eq!(
        rollback.get("op").and_then(QueryValue::as_str),
        Some("tx_rollback")
    );
    assert_eq!(
        rollback.get("tx_handle").and_then(QueryValue::as_i64),
        Some(7)
    );
}

#[test]
fn tx_opened_response_roundtrip_and_tag() {
    let resp = DbResponse::TxOpened {
        tx_handle: 99,
        snapshot_version: 1234,
        isolation: "snapshot".into(),
    };
    let v = to_qv(&resp);
    assert_eq!(
        v.get("kind").and_then(QueryValue::as_str),
        Some("tx_opened")
    );
    assert_eq!(v.get("tx_handle").and_then(QueryValue::as_i64), Some(99));

    let back: DbResponse = from_qv(v);
    assert!(matches!(
        back,
        DbResponse::TxOpened { snapshot_version, .. } if snapshot_version == 1234
    ));
}

#[test]
fn tx_committed_response_carries_transaction_info() {
    let info = TransactionInfo::committed(5, 100, 105, true);
    let resp = DbResponse::TxCommitted { transaction: info };
    let v = to_qv(&resp);
    assert_eq!(
        v.get("kind").and_then(QueryValue::as_str),
        Some("tx_committed")
    );
    let tx = v.get("transaction").expect("transaction key");
    assert_eq!(
        tx.get("status").and_then(QueryValue::as_str),
        Some("committed")
    );

    let back: DbResponse = from_qv(v);
    assert!(matches!(
        back,
        DbResponse::TxCommitted { transaction } if transaction.is_committed()
    ));
}

#[test]
fn tx_rolled_back_response_tag() {
    let v = to_qv(&DbResponse::TxRolledBack { tx_handle: 3 });
    assert_eq!(
        v.get("kind").and_then(QueryValue::as_str),
        Some("tx_rolled_back")
    );
    assert_eq!(v.get("tx_handle").and_then(QueryValue::as_i64), Some(3));
}

#[test]
fn create_scram_user_debug_redacts_password() {
    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "hunter2".into(),
        roles: vec![],
        hmac: None,
    };
    let dbg = format!("{:?}", req);
    assert!(
        !dbg.contains("hunter2"),
        "Debug output must not leak the cleartext password: {dbg}"
    );
    assert!(
        dbg.contains("SecretString(***)"),
        "Debug output must show the redacted SecretString marker: {dbg}"
    );
}

#[test]
fn create_scram_user_wire_roundtrip_preserves_password_and_shape() {
    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "hunter2".into(),
        roles: vec!["reader".to_string()],
        hmac: Some("deadbeef".to_string()),
    };

    // The wire shape is unchanged: `password` still serializes as a plain
    // string, not a wrapped object — `SecretString`'s Serialize impl is a
    // transparent pass-through.
    let v = to_qv(&req);
    assert_eq!(
        v.get("password").and_then(QueryValue::as_str),
        Some("hunter2"),
        "password must serialize as a plain string on the wire"
    );

    let bytes = rmp_serde::to_vec_named(&req).unwrap();
    let back: DbRequest = rmp_serde::from_slice(&bytes).unwrap();
    match back {
        DbRequest::CreateScramUser {
            name,
            password,
            roles,
            hmac,
        } => {
            assert_eq!(name, "bob");
            assert_eq!(password.reveal(), "hunter2");
            assert_eq!(roles, vec!["reader".to_string()]);
            assert_eq!(hmac, Some("deadbeef".to_string()));
        }
        _ => panic!("expected CreateScramUser"),
    }
}
