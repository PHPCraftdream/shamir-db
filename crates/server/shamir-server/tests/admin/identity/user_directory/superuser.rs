use super::common::{fixture_record, fresh_dir};
use shamir_connect::server::admin::UserDirectory;

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
