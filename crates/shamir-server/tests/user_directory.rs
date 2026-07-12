//! Integration tests for [`shamir_server::user_directory::FjallUserDirectory`].
//!
//! Covers the durability requirement (state survives restart), the
//! atomicity / monotonicity rules from spec §12.6, and concurrent insert
//! ID-uniqueness expectations.
//!
//! All tests use `tempfile::TempDir` so the redb file is cleaned up after
//! each run.

use shamir_collections::TFxSet;
use std::sync::Arc;
use std::thread;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use shamir_server::user_directory::FjallUserDirectory;
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

fn fresh_dir() -> (TempDir, FjallUserDirectory) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let store = FjallUserDirectory::open(&path).expect("open redb user dir");
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
    assert!(store.lookup_roles("nobody").unwrap().is_none());
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
        store.lookup_roles("alice").unwrap(),
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
        store.lookup_roles("alice").unwrap(),
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

    let before = store.lookup_roles("alice").unwrap().unwrap();

    let now_ns = 5_000u64;
    let bumped = store.bump_tickets_invalid("alice", now_ns).unwrap();
    assert!(bumped, "advancing timestamp should report change");

    let after = store.lookup_roles("alice").unwrap().unwrap();
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
        let store = FjallUserDirectory::open(&path).unwrap();
        initial_uid = store.insert("alice".to_string(), original.clone()).unwrap();
        // Stamp roles + ts so the persisted blob covers all fields.
        store
            .update_roles("alice", initial_roles.clone(), 12_345)
            .unwrap();
        // store dropped here → file flushed.
    }

    // Reopen & verify byte-identical record.
    {
        let store = FjallUserDirectory::open(&path).unwrap();
        let loaded = store
            .lookup_by_name("alice")
            .expect("alice must persist across restart");
        assert_eq!(loaded.salt, original.salt);
        assert_eq!(loaded.stored_key.0, original.stored_key.0);
        assert_eq!(loaded.server_key.as_slice(), original.server_key.as_slice());
        assert_eq!(loaded.kdf_params, original.kdf_params);
        assert_eq!(loaded.tickets_invalid_before_ns, 12_345);
        assert_eq!(store.user_id("alice"), Some(initial_uid));
        assert_eq!(store.lookup_roles("alice").unwrap(), Some(initial_roles));
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
fn tickets_invalid_before_ns_lookup_by_user_id_reflects_updates() {
    // Spec §7.5: changing roles must bump tickets_invalid_before_ns so the
    // connection layer can invalidate live sessions on the next request.
    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();

    // Fresh user — no invalidation yet.
    assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid), 0);

    // Bumping via update_roles writes the new timestamp; lookup MUST see it.
    store
        .update_roles("alice", vec!["read_write".to_string()], 12_345)
        .unwrap();
    assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid), 12_345);

    // Direct bump_tickets_invalid also visible.
    store.bump_tickets_invalid("alice", 99_999).unwrap();
    assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid), 99_999);

    // Unknown user_id → 0 (fail-open default; spec §7.5 treats 0 as no invalidation).
    let unknown_uid = [0xFFu8; 16];
    assert_eq!(store.tickets_invalid_before_ns_by_user_id(&unknown_uid), 0);
}

#[test]
fn user_id_index_survives_restart() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let uid = {
        let store = FjallUserDirectory::open(&path).unwrap();
        let uid = store.insert("alice".to_string(), fixture_record()).unwrap();
        store.bump_tickets_invalid("alice", 7_777).unwrap();
        uid
    };
    {
        let store = FjallUserDirectory::open(&path).unwrap();
        assert_eq!(
            store.tickets_invalid_before_ns_by_user_id(&uid),
            7_777,
            "secondary index must survive restart"
        );
    }
}

/// **SECURITY-CRITICAL**: A revocation must survive a restart — i.e. the
/// in-memory cache is hydrated from the durable store at `open` time, not
/// left empty (an empty cache would return 0 = "no invalidation" = revoked
/// ticket accepted).  This is the single test that catches a cache that
/// fails to warm.
#[test]
fn revocation_survives_restart_via_cache_hydration() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let uid = {
        let store = FjallUserDirectory::open(&path).unwrap();
        let uid = store.insert("alice".to_string(), fixture_record()).unwrap();
        // Revoke all tickets issued before 100_000.
        store.bump_tickets_invalid("alice", 100_000).unwrap();
        // Verify the cache reflects the revocation immediately.
        assert_eq!(
            store.tickets_invalid_before_ns_by_user_id(&uid),
            100_000,
            "cache must reflect revocation before restart"
        );
        uid
    };
    // Reopen — the cache must be hydrated from the durable store so the
    // revocation is still visible without any additional write.
    {
        let store = FjallUserDirectory::open(&path).unwrap();
        assert_eq!(
            store.tickets_invalid_before_ns_by_user_id(&uid),
            100_000,
            "cache must be hydrated from durable store on restart — \
             a stale/empty cache would return 0 and accept a revoked ticket"
        );
    }
}

/// Multiple users with different revocation timestamps must each be hydrated
/// correctly — a partial or last-write-wins warm would fail here.
#[test]
fn restart_hydrates_multiple_users_independently() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");

    let (uid_a, uid_b, uid_c) = {
        let store = FjallUserDirectory::open(&path).unwrap();
        let uid_a = store.insert("alice".to_string(), fixture_record()).unwrap();
        let uid_b = store.insert("bob".to_string(), fixture_record()).unwrap();
        let uid_c = store.insert("carol".to_string(), fixture_record()).unwrap();

        store.bump_tickets_invalid("alice", 111).unwrap();
        store.bump_tickets_invalid("bob", 222_222).unwrap();
        // carol is never bumped — should read 0.
        (uid_a, uid_b, uid_c)
    };

    {
        let store = FjallUserDirectory::open(&path).unwrap();
        assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid_a), 111);
        assert_eq!(store.tickets_invalid_before_ns_by_user_id(&uid_b), 222_222);
        assert_eq!(
            store.tickets_invalid_before_ns_by_user_id(&uid_c),
            0,
            "user with no revocation must read 0 after restart"
        );
    }
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

    let mut ids: TFxSet<[u8; 16]> = TFxSet::default();
    for h in handles {
        let uid = h.join().expect("thread panicked");
        assert!(ids.insert(uid), "user_id collision detected across threads");
    }
    assert_eq!(ids.len(), 8);
}

// ----------------------------------------------------------------------------
// Task #556 — directory v2: boot normalization, remove(), state_by_user_id
// ----------------------------------------------------------------------------

/// Helper: open the directory at `path` (mirrors the test-local reopen idiom).
fn reopen(path: &std::path::Path) -> FjallUserDirectory {
    FjallUserDirectory::open(path).expect("reopen user dir")
}

/// **Red test #1 — boot normalization idempotence.**
///
/// Seeds a user whose role list still carries the legacy `"superuser"`
/// string (the shape pre-#556 data has, and the shape `update_roles`
/// produces until normalization runs), then reopens the directory to
/// trigger normalization, and asserts:
///   - the `"superuser"` string is migrated into the `superuser` flag,
///   - the role list no longer contains the string,
///   - reopening a second time changes nothing further (idempotence).
///
/// The `principal64_index` is a pure re-projection of the immutable
/// `user_id` each boot, so byte-identical contents across reopens is
/// structurally guaranteed and unit-tested separately in
/// `project_user_ids_*`; here we assert the observable effect: both users
/// still resolve through `state_by_user_id` identically after each reopen.
#[test]
fn normalization_migrates_superuser_role_string_and_is_idempotent() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");

    let (alice_uid, bob_uid) = {
        let store = reopen(&path);
        let alice_uid = store.insert("alice".to_string(), fixture_record()).unwrap();
        let bob_uid = store.insert("bob".to_string(), fixture_record()).unwrap();
        // Seed the legacy shape: "superuser" carried as a role STRING, flag
        // is false (pre-#556 persisted blobs lacked the field entirely;
        // `update_roles` reproduces the roles-list side of that shape).
        store
            .update_roles("alice", vec!["superuser".to_string()], 1_000)
            .unwrap();
        (alice_uid, bob_uid)
    };

    // First normalization pass. (Each reopen is in its own block so the
    // fjall directory lock is released on drop before the next open —
    // shadowing `store` would NOT drop the prior handle.)
    let first_alice = {
        let store = reopen(&path);
        let alice = store
            .state_by_user_id(&alice_uid)
            .expect("alice resolves after normalization");
        assert!(
            alice.superuser,
            "the `superuser` role string must be migrated into the flag"
        );
        assert!(
            !alice.roles.iter().any(|r| r == "superuser"),
            "the `superuser` string must be removed from roles after migration; got {:?}",
            alice.roles
        );
        // lookup_roles synthesises the "superuser" string back from the flag
        // (transitional compat: SessionPermissions::from_roles still keys off
        // the string until task #557 rewires it to the flag). The PERSISTED
        // roles (read via state_by_user_id) are empty — only the lookup API
        // bridges the two.
        assert_eq!(
            store.lookup_roles("alice").unwrap(),
            Some(vec!["superuser".to_string()])
        );
        // Bob was never a superuser — must stay a normal account.
        let bob = store.state_by_user_id(&bob_uid).expect("bob resolves");
        assert!(!bob.superuser);
        alice
    };

    // Second normalization pass: nothing changes (idempotence).
    {
        let store = reopen(&path);
        let alice_again = store
            .state_by_user_id(&alice_uid)
            .expect("alice still resolves after second normalization");
        assert!(alice_again.superuser, "flag must survive a second boot");
        assert!(
            !alice_again.roles.iter().any(|r| r == "superuser"),
            "roles must stay migrated after a second boot"
        );
        assert_eq!(
            first_alice.roles, alice_again.roles,
            "roles stable across boots"
        );
        assert_eq!(
            first_alice.tickets_invalid_before_ns, alice_again.tickets_invalid_before_ns,
            "tib stable across boots"
        );
        assert!(store.state_by_user_id(&bob_uid).is_some());
    }
}

/// **Red test #2 — unknown-user resume rejected (fail-open fix).**
///
/// `state_by_user_id` (the primitive the `RedbUserStateLookup` adapter wraps)
/// must return `None` for a never-inserted user_id, while a REAL inserted
/// user — even one with `tickets_invalid_before_ns == 0` (the default) —
/// must return `Some` with tib=0. This proves the fix distinguishes
/// "unknown" from "known-but-zero" rather than collapsing both to `None`
/// or both to `Some(0)` (the old fail-open behaviour). The adapter itself
/// is exercised directly in `src/tests/user_state_lookup_tests`.
#[test]
fn state_by_user_id_distinguishes_unknown_from_known_but_zero() {
    let (_tmp, store) = fresh_dir();
    let alice_uid = store.insert("alice".to_string(), fixture_record()).unwrap();

    // Known user, tib == 0 (default) → Some(tib=0), NOT None.
    let alice_state = store
        .state_by_user_id(&alice_uid)
        .expect("a known user with tib=0 must resolve, not collapse to None");
    assert_eq!(
        alice_state.tickets_invalid_before_ns, 0,
        "known-but-zero must read 0, not be treated as unknown"
    );
    assert_eq!(alice_state.username, "alice");

    // Unknown user_id → None (resume must reject).
    let unknown = [0xFFu8; 16];
    assert!(
        store.state_by_user_id(&unknown).is_none(),
        "an unknown user_id must resolve to None so resume rejects it"
    );
}

/// **Red test #3 — last-superuser removal refused, then allowed with two.**
///
/// With exactly one superuser account, `remove()` must refuse (typed error)
/// and leave the account intact. After a second superuser exists, removing
/// the first must succeed — proving the guard is a genuine COUNT check, not
/// an unconditional "superuser accounts are undeletable" block.
#[test]
fn remove_refuses_last_superuser_then_succeeds_with_two() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");

    // Seed a single superuser account (the role-string → flag migration
    // runs on reopen).
    {
        let store = reopen(&path);
        store.insert("admin".to_string(), fixture_record()).unwrap();
        store
            .update_roles("admin", vec!["superuser".to_string()], 1_000)
            .unwrap();
    }
    // Each reopen below is its own block so the fjall directory lock
    // releases on drop before the next open (shadowing `store` would not
    // drop the prior handle → fjall returns `Locked`).
    {
        let store = reopen(&path);
        // Sanity: normalization promoted admin to a superuser.
        let admin_uid = store.user_id("admin").expect("admin present");
        assert!(
            store
                .state_by_user_id(&admin_uid)
                .expect("admin resolves")
                .superuser,
            "admin must be a superuser after normalization"
        );

        // Last-superuser guard fires.
        let err = store
            .remove("admin")
            .expect_err("removing the last superuser must be refused");
        match err {
            shamir_connect::common::error::Error::InvalidInput(msg) => {
                assert!(
                    msg.contains("last remaining superuser"),
                    "wrong InvalidInput message: {msg}"
                );
            }
            other => panic!("expected InvalidInput for last-superuser removal, got {other:?}"),
        }
        // Account is still present.
        assert!(
            store.lookup_by_name("admin").is_some(),
            "the refused-remove target must still exist"
        );
    }

    // Add a SECOND superuser, then the first removal must succeed.
    {
        let store2 = reopen(&path);
        store2
            .insert("admin2".to_string(), fixture_record())
            .unwrap();
        store2
            .update_roles("admin2", vec!["superuser".to_string()], 2_000)
            .unwrap();
    }
    {
        let store = reopen(&path);
        let removed = store
            .remove("admin")
            .expect("removal with two superusers succeeds");
        assert!(removed, "remove must report it deleted the account");
        assert!(
            store.lookup_by_name("admin").is_none(),
            "admin must be gone after a successful remove"
        );
        assert!(
            store.lookup_by_name("admin2").is_some(),
            "the second superuser must survive"
        );
    }
}

/// `remove()` must evict the `tickets_cache` entry — otherwise a deleted
/// account's `user_id` would still resolve to a stale `tib` value via
/// `tickets_invalid_before_ns_by_user_id`, reopening the fail-open bug the
/// `UserStateLookup` fix closes through a different path. (Definition of
/// done: cache eviction is covered by a test, not just present in the diff.)
#[test]
fn remove_evicts_tickets_cache() {
    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();
    store.bump_tickets_invalid("alice", 99_999).unwrap();
    assert_eq!(
        store.tickets_invalid_before_ns_by_user_id(&uid),
        99_999,
        "precondition: tib is cached"
    );

    assert!(store.remove("alice").expect("remove known user"), "removed");
    // Cache entry must be gone → hot-path lookup falls back to 0 (miss),
    // and the authoritative reverse lookup returns None.
    assert_eq!(
        store.tickets_invalid_before_ns_by_user_id(&uid),
        0,
        "stale cache entry must NOT survive remove (fail-open bug)"
    );
    assert!(
        store.state_by_user_id(&uid).is_none(),
        "removed user must not resolve via state_by_user_id"
    );
}

/// `remove()` is idempotent for an already-absent username (no-op, Ok(false)).
#[test]
fn remove_is_idempotent_for_unknown_user() {
    let (_tmp, store) = fresh_dir();
    let removed = store.remove("ghost").expect("remove unknown is Ok(false)");
    assert!(
        !removed,
        "removing a never-inserted user is an idempotent no-op"
    );
}

/// **`insert` maintains the third keyspace.** A round-trip insert +
/// reopen must keep the account resolvable and the principal64 index
/// consistent (the projection is re-derived each boot, so a second open
/// must not produce a collision against the existing entry).
#[test]
fn insert_populates_principal64_index_surviving_restart() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");
    let uid = {
        let store = reopen(&path);
        store.insert("alice".to_string(), fixture_record()).unwrap()
    };
    // Reopen re-derives the index from user_id_index; the entry must not
    // collide with itself and the account must still resolve.
    let store = reopen(&path);
    assert!(
        store.state_by_user_id(&uid).is_some(),
        "account must resolve after restart (principal64 index rebuilt)"
    );
    // Inserting a brand-new user after reopen must succeed — the rebuilt
    // index correctly rejects only genuinely-taken projections.
    let bob_uid = store
        .insert("bob".to_string(), fixture_record())
        .expect("new insert after reopen succeeds");
    assert_ne!(uid, bob_uid, "distinct user_ids");
}
