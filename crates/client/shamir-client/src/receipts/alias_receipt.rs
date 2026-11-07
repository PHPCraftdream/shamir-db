//! [`AliasReceipt`] — a per-alias implicit-commit receipt of a
//! non-transactional batch.

use shamir_query_types::batch::{TransactionInfo, VisibilityState};

/// One write alias's implicit commit receipt, extracted from a
/// [`BatchResponse`](shamir_query_types::batch::BatchResponse).
///
/// The receipt is a KNOWN COMMITTED decision; pending visibility is never
/// an error and never an instruction to resend. `commit_version` /
/// `visibility` positions are scoped to `repo`'s CURRENT incarnation: a
/// repository deleted and recreated under the same name is a different
/// incarnation, and the wire carries no incarnation id — never reconcile
/// a receipt across a delete/recreate.
#[derive(Debug, Clone, PartialEq)]
#[must_use = "inspect visibility and reconcile pending positions per repository"]
pub struct AliasReceipt {
    /// The batch alias this receipt belongs to.
    pub alias: String,
    /// The validated committed transaction metadata (also carries any
    /// published `wal_durability` warning in `error`).
    pub transaction: TransactionInfo,
    /// The sampled settled-prefix observation at acknowledgement time.
    pub visibility: VisibilityState,
    /// Repository the position belongs to, derived from the request's
    /// table reference. `None` when it cannot be derived unambiguously
    /// (e.g. the alias is a nested sub-batch / for-each body) —
    /// reconciliation refuses such receipts rather than guess.
    pub repo: Option<String>,
}
