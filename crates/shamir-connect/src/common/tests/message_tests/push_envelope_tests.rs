use crate::common::push_envelope::{PushEnvelope, PushKind};

#[test]
fn push_kind_round_trip() {
    for kind in [
        PushKind::Event,
        PushKind::Gap,
        PushKind::SlowConsumer,
        PushKind::Closed,
    ] {
        let bytes = rmp_serde::to_vec_named(&kind).unwrap();
        let back: PushKind = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, kind);
    }
}

#[test]
fn push_envelope_round_trip() {
    let envelope = PushEnvelope {
        push: PushKind::Event,
        sub: 42,
        seq: 7,
        data: Some(vec![1, 2, 3]),
        gap_at: None,
    };
    let bytes = rmp_serde::to_vec_named(&envelope).unwrap();
    let back: PushEnvelope = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, envelope);
}

#[test]
fn push_envelope_gap() {
    let envelope = PushEnvelope {
        push: PushKind::Gap,
        sub: 1,
        seq: 100,
        data: None,
        gap_at: Some(50),
    };
    let bytes = rmp_serde::to_vec_named(&envelope).unwrap();
    let back: PushEnvelope = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, envelope);
    assert_eq!(back.gap_at, Some(50));
}

/// RV-CONNECT-60D4DAB1: `data` is a binary field — the msgpack wire marker
/// for the payload must be `bin 8` (0xc4), not a `u8` array (0x93/0x99...).
/// Pre-fix (no `serde_bytes`), rmp-serde encoded `Option<Vec<u8>>` as an
/// array, so the 0xc4 marker never appeared.
#[test]
fn rv_connect_60d4dab1_push_data_encodes_as_msgpack_bin() {
    let envelope = PushEnvelope {
        push: PushKind::Event,
        sub: 1,
        seq: 2,
        data: Some(vec![1, 2, 3]),
        gap_at: None,
    };
    let bytes = rmp_serde::to_vec_named(&envelope).unwrap();
    // bin 8 header + 3-byte length + payload
    let expected: [u8; 5] = [0xc4, 0x03, 0x01, 0x02, 0x03];
    assert!(
        bytes.windows(5).any(|w| w == expected),
        "data must encode as bin, got {bytes:02x?}"
    );
    // Still round-trips.
    let back: PushEnvelope = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, envelope);
}
