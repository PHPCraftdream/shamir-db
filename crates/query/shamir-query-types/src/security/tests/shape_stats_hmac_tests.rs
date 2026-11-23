use crate::batch::BatchOp;
use crate::hmac::{canonical_batch_op, hmac_policy, HmacPolicy};

#[test]
fn shape_stats_ops_are_not_hmac_gated() {
    // R4b-4: shape-statistics introspection and the local telemetry wipe are
    // permission-checked at the handler, not destructive to stored data, so
    // they are NotGated with no canonical v2 arm.
    //
    // mutant: move `ListShapeStats` / `ResetShapeStats` into the `Required`
    // arm of `hmac_policy` => the first assertion fails.
    let list = BatchOp::ListShapeStats(crate::admin::shape_stats::ListShapeStatsReq {
        repo: "main".into(),
        limit: None,
    });
    assert_eq!(hmac_policy(&list), HmacPolicy::NotGated);
    assert!(matches!(
        canonical_batch_op("", &list),
        Err(crate::hmac::CanonicalOpError::UnsupportedOperation)
    ));

    let reset = BatchOp::ResetShapeStats(crate::admin::shape_stats::ResetShapeStatsReq {
        repo: "main".into(),
    });
    assert_eq!(hmac_policy(&reset), HmacPolicy::NotGated);
    assert!(matches!(
        canonical_batch_op("", &reset),
        Err(crate::hmac::CanonicalOpError::UnsupportedOperation)
    ));
}
