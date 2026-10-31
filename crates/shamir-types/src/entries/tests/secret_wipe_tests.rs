//! Drop-dispatch regression for [`SecretString`] (L3-RV-TYPES-FE40CB54).
//!
//! These tests exercise `Drop` dispatch — not just the `wipe` helper — via
//! the test-only `wipe_calls()` counter. They fail if `Drop` ever stops
//! calling `wipe`, and they read no freed memory and use no `unsafe`.

use super::{last_wiped_len, wipe, wipe_calls, SecretString};

#[test]
fn wipe_clears_live_string_without_releasing_capacity() {
    for text in ["", "пароль 🔐"] {
        let mut buffer = String::with_capacity(64);
        buffer.push_str(text);
        let capacity = buffer.capacity();
        wipe(&mut buffer);
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), capacity);
        // Safe String APIs cannot inspect the now-uninitialized spare capacity.
    }
}

/// Red on the old code: if `Drop` stops calling `wipe`, the counter stays
/// flat and both asserts below fail. Covers empty and Unicode secrets,
/// separately for a value and for its clone.
#[test]
fn drop_dispatches_wipe_exactly_once_for_value_and_clone() {
    for text in ["", "пароль 🔐"] {
        let before = wipe_calls();
        let value = SecretString::new(text.to_owned());
        let clone = value.clone();

        drop(value);
        assert_eq!(
            last_wiped_len(),
            Some(text.len()),
            "value Drop must wipe its original buffer length ({text:?})"
        );
        assert_eq!(
            wipe_calls(),
            before + 1,
            "dropping a SecretString must dispatch exactly one wipe ({text:?})"
        );

        drop(clone);
        assert_eq!(
            wipe_calls(),
            before + 2,
            "dropping a clone must also dispatch exactly one wipe ({text:?})"
        );
    }
}

/// Red on the old code: `into_inner` must hand back the full cleartext
/// (the caller now owns it) while the emptied `SecretString` is still
/// dropped exactly once — the counter must increment on that emptied
/// value's drop. If `Drop` stopped calling `wipe`, the counter assert
/// fails; if `into_inner` zeroized prematurely (or the drop read the
/// moved-out buffer), the cleartext assert fails.
#[test]
fn into_inner_returns_cleartext_and_emptied_drop_still_wipes_once() {
    for text in ["", "пароль 🔐"] {
        let before = wipe_calls();
        let secret = SecretString::new(text.to_owned());
        let inner = secret.into_inner(); // consumes `secret`; its emptied Drop runs here

        assert_eq!(inner, text, "into_inner must return the full cleartext");
        assert_eq!(
            last_wiped_len(),
            Some(0),
            "Drop must wipe only the emptied buffer"
        );
        assert_eq!(
            wipe_calls(),
            before + 1,
            "the emptied SecretString's drop must still dispatch exactly one wipe ({text:?})"
        );
    }
}
