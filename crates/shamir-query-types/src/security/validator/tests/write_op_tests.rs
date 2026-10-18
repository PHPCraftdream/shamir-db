use crate::validator::WriteOp;

#[test]
fn serde_round_trip_insert() {
    let bytes = rmp_serde::to_vec_named(&WriteOp::Insert).unwrap();
    let back: WriteOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, WriteOp::Insert);
}

#[test]
fn serde_round_trip_update() {
    let bytes = rmp_serde::to_vec_named(&WriteOp::Update).unwrap();
    let back: WriteOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, WriteOp::Update);
}

#[test]
fn serde_round_trip_upsert() {
    let bytes = rmp_serde::to_vec_named(&WriteOp::Upsert).unwrap();
    let back: WriteOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, WriteOp::Upsert);
}

#[test]
fn serde_round_trip_delete() {
    let bytes = rmp_serde::to_vec_named(&WriteOp::Delete).unwrap();
    let back: WriteOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, WriteOp::Delete);
}
