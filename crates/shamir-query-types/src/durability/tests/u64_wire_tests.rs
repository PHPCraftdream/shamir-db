use super::super::DurabilityOverride;
use core::num::NonZeroU64;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

#[test]
fn override_u64_roundtrips_messagepack_and_query_value_big() {
    let expected = NonZeroU64::new(u64::MAX).unwrap();
    let value = DurabilityOverride {
        wal_retention_max_bytes: Some(expected),
        ..Default::default()
    };
    let bytes = rmp_serde::to_vec_named(&value).unwrap();
    let decoded: DurabilityOverride = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, value);

    let qv = mpack!({ "wal_retention_max_bytes": @(QueryValue::Str(u64::MAX.to_string())) });
    let qv_bytes = rmp_serde::to_vec_named(&qv).unwrap();
    let decoded_qv: QueryValue = rmp_serde::from_slice(&qv_bytes).unwrap();
    assert_eq!(decoded_qv, qv);
}
