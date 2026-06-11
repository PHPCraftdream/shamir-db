use crate::subscribe::DeliverMode;

#[test]
fn deliver_mode_round_trip_records() {
    let mode = DeliverMode::Records;
    let json = serde_json::to_value(&mode).unwrap();
    assert_eq!(json, serde_json::json!("records"));
    let back: DeliverMode = serde_json::from_value(json).unwrap();
    assert_eq!(back, mode);
}

#[test]
fn deliver_mode_round_trip_keys() {
    let mode = DeliverMode::Keys;
    let json = serde_json::to_value(&mode).unwrap();
    assert_eq!(json, serde_json::json!("keys"));
    let back: DeliverMode = serde_json::from_value(json).unwrap();
    assert_eq!(back, mode);
}

#[test]
fn deliver_mode_default_is_records() {
    assert_eq!(DeliverMode::default(), DeliverMode::Records);
}
