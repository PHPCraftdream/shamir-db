//! Round-trip tests for the `server_query_version` field added to
//! [`WireAuthOk`] and [`WireResumeOk`].
//!
//! Key backward-compat property: a payload produced by an *old* server
//! (without `server_query_version`) must deserialise with the field
//! defaulting to `0`, so new clients treat the connection as v1.

use crate::client::transport::{WireAuthOk, WireResumeOk};
use shamir_connect::common::wire::RotationInProgressWire;

// ── WireAuthOk ────────────────────────────────────────────────────────────────

/// Minimal helper that mirrors WireAuthOk as an old server would send it
/// (no `server_query_version` field).
#[derive(serde::Serialize)]
struct OldServerAuthOk {
    #[serde(with = "serde_bytes")]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
}

/// Helper that mirrors WireAuthOk as a new server would send it
/// (includes `server_query_version`).
#[derive(serde::Serialize)]
struct NewServerAuthOk {
    #[serde(with = "serde_bytes")]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    server_query_version: u8,
}

#[test]
fn wire_auth_ok_carries_server_query_version() {
    let helper = NewServerAuthOk {
        server_signature: vec![0x01; 32],
        server_pub_key: vec![0x02; 32],
        identity_sig: vec![0x03; 64],
        session_id: vec![0x04; 32],
        expires_at_ns: 1_000_000_000,
        server_query_version: 2,
    };
    // Named (map) encoding: field order between helper and target does not need
    // to match; the decoder picks fields by name.
    let bytes = rmp_serde::to_vec_named(&helper).expect("serialize NewServerAuthOk");
    let ok: WireAuthOk = rmp_serde::from_slice(&bytes).expect("deserialize WireAuthOk");

    assert_eq!(ok.server_query_version, 2);
    assert_eq!(ok.expires_at_ns, 1_000_000_000);
}

#[test]
fn wire_auth_ok_old_payload_defaults_server_query_version_to_zero() {
    // Old server omits the field entirely — client must read 0 (treat as v1).
    let old = OldServerAuthOk {
        server_signature: vec![0x01; 32],
        server_pub_key: vec![0x02; 32],
        identity_sig: vec![0x03; 64],
        session_id: vec![0x04; 32],
        expires_at_ns: 42,
    };
    let bytes = rmp_serde::to_vec_named(&old).expect("serialize OldServerAuthOk");
    let ok: WireAuthOk = rmp_serde::from_slice(&bytes).expect("deserialize WireAuthOk");

    assert_eq!(
        ok.server_query_version, 0,
        "absent server_query_version must default to 0"
    );
}

// ── WireResumeOk ─────────────────────────────────────────────────────────────

/// Helper: old ResumeOk without `server_query_version`.
#[derive(serde::Serialize)]
struct OldServerResumeOk {
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
}

/// Helper: new ResumeOk with `server_query_version`.
#[derive(serde::Serialize)]
struct NewServerResumeOk {
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    server_query_version: u8,
}

#[test]
fn wire_resume_ok_carries_server_query_version() {
    let helper = NewServerResumeOk {
        session_id: vec![0xAB; 32],
        expires_at_ns: 999,
        server_query_version: 2,
    };
    let bytes = rmp_serde::to_vec_named(&helper).expect("serialize NewServerResumeOk");
    let ok: WireResumeOk = rmp_serde::from_slice(&bytes).expect("deserialize WireResumeOk");

    assert_eq!(ok.server_query_version, 2);
    assert_eq!(ok.expires_at_ns, 999);
}

#[test]
fn wire_resume_ok_old_payload_defaults_server_query_version_to_zero() {
    // Old server omits the field — client must read 0 (treat as v1).
    let old = OldServerResumeOk {
        session_id: vec![0xCD; 32],
        expires_at_ns: 123,
    };
    let bytes = rmp_serde::to_vec_named(&old).expect("serialize OldServerResumeOk");
    let ok: WireResumeOk = rmp_serde::from_slice(&bytes).expect("deserialize WireResumeOk");

    assert_eq!(
        ok.server_query_version, 0,
        "absent server_query_version must default to 0"
    );
}

#[test]
fn rv_client_f20c09bf_auth_ok_hints_survive_wire_decode() {
    #[derive(serde::Serialize)]
    struct AuthOkWithHints {
        #[serde(with = "serde_bytes")]
        server_signature: Vec<u8>,
        #[serde(with = "serde_bytes")]
        server_pub_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        identity_sig: Vec<u8>,
        #[serde(with = "serde_bytes")]
        session_id: Vec<u8>,
        expires_at_ns: u64,
        #[serde(with = "serde_bytes")]
        resumption_ticket: Vec<u8>,
        resumption_expires_at_ns: u64,
        server_query_version: u8,
        rotation_in_progress: Option<RotationInProgressWire>,
        kdf_upgrade_required: Option<bool>,
    }

    let expected = RotationInProgressWire {
        previous_pub: vec![0x11; 32],
        identity_sig_previous: vec![0x22; 64],
        transition_until_ns: 42,
        rotation_proof: vec![0x33; 64],
    };
    let mut invalid = expected.clone();
    invalid.previous_pub.pop();
    let invalid_encoded = rmp_serde::to_vec_named(&AuthOkWithHints {
        server_signature: vec![1; 32],
        server_pub_key: vec![2; 32],
        identity_sig: vec![3; 64],
        session_id: vec![4; 32],
        expires_at_ns: 99,
        resumption_ticket: Vec::new(),
        resumption_expires_at_ns: 0,
        server_query_version: 2,
        rotation_in_progress: Some(invalid),
        kdf_upgrade_required: Some(false),
    })
    .expect("encode malformed rotation hint");
    let invalid_decoded: WireAuthOk =
        rmp_serde::from_slice(&invalid_encoded).expect("decode malformed rotation hint");
    assert!(invalid_decoded
        .rotation_in_progress
        .expect("rotation hint retained")
        .into_payload()
        .is_none());

    let encoded = rmp_serde::to_vec_named(&AuthOkWithHints {
        server_signature: vec![1; 32],
        server_pub_key: vec![2; 32],
        identity_sig: vec![3; 64],
        session_id: vec![4; 32],
        expires_at_ns: 99,
        resumption_ticket: Vec::new(),
        resumption_expires_at_ns: 0,
        server_query_version: 2,
        rotation_in_progress: Some(expected.clone()),
        kdf_upgrade_required: Some(true),
    })
    .expect("encode auth_ok hints");
    let decoded: WireAuthOk = rmp_serde::from_slice(&encoded).expect("decode auth_ok hints");
    let rotation = decoded
        .rotation_in_progress
        .expect("rotation hint retained")
        .into_payload()
        .expect("fixed-size rotation fields");
    assert_eq!(rotation.previous_pub, [0x11; 32]);
    assert_eq!(rotation.identity_sig_previous, [0x22; 64]);
    assert_eq!(rotation.transition_until_ns, 42);
    assert_eq!(rotation.rotation_proof, [0x33; 64]);
    assert_eq!(decoded.kdf_upgrade_required, Some(true));
}

// ── Shared wire fixtures (L4-RV-SERVER-8999F6B7) ─────────────────────────────
//
// The committed msgpack bytes under crates/shamir-connect/test-vectors/ are
// the same bytes the server tests decode through the production
// `AuthOk` / `ResumeOkWire` DTOs (crates/shamir-server/src/tests/engine/
// auth_ok_hint_tests.rs). Here they are decoded through the production
// client DTOs `WireAuthOk` / `WireResumeOk` to prove one wire shape serves
// both sides.

#[test]
fn rv_server_8999f6b7_shared_fixtures_decode_production_client_dtos() {
    let base: WireAuthOk = rmp_serde::from_slice(include_bytes!(
        "../../../../shamir-connect/test-vectors/auth_ok_base.msgpack"
    ))
    .expect("decode shared base fixture into production WireAuthOk");
    assert_eq!(base.server_signature, vec![1; 32]);
    assert_eq!(base.server_pub_key, vec![2; 32]);
    assert_eq!(base.identity_sig, vec![3; 64]);
    assert_eq!(base.session_id, vec![4; 32]);
    assert_eq!(base.expires_at_ns, 7);
    assert!(base.resumption_ticket.is_empty());
    assert_eq!(base.resumption_expires_at_ns, 0);
    assert_eq!(base.server_query_version, 1);
    assert!(base.rotation_in_progress.is_none());
    assert_eq!(base.kdf_upgrade_required, None);

    let hinted: WireAuthOk = rmp_serde::from_slice(include_bytes!(
        "../../../../shamir-connect/test-vectors/auth_ok_hinted.msgpack"
    ))
    .expect("decode shared hinted fixture into production WireAuthOk");
    assert_eq!(hinted.session_id, vec![4; 32]);
    assert_eq!(hinted.server_query_version, 1);
    assert!(hinted.rotation_in_progress.is_none());
    assert_eq!(hinted.kdf_upgrade_required, Some(true));

    let resume: WireResumeOk = rmp_serde::from_slice(include_bytes!(
        "../../../../shamir-connect/test-vectors/resume_ok.msgpack"
    ))
    .expect("decode shared resume fixture into production WireResumeOk");
    assert_eq!(resume.session_id, vec![0xAB; 32]);
    assert_eq!(resume.expires_at_ns, 99);
    assert_eq!(resume.resumption_expires_at_ns, 0);
    assert_eq!(resume.server_query_version, 2);
}
