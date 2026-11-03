//! Three-way result of persisting a table rename.

use shamir_storage::error::DbError;

/// Describes what the catalogue persistence layer knows about a rename.
#[derive(Debug)]
pub enum RenamePersistOutcome {
    /// The destination catalogue row is committed and durable.
    Committed,
    /// This fresh attempt is known not to have committed.
    NotCommitted(DbError),
    /// Persistence failed without enough evidence to determine the outcome.
    InDoubt(DbError),
}
