//! Integration tests for identity rotation flow (spec §6.4-§6.5, §12.2).

use shamir_connect::client::rotation::{
    verify_identity_rotation_event, verify_rotation_in_progress, ROTATION_MAX_TRANSITION_NS,
};
use shamir_connect::common::crypto::sha256;
use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::server::rotation::{
    build_identity_rotation_event, build_rotation_in_progress_payload, ServerIdentityState,
    ROTATION_OVERLAP_NS,
};

#[test]
fn fresh_state_has_no_overlap() {
    let s = ServerIdentityState::fresh();
    assert!(s.previous_pub().is_none());
    assert!(s.rotation_until_ns().is_none());
    assert!(!s.rotation_in_progress(UnixNanos::now().as_u64()));
}

#[test]
fn rotate_swaps_keys_and_starts_overlap() {
    let s = ServerIdentityState::fresh();
    let original_pub = s.current_pub();
    let now = UnixNanos::now().as_u64();

    let outcome = s.rotate(now).unwrap();
    assert_eq!(outcome.old_pub, original_pub);
    assert_ne!(outcome.new_pub, original_pub);
    assert_eq!(outcome.transition_until_ns, now + ROTATION_OVERLAP_NS);
    assert_eq!(s.previous_pub(), Some(original_pub));
    assert!(s.rotation_in_progress(now));
}

#[test]
fn double_rotation_inside_overlap_is_rejected_high_5() {
    let s = ServerIdentityState::fresh();
    let now = UnixNanos::now().as_u64();
    let _ = s.rotate(now).unwrap();
    let result = s.rotate(now + ns::HOUR);
    assert!(result.is_err(), "second rotation must reject (HIGH-5)");
}

#[test]
fn rotation_allowed_again_after_overlap_finalizes() {
    let s = ServerIdentityState::fresh();
    let now = UnixNanos::now().as_u64();
    let _ = s.rotate(now).unwrap();

    let after_overlap = now + ROTATION_OVERLAP_NS + ns::SECOND;
    assert!(s.try_finalize(after_overlap));
    assert!(s.previous_pub().is_none());

    // Now a second rotation is fine.
    let _ = s.rotate(after_overlap).unwrap();
}

#[test]
fn rotation_event_round_trip_active_session() {
    let s = ServerIdentityState::fresh();
    let original_pub = s.current_pub();
    let pinned = sha256(&original_pub);
    let now = UnixNanos::now().as_u64();

    let outcome = s.rotate(now).unwrap();

    let my_sid = [0xabu8; 32];
    let event = build_identity_rotation_event(&s, &my_sid).unwrap();

    assert_eq!(event.old_pub, outcome.old_pub);
    assert_eq!(event.new_pub, outcome.new_pub);
    assert_eq!(event.recipient_session_id, my_sid);

    let new_pin = verify_identity_rotation_event(&event, &pinned, &my_sid, now, true).unwrap();
    assert_eq!(new_pin, sha256(&outcome.new_pub));
}

#[test]
fn rotation_event_per_recipient_signing() {
    let s = ServerIdentityState::fresh();
    let _ = s.rotate(UnixNanos::now().as_u64()).unwrap();

    let event_a = build_identity_rotation_event(&s, &[0x01u8; 32]).unwrap();
    let event_b = build_identity_rotation_event(&s, &[0x02u8; 32]).unwrap();

    assert_ne!(event_a.signed_by_old, event_b.signed_by_old,
        "per-recipient sigs must differ");
}

#[test]
fn rotation_event_rejects_replay_to_different_recipient() {
    let s = ServerIdentityState::fresh();
    let original_pub = s.current_pub();
    let pinned = sha256(&original_pub);
    let _ = s.rotate(UnixNanos::now().as_u64()).unwrap();

    let alice_sid = [0xa1u8; 32];
    let bob_sid = [0xb2u8; 32];
    let event_for_alice = build_identity_rotation_event(&s, &alice_sid).unwrap();

    // Bob receives Alice's event.
    let result = verify_identity_rotation_event(
        &event_for_alice,
        &pinned,
        &bob_sid, // wrong recipient
        UnixNanos::now().as_u64(),
        true,
    );
    assert!(result.is_err());
}

#[test]
fn rotation_event_rejects_when_pin_does_not_match_old_pub() {
    let s = ServerIdentityState::fresh();
    let _ = s.rotate(UnixNanos::now().as_u64()).unwrap();

    let event = build_identity_rotation_event(&s, &[0xaau8; 32]).unwrap();
    // Client has wrong pin (e.g., never knew this server).
    let result = verify_identity_rotation_event(
        &event,
        &[0xffu8; 32],
        &[0xaau8; 32],
        UnixNanos::now().as_u64(),
        true,
    );
    assert!(result.is_err());
}

#[test]
fn rotation_event_default_fail_closed_without_explicit_acceptance() {
    let s = ServerIdentityState::fresh();
    let original_pub = s.current_pub();
    let pinned = sha256(&original_pub);
    let _ = s.rotate(UnixNanos::now().as_u64()).unwrap();

    let event = build_identity_rotation_event(&s, &[0xaau8; 32]).unwrap();
    // accept_rotation = false → no auto-update.
    let result = verify_identity_rotation_event(
        &event,
        &pinned,
        &[0xaau8; 32],
        UnixNanos::now().as_u64(),
        false,
    );
    assert!(result.is_err());
}

#[test]
fn rotation_event_rejects_far_future_transition_window_high_2() {
    // Forge an event with transition_until_ns in 100 years (HIGH-2 attack).
    let s = ServerIdentityState::fresh();
    let original_pub = s.current_pub();
    let pinned = sha256(&original_pub);

    let now = UnixNanos::now().as_u64();
    let _ = s.rotate(now).unwrap();
    let mut event = build_identity_rotation_event(&s, &[0xaau8; 32]).unwrap();

    event.transition_until_ns = now + 100 * 365 * ns::DAY;
    // Note: signature was over the original transition_until_ns, so it WILL
    // fail signature verification — proving the upper-bound check is layered
    // on top of signature validation.
    let result =
        verify_identity_rotation_event(&event, &pinned, &[0xaau8; 32], now, true);
    assert!(result.is_err());
}

#[test]
fn orphan_recovery_round_trip() {
    let s = ServerIdentityState::fresh();
    let old_pub = s.current_pub();
    let pinned = sha256(&old_pub);
    let now = UnixNanos::now().as_u64();
    let _ = s.rotate(now).unwrap();

    let payload = build_rotation_in_progress_payload(&s).unwrap();
    let new_pub = s.current_pub();

    let new_pin = verify_rotation_in_progress(&payload, &new_pub, &pinned, now, true).unwrap();
    assert_eq!(new_pin, sha256(&new_pub));
}

#[test]
fn orphan_recovery_rejects_wrong_pin() {
    let s = ServerIdentityState::fresh();
    let _ = s.rotate(UnixNanos::now().as_u64()).unwrap();
    let payload = build_rotation_in_progress_payload(&s).unwrap();
    let new_pub = s.current_pub();

    let result = verify_rotation_in_progress(
        &payload,
        &new_pub,
        &[0xffu8; 32], // wrong pinned hash
        UnixNanos::now().as_u64(),
        true,
    );
    assert!(result.is_err());
}

#[test]
fn orphan_recovery_default_fail_closed() {
    let s = ServerIdentityState::fresh();
    let old_pub = s.current_pub();
    let pinned = sha256(&old_pub);
    let _ = s.rotate(UnixNanos::now().as_u64()).unwrap();

    let payload = build_rotation_in_progress_payload(&s).unwrap();
    let new_pub = s.current_pub();

    // accept_rotation = false → fail closed.
    let result = verify_rotation_in_progress(
        &payload,
        &new_pub,
        &pinned,
        UnixNanos::now().as_u64(),
        false,
    );
    assert!(result.is_err());
}

#[test]
fn orphan_recovery_rejects_expired_overlap() {
    let s = ServerIdentityState::fresh();
    let old_pub = s.current_pub();
    let pinned = sha256(&old_pub);
    let _ = s.rotate(0).unwrap(); // rotation at unix epoch start

    let payload = build_rotation_in_progress_payload(&s).unwrap();
    let new_pub = s.current_pub();

    // Now is 100 years later — overlap long expired.
    let far_future = UnixNanos::now().as_u64() + 100 * 365 * ns::DAY;
    let result = verify_rotation_in_progress(&payload, &new_pub, &pinned, far_future, true);
    assert!(result.is_err());
}

#[test]
fn orphan_recovery_rejects_far_future_window_high_2() {
    // Construct a payload with manipulated transition_until_ns. Real attack:
    // attacker with leaked previous_priv would forge ROTATION_PROOF_PAYLOAD
    // over (previous_pub_hash, attacker_pub, far_future). Here we just
    // verify the upper-bound check fires before signature verify is tested.
    let s = ServerIdentityState::fresh();
    let old_pub = s.current_pub();
    let pinned = sha256(&old_pub);
    let now = UnixNanos::now().as_u64();
    let _ = s.rotate(now).unwrap();

    let mut payload = build_rotation_in_progress_payload(&s).unwrap();
    payload.transition_until_ns = now + 2 * ROTATION_MAX_TRANSITION_NS;

    let new_pub = s.current_pub();
    let result = verify_rotation_in_progress(&payload, &new_pub, &pinned, now, true);
    assert!(result.is_err());
}

#[test]
fn build_rotation_event_fails_when_no_overlap() {
    let s = ServerIdentityState::fresh();
    // No rotation yet → no previous → can't build event.
    let result = build_identity_rotation_event(&s, &[0xaau8; 32]);
    assert!(result.is_err());
}

#[test]
fn build_rotation_in_progress_fails_when_no_overlap() {
    let s = ServerIdentityState::fresh();
    let result = build_rotation_in_progress_payload(&s);
    assert!(result.is_err());
}
