//! Integration tests for [`shamir_server::user_directory::RedbUserDirectory`].
//!
//! Covers the durability requirement (state survives restart), the
//! atomicity / monotonicity rules from spec §12.6, and concurrent insert
//! ID-uniqueness expectations.
//!
//! All tests use `tempfile::TempDir` so the redb file is cleaned up after
//! each run.

use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use shamir_server::user_directory::RedbUserDirectory;
use tempfile::TempDir;
use zeroize::Zeroizing;

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn fixture_record() -> UserRecord {
    let salt = [0xa1u8; 16];
    let stored = StoredKey([0xc3u8; 32]);
    let mut server_key = Zeroizing::new([0u8; 32]);
    for (i, b) in server_key.iter_mut().enumerate() {
        *b = i as u8;
    }
    UserRecord {
        salt,
        stored_key: stored,
        server_key,
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}

fn fresh_dir() -> (TempDir, RedbUserDirectory) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let store = RedbUserDirectory::open(&path).expect("open redb user dir");
    (dir, store)
}

fn assert_records_equal(actual: &UserRecord, expected: &UserRecord) {
    assert_eq!(actual.salt, expected.salt, "salt mismatch");
    assert_eq!(
        actual.stored_key.0, expected.stored_key.0,
        "stored_key mismatch"
    );
    assert_eq!(
        actual.server_key.as_slice(),
        expected.server_key.as_slice(),
        "server_key mismatch"
    );
    assert_eq!(
        actual.kdf_params, expected.kdf_params,
        "kdf_params mismatch"
    );
    assert_eq!(
        actual.tickets_invalid_before_ns, expected.tickets_invalid_before_ns,
        "tickets_invalid_before_ns mismatch"
    );
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[test]
fn roundtrip_insert_lookup() {
    let (_tmp, store) = fresh_dir();
    let record = fixture_record();
    let uid = store.insert("alice".to_string(), record.clone()).unwrap();

    let loaded = store.lookup_by_name("alice").expect("alice present");
    assert_records_equal(&loaded, &record);

    // user_id() must agree with the value insert returned.
    assert_eq!(store.user_id("alice"), Some(uid));
}

#[test]
fn lookup_unknown_returns_none() {
    let (_tmp, store) = fresh_dir();
    assert!(store.lookup_by_name("nobody").is_none());
    assert!(store.user_id("nobody").is_none());
    assert!(store.lookup_roles("nobody").is_none());
}

#[test]
fn insert_rejects_duplicate_username() {
    let (_tmp, store) = fresh_dir();
    let record = fixture_record();
    store.insert("alice".to_string(), record.clone()).unwrap();
    let err = store
        .insert("alice".to_string(), record)
        .expect_err("duplicate must reject");
    match err {
        shamir_connect::common::error::Error::InvalidInput(msg) => {
            assert_eq!(msg, "username exists");
        }
        other => panic!("expected InvalidInput(\"username exists\"), got {other:?}"),
    }
}

#[test]
fn update_roles_changes_roles_and_bumps_timestamp() {
    let (_tmp, store) = fresh_dir();
    store.insert("alice".to_string(), fixture_record()).unwrap();

    // Seed initial roles.
    let now_initial = 1_000_000u64;
    let changed = store
        .update_roles("alice", vec!["read_write".to_string()], now_initial)
        .unwrap();
    assert!(changed, "first role assignment must change state");
    assert_eq!(
        store.lookup_roles("alice"),
        Some(vec!["read_write".to_string()])
    );

    // Update to a different set with a higher timestamp.
    let now_ns = 2_000_000u64;
    let changed = store
        .update_roles("alice", vec!["superuser".to_string()], now_ns)
        .unwrap();
    assert!(changed, "role change must report change");

    // Roles persisted.
    assert_eq!(
        store.lookup_roles("alice"),
        Some(vec!["superuser".to_string()])
    );

    // Spec §12.6: timestamp must be at least the supplied `now_ns`.
    let rec = store.lookup_by_name("alice").unwrap();
    assert!(
        rec.tickets_invalid_before_ns >= now_ns,
        "tickets_invalid_before_ns should advance to >= {now_ns}, got {}",
        rec.tickets_invalid_before_ns
    );
}

#[test]
fn bump_tickets_invalid_does_not_change_roles() {
    let (_tmp, store) = fresh_dir();
    store.insert("alice".to_string(), fixture_record()).unwrap();
    store
        .update_roles("alice", vec!["read_write".to_string()], 1_000)
        .unwrap();

    let before = store.lookup_roles("alice").unwrap();

    let now_ns = 5_000u64;
    let bumped = store.bump_tickets_invalid("alice", now_ns).unwrap();
    assert!(bumped, "advancing timestamp should report change");

    let after = store.lookup_roles("alice").unwrap();
    assert_eq!(before, after, "roles must not change on bump-only op");

    let rec = store.lookup_by_name("alice").unwrap();
    assert_eq!(rec.tickets_invalid_before_ns, now_ns);

    // Monotonicity: re-bumping with an older timestamp is a no-op.
    let bumped_again = store.bump_tickets_invalid("alice", now_ns - 1).unwrap();
    assert!(!bumped_again, "non-monotonic bump must report no-change");
    let rec2 = store.lookup_by_name("alice").unwrap();
    assert_eq!(rec2.tickets_invalid_before_ns, now_ns);
}

/// **CRITICAL** durability test: after dropping the directory, reopening the
/// same file must surface the same record byte-identically. This is the spec
/// §3.5 / §6.2 fsync guarantee.
#[test]
fn state_survives_restart() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");

    let original = fixture_record();
    let initial_uid;
    let initial_roles = vec!["read_write".to_string()];

    {
        let store = RedbUserDirectory::open(&path).unwrap();
        initial_uid = store.insert("alice".to_string(), original.clone()).unwrap();
        // Stamp roles + ts so the persisted blob covers all fields.
        store
            .update_roles("alice", initial_roles.clone(), 12_345)
            .unwrap();
        // store dropped here → file flushed.
    }

    // Reopen & verify byte-identical record.
    {
        let store = RedbUserDirectory::open(&path).unwrap();
        let loaded = store
            .lookup_by_name("alice")
            .expect("alice must persist across restart");
        assert_eq!(loaded.salt, original.salt);
        assert_eq!(loaded.stored_key.0, original.stored_key.0);
        assert_eq!(loaded.server_key.as_slice(), original.server_key.as_slice());
        assert_eq!(loaded.kdf_params, original.kdf_params);
        assert_eq!(loaded.tickets_invalid_before_ns, 12_345);
        assert_eq!(store.user_id("alice"), Some(initial_uid));
        assert_eq!(store.lookup_roles("alice"), Some(initial_roles));
    }
}

#[test]
fn user_id_lookup_consistent_with_lookup_by_name() {
    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();

    // Both APIs must agree.
    assert!(store.lookup_by_name("alice").is_some());
    assert_eq!(store.user_id("alice"), Some(uid));

    // After update, identity stays the same.
    store
        .update_roles("alice", vec!["read_write".to_string()], 1_000)
        .unwrap();
    assert_eq!(store.user_id("alice"), Some(uid));
    assert!(store.lookup_by_name("alice").is_some());
}

#[test]
fn concurrent_inserts_assign_distinct_user_ids() {
    let (_tmp, store) = fresh_dir();
    let store = Arc::new(store);

    let mut handles = Vec::new();
    for i in 0..8 {
        let s = Arc::clone(&store);
        let handle = thread::spawn(move || {
            let username = format!("user_{i:02}");
            s.insert(username, fixture_record()).unwrap()
        });
        handles.push(handle);
    }

    let mut ids: HashSet<[u8; 16]> = HashSet::new();
    for h in handles {
        let uid = h.join().expect("thread panicked");
        assert!(ids.insert(uid), "user_id collision detected across threads");
    }
    assert_eq!(ids.len(), 8);
}
