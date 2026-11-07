use crate::{
    ClientError, CommitResult, DbResponse, TransactionInfo, VisibilityPendingReason,
    VisibilityState,
};

#[test]
fn pending_decode_is_committed_and_retains_durability_warning() {
    let visibility = VisibilityState::Pending {
        visible_through: 9_007_199_254_740_992,
        blocking_version: 9_007_199_254_740_993,
        reason: VisibilityPendingReason::Timeout,
    };
    let transaction =
        TransactionInfo::committed_with_error(7, 40, u64::MAX, false, "wal_durability")
            .with_visibility(visibility);
    let bytes = rmp_serde::to_vec_named(&DbResponse::TxCommitted {
        transaction: transaction.clone(),
    })
    .unwrap();
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).unwrap();
    let DbResponse::TxCommitted { transaction: info } = decoded else {
        panic!("wrong response: {decoded:?}");
    };
    assert_eq!(
        CommitResult::from_transaction(info).unwrap(),
        CommitResult::Committed {
            transaction,
            visibility,
        },
    );
}

#[test]
fn abort_and_indeterminate_keep_their_classification() {
    for reason in [
        "tx_conflict",
        "wal_requires_reopen",
        "wal_retention_exceeded",
    ] {
        let result = CommitResult::from_transaction(TransactionInfo::aborted(9, reason));
        let Err(ClientError::TransactionOutcome(info)) = result else {
            panic!("expected noncommitted outcome: {result:?}");
        };
        assert_eq!(info.status, "aborted");
        assert_eq!(info.reason.as_deref(), Some(reason));
        assert_eq!(info.commit_version, None);
        assert_eq!(info.visibility, None);
    }
}

#[test]
fn typed_errors_use_wire_fields_not_message_parsing() {
    let response = DbResponse::AsOfVisibilityTimeout {
        code: "as_of_visibility_timeout".into(),
        message: "opaque".into(),
        requested: u64::MAX,
        visible_through: 9_007_199_254_740_992,
        blocking_version: 9_007_199_254_740_993,
    };
    let result = crate::visibility::map_response(response);
    assert!(
        matches!(
            result,
            Err(ClientError::AsOfVisibilityTimeout {
                requested: u64::MAX,
                visible_through: 9_007_199_254_740_992,
                blocking_version: 9_007_199_254_740_993,
                ..
            })
        ),
        "{result:?}",
    );
    let bytes = rmp_serde::to_vec_named(&DbResponse::Error {
        code: "wal_retention_exceeded".into(),
        message: "retained WAL budget is full".into(),
    })
    .unwrap();
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).unwrap();
    let mapped = crate::visibility::map_response(decoded).unwrap_err();
    assert!(matches!(
        mapped,
        ClientError::Db { code, message }
            if code == "wal_retention_exceeded" && message == "retained WAL budget is full"
    ));

    for code in [
        "as_of_future_version",
        "as_of_reopen_required",
        "resync_required",
        "visibility_future_version",
        "visibility_wait_invalid",
        "write_committed_visibility_pending",
    ] {
        let result = crate::visibility::map_response(DbResponse::Error {
            code: code.into(),
            message: "opaque".into(),
        })
        .unwrap_err();
        let message = match (code, result) {
            ("as_of_future_version", ClientError::AsOfFutureVersion { message })
            | ("as_of_reopen_required", ClientError::AsOfReopenRequired { message })
            | ("resync_required", ClientError::ResyncRequired { message })
            | ("visibility_future_version", ClientError::VisibilityFutureVersion { message })
            | ("visibility_wait_invalid", ClientError::VisibilityWaitInvalid { message })
            | (
                "write_committed_visibility_pending",
                ClientError::WriteCommittedVisibilityPending { message },
            ) => message,
            (_, other) => panic!("unexpected mapping: {other:?}"),
        };
        assert_eq!(message, "opaque");
    }
}

// RV-WAL-3B29232E: indeterminate outcomes keep TransactionOutcome error
// classification across a wire round-trip.
#[test]
fn rv_wal_3b29232e_indeterminate_keeps_error_classification() {
    let info = TransactionInfo::indeterminate(9, 100, 9);
    let result = CommitResult::from_transaction(info.clone());
    let Err(ClientError::TransactionOutcome(outcome)) = result else {
        panic!("expected indeterminate outcome error: {result:?}");
    };
    assert_eq!(outcome.status, "indeterminate");
    assert_eq!(outcome.commit_version, Some(9));

    let bytes = rmp_serde::to_vec_named(&DbResponse::TxCommitted {
        transaction: info.clone(),
    })
    .unwrap();
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).unwrap();
    let DbResponse::TxCommitted { transaction } = decoded else {
        panic!("wrong response");
    };
    assert_eq!(transaction, info);
    assert!(matches!(
        CommitResult::from_transaction(transaction),
        Err(ClientError::TransactionOutcome(_))
    ));
}
