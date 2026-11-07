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
    let wire_value: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        wire_value["wal_retention_max_bytes"],
        QueryValue::Str("18446744073709551615".to_string())
    );
    let decoded: DurabilityOverride = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, value);

    let alter = crate::admin::AlterRepoOp {
        alter_repo: "main".to_string(),
        durability: Some(value),
    };
    let op = crate::batch::BatchOp::AlterRepo(alter);
    let op_bytes = rmp_serde::to_vec_named(&op).unwrap();
    let wire_op: QueryValue = rmp_serde::from_slice(&op_bytes).unwrap();
    assert_eq!(
        wire_op["durability"]["wal_retention_max_bytes"],
        QueryValue::Str("18446744073709551615".to_string())
    );
    let decoded_op: crate::batch::BatchOp = rmp_serde::from_slice(&op_bytes).unwrap();
    assert!(matches!(decoded_op, crate::batch::BatchOp::AlterRepo(_)));
    let qv = mpack!({
        "alter_repo": "main",
        "durability": {
            "wal_retention_max_bytes": @(QueryValue::Str(u64::MAX.to_string()))
        }
    });
    let qv_bytes = rmp_serde::to_vec_named(&qv).unwrap();
    let decoded_op: crate::batch::BatchOp = rmp_serde::from_slice(&qv_bytes).unwrap();
    let crate::batch::BatchOp::AlterRepo(decoded_op) = decoded_op else {
        panic!("expected AlterRepo operation");
    };
    assert_eq!(
        decoded_op.durability.unwrap().wal_retention_max_bytes,
        Some(expected)
    );
}
