//! Wire-format round-trip tests for the resumption frames.
//!
//! These tests exercise only serialisation/deserialisation of
//! [`WireResumeInit`] and [`WireResumeOk`] — no server required.

use crate::client::resume_wire_exchange;
use crate::client::transport::{WireResumeInit, WireResumeOk, WireResumeProofOk};
use crate::client::verify_resume_identity_proof;
use shamir_connect::common::crypto::Ed25519Keypair;
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use std::time::Duration;
use tokio::io::{duplex, split, AsyncReadExt};

/// No-hang revert oracle: every client-side exchange below is wrapped in this
/// budget so a revert that makes the client await a reply forever fails the
/// test with a timeout instead of hanging CI.
const SEND_BUDGET: Duration = Duration::from_secs(30);

/// A mirror of WireResumeInit with Deserialize added, so we can round-trip
/// through msgpack in these tests without publishing Deserialize on the
/// production type.
#[derive(serde::Deserialize)]
struct WireResumeInitDeser {
    #[serde(with = "serde_bytes")]
    ticket: Vec<u8>,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
}

#[test]
fn resume_init_roundtrip() {
    let ticket = vec![0xAA_u8; 64];
    let nonce = vec![0x01_u8; 32];
    let frame = WireResumeInit {
        ticket: ticket.clone(),
        client_nonce: nonce.clone(),
        binding_mode: 2,
    };
    let bytes = rmp_serde::to_vec(&frame).expect("serialize WireResumeInit");
    let decoded: WireResumeInitDeser =
        rmp_serde::from_slice(&bytes).expect("deserialize WireResumeInit");

    assert_eq!(decoded.ticket, ticket);
    assert_eq!(decoded.client_nonce, nonce);
    assert_eq!(decoded.binding_mode, 2);
}

#[test]
fn resume_ok_roundtrip() {
    let session_id = vec![0x42_u8; 32];
    let ticket = vec![0xBB_u8; 48];

    // Craft a valid WireResumeOk payload via a helper struct (simulating what
    // the server would send) then decode it as WireResumeOk.
    #[derive(serde::Serialize)]
    struct WireResumeOkHelper {
        #[serde(with = "serde_bytes")]
        session_id: Vec<u8>,
        expires_at_ns: u64,
        #[serde(with = "serde_bytes")]
        resumption_ticket: Vec<u8>,
        resumption_expires_at_ns: u64,
    }

    let helper = WireResumeOkHelper {
        session_id: session_id.clone(),
        expires_at_ns: 999_000_000_000,
        resumption_ticket: ticket.clone(),
        resumption_expires_at_ns: 888_000_000_000,
    };
    let bytes = rmp_serde::to_vec(&helper).expect("serialize helper");
    let ok: WireResumeOk = rmp_serde::from_slice(&bytes).expect("deserialize WireResumeOk");

    assert_eq!(ok.session_id, session_id);
    assert_eq!(ok.expires_at_ns, 999_000_000_000);
    assert_eq!(ok.resumption_ticket, ticket);
    assert_eq!(ok.resumption_expires_at_ns, 888_000_000_000);
}

#[test]
fn resume_ok_empty_ticket_defaults() {
    // When the server sends no ticket, the optional fields should default.
    #[derive(serde::Serialize)]
    struct WireResumeOkMinimal {
        #[serde(with = "serde_bytes")]
        session_id: Vec<u8>,
        expires_at_ns: u64,
    }
    let minimal = WireResumeOkMinimal {
        session_id: vec![0u8; 32],
        expires_at_ns: 123,
    };
    let bytes = rmp_serde::to_vec(&minimal).expect("serialize minimal");
    let ok: WireResumeOk = rmp_serde::from_slice(&bytes).expect("deserialize WireResumeOk");

    assert!(ok.resumption_ticket.is_empty());
    assert_eq!(ok.resumption_expires_at_ns, 0);
}

// ── RV-CLIENT-DE75ADDD ─────────────────────────────────────────────────────
// The resume flow must verify the pinned server identity BEFORE the bearer
// ticket is written. These tests pin the verification gate itself: a proof
// from any other key, or over any other nonce/exporter, is rejected before
// `verify_resume_identity_proof` returns Ok (the caller then never reaches
// the ticket write).

fn signed_proof(kp: &Ed25519Keypair, nonce: &[u8; 32], exporter: &[u8; 32]) -> WireResumeProofOk {
    let msg = shamir_connect::common::identity::resume_proof_message(nonce, exporter);
    WireResumeProofOk {
        server_pub_key: kp.public_bytes().to_vec(),
        signature: kp.sign(&msg).to_vec(),
    }
}

#[test]
fn rv_client_de75addd_rejects_proof_from_unpinned_key() {
    let server = Ed25519Keypair::from_seed(&[7u8; 32]);
    let attacker = Ed25519Keypair::from_seed(&[9u8; 32]);
    let nonce = [0x11u8; 32];
    let exporter = [0x22u8; 32];
    let pinned_hash = shamir_connect::common::crypto::sha256(&server.public_bytes());

    // The substitute endpoint signs with ITS OWN key: hash mismatch must
    // fail the gate before any ticket byte is written.
    let proof = signed_proof(&attacker, &nonce, &exporter);
    assert!(verify_resume_identity_proof(&proof, &nonce, &exporter, &pinned_hash).is_err());
}

#[test]
fn rv_client_de75addd_rejects_signature_over_other_context() {
    let server = Ed25519Keypair::from_seed(&[7u8; 32]);
    let nonce = [0x11u8; 32];
    let exporter = [0x22u8; 32];
    let pinned_hash = shamir_connect::common::crypto::sha256(&server.public_bytes());

    // Correct key, but the signature covers a DIFFERENT nonce/exporter —
    // a replayed or cross-connection proof must not pass.
    let proof = signed_proof(&server, &[0x33u8; 32], &exporter);
    assert!(verify_resume_identity_proof(&proof, &nonce, &exporter, &pinned_hash).is_err());
    let proof = signed_proof(&server, &nonce, &[0x44u8; 32]);
    assert!(verify_resume_identity_proof(&proof, &nonce, &exporter, &pinned_hash).is_err());
}

#[test]
fn rv_client_de75addd_accepts_proof_from_pinned_key_over_this_connection() {
    let server = Ed25519Keypair::from_seed(&[7u8; 32]);
    let nonce = [0x11u8; 32];
    let exporter = [0x22u8; 32];
    let pinned_hash = shamir_connect::common::crypto::sha256(&server.public_bytes());

    let proof = signed_proof(&server, &nonce, &exporter);
    assert!(verify_resume_identity_proof(&proof, &nonce, &exporter, &pinned_hash).is_ok());
}

// ── RV-CLIENT-DE75ADDD: behavioral tests through the PRODUCTION wire flow ──
// The unit tests above call `verify_resume_identity_proof` directly and would
// still pass if `resume_wire_exchange` stopped calling it. These tests run
// the REAL `resume_wire_exchange` over tokio duplex pipes against an
// EMULATED server and assert the ticket is never disclosed to a peer whose
// proof is rejected.
//
// REVERT ORACLE (tests 1 + 2): reverting ONLY the
// `verify_resume_identity_proof` call inside `resume_wire_exchange` (or
// moving it after the ResumeInit write) makes the server's `read_to_end`
// after the proof-init frame observe the ResumeInit bytes (`rest` non-empty)
// instead of zero bytes, failing the assertion. Test 3 fails if the proof
// exchange is broken outright: the server never sends a valid proof → the
// client errors before writing ResumeInit → the server's read EOFs and the
// `expect` on the decoded ResumeInit fails.

/// Deserialize mirror of `WireResumeProofInit` (production type is
/// Serialize-only, so the emulated server needs its own Deserialize view).
#[derive(serde::Deserialize)]
struct WireResumeProofInitDeser {
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
}

/// Serialize mirror of `WireResumeProofOk` (production type is
/// Deserialize-only, so the emulated server needs its own Serialize view).
#[derive(serde::Serialize)]
struct WireResumeProofOkSer {
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    signature: Vec<u8>,
}

/// Serialize mirror of `WireResumeOk` (production type is Deserialize-only).
#[derive(serde::Serialize)]
struct WireResumeOkSer {
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    #[serde(with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    resumption_expires_at_ns: u64,
    server_query_version: u8,
}

fn proof_ser(kp: &Ed25519Keypair, nonce: &[u8; 32], exporter: &[u8; 32]) -> WireResumeProofOkSer {
    let msg = shamir_connect::common::identity::resume_proof_message(nonce, exporter);
    WireResumeProofOkSer {
        server_pub_key: kp.public_bytes().to_vec(),
        signature: kp.sign(&msg).to_vec(),
    }
}

/// Duplex halves wired for `resume_wire_exchange`: returns the client-side
/// reader/writer plus the test-held server-side halves.
fn wire_halves() -> (
    impl tokio::io::AsyncRead + Unpin,
    impl tokio::io::AsyncWrite + Unpin,
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
) {
    // client→server: the client writes; the emulated server reads.
    let (c2s_client_side, c2s_server_side) = duplex(64 * 1024);
    // server→client: the emulated server writes; the client reads.
    let (s2c_client_side, s2c_server_side) = duplex(64 * 1024);
    let (client_reader, _unused_s2c_write) = split(s2c_client_side);
    let (_unused_c2s_read, client_writer) = split(c2s_client_side);
    (
        client_reader,
        client_writer,
        c2s_server_side,
        s2c_server_side,
    )
}

#[tokio::test]
async fn rv_client_de75addd_substituted_identity_never_receives_ticket() {
    let server_kp = Ed25519Keypair::from_seed(&[7u8; 32]);
    let attacker = Ed25519Keypair::from_seed(&[9u8; 32]);
    let pinned_hash = shamir_connect::common::crypto::sha256(&server_kp.public_bytes());
    let exporter = [0x22u8; 32];
    let ticket = vec![0xAA_u8; 64];

    let (mut client_reader, mut client_writer, mut c2s, mut s2c) = wire_halves();

    let server = tokio::spawn(async move {
        // 1) read the proof preamble, learn the client nonce.
        let frame = read_frame(&mut c2s, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("proof init frame");
        let init: WireResumeProofInitDeser =
            rmp_serde::from_slice(&frame).expect("decode proof init");
        let nonce: [u8; 32] = init.client_nonce.try_into().expect("nonce size");

        // 2) ATTACKER signs with its own key (pin mismatch ahead).
        let proof = proof_ser(&attacker, &nonce, &exporter);
        write_frame(&mut s2c, &rmp_serde::to_vec(&proof).expect("encode proof"))
            .await
            .expect("send proof");

        // 3) ZERO-BYTE ORACLE: after the proof-init frame the server must
        //    receive ZERO further bytes — no ticket byte may be written after
        //    a rejected proof.
        let mut rest = Vec::new();
        c2s.read_to_end(&mut rest).await.expect("read to eof");
        assert!(
            rest.is_empty(),
            "server must receive ZERO bytes after the proof-init frame, got {rest:?} — no ResumeInit/ticket frame may be written"
        );
    });

    // No-hang revert oracle: on revert the client awaits ResumeOk forever.
    let res = tokio::time::timeout(
        SEND_BUDGET,
        resume_wire_exchange(
            &mut client_reader,
            &mut client_writer,
            ticket,
            pinned_hash,
            exporter,
        ),
    )
    .await
    .expect("resume_wire_exchange must not hang");
    assert!(
        res.is_err(),
        "exchange must fail on the pinned-hash mismatch"
    );

    // Release the test-held halves so the server's read sees the true EOF.
    drop(client_reader);
    drop(client_writer);
    server.await.expect("emulated server task panicked");
}

#[tokio::test]
async fn rv_client_de75addd_bad_signature_never_receives_ticket() {
    let server_kp = Ed25519Keypair::from_seed(&[7u8; 32]);
    let pinned_hash = shamir_connect::common::crypto::sha256(&server_kp.public_bytes());
    let exporter = [0x22u8; 32];
    let ticket = vec![0xBB_u8; 64];

    let (mut client_reader, mut client_writer, mut c2s, mut s2c) = wire_halves();

    let server = tokio::spawn(async move {
        let frame = read_frame(&mut c2s, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("proof init frame");
        let init: WireResumeProofInitDeser =
            rmp_serde::from_slice(&frame).expect("decode proof init");
        let _nonce: [u8; 32] = init.client_nonce.try_into().expect("nonce size");

        // Correct key, but the signature covers a DIFFERENT nonce/exporter.
        let proof = proof_ser(&server_kp, &[0x33u8; 32], &[0x44u8; 32]);
        write_frame(&mut s2c, &rmp_serde::to_vec(&proof).expect("encode proof"))
            .await
            .expect("send proof");

        // ZERO-BYTE ORACLE as in the substituted-identity test.
        let mut rest = Vec::new();
        c2s.read_to_end(&mut rest).await.expect("read to eof");
        assert!(
            rest.is_empty(),
            "server must receive ZERO bytes after the proof-init frame, got {rest:?} — no ResumeInit/ticket frame may be written"
        );
    });

    // No-hang revert oracle: on revert the client awaits ResumeOk forever.
    let res = tokio::time::timeout(
        SEND_BUDGET,
        resume_wire_exchange(
            &mut client_reader,
            &mut client_writer,
            ticket,
            pinned_hash,
            exporter,
        ),
    )
    .await
    .expect("resume_wire_exchange must not hang");
    assert!(res.is_err(), "exchange must fail on the invalid signature");

    drop(client_reader);
    drop(client_writer);
    server.await.expect("emulated server task panicked");
}

#[tokio::test]
async fn rv_client_de75addd_valid_proof_proceeds_and_sends_ticket_once() {
    let server_kp = Ed25519Keypair::from_seed(&[7u8; 32]);
    let pinned_hash = shamir_connect::common::crypto::sha256(&server_kp.public_bytes());
    let exporter = [0x22u8; 32];
    let ticket = vec![0xCC_u8; 64];
    let server_ticket = ticket.clone();
    let session_id = vec![0x42_u8; 32];
    let rotated = vec![0xDD_u8; 48];

    let (mut client_reader, mut client_writer, mut c2s, mut s2c) = wire_halves();

    let server = tokio::spawn(async move {
        // 1) proof preamble → sign with the PINNED key over the real nonce.
        let frame = read_frame(&mut c2s, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("proof init frame");
        let init: WireResumeProofInitDeser =
            rmp_serde::from_slice(&frame).expect("decode proof init");
        let nonce: [u8; 32] = init.client_nonce.try_into().expect("nonce size");
        let proof = proof_ser(&server_kp, &nonce, &exporter);
        write_frame(&mut s2c, &rmp_serde::to_vec(&proof).expect("encode proof"))
            .await
            .expect("send proof");

        // 2) THE ORACLE: with a valid proof the client proceeds and the
        //    next frame MUST decode as a ResumeInit carrying the ticket.
        let frame = read_frame(&mut c2s, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("resume init frame must follow a valid proof");
        let resume_init: WireResumeInitDeser =
            rmp_serde::from_slice(&frame).expect("decode ResumeInit");
        assert_eq!(resume_init.ticket, server_ticket, "ticket must match input");
        assert_eq!(
            resume_init.client_nonce.len(),
            32,
            "client_nonce must be 32 bytes"
        );

        // 3) reply a valid ResumeOkWire.
        let ok = WireResumeOkSer {
            session_id,
            expires_at_ns: 999_000_000_000,
            resumption_ticket: rotated,
            resumption_expires_at_ns: 888_000_000_000,
            server_query_version: 2,
        };
        write_frame(&mut s2c, &rmp_serde::to_vec(&ok).expect("encode resume ok"))
            .await
            .expect("send resume ok");
    });

    let resumed = resume_wire_exchange(
        &mut client_reader,
        &mut client_writer,
        ticket,
        pinned_hash,
        exporter,
    )
    .await
    .expect("valid-proof exchange must succeed");
    assert_eq!(resumed.session_id, [0x42_u8; 32]);
    assert_eq!(resumed.expires_at_ns, 999_000_000_000);
    assert_eq!(
        resumed.resumption_ticket.as_deref(),
        Some(&[0xDD_u8; 48][..])
    );
    assert_eq!(resumed.resumption_expires_at_ns, Some(888_000_000_000));
    assert_eq!(resumed.server_query_version, 2);

    drop(client_reader);
    drop(client_writer);
    server.await.expect("emulated server task panicked");
}
