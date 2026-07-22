use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::TransactionInfo;
use crate::read::ReadQuery;
use crate::wire::cursor_id::CursorId;
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

// ============================================================================
// FG-5a: cursor wire protocol round-trips
// ============================================================================

#[test]
fn create_cursor_request_roundtrip_and_tag() {
    let req = DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        query: ReadQuery::new("users"),
        page_size: 50,
    };
    let v = to_qv(&req);
    assert_eq!(
        v.get("op").and_then(QueryValue::as_str),
        Some("create_cursor")
    );
    assert_eq!(v.get("db").and_then(QueryValue::as_str), Some("app"));
    assert_eq!(v.get("page_size").and_then(QueryValue::as_i64), Some(50));

    let back: DbRequest = from_qv(v);
    match back {
        DbRequest::CreateCursor {
            db,
            page_size,
            query,
            ..
        } => {
            assert_eq!(db, "app");
            assert_eq!(page_size, 50);
            assert_eq!(query.from, ReadQuery::new("users").from);
        }
        _ => panic!("expected CreateCursor"),
    }
}

#[test]
fn create_cursor_query_version_defaults_when_absent() {
    let v = mpack!({
        "op": "create_cursor",
        "db": "app",
        "query": { "from": "users" },
        "page_size": 20_i64
    });
    let req: DbRequest = from_qv(v);
    match req {
        DbRequest::CreateCursor { query_version, .. } => {
            assert_eq!(
                query_version, CURRENT_QUERY_LANG_VERSION,
                "absent query_version must default"
            );
        }
        _ => panic!("expected CreateCursor"),
    }
}

#[test]
fn fetch_next_request_roundtrip_and_tag() {
    let req = DbRequest::FetchNext {
        cursor_id: CursorId(42),
        page_size: 10,
    };
    let v = to_qv(&req);
    assert_eq!(v.get("op").and_then(QueryValue::as_str), Some("fetch_next"));
    assert_eq!(v.get("cursor_id").and_then(QueryValue::as_i64), Some(42));
    assert_eq!(v.get("page_size").and_then(QueryValue::as_i64), Some(10));

    let back: DbRequest = from_qv(v);
    assert!(matches!(
        back,
        DbRequest::FetchNext { cursor_id, page_size }
            if cursor_id == CursorId(42) && page_size == 10
    ));
}

#[test]
fn cancel_cursor_request_roundtrip_and_tag() {
    let req = DbRequest::CancelCursor {
        cursor_id: CursorId(7),
    };
    let v = to_qv(&req);
    assert_eq!(
        v.get("op").and_then(QueryValue::as_str),
        Some("cancel_cursor")
    );
    assert_eq!(v.get("cursor_id").and_then(QueryValue::as_i64), Some(7));

    let back: DbRequest = from_qv(v);
    assert!(matches!(
        back,
        DbRequest::CancelCursor { cursor_id } if cursor_id == CursorId(7)
    ));
}

#[test]
fn cursor_id_serializes_as_bare_integer_not_wrapped_object() {
    // `#[serde(transparent)]` — must round-trip as a plain integer on the
    // wire, not `{ "0": 42 }`.
    let v = to_qv(&CursorId(42));
    assert_eq!(v.as_i64(), Some(42));
}

#[test]
fn cursor_page_response_roundtrip_and_tag() {
    let resp = DbResponse::CursorPage {
        cursor_id: CursorId(5),
        page: crate::read::QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
            versions: None,
        },
        has_more: true,
    };
    let v = to_qv(&resp);
    assert_eq!(
        v.get("kind").and_then(QueryValue::as_str),
        Some("cursor_page")
    );
    assert_eq!(v.get("cursor_id").and_then(QueryValue::as_i64), Some(5));
    assert_eq!(v.get("has_more").and_then(QueryValue::as_bool), Some(true));

    let back: DbResponse = from_qv(v);
    assert!(matches!(
        back,
        DbResponse::CursorPage { cursor_id, has_more, .. }
            if cursor_id == CursorId(5) && has_more
    ));
}

#[test]
fn cursor_closed_response_roundtrip_and_tag() {
    let resp = DbResponse::CursorClosed {
        cursor_id: CursorId(11),
    };
    let v = to_qv(&resp);
    assert_eq!(
        v.get("kind").and_then(QueryValue::as_str),
        Some("cursor_closed")
    );
    assert_eq!(v.get("cursor_id").and_then(QueryValue::as_i64), Some(11));

    let back: DbResponse = from_qv(v);
    assert!(matches!(
        back,
        DbResponse::CursorClosed { cursor_id } if cursor_id == CursorId(11)
    ));
}
