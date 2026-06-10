//! Wire-format round-trip tests for the resumption frames.
//!
//! These tests exercise only serialisation/deserialisation of
//! [`WireResumeInit`] and [`WireResumeOk`] — no server required.

use crate::wire_frames::{WireResumeInit, WireResumeOk};

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
