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
}
