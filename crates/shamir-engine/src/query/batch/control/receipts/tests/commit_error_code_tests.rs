//! Wire-code mapping for failed implicit-batch commits
//! (`receipts::commit::receipt_error_code` → `BatchError::QueryError.code`).

use crate::query::batch::receipts::commit::receipt_error_code;
use crate::tx::CommitError;
use shamir_storage::error::DbError;

#[test]
fn rv_types_2eb39dfd_receipt_storage_error_preserves_wire_code() {
    let error = DbError::InternerQuotaExceeded {
        repo: "repo-17".to_string(),
        kind: shamir_types::core::interner::InternerQuotaKind::Names,
        limit: 7,
    };
    assert_eq!(
        receipt_error_code(&CommitError::Storage(error)),
        Some("interner_quota_exceeded"),
    );
}

/// S5c-X: commit admission refusal under `on_wal_retention_exceeded:
/// refuse_writes` must carry the stable wire code `wal_retention_exceeded`
/// (same vocabulary as the interactive `TxError` mapping in
/// `tx/commit.rs::into_transaction_info`). The refusal is retryable: it
/// clears by itself once the drainer truncates the retained sealed WAL back
/// under `durability.wal_retention_max_bytes`.
#[test]
fn wal_retention_exceeded_maps_to_stable_wire_code() {
    assert_eq!(
        receipt_error_code(&CommitError::WalRetentionExceeded),
        Some("wal_retention_exceeded"),
    );
}

/// The neighbouring WAL gate keeps its existing code; the two codes must not
/// collapse into one (reopen = reconcile first, no blind retry; retention =
/// transient admission refusal).
#[test]
fn wal_requires_reopen_keeps_its_own_wire_code() {
    assert_eq!(
        receipt_error_code(&CommitError::WalRequiresReopen),
        Some("wal_requires_reopen"),
    );
}
