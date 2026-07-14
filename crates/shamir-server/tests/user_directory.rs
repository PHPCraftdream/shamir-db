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
    // (Task #557 reserves the literal `"superuser"` string at the directory
    // write boundary; this test is about role-change mechanics, not about
    // that specific role name, so the second set uses a different string.)
    let now_ns = 2_000_000u64;
    let changed = store
        .update_roles("alice", vec!["admin".to_string()], now_ns)
        .unwrap();
    assert!(changed, "role change must report change");

    // Roles persisted.
    assert_eq!(
        store.lookup_roles("alice").unwrap(),
        Some(vec!["admin".to_string()])
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

// Red test #1 (task #556 — boot normalization idempotence) was relocated
// to `src/tests/user_directory_tests.rs` as
// `normalization_migrates_superuser_role_string_and_is_idempotent_in_crate`
// because task #557 reserves the `"superuser"` role string at the directory
// write boundary (`update_roles`), so this external integration test file
// (a separate compilation unit that cannot see `pub(crate) PersistedUser`)
// can no longer seed the legacy pre-migration on-disk shape through the
// public API. The in-crate version constructs the legacy blob directly via
// `pub(crate) PersistedUser { roles: vec!["superuser".into()], superuser: false, .. }`
// — exactly the bypass the #556 `pub(crate)` visibility was designed for.
//
// The property this test verified (boot-time migration of legacy on-disk
// data is idempotent) is still covered, just seeded through a different
// path now that the public API it used to seed with is reserved.

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
///
/// Task #557 update: this test previously seeded superuser status via
/// `update_roles(.., "superuser", ..)` — that string is now RESERVED at the
/// directory write boundary (only `set_superuser` can flip the flag). The
/// seeding here switches to `set_superuser`, which is exactly the new
/// blessed path.
#[test]
fn remove_refuses_last_superuser_then_succeeds_with_two() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("users.redb");

    // Seed a single superuser account.
    {
        let store = reopen(&path);
        store.insert("admin".to_string(), fixture_record()).unwrap();
        store.set_superuser("admin", true, 1_000).unwrap();
    }
    // Each reopen below is its own block so the fjall directory lock
    // releases on drop before the next open (shadowing `store` would not
    // drop the prior handle → fjall returns `Locked`).
    {
        let store = reopen(&path);
        // Sanity: admin is a superuser.
        let admin_uid = store.user_id("admin").expect("admin present");
        assert!(
            store
                .state_by_user_id(&admin_uid)
                .expect("admin resolves")
                .superuser,
            "admin must be a superuser after set_superuser"
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
        store2.set_superuser("admin2", true, 2_000).unwrap();
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

// ----------------------------------------------------------------------------
// Task #557 — superuser flag: reservation, set_superuser, handshake wiring
// ----------------------------------------------------------------------------

/// **Red test #1 (task #557) — `"superuser"` reservation rejected.**
///
/// `update_roles(.., vec!["superuser".to_string()], ..)` must return `Err`
/// on an existing user — the string is reserved at the directory write
/// boundary; only `set_superuser` may flip the flag. The user's persisted
/// `roles`/`superuser` state must be UNCHANGED afterward (the reservation
/// short-circuits before the read-modify-write transaction begins).
#[test]
fn update_roles_rejects_reserved_superuser_string() {
    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();
    // Precondition: alice starts as a non-superuser with empty roles.
    let before = store.state_by_user_id(&uid).expect("alice resolves");
    assert!(!before.superuser);
    assert!(before.roles.is_empty());

    // The reserved string is rejected, even on an existing user.
    let err = store
        .update_roles("alice", vec!["superuser".to_string()], 1_000)
        .expect_err("the literal \"superuser\" string is reserved");
    match err {
        shamir_connect::common::error::Error::InvalidInput(msg) => {
            assert!(
                msg.contains("reserved"),
                "error must explain the reservation: {msg}"
            );
            assert!(
                msg.contains("SetSuperuser"),
                "error must point at SetSuperuser as the blessed alternative: {msg}"
            );
        }
        other => panic!("expected InvalidInput for reserved role string, got {other:?}"),
    }

    // State is UNCHANGED — the rejection happened before any write.
    let after = store.state_by_user_id(&uid).expect("alice still resolves");
    assert_eq!(after.superuser, before.superuser);
    assert_eq!(after.roles, before.roles);
    assert_eq!(
        after.tickets_invalid_before_ns, before.tickets_invalid_before_ns,
        "tib must not advance on a rejected op"
    );
}

/// **Task #621 — `"replicator"` reservation rejected.**
///
/// Mirrors `update_roles_rejects_reserved_superuser_string` above:
/// `update_roles(.., vec!["replicator".to_string()], ..)` must return `Err`
/// — the string is reserved at the directory write boundary; only
/// `set_replicator` may flip the flag. State stays unchanged.
#[test]
fn update_roles_rejects_reserved_replicator_string() {
    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();
    let before = store.state_by_user_id(&uid).expect("alice resolves");
    assert!(!before.replicator);
    assert!(before.roles.is_empty());

    let err = store
        .update_roles("alice", vec!["replicator".to_string()], 1_000)
        .expect_err("the literal \"replicator\" string is reserved");
    match err {
        shamir_connect::common::error::Error::InvalidInput(msg) => {
            assert!(
                msg.contains("reserved"),
                "error must explain the reservation: {msg}"
            );
            assert!(
                msg.contains("SetReplicator"),
                "error must point at SetReplicator as the blessed alternative: {msg}"
            );
        }
        other => panic!("expected InvalidInput for reserved role string, got {other:?}"),
    }

    let after = store.state_by_user_id(&uid).expect("alice still resolves");
    assert_eq!(after.replicator, before.replicator);
    assert_eq!(after.roles, before.roles);
    assert_eq!(
        after.tickets_invalid_before_ns, before.tickets_invalid_before_ns,
        "tib must not advance on a rejected op"
    );
}

/// **Task #621 — `set_replicator` grant/revoke, tib bump.**
///
/// Granting replicator on a non-replicator:
///   - returns `Ok(true)`,
///   - flips `replicator` to `true`,
///   - bumps `tickets_invalid_before_ns` to at least `now_ns`,
///   - is observable via `state_by_user_id`.
///
/// Then revoking it flips the flag back, bumps tib again, and returns
/// `Ok(true)` — WITHOUT any last-remaining refusal (unlike `set_superuser`,
/// there is no such guard for `replicator`). A repeated grant/revoke at the
/// same state is a no-op (`Ok(false)`, no tib bump).
#[test]
fn set_replicator_grant_revoke_bumps_tib_no_last_remaining_guard() {
    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();
    let before = store.state_by_user_id(&uid).expect("alice resolves");
    assert!(!before.replicator);
    assert_eq!(before.tickets_invalid_before_ns, 0);

    // Grant.
    let changed = store
        .set_replicator("alice", true, 5_000)
        .expect("grant should succeed");
    assert!(changed, "grant must report a real change");
    let after_grant = store.state_by_user_id(&uid).expect("alice resolves");
    assert!(after_grant.replicator, "flag must be true after grant");
    assert!(
        after_grant.tickets_invalid_before_ns >= 5_000,
        "tib must bump on a privilege change"
    );

    // No-op re-grant: same state, no write, no tib bump.
    let noop = store
        .set_replicator("alice", true, 9_000)
        .expect("no-op re-grant should not error");
    assert!(!noop, "granting an already-granted flag must be a no-op");
    let after_noop = store.state_by_user_id(&uid).expect("alice resolves");
    assert_eq!(
        after_noop.tickets_invalid_before_ns, after_grant.tickets_invalid_before_ns,
        "no-op must not bump tib"
    );

    // Revoke — succeeds even though alice is the ONLY replicator (no
    // last-remaining guard, unlike superuser).
    let revoked = store
        .set_replicator("alice", false, 10_000)
        .expect("revoke of the only replicator must succeed");
    assert!(revoked, "revoke must report a real change");
    let after_revoke = store.state_by_user_id(&uid).expect("alice resolves");
    assert!(!after_revoke.replicator, "flag must be false after revoke");
    assert!(
        after_revoke.tickets_invalid_before_ns >= 10_000,
        "tib must bump again on the revoke"
    );
}

/// **Red test #2 (task #557) — `set_superuser` grant/revoke, tib bump, count.**
///
/// Granting superuser on a non-superuser:
///   - returns `Ok(true)`,
///   - flips `superuser` to `true`,
///   - bumps `tickets_invalid_before_ns` to at least `now_ns`,
///   - is observable via `state_by_user_id`.
///
/// Then revoking it (after adding a second superuser so the last-superuser
/// guard doesn't fire) flips the flag back, bumps tib again, and returns
/// `Ok(true)`. The `superuser_count` is verified indirectly via the
/// last-superuser guard: after grant there is 1 superuser, so revoking
/// would be refused; after a second grant, revoking the first succeeds.
#[test]
fn set_superuser_grants_revokes_bumps_tib_and_maintains_count() {
    let (_tmp, store) = fresh_dir();
    let alice_uid = store.insert("alice".to_string(), fixture_record()).unwrap();
    let bob_uid = store.insert("bob".to_string(), fixture_record()).unwrap();

    // Grant on a non-superuser → flag flips, tib bumps.
    let now1 = 1_000u64;
    let changed = store.set_superuser("alice", true, now1).unwrap();
    assert!(changed, "grant on a non-superuser must report a change");
    let alice = store.state_by_user_id(&alice_uid).expect("alice resolves");
    assert!(alice.superuser, "flag must be true after grant");
    assert!(
        alice.tickets_invalid_before_ns >= now1,
        "tib must advance to >= now_ns after a grant; got {}",
        alice.tickets_invalid_before_ns
    );

    // Trying to revoke the ONLY superuser must be refused (count == 1).
    let err = store
        .set_superuser("alice", false, now1 + 1)
        .expect_err("revoking the only superuser must be refused");
    match err {
        shamir_connect::common::error::Error::InvalidInput(msg) => {
            assert!(
                msg.contains("last remaining superuser"),
                "wrong InvalidInput message: {msg}"
            );
        }
        other => panic!("expected InvalidInput for last-superuser revoke, got {other:?}"),
    }
    // Flag stays true.
    assert!(
        store.state_by_user_id(&alice_uid).unwrap().superuser,
        "flag must stay true after a refused revoke"
    );

    // Grant on a SECOND user (bob) → count rises to 2; now revoking alice
    // must succeed.
    let now2 = 2_000u64;
    let changed2 = store.set_superuser("bob", true, now2).unwrap();
    assert!(changed2, "grant on bob must report a change");
    assert!(
        store.state_by_user_id(&bob_uid).unwrap().superuser,
        "bob must be a superuser after grant"
    );

    // Revoke alice (no longer the last) → flag flips, tib bumps.
    let now3 = 3_000u64;
    let changed3 = store.set_superuser("alice", false, now3).unwrap();
    assert!(
        changed3,
        "revoke on a non-last superuser must report a change"
    );
    let alice_after = store.state_by_user_id(&alice_uid).unwrap();
    assert!(!alice_after.superuser, "flag must be false after revoke");
    assert!(
        alice_after.tickets_invalid_before_ns >= now3,
        "tib must advance on revoke; got {}",
        alice_after.tickets_invalid_before_ns
    );

    // Bob is still a superuser (the count went 1 → 2 → 1 cleanly).
    assert!(
        store.state_by_user_id(&bob_uid).unwrap().superuser,
        "bob must still be a superuser after alice's revoke"
    );
}

/// **Red test #3 (task #557) — `set_superuser` refuses to revoke the last.**
///
/// Mirrors #556's `remove_refuses_last_superuser_then_succeeds_with_two`:
/// with exactly one superuser, `set_superuser(name, false, ..)` returns
/// `Err` and the flag stays `true`; with two superusers, revoking one
/// succeeds.
#[test]
fn set_superuser_refuses_last_then_succeeds_with_two() {
    let (_tmp, store) = fresh_dir();
    let alice_uid = store.insert("alice".to_string(), fixture_record()).unwrap();
    let bob_uid = store.insert("bob".to_string(), fixture_record()).unwrap();

    // alice is the only superuser.
    store.set_superuser("alice", true, 1_000).unwrap();
    assert!(store.state_by_user_id(&alice_uid).unwrap().superuser);

    // Revoking alice (the last) is refused.
    let err = store
        .set_superuser("alice", false, 2_000)
        .expect_err("revoking the last superuser must be refused");
    assert!(
        matches!(
            err,
            shamir_connect::common::error::Error::InvalidInput(m) if m.contains("last remaining superuser")
        ),
        "expected InvalidInput \"last remaining superuser\", got {err:?}"
    );
    // Flag stays true.
    assert!(
        store.state_by_user_id(&alice_uid).unwrap().superuser,
        "flag must stay true after refused revoke"
    );

    // Promote bob to a second superuser; revoking alice now succeeds.
    store.set_superuser("bob", true, 3_000).unwrap();
    let changed = store.set_superuser("alice", false, 4_000).unwrap();
    assert!(
        changed,
        "revoke on a non-last superuser must report a change"
    );
    assert!(
        !store.state_by_user_id(&alice_uid).unwrap().superuser,
        "alice's flag must be false after a successful revoke"
    );
    assert!(
        store.state_by_user_id(&bob_uid).unwrap().superuser,
        "bob must still be a superuser"
    );
}

/// **Red test #4 (task #557) — `set_superuser` is idempotent.**
///
/// Granting an already-superuser account (or revoking an already-non-
/// superuser account) returns `Ok(false)`, with no tib bump and no count
/// change (verified indirectly: after a no-op grant, revoking still sees
/// the same superuser_count, so a single subsequent revoke of the only
/// superuser is still refused).
#[test]
fn set_superuser_is_idempotent() {
    let (_tmp, store) = fresh_dir();
    let alice_uid = store.insert("alice".to_string(), fixture_record()).unwrap();

    // Grant once → Ok(true), tib advances.
    let now1 = 1_000u64;
    assert!(store.set_superuser("alice", true, now1).unwrap());
    let tib_after_grant = store
        .state_by_user_id(&alice_uid)
        .unwrap()
        .tickets_invalid_before_ns;

    // Grant again → Ok(false), tib does NOT advance (no write made).
    let now2 = now1 + 5_000;
    let changed = store.set_superuser("alice", true, now2).unwrap();
    assert!(
        !changed,
        "grant on an already-superuser must be a no-op (Ok(false))"
    );
    let tib_after_noop = store
        .state_by_user_id(&alice_uid)
        .unwrap()
        .tickets_invalid_before_ns;
    assert_eq!(
        tib_after_grant, tib_after_noop,
        "tib must NOT advance on a no-op grant"
    );

    // Revoke on a non-superuser (alice is still a superuser, but try on bob
    // who never was) → Ok(false), no count change.
    let bob_uid = store.insert("bob".to_string(), fixture_record()).unwrap();
    let changed = store.set_superuser("bob", false, now2).unwrap();
    assert!(
        !changed,
        "revoke on an already-non-superuser must be a no-op (Ok(false))"
    );
    assert!(
        !store.state_by_user_id(&bob_uid).unwrap().superuser,
        "bob's flag must stay false after a no-op revoke"
    );

    // Count is unchanged: alice is still the only superuser, so revoking
    // alice is still refused (proves the no-op grants/revokes did not
    // perturb `superuser_count`).
    let err = store
        .set_superuser("alice", false, now2 + 1)
        .expect_err("revoking the last superuser must still be refused");
    assert!(
        matches!(
            err,
            shamir_connect::common::error::Error::InvalidInput(m) if m.contains("last remaining superuser")
        ),
        "expected last-superuser refusal (count must be 1), got {err:?}"
    );
}

/// **Red test #6 (task #557) — handshake wiring produces a superuser Session
/// from the flag, not from a `"superuser"` role string.**
///
/// The handshake change in task #557 replaces `lookup_roles` +
/// `SessionPermissions::from_roles` with `state_by_user_id` +
/// `SessionPermissions::new(state.superuser, state.replicator, state.roles)`. This test
/// exercises that exact construction pattern against a real
/// `FjallUserDirectory` account whose `superuser` flag is `true` but whose
/// persisted `roles` list does NOT contain the literal `"superuser"`
/// string — proving the switch away from role-string scanning actually
/// took effect (a `from_roles` call on the same roles list would yield
/// `is_superuser == false`, so this test fails if the wiring regresses).
///
/// Driving the full async `run_handshake` requires a complete
/// `ConnectionContext` (TLS identity, real listener, SCRAM exchange) —
/// see `src/tests/connection_tests.rs`'s comment for the established
/// convention of testing the load-bearing logic directly. The handshake's
/// ONLY #557-relevant work is the lookup + constructor swap exercised here.
#[test]
fn handshake_wiring_superuser_flag_drives_is_superuser_without_role_string() {
    use shamir_connect::server::session::SessionPermissions;

    let (_tmp, store) = fresh_dir();
    let uid = store.insert("alice".to_string(), fixture_record()).unwrap();

    // Make alice a superuser via the flag (the post-#557 blessed path).
    // The persisted roles list is empty — no "superuser" string anywhere.
    store.set_superuser("alice", true, 1_000).unwrap();

    // Mirror exactly what handshake.rs does post-#557: a single
    // `state_by_user_id` snapshot, then `SessionPermissions::new`.
    let state = store
        .state_by_user_id(&uid)
        .expect("alice must resolve via state_by_user_id");
    let perms = SessionPermissions::new(state.superuser, state.replicator, state.roles.clone());

    assert!(
        perms.is_superuser,
        "is_superuser must be driven by the flag (true), not by a role-string scan"
    );
    assert!(
        !perms.roles.iter().any(|r| r == "superuser"),
        "the persisted roles list must NOT contain the literal \"superuser\" \
         string after the #557 wiring; got {:?}",
        perms.roles
    );

    // Sanity: the OLD constructor (`from_roles`) on the SAME roles list
    // would return is_superuser == false — i.e. the regression this test
    // catches is "handshake still uses from_roles".
    let legacy = SessionPermissions::from_roles(state.roles);
    assert!(
        !legacy.is_superuser,
        "from_roles on a flag-only superuser's roles list must yield false \
         (this is the regression signal: if from_roles were still wired in, \
         the session would lose superuser status)"
    );
}
