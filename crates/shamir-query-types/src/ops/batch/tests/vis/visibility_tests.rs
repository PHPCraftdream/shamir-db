use serde::{de::DeserializeOwned, Serialize};
use shamir_types::mpack;

use crate::batch::{TransactionInfo, VisibilityPendingReason, VisibilityState, VisibilityWait};

fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
    let bytes = rmp_serde::to_vec_named(&value).expect("encode");
    let back: T = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(back, value);
}

#[test]
fn visibility_contract_roundtrips_every_variant_and_u64_boundary() {
    roundtrip(VisibilityWait::Strict);
    for timeout_ms in [0, 1, u64::MAX] {
        roundtrip(VisibilityWait::Bounded { timeout_ms });
    }
    roundtrip(VisibilityState::Visible);
    for reason in [
        VisibilityPendingReason::Timeout,
        VisibilityPendingReason::ReopenRequired,
    ] {
        roundtrip(reason);
        for visible_through in [0, 41, u64::MAX - 1] {
            let visibility = VisibilityState::Pending {
                visible_through,
                blocking_version: visible_through + 1,
                reason,
            };
            roundtrip(visibility);
            for materialized in [true, false] {
                let info = TransactionInfo::committed_with_error(
                    7,
                    40,
                    u64::MAX,
                    materialized,
                    "wal_durability",
                )
                .with_visibility(visibility);
                assert!(info.is_committed());
                assert!(!info.is_visible());
                assert_eq!(info.reason, None);
                assert_eq!(info.error.as_deref(), Some("wal_durability"));
                assert_eq!(info.validate(), Ok(()));
                roundtrip(info);
            }
        }
    }
    let visible = TransactionInfo::committed(7, 40, 43, false);
    assert!(visible.is_visible());
    roundtrip(visible);
    let aborted = TransactionInfo::aborted(7, "tx_conflict");
    assert!(!aborted.is_visible());
    assert_eq!(aborted.visibility, None);
    roundtrip(aborted);
}

#[test]
fn wait_validation_preserves_zero_and_rejects_instead_of_clamping() {
    assert_eq!(
        VisibilityWait::Bounded { timeout_ms: 0 }.validate(0, false),
        Ok(())
    );
    assert_eq!(
        VisibilityWait::Bounded { timeout_ms: 30 }.validate(30, false),
        Ok(())
    );
    assert!(VisibilityWait::Bounded { timeout_ms: 31 }
        .validate(30, true)
        .is_err());
    assert!(VisibilityWait::Strict.validate(30, false).is_err());
    assert_eq!(VisibilityWait::Strict.validate(0, true), Ok(()));
}

#[test]
fn malformed_committed_visibility_is_not_assumed_visible() {
    // Wire-format subject: legacy committed metadata must not opt into v3.
    for payload in [
        mpack!({
            "tx_id": 7_i64,
            "status": "committed"
        }),
        mpack!({
            "tx_id": 7_i64,
            "status": "committed",
            "visibility": null
        }),
        mpack!({
            "tx_id": 7_i64,
            "status": "aborted",
            "visibility": { "state": "visible" }
        }),
        mpack!({
            "tx_id": 7_i64,
            "status": "committed",
            "visibility": {
                "state": "pending",
                "visible_through": 41_i64,
                "blocking_version": 43_i64,
                "reason": "timeout"
            }
        }),
    ] {
        let bytes = rmp_serde::to_vec_named(&payload).expect("encode fixture");
        assert!(rmp_serde::from_slice::<TransactionInfo>(&bytes).is_err());
    }
    let overflow = VisibilityState::Pending {
        visible_through: u64::MAX,
        blocking_version: 0,
        reason: VisibilityPendingReason::Timeout,
    };
    assert!(overflow.validate().is_err());
}

#[test]
fn visibility_tags_match_independent_wire_vectors() {
    // Wire-format subject: vectors come from the VIS-1 brief, not the encoder.
    let bytes = rmp_serde::to_vec_named(&mpack!({
        "mode": "bounded",
        "timeout_ms": 0_i64
    }))
    .unwrap();
    assert_eq!(
        rmp_serde::from_slice::<VisibilityWait>(&bytes).unwrap(),
        VisibilityWait::Bounded { timeout_ms: 0 }
    );
    let bytes = rmp_serde::to_vec_named(&mpack!({
        "state": "pending",
        "visible_through": 41_i64,
        "blocking_version": 42_i64,
        "reason": "reopen_required"
    }))
    .unwrap();
    assert_eq!(
        rmp_serde::from_slice::<VisibilityState>(&bytes).unwrap(),
        VisibilityState::Pending {
            visible_through: 41,
            blocking_version: 42,
            reason: VisibilityPendingReason::ReopenRequired,
        }
    );
}
