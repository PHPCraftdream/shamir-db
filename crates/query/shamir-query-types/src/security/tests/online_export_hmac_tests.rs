use crate::batch::BatchOp;
use crate::hmac::{canonical_batch_op, hmac_policy, HmacPolicy};

#[test]
fn online_export_ops_are_not_hmac_gated() {
    // R3 2b: the online logical-export ops follow the online-backup
    // policy — NotGated (nothing destructive; superuser-only admin
    // classification at the handler), and no canonical v2 arm, so the
    // canonical encoder rejects them as unsupported.
    for wire in [
        serde_json::json!({
            "start_export": true,
            "database": "appdb"
        }),
        serde_json::json!({
            "export_status": true
        }),
        serde_json::json!({
            "cancel_export": true
        }),
    ] {
        let op: BatchOp = serde_json::from_value(wire).unwrap();
        assert_eq!(hmac_policy(&op), HmacPolicy::NotGated);
        assert!(matches!(
            canonical_batch_op("", &op),
            Err(crate::hmac::CanonicalOpError::UnsupportedOperation)
        ));
    }
}
