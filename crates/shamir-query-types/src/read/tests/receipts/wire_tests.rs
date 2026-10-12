use crate::batch::{TransactionInfo, VisibilityPendingReason, VisibilityState};
use crate::read::{QueryResult, VisibilityDependencyCode, VisibilityDependencyPending};

// Wire-format tests intentionally inspect named fields, not query construction.
#[test]
fn receipts_roundtrip_absence_visible_pending_and_durability_warning() {
    for visibility in [
        VisibilityState::Visible,
        VisibilityState::Pending {
            visible_through: 41,
            blocking_version: 42,
            reason: VisibilityPendingReason::Timeout,
        },
    ] {
        let result = QueryResult {
            receipt: Some(
                TransactionInfo::committed_with_error(7, 40, 43, true, "wal_durability")
                    .with_visibility(visibility),
            ),
            ..QueryResult::default()
        };
        let bytes = rmp_serde::to_vec_named(&result).unwrap();
        let decoded: QueryResult = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, result);
        let wire: serde_json::Value = serde_json::to_value(&result).unwrap();
        assert_eq!(wire["receipt"]["status"], "committed");
        assert_eq!(wire["receipt"]["error"], "wal_durability");
        assert!(wire["receipt"]["visibility"].is_object());
    }
    let old = serde_json::json!({
        "records": []
    });
    let result: QueryResult = serde_json::from_value(old).unwrap();
    assert!(result.receipt.is_none());
    assert!(serde_json::to_value(result)
        .unwrap()
        .get("receipt")
        .is_none());
}

#[test]
fn dependency_failure_has_stable_code_and_lossless_positions() {
    let failure = VisibilityDependencyPending {
        code: VisibilityDependencyCode::VisibilityDependencyPending,
        commit_version: u64::MAX,
        visible_through: u64::MAX - 2,
    };
    let wire = serde_json::to_value(&failure).unwrap();
    assert_eq!(wire["code"], "visibility_dependency_pending");
    let bytes = rmp_serde::to_vec_named(&failure).unwrap();
    let decoded: VisibilityDependencyPending = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, failure);
    let malformed = serde_json::json!({
        "records": [],
        "receipt": {
            "tx_id": 7,
            "status": "committed",
            "commit_version": 43
        }
    });
    assert!(serde_json::from_value::<QueryResult>(malformed).is_err());
}
