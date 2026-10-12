use serde::Serialize;
use shamir_types::types::value::QueryValue;

use crate::batch::{TransactionInfo, VisibilityPendingReason, VisibilityState};
use crate::read::{QueryResult, VisibilityDependencyCode, VisibilityDependencyPending};

/// Old-protocol fixture: a result as peers encoded it before the receipt field.
#[derive(Serialize)]
struct LegacyResult {
    records: Vec<u8>,
}

#[derive(Serialize)]
struct MalformedReceipt {
    tx_id: u64,
    status: &'static str,
    commit_version: u64,
    materialized: bool,
}

#[derive(Serialize)]
struct MalformedResult {
    records: Vec<u8>,
    receipt: MalformedReceipt,
}

/// Wire-format tests intentionally inspect named fields, not query construction.
fn named(value: &impl Serialize) -> QueryValue {
    rmp_serde::from_slice(&rmp_serde::to_vec_named(value).unwrap()).unwrap()
}

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
        let wire = named(&result);
        assert_eq!(wire["receipt"]["status"].as_str(), Some("committed"));
        assert_eq!(wire["receipt"]["error"].as_str(), Some("wal_durability"));
        assert!(wire["receipt"].get("visibility").is_some());
    }
    let legacy = rmp_serde::to_vec_named(&LegacyResult {
        records: Vec::new(),
    })
    .unwrap();
    let result: QueryResult = rmp_serde::from_slice(&legacy).unwrap();
    assert!(result.receipt.is_none());
    assert!(named(&result).get("receipt").is_none());
}

#[test]
fn dependency_failure_has_stable_code_and_lossless_positions() {
    let failure = VisibilityDependencyPending {
        code: VisibilityDependencyCode::VisibilityDependencyPending,
        commit_version: u64::MAX,
        visible_through: u64::MAX - 2,
    };
    let wire = named(&failure);
    assert_eq!(wire["code"].as_str(), Some("visibility_dependency_pending"));
    let bytes = rmp_serde::to_vec_named(&failure).unwrap();
    let decoded: VisibilityDependencyPending = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, failure);
    let malformed = rmp_serde::to_vec_named(&MalformedResult {
        records: Vec::new(),
        receipt: MalformedReceipt {
            tx_id: 7,
            status: "committed",
            commit_version: 43,
            materialized: false,
        },
    })
    .unwrap();
    assert!(rmp_serde::from_slice::<QueryResult>(&malformed).is_err());
}
