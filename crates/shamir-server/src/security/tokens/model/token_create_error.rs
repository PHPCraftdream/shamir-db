use crate::security::tokens::TokenStoreError;

/// Token issuance failure.
#[derive(Debug, thiserror::Error)]
pub enum TokenCreateError {
    /// Invalid requested token lifetime.
    #[error("token lifetime is invalid")]
    TokenTtlInvalid,
    /// Owner already has an active token with this label.
    #[error("token label already exists")]
    LabelExists,
    /// Owner reached the active-token cap.
    #[error("active token limit reached")]
    LimitReached,
    /// Label or database allowlist violates shape constraints.
    #[error("token fields are invalid")]
    InvalidInput,
    /// Mutation is unavailable on this node.
    #[error("token mutation is unavailable on a read-only node")]
    ReadOnlyNode,
    /// Token does not exist in durable storage.
    #[error("token not found")]
    NotFound,
    /// Durable store failed; no memory publication was made.
    #[error("token store failed")]
    Store(#[source] TokenStoreError),
    /// Repeated generated token identifiers collided with durable records.
    #[error("token identifier collision retries exhausted")]
    IdCollision,
    /// Entropy source failed.
    #[error("token entropy unavailable")]
    Entropy,
    /// Current clock could not be read.
    #[error("token clock unavailable")]
    Clock,
}
