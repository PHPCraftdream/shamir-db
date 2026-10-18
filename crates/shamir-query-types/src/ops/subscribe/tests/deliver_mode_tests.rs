use shamir_types::types::value::QueryValue;

use crate::subscribe::DeliverMode;

fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
    v: &T,
) -> T {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn to_str_qv<T: serde::Serialize>(v: &T) -> Option<String> {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    qv.as_str().map(|s| s.to_owned())
}

#[test]
fn deliver_mode_round_trip_records() {
    let mode = DeliverMode::Records;
    assert_eq!(to_str_qv(&mode).as_deref(), Some("records"));
    let back = roundtrip(&mode);
    assert_eq!(back, mode);
}

#[test]
fn deliver_mode_round_trip_keys() {
    let mode = DeliverMode::Keys;
    assert_eq!(to_str_qv(&mode).as_deref(), Some("keys"));
    let back = roundtrip(&mode);
    assert_eq!(back, mode);
}

#[test]
fn deliver_mode_default_is_records() {
    assert_eq!(DeliverMode::default(), DeliverMode::Records);
}
