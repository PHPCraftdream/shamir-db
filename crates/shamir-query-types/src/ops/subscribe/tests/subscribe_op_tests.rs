use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::BatchOp;
use crate::subscribe::{SubscribeOp, SubscriptionSource, UnsubscribeOp};
use crate::TableRef;

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

#[test]
fn subscribe_op_round_trip() {
    let op = SubscribeOp {
        subscribe: vec![SubscriptionSource {
            table: TableRef::new("users"),
            filter: None,
            events: Default::default(),
        }],
        deliver: Default::default(),
        initial: true,
        from_version: Some(42),
    };
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let back: SubscribeOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(back, op);
}

#[test]
fn unsubscribe_op_round_trip() {
    let op = UnsubscribeOp { unsubscribe: 7 };
    let qv = to_qv(&op);
    assert_eq!(qv.get("unsubscribe").and_then(QueryValue::as_i64), Some(7));
    let back: UnsubscribeOp = from_qv(qv);
    assert_eq!(back, op);
}

#[test]
fn batch_op_subscribe_discriminator() {
    let v = mpack!({
        "subscribe": [
            {
                "table": "users"
            }
        ]
    });
    let op: BatchOp = from_qv(v);
    assert!(matches!(op, BatchOp::Subscribe(_)));
}

#[test]
fn batch_op_unsubscribe_discriminator() {
    let v = mpack!({"unsubscribe": 99_i64});
    let op: BatchOp = from_qv(v);
    assert!(matches!(op, BatchOp::Unsubscribe(_)));
}

#[test]
fn subscribe_is_not_admin() {
    let op = BatchOp::Subscribe(SubscribeOp {
        subscribe: vec![],
        deliver: Default::default(),
        initial: false,
        from_version: None,
    });
    assert!(!op.is_admin());
}

#[test]
fn unsubscribe_is_not_admin() {
    let op = BatchOp::Unsubscribe(UnsubscribeOp { unsubscribe: 1 });
    assert!(!op.is_admin());
}
