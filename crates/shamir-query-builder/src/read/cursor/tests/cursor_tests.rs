use shamir_query_types::wire::{CursorId, DbRequest, CURRENT_QUERY_LANG_VERSION};

use crate::cursor::{cancel_cursor, create_cursor, create_cursor_with_version, fetch_next};
use crate::query::Query;

#[test]
fn create_cursor_builds_expected_request() {
    let req = create_cursor("app", Query::from("users"), 50);
    match req {
        DbRequest::CreateCursor {
            query_version,
            db,
            query,
            page_size,
        } => {
            assert_eq!(query_version, CURRENT_QUERY_LANG_VERSION);
            assert_eq!(db, "app");
            assert_eq!(page_size, 50);
            assert_eq!(query.from, Query::from("users").build().from);
        }
        other => panic!("expected DbRequest::CreateCursor, got {other:?}"),
    }
}

#[test]
fn create_cursor_matches_hand_constructed_request() {
    let built = create_cursor("app", Query::from("users"), 50);
    let expected = DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query: Query::from("users").build(),
        page_size: 50,
    };

    let built_bytes = rmp_serde::to_vec_named(&built).unwrap();
    let expected_bytes = rmp_serde::to_vec_named(&expected).unwrap();
    assert_eq!(built_bytes, expected_bytes);
}

#[test]
fn create_cursor_with_version_pins_explicit_version() {
    let req = create_cursor_with_version(1, "app", Query::from("users"), 10);
    match req {
        DbRequest::CreateCursor { query_version, .. } => assert_eq!(query_version, 1),
        other => panic!("expected DbRequest::CreateCursor, got {other:?}"),
    }
}

#[test]
fn fetch_next_builds_expected_request() {
    let req = fetch_next(CursorId(7), Some(25));
    match req {
        DbRequest::FetchNext {
            cursor_id,
            page_size,
        } => {
            assert_eq!(cursor_id, CursorId(7));
            assert_eq!(page_size, Some(25));
        }
        other => panic!("expected DbRequest::FetchNext, got {other:?}"),
    }
}

#[test]
fn fetch_next_accepts_bare_u64_via_into() {
    let req = fetch_next(7u64, Some(25));
    assert!(matches!(
        req,
        DbRequest::FetchNext { cursor_id, .. } if cursor_id == CursorId(7)
    ));
}

#[test]
fn fetch_next_none_omits_page_size_override() {
    // CR-B3 (#769): omitting page_size falls back to the cursor's stored
    // CreateCursor-time default server-side.
    let req = fetch_next(CursorId(7), None);
    match req {
        DbRequest::FetchNext {
            cursor_id,
            page_size,
        } => {
            assert_eq!(cursor_id, CursorId(7));
            assert_eq!(page_size, None);
        }
        other => panic!("expected DbRequest::FetchNext, got {other:?}"),
    }
}

#[test]
fn cancel_cursor_builds_expected_request() {
    let req = cancel_cursor(CursorId(9));
    match req {
        DbRequest::CancelCursor { cursor_id } => assert_eq!(cursor_id, CursorId(9)),
        other => panic!("expected DbRequest::CancelCursor, got {other:?}"),
    }
}
