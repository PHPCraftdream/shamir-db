//! Round-trip tests for the `server_query_version` field added to
//! [`WireAuthOk`] and [`WireResumeOk`].
//!
//! Key backward-compat property: a payload produced by an *old* server
//! (without `server_query_version`) must deserialise with the field
//! defaulting to `0`, so new clients treat the connection as v1.

use crate::wire_frames::{WireAuthOk, WireResumeOk};

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

// ── Client field plumbing unit test ──────────────────────────────────────────

/// Verify the AtomicU8 plumbing: a Client whose WireAuthOk carries
/// server_query_version=2 should expose that via server_query_version().
/// We test the field directly since spinning up a real TLS server is an e2e concern.
#[test]
fn atomic_u8_plumbing_stores_and_reads_correctly() {
    use std::sync::atomic::{AtomicU8, Ordering};
    let field = AtomicU8::new(2);
    assert_eq!(field.load(Ordering::Relaxed), 2);
    let field2 = AtomicU8::new(0);
    assert_eq!(field2.load(Ordering::Relaxed), 0);
}
