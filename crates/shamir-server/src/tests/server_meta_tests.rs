//! Unit tests for [`crate::server_meta::ServerMetaStore`]'s bootstrap-token
//! lifecycle fields/getters (RI-9: username, token_path, expiry).
//!
//! This is a fast `src/tests/` unit file — no server boot, just
//! `ServerMetaStore::open_or_init` against a `tempfile::TempDir`-backed
//! path. The slower integration coverage (crash/reopen simulation across
//! many rotations) lives in `crates/shamir-server/tests/server_meta.rs`.

use std::path::PathBuf;

use tempfile::TempDir;

use crate::server_meta::ServerMetaStore;

fn tmp_store() -> (TempDir, ServerMetaStore) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    let store = ServerMetaStore::open_or_init(&path).expect("open_or_init");
    (dir, store)
}

#[test]
fn fresh_store_returns_false_none_not_panic() {
    let (_dir, store) = tmp_store();

    assert!(!store.bootstrap_token_active());
    assert_eq!(store.bootstrap_username(), None);
    assert_eq!(store.bootstrap_token_path(), None);
    assert!(!store.bootstrap_token_expired(u64::MAX));
    assert!(!store.superuser_ever_existed());
}

#[test]
fn set_bootstrap_token_round_trips_username_and_path() {
    let (_dir, store) = tmp_store();

    let hash = [0x42u8; 32];
    let expires_at_ns = 5_000_000_000_000_000_000u64;
    let token_path = PathBuf::from("/run/shamir/bootstrap_token.txt");

    store
        .set_bootstrap_token("admin", hash, expires_at_ns, token_path.clone())
        .expect("set_bootstrap_token");

    assert!(store.bootstrap_token_active());
    assert_eq!(store.bootstrap_username(), Some("admin".to_string()));
    assert_eq!(store.bootstrap_token_path(), Some(token_path));
}

#[test]
fn consume_bootstrap_token_clears_all_three_fields_and_sets_sticky_flag() {
    let (_dir, store) = tmp_store();

    let hash = [0x11u8; 32];
    let expires_at_ns = 5_000_000_000_000_000_000u64;
    let token_path = PathBuf::from("/run/shamir/bootstrap_token.txt");
    store
        .set_bootstrap_token("root", hash, expires_at_ns, token_path)
        .expect("set_bootstrap_token");
    assert!(store.bootstrap_token_active());
    assert!(!store.superuser_ever_existed());

    store.consume_bootstrap_token().expect("consume");

    assert!(!store.bootstrap_token_active());
    assert_eq!(store.bootstrap_username(), None);
    assert_eq!(store.bootstrap_token_path(), None);
    assert!(store.superuser_ever_existed());
}

#[test]
fn bootstrap_token_expired_false_before_true_at_and_after_expiry() {
    let (_dir, store) = tmp_store();

    let hash = [0x33u8; 32];
    let expires_at_ns = 1_000_000_000_000u64;
    let token_path = PathBuf::from("/run/shamir/bootstrap_token.txt");
    store
        .set_bootstrap_token("admin", hash, expires_at_ns, token_path)
        .expect("set_bootstrap_token");

    // Strictly before expiry.
    assert!(!store.bootstrap_token_expired(expires_at_ns - 1));
    // At expiry — `<=` per spec, so this counts as expired.
    assert!(store.bootstrap_token_expired(expires_at_ns));
    // After expiry.
    assert!(store.bootstrap_token_expired(expires_at_ns + 1));
}

#[test]
fn bootstrap_token_expired_false_when_no_token_outstanding() {
    let (_dir, store) = tmp_store();
    // No token was ever set — must not report "expired" (that would imply
    // a token existed at all).
    assert!(!store.bootstrap_token_expired(u64::MAX));
}
