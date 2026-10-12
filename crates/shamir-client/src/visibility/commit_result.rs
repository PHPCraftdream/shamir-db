use shamir_query_types::batch::{TransactionInfo, VisibilityState};

use crate::ClientError;

/// A known committed decision, independent of visibility and WAL durability.
#[derive(Debug, Clone, PartialEq)]
#[must_use = "inspect visibility and the retained transaction durability warning"]
pub enum CommitResult {
    Committed {
        transaction: TransactionInfo,
        visibility: VisibilityState,
    },
}

impl CommitResult {
    pub fn from_transaction(transaction: TransactionInfo) -> Result<Self, ClientError> {
        transaction
            .validate()
            .map_err(|e| ClientError::Protocol(e.into()))?;
        if !transaction.is_committed() {
            return Err(ClientError::TransactionOutcome(Box::new(transaction)));
        }
        let visibility = transaction
            .visibility
            .ok_or_else(|| ClientError::Protocol("missing committed visibility".into()))?;
        Ok(Self::Committed {
            transaction,
            visibility,
        })
    }
}
