use crate::batch::BatchOp;
use crate::subscribe::{SubscribeOp, SubscriptionSource, UnsubscribeOp};
use crate::TableRef;

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
    let json = serde_json::to_value(&op).unwrap();
    let back: SubscribeOp = serde_json::from_value(json).unwrap();
    assert_eq!(back, op);
}

#[test]
fn unsubscribe_op_round_trip() {
    let op = UnsubscribeOp { unsubscribe: 7 };
    let json = serde_json::to_value(&op).unwrap();
    assert_eq!(json, serde_json::json!({"unsubscribe": 7}));
    let back: UnsubscribeOp = serde_json::from_value(json).unwrap();
    assert_eq!(back, op);
}

#[test]
fn batch_op_subscribe_discriminator() {
    let json = serde_json::json!({
        "subscribe": [
            {
                "table": "users"
            }
        ]
    });
    let op: BatchOp = serde_json::from_value(json).unwrap();
    assert!(matches!(op, BatchOp::Subscribe(_)));
}

#[test]
fn batch_op_unsubscribe_discriminator() {
    let json = serde_json::json!({"unsubscribe": 99});
    let op: BatchOp = serde_json::from_value(json).unwrap();
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
