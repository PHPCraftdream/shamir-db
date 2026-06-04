use crate::validator::WriteOp;

#[test]
fn serde_round_trip_insert() {
    let json = serde_json::to_string(&WriteOp::Insert).unwrap();
    assert_eq!(json, r#""insert""#);
    let back: WriteOp = serde_json::from_str(&json).unwrap();
    assert_eq!(back, WriteOp::Insert);
}

#[test]
fn serde_round_trip_update() {
    let json = serde_json::to_string(&WriteOp::Update).unwrap();
    assert_eq!(json, r#""update""#);
    let back: WriteOp = serde_json::from_str(&json).unwrap();
    assert_eq!(back, WriteOp::Update);
}

#[test]
fn serde_round_trip_upsert() {
    let json = serde_json::to_string(&WriteOp::Upsert).unwrap();
    assert_eq!(json, r#""upsert""#);
    let back: WriteOp = serde_json::from_str(&json).unwrap();
    assert_eq!(back, WriteOp::Upsert);
}

#[test]
fn serde_round_trip_delete() {
    let json = serde_json::to_string(&WriteOp::Delete).unwrap();
    assert_eq!(json, r#""delete""#);
    let back: WriteOp = serde_json::from_str(&json).unwrap();
    assert_eq!(back, WriteOp::Delete);
}
