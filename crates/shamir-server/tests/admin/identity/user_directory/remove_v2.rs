use super::common::{fixture_record, fresh_dir, reopen};
use shamir_connect::server::admin::UserDirectory;
use tempfile::TempDir;

// ----------------------------------------------------------------------------
// Task #556 — directory v2: boot normalization, remove(), state_by_user_id
// ----------------------------------------------------------------------------

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
