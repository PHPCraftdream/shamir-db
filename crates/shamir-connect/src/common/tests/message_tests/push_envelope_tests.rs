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
