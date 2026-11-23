//! Unit tests for [`crate::server_meta::ServerMetaStore`]'s bootstrap-token
//! lifecycle fields/getters (RI-9: username, token_path, expiry).
//!
//! This is a fast `src/tests/` unit file — no server boot, just
//! `ServerMetaStore::open_or_init` against a `tempfile::TempDir`-backed
//! path. The slower integration coverage (crash/reopen simulation across
//! many rotations) lives in `crates/server/shamir-server/tests/server_meta.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serial_test::serial;
use tempfile::TempDir;

use crate::boot::config::diagnostics::backup_online::metrics::{
    backup_metrics_snapshot, install_backup_meta_store, set_export_last_success_for_test,
    ClockOverride,
};
use crate::server_meta::{MetaError, ServerMetaStore};

fn tmp_store() -> (TempDir, ServerMetaStore) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    let store = ServerMetaStore::open_or_init(&path).expect("open_or_init");
    (dir, store)
}

/// Reopen a store path, retrying ONLY the documented fjall file-lock release
/// race (`MetaError::Fjall(fjall::Error::Locked)` — the exact match
/// `boot::config::diagnostics::restore` uses for its liveness probe). Bounded
/// and sleep-free: `std::thread::yield_now()` between attempts.
fn reopen_with_retry(path: &Path) -> ServerMetaStore {
    const ATTEMPTS: usize = 10_000;
    for _ in 0..ATTEMPTS {
        match ServerMetaStore::open_or_init(path) {
            Ok(store) => return store,
            Err(MetaError::Fjall(fjall::Error::Locked)) => std::thread::yield_now(),
            Err(other) => panic!("reopen {path:?} failed with a non-lock error: {other}"),
        }
    }
    panic!("server_meta at {path:?} stayed locked across {ATTEMPTS} yields");
}

#[test]
fn http_login_listener_id_is_stable_across_reopen_and_address_scoped() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    let first = ServerMetaStore::open_or_init(&path).expect("open");
    let id = first.http_login_listener_id("127.0.0.1:7340").expect("id");
    drop(first);
    let reopened = ServerMetaStore::open_or_init(&path).expect("reopen");
    assert_eq!(
        reopened.http_login_listener_id("127.0.0.1:7340").unwrap(),
        id
    );
    assert_ne!(
        reopened.http_login_listener_id("127.0.0.1:7341").unwrap(),
        id
    );
}

#[test]
fn http_login_listener_id_rejects_malformed_persisted_length() {
    let (_dir, store) = tmp_store();
    store.set_http_login_listener_id_for_test("broken", &[1, 2, 3]);
    assert!(matches!(
        store.http_login_listener_id("broken"),
        Err(crate::server_meta::MetaError::Encoding(_))
    ));
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

/// Brief item 4: the last-successful-backup stamp must be durable, so a
/// process restarted before the first 24 h tick still exposes staleness.
///
/// Fixed epoch-seconds constant, no wall-clock read, no sleep. Mutants
/// caught: (a) a setter that never inserts the row → the reopen reads `None`;
/// (b) a setter that persists nanoseconds, `0`, or any other derived value →
/// both the pre-drop and the post-reopen assertions see the wrong number;
/// (c) an accessor that maps an absent row to `Some(0)` → the fresh-store
/// assertion fails. Nothing here touches the process-global metrics state
/// (export cell, durable handle, clock), so the test is order-independent.
#[test]
fn backup_last_success_stamp_survives_reopen() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    // 2025-12-09T00:00:00Z, arbitrary but fixed: no clock comparison anywhere.
    let stamp_seconds = 1_765_238_400u64;

    let store = ServerMetaStore::open_or_init(&path).expect("open");
    assert_eq!(
        store.backup_last_success_unix_ns().expect("read absent"),
        None,
        "absent row must be None, not Some(0)"
    );
    store
        .store_backup_last_success_unix_ns(stamp_seconds)
        .expect("store stamp");
    assert_eq!(
        store.backup_last_success_unix_ns().expect("read back"),
        Some(stamp_seconds)
    );
    // The first handle must be fully gone before the path is reopened (fjall
    // holds the file lock until every handle on it drops).
    drop(store);

    let reopened = reopen_with_retry(&path);
    assert_eq!(
        reopened
            .backup_last_success_unix_ns()
            .expect("read reopened"),
        Some(stamp_seconds),
        "stamp must survive a restart"
    );
    drop(reopened);
    assert!(dir.path().exists(), "the TempDir stays alive for the test");
}

/// Brief item B: the armed-at row is the restart-independent evidence when no
/// copy has ever succeeded. Reopening proves the write (SyncAll) is durable and
/// that removing the row is durable too, so re-enabling later starts a fresh
/// clock. Every `TempDir` stays alive for the whole test.
///
/// Mutants caught: (a) a setter that never persists → the reopen reads `None`;
/// (b) a remover that never persists → the second reopen reads the old stamp;
/// (c) a getter that maps an absent row to `Some(0)` → the fresh-store
/// assertion fails.
#[test]
fn armed_at_stamp_survives_reopen_and_removal_survives_reopen() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    // Fixed epoch seconds; no clock comparison anywhere.
    let armed_at = 1_765_238_400u64;

    let store = ServerMetaStore::open_or_init(&path).expect("open");
    let absent = store.backup_armed_at_unix_secs().expect("absent");
    assert_eq!(absent, None, "absent row must be None, not Some(0)");
    store
        .store_backup_armed_at_unix_secs(armed_at)
        .expect("store armed-at");
    let read_back = store.backup_armed_at_unix_secs().expect("back");
    assert_eq!(read_back, Some(armed_at));
    drop(store);

    let reopened = reopen_with_retry(&path);
    let stamp = reopened.backup_armed_at_unix_secs().expect("reopened");
    assert_eq!(stamp, Some(armed_at), "the arming stamp survives a restart");
    reopened
        .remove_backup_armed_at_unix_secs()
        .expect("remove armed-at");
    let removed = reopened.backup_armed_at_unix_secs().expect("removed");
    assert_eq!(removed, None);
    drop(reopened);

    let reopened_again = reopen_with_retry(&path);
    let after_removal = reopened_again
        .backup_armed_at_unix_secs()
        .expect("after removal");
    assert_eq!(
        after_removal, None,
        "removing the row must survive a restart (re-enabling starts a fresh clock)"
    );
    drop(reopened_again);
    assert!(dir.path().exists(), "the TempDir stays alive for the test");
}

/// Brief item B (P1): with `run_on_start = false` and a 24 h interval, a
/// process restarted every 12 h reaches no tick and never succeeds — the
/// exported stamp must still be the FIRST arming time, not the latest boot's,
/// and a boot with no schedule must clear the row.
///
/// Coverage: this drives `install_backup_meta_store` — the exact call the
/// production launch path makes once, in `ServerLauncher::launch_inner`
/// (`server/server_launcher/launch.rs`), with `backup.is_some()` as the
/// schedule flag — against a REAL `ServerMetaStore` reopened between boots.
/// It does NOT construct a `ServerHandle` or spawn the scheduler loop: "no
/// tick" is modelled by never calling the runner, which is exactly what the
/// defect scenario observes.
///
/// Mutants caught: (a) a boot that re-arms (overwrites) the row → the export
/// and the row move to the second boot time; (b) a boot with no schedule that
/// leaves the row → the removal assertion fails; (c) a seed that keeps a stale
/// cell value when nothing is armed or stored → the final `0` assertion fails.
#[serial]
#[test]
fn arming_stamp_survives_a_second_boot_without_a_tick() {
    set_export_last_success_for_test(0);
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta");
    let armed_at = 1_765_238_400u64;
    let twelve_hours = 12 * 60 * 60;

    // Boot 1: schedule configured, no success, no tick.
    let clock = ClockOverride::set(armed_at);
    let first = Arc::new(reopen_with_retry(&path));
    install_backup_meta_store(Arc::clone(&first), true);
    let armed = first.backup_armed_at_unix_secs().expect("armed");
    assert_eq!(armed, Some(armed_at), "the first boot must arm the row");
    let exported = backup_metrics_snapshot().last_success_timestamp_seconds;
    assert_eq!(
        exported, armed_at,
        "with no success the export is the armed-at"
    );
    drop(first);

    // Boot 2, 12 h later: run_on_start = false, a 24 h interval, NO tick.
    clock.pin(armed_at + twelve_hours);
    let second = Arc::new(reopen_with_retry(&path));
    install_backup_meta_store(Arc::clone(&second), true);
    let rearmed = second.backup_armed_at_unix_secs().expect("armed");
    assert_eq!(
        rearmed,
        Some(armed_at),
        "a later boot must NOT rewrite the first arming stamp"
    );
    let exported = backup_metrics_snapshot().last_success_timestamp_seconds;
    assert_eq!(
        exported, armed_at,
        "no tick and no success at all: the export must be the FIRST arming time"
    );
    drop(second);

    // Boot 3 with NO schedule: the row is removed so a later re-enable starts
    // a fresh clock.
    clock.pin(armed_at + 2 * twelve_hours);
    let third = Arc::new(reopen_with_retry(&path));
    install_backup_meta_store(Arc::clone(&third), false);
    let removed = third.backup_armed_at_unix_secs().expect("removed");
    assert_eq!(removed, None, "no schedule must remove the armed-at row");
    let exported = backup_metrics_snapshot().last_success_timestamp_seconds;
    assert_eq!(exported, 0, "no row and no success means no evidence");
    drop(third);

    drop(clock);
    set_export_last_success_for_test(0);
    assert!(dir.path().exists(), "the TempDir stays alive for the test");
}
