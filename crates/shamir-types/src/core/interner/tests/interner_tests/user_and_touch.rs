//! `UserKey` / `TouchInd` wire-format and `touch_with_id` semantics tests.

use super::basic::arc_str;
use crate::core::interner::{Interner, InternerKey, UserKey};

#[test]
fn test_user_key_from_str_and_display() {
    let key = UserKey::from_str("hello");
    assert_eq!(key.as_str(), "hello");
    assert_eq!(key.to_string(), "hello");
}

#[test]
fn test_user_key_std_from_str() {
    let key: UserKey = "world".parse().unwrap();
    assert_eq!(key.as_str(), "world");
}

#[test]
fn test_user_key_as_ref() {
    let key = UserKey::from_str("test");
    let r: &str = key.as_ref();
    assert_eq!(r, "test");
}

#[test]
fn test_user_key_borrow_str() {
    use std::borrow::Borrow;
    let key = UserKey::from_str("borrowed");
    let b: &str = key.borrow();
    assert_eq!(b, "borrowed");
}

#[test]
fn test_user_key_equality() {
    let k1 = UserKey::from_str("abc");
    let k2 = UserKey::from_str("abc");
    let k3 = UserKey::from_str("xyz");
    assert_eq!(k1, k2);
    assert_ne!(k1, k3);
}

#[test]
fn test_user_key_serde_roundtrip() {
    let key = UserKey::from_str("serde_test");
    let bytes = rmp_serde::to_vec(&key).unwrap();
    let decoded: UserKey = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(key, decoded);
}

// ---------------------------------------------------------------------------
// TouchInd: AsRef<[u8]>, into_key
// ---------------------------------------------------------------------------

#[test]
fn test_touch_ind_wire_bytes() {
    let interner = Interner::new();
    let touch = interner.touch_ind("test").unwrap();
    let bytes = touch.key().to_wire_bytes();
    assert!(!bytes.is_empty());
}

#[test]
fn test_touch_ind_into_key() {
    let interner = Interner::new();
    let touch = interner.touch_ind("test").unwrap();
    let key = touch.into_key();
    assert_eq!(key.id(), 1);
}

// ---------------------------------------------------------------------------
// Interner::touch_with_id
// ---------------------------------------------------------------------------

#[test]
fn touch_with_id_fresh_insert() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    assert_eq!(interner.get_ind("email"), Some(InternerKey::new(5)));
    assert_eq!(
        interner.get_str(&InternerKey::new(5)),
        Some(arc_str("email"))
    );
}

#[test]
fn touch_with_id_idempotent() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    // Same call again — should be no-op.
    interner.touch_with_id("email", 5).unwrap();
    assert_eq!(interner.len(), 1);
}

#[test]
fn touch_with_id_conflict_different_id_for_known_name() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    let err = interner.touch_with_id("email", 10).unwrap_err();
    assert!(err.contains("already mapped"), "unexpected error: {err}");
}

#[test]
fn touch_with_id_collision_id_used_by_different_name() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    let err = interner.touch_with_id("score", 5).unwrap_err();
    assert!(err.contains("already used"), "unexpected error: {err}");
}

#[test]
fn touch_with_id_then_touch_ind_no_reuse() {
    let interner = Interner::new();
    // Pre-assign id 3 via touch_with_id.
    interner.touch_with_id("email", 3).unwrap();
    // touch_ind should allocate id > 3.
    let ti = interner.touch_ind("score").unwrap();
    assert!(
        ti.key().id() > 3,
        "touch_ind should not reuse id 3, got {}",
        ti.key().id()
    );
    // Both keys accessible.
    assert_eq!(interner.get_ind("email"), Some(InternerKey::new(3)));
    assert!(interner.get_ind("score").is_some());
}

// ---------------------------------------------------------------------------
// #501: doubling-Vec-of-OnceLock reverse spine — concurrent growth + gaps
// ---------------------------------------------------------------------------
