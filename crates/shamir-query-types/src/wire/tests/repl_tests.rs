//! Round-trip tests for the `Repl` variant of `DbRequest`/`DbResponse`.
//!
//! The nested internally-tagged enums (`DbRequest` tag = `op`,
//! `ReplRequest` tag = `repl_op`; same for responses) must survive a
//! `to_vec_named` → `from_slice` round-trip with full structural equality.
//! This is the main acceptance gate from the R0-a brief.

use crate::wire::db_message::{DbRequest, DbResponse};
use crate::wire::repl::{ReplRepoInfo, ReplRequest, ReplResponse};

fn roundtrip_db_request(req: &DbRequest) -> DbRequest {
    let bytes = rmp_serde::to_vec_named(req).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("deserialize")
}

fn roundtrip_db_response(resp: &DbResponse) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(resp).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("deserialize")
}

fn repl_op_tag(req: &DbRequest) -> String {
    use shamir_types::types::value::QueryValue;
    let bytes = rmp_serde::to_vec_named(req).unwrap();
    let map: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    map.get("op")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn repl_kind_tag(resp: &DbResponse) -> String {
    use shamir_types::types::value::QueryValue;
    let bytes = rmp_serde::to_vec_named(resp).unwrap();
    let map: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    map.get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn repl_hello_request_roundtrip() {
    let inner = ReplRequest::Hello {
        proto_ver: 1,
        node_id: "follower-7".into(),
    };
    let req = DbRequest::Repl(inner.clone());
    let back = roundtrip_db_request(&req);
    match back {
        DbRequest::Repl(ReplRequest::Hello { proto_ver, node_id }) => {
            assert_eq!((proto_ver, node_id), (1, "follower-7".to_string()));
        }
        other => panic!("expected Repl(Hello), got {other:?}"),
    }
    // Confirm the nested tag lands correctly on the wire.
    assert_eq!(repl_op_tag(&req), "repl");
    // And that the inner enum is untouched by the wrapper.
    assert_eq!(
        inner,
        ReplRequest::Hello {
            proto_ver: 1,
            node_id: "follower-7".into(),
        }
    );
}

#[test]
fn repl_pull_request_roundtrip_with_wait() {
    let req = DbRequest::Repl(ReplRequest::Pull {
        db: "app".into(),
        repo: "main".into(),
        from_version: 100,
        limit: 256,
        wait_ms: Some(500),
    });
    match roundtrip_db_request(&req) {
        DbRequest::Repl(ReplRequest::Pull {
            db,
            repo,
            from_version,
            limit,
            wait_ms,
        }) => {
            assert_eq!(db, "app");
            assert_eq!(repo, "main");
            assert_eq!(from_version, 100);
            assert_eq!(limit, 256);
            assert_eq!(wait_ms, Some(500));
        }
        other => panic!("expected Repl(Pull), got {other:?}"),
    }
}

#[test]
fn repl_pull_request_roundtrip_without_wait() {
    let req = DbRequest::Repl(ReplRequest::Pull {
        db: "app".into(),
        repo: "main".into(),
        from_version: 0,
        limit: 10,
        wait_ms: None,
    });
    match roundtrip_db_request(&req) {
        DbRequest::Repl(ReplRequest::Pull { wait_ms, .. }) => {
            assert_eq!(wait_ms, None, "wait_ms must stay None");
        }
        other => panic!("expected Repl(Pull), got {other:?}"),
    }
}

#[test]
fn repl_hello_response_roundtrip_with_repos() {
    let resp = DbResponse::Repl(ReplResponse::Hello {
        leader_epoch: 42,
        repos: vec![ReplRepoInfo {
            db: "app".into(),
            repo: "main".into(),
            current_version: 1234,
            journal_floor: 0,
        }],
    });
    match roundtrip_db_response(&resp) {
        DbResponse::Repl(ReplResponse::Hello {
            leader_epoch,
            repos,
        }) => {
            assert_eq!(leader_epoch, 42);
            assert_eq!(repos.len(), 1);
            assert_eq!(
                repos[0],
                ReplRepoInfo {
                    db: "app".into(),
                    repo: "main".into(),
                    current_version: 1234,
                    journal_floor: 0,
                }
            );
        }
        other => panic!("expected Repl(Hello), got {other:?}"),
    }
    assert_eq!(repl_kind_tag(&resp), "repl");
}

#[test]
fn repl_pull_response_roundtrip_with_events_and_gap() {
    let resp = DbResponse::Repl(ReplResponse::Pull {
        leader_epoch: 7,
        events: vec![0x01, 0x02, 0x03, 0xDE, 0xAD],
        gap_at: Some(50),
        current_version: 999,
    });
    match roundtrip_db_response(&resp) {
        DbResponse::Repl(ReplResponse::Pull {
            leader_epoch,
            events,
            gap_at,
            current_version,
        }) => {
            assert_eq!(leader_epoch, 7);
            assert_eq!(events, vec![0x01, 0x02, 0x03, 0xDE, 0xAD]);
            assert_eq!(gap_at, Some(50));
            assert_eq!(current_version, 999);
        }
        other => panic!("expected Repl(Pull), got {other:?}"),
    }
}

#[test]
fn repl_pull_response_roundtrip_without_gap() {
    let resp = DbResponse::Repl(ReplResponse::Pull {
        leader_epoch: 7,
        events: vec![],
        gap_at: None,
        current_version: 999,
    });
    match roundtrip_db_response(&resp) {
        DbResponse::Repl(ReplResponse::Pull { gap_at, events, .. }) => {
            assert_eq!(gap_at, None);
            assert!(events.is_empty());
        }
        other => panic!("expected Repl(Pull), got {other:?}"),
    }
}

#[test]
fn repl_error_response_roundtrip() {
    let resp = DbResponse::Repl(ReplResponse::Error {
        leader_epoch: 3,
        code: "unknown_repo".into(),
        message: "repo 'ghost' not found".into(),
    });
    match roundtrip_db_response(&resp) {
        DbResponse::Repl(ReplResponse::Error {
            leader_epoch,
            code,
            message,
        }) => {
            assert_eq!(leader_epoch, 3);
            assert_eq!(code, "unknown_repo");
            assert_eq!(message, "repo 'ghost' not found");
        }
        other => panic!("expected Repl(Error), got {other:?}"),
    }
}
