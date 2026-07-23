//! [`BatchError`] Display coverage — including the FG-5a cursor variants.
//!
//! `BatchError` itself is not `Serialize`/`Deserialize` (it never rides the
//! wire directly — `shamir-server`'s `error_code` maps it to a
//! `DbResponse::Error { code, message }`); these tests exercise the
//! `Display` impl, which is what `message` is built from.

use crate::batch::BatchError;
use crate::wire::CursorId;

#[test]
fn cursor_not_found_display_mentions_the_cursor_id() {
    let err = BatchError::CursorNotFound {
        cursor_id: CursorId(42),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("42"),
        "Display must mention the cursor id: {msg}"
    );
    assert!(
        msg.contains("not found"),
        "Display should say not found: {msg}"
    );
}

#[test]
fn cursor_expired_display_mentions_the_cursor_id() {
    let err = BatchError::CursorExpired {
        cursor_id: CursorId(7),
    };
    let msg = err.to_string();
    assert!(
        msg.contains('7'),
        "Display must mention the cursor id: {msg}"
    );
    assert!(msg.contains("expired"), "Display should say expired: {msg}");
}

#[test]
fn cursor_limit_exceeded_display_mentions_the_limit() {
    let err = BatchError::CursorLimitExceeded { limit: 10 };
    let msg = err.to_string();
    assert!(msg.contains("10"), "Display must mention the limit: {msg}");
}

#[test]
fn cursor_not_found_and_cursor_expired_are_distinguishable() {
    // The wire contract (CURSORS.md) requires a fetch against an unknown id
    // and a fetch against an idle-timeout-evicted id to be distinguishable
    // by error code/message — assert the two variants never collide.
    let not_found = BatchError::CursorNotFound {
        cursor_id: CursorId(1),
    };
    let expired = BatchError::CursorExpired {
        cursor_id: CursorId(1),
    };
    assert_ne!(not_found.to_string(), expired.to_string());
    assert_ne!(not_found, expired);
}

#[test]
fn cursor_temporal_not_supported_display_mentions_temporal_scope_cut() {
    // FG-5b: `CreateCursor` on a query whose `Temporal` is `AsOf`/`History`
    // is rejected with a distinct, clearly-labelled error rather than a
    // silent downgrade to `Latest` (a wrong-results bug) or a generic
    // validation error indistinguishable from other rejections.
    let err = BatchError::CursorTemporalNotSupported;
    let msg = err.to_string();
    assert!(
        msg.contains("Latest"),
        "Display should name the only supported temporal mode: {msg}"
    );
}

#[test]
fn cursor_temporal_not_supported_distinguishable_from_other_cursor_errors() {
    let temporal_err = BatchError::CursorTemporalNotSupported;
    let not_found = BatchError::CursorNotFound {
        cursor_id: CursorId(1),
    };
    assert_ne!(temporal_err, not_found);
    assert_ne!(temporal_err.to_string(), not_found.to_string());
}

#[test]
fn invalid_page_size_display_mentions_the_range() {
    // CR-A3: `page_size == 0` (and `page_size > max`) is rejected with a
    // distinct, specific error — the message must state the valid range so
    // a client can self-correct instead of guessing.
    let err = BatchError::InvalidPageSize {
        page_size: 0,
        max: 10_000,
    };
    let msg = err.to_string();
    assert!(msg.contains('0'), "Display must mention page_size: {msg}");
    assert!(msg.contains("10000"), "Display must mention max: {msg}");
}

#[test]
fn invalid_page_size_distinguishable_from_other_cursor_errors() {
    let invalid = BatchError::InvalidPageSize {
        page_size: 0,
        max: 10_000,
    };
    let not_found = BatchError::CursorNotFound {
        cursor_id: CursorId(1),
    };
    assert_ne!(invalid, not_found);
    assert_ne!(invalid.to_string(), not_found.to_string());
}

#[test]
fn cursor_page_too_large_display_mentions_size_and_max() {
    // CR-A5: a cursor page whose serialized size exceeds
    // `max_result_size_bytes` is rejected with a distinct error naming both
    // the offending size and the configured cap.
    let err = BatchError::CursorPageTooLarge {
        size: 5_000_000,
        max: 1_000_000,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("5000000"),
        "Display must mention the rejected size: {msg}"
    );
    assert!(
        msg.contains("1000000"),
        "Display must mention the configured max: {msg}"
    );
}

#[test]
fn cursor_page_too_large_distinguishable_from_other_cursor_errors() {
    let too_large = BatchError::CursorPageTooLarge {
        size: 5_000_000,
        max: 1_000_000,
    };
    let not_found = BatchError::CursorNotFound {
        cursor_id: CursorId(1),
    };
    assert_ne!(too_large, not_found);
    assert_ne!(too_large.to_string(), not_found.to_string());
}

#[test]
fn cursor_errors_have_no_structured_code_via_batch_error_code() {
    // `BatchError::code()` only ever returns `Some` for `QueryError` — the
    // new cursor variants correctly fall through to `None` there. The
    // actual wire `code` string for these variants comes from
    // `shamir-server`'s `error_code()` classifier (`cursor_not_found` /
    // `cursor_expired` / `cursor_limit_exceeded`), not from this method.
    assert_eq!(
        BatchError::CursorNotFound {
            cursor_id: CursorId(1)
        }
        .code(),
        None
    );
    assert_eq!(
        BatchError::CursorExpired {
            cursor_id: CursorId(1)
        }
        .code(),
        None
    );
    assert_eq!(BatchError::CursorLimitExceeded { limit: 5 }.code(), None);
    assert_eq!(BatchError::CursorTemporalNotSupported.code(), None);
    assert_eq!(
        BatchError::InvalidPageSize {
            page_size: 0,
            max: 10_000
        }
        .code(),
        None
    );
    assert_eq!(
        BatchError::CursorPageTooLarge {
            size: 5_000_000,
            max: 1_000_000
        }
        .code(),
        None
    );
}
