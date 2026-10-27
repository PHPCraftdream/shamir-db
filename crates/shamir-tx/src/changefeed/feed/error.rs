//! Typed error for the changefeed journal boundary (RV-TX-FF6AB063).

use thiserror::Error;

/// Error surfaced by [`crate::ChangelogStore`] and the journal writer.
#[derive(Debug, Error)]
pub enum ChangelogError {
    /// Durable-store put/range failure or flush boundary violation.
    #[error("changelog journal: {0}")]
    Journal(String),
    /// Journal record serialization failure.
    #[error("changelog serialize: {0}")]
    Serialize(String),
    /// Durable-install put failure for one commit version.
    #[error("journal install put v{version}: {message}")]
    Install { version: u64, message: String },
    /// The awaiting caller dropped its acknowledgement channel.
    #[error("changelog caller cancelled")]
    Cancelled,
    /// A single journal event exceeds the read byte budget.
    #[error("changelog event exceeds {budget} byte budget")]
    Oversized { budget: usize },
}

/// Convenience alias for results at the changefeed boundary.
pub type ChangelogResult<T> = Result<T, ChangelogError>;
