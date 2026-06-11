use crate::subscribe::EventMask;

#[test]
fn event_mask_round_trip_all() {
    let mask = EventMask::All;
    let json = serde_json::to_value(&mask).unwrap();
    assert_eq!(json, serde_json::json!("all"));
    let back: EventMask = serde_json::from_value(json).unwrap();
    assert_eq!(back, mask);
}

#[test]
fn event_mask_round_trip_put() {
    let mask = EventMask::Put;
    let json = serde_json::to_value(&mask).unwrap();
    assert_eq!(json, serde_json::json!("put"));
    let back: EventMask = serde_json::from_value(json).unwrap();
    assert_eq!(back, mask);
}

#[test]
fn event_mask_round_trip_delete() {
    let mask = EventMask::Delete;
    let json = serde_json::to_value(&mask).unwrap();
    assert_eq!(json, serde_json::json!("delete"));
    let back: EventMask = serde_json::from_value(json).unwrap();
    assert_eq!(back, mask);
}

#[test]
fn event_mask_default_is_all() {
    assert_eq!(EventMask::default(), EventMask::All);
}
