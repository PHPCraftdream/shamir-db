use crate::common::push_envelope::{PushEnvelope, PushKind};

#[test]
fn push_kind_round_trip() {
    for (kind, expected) in [
        (PushKind::Event, "event"),
        (PushKind::Gap, "gap"),
        (PushKind::SlowConsumer, "slow_consumer"),
        (PushKind::Closed, "closed"),
    ] {
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(json, serde_json::json!(expected));
        let back: PushKind = serde_json::from_value(json).unwrap();
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
    let json = serde_json::to_value(&envelope).unwrap();
    let back: PushEnvelope = serde_json::from_value(json).unwrap();
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
    let json = serde_json::to_value(&envelope).unwrap();
    let back: PushEnvelope = serde_json::from_value(json).unwrap();
    assert_eq!(back, envelope);
    assert_eq!(back.gap_at, Some(50));
}
