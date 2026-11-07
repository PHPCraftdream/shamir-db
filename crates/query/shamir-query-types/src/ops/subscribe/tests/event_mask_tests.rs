use shamir_types::types::value::QueryValue;

use crate::subscribe::EventMask;

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
fn event_mask_round_trip_all() {
    let mask = EventMask::All;
    assert_eq!(to_str_qv(&mask).as_deref(), Some("all"));
    let back = roundtrip(&mask);
    assert_eq!(back, mask);
}

#[test]
fn event_mask_round_trip_put() {
    let mask = EventMask::Put;
    assert_eq!(to_str_qv(&mask).as_deref(), Some("put"));
    let back = roundtrip(&mask);
    assert_eq!(back, mask);
}

#[test]
fn event_mask_round_trip_delete() {
    let mask = EventMask::Delete;
    assert_eq!(to_str_qv(&mask).as_deref(), Some("delete"));
    let back = roundtrip(&mask);
    assert_eq!(back, mask);
}

#[test]
fn event_mask_default_is_all() {
    assert_eq!(EventMask::default(), EventMask::All);
}
