use thiserror::Error;

use shamir_types::codecs::CodecError;

/// A generic error type for all database and storage operations.
#[derive(Error, Debug)]
pub enum DbError {
    /// The requested item was not found.
    #[error("Item not found: {0}")]
    NotFound(String),

    /// The key already exists and cannot be inserted again.
    #[error("Key already exists: {0}")]
    KeyExists(String),

    /// Duplicate value for unique index.
    #[error("Duplicate key: {0}")]
    DuplicateKey(String),

    /// Cannot create unique index due to duplicate values.
    /// Contains: (index_name, duplicate_count, sample_value)
    #[error(
        "Cannot create unique index '{0}': found {1} records with duplicate values (example: {2})"
    )]
    UniqueIndexCreationFailed(String, usize, String),

    /// An error originating from the underlying storage backend.
    #[error("Storage backend error: {0}")]
    Storage(String),

    /// An error related to configuration.
    #[error("Configuration error: {0}")]
    Config(String),

    /// An error during serialization or deserialization.
    #[error("Codec error: {0}")]
    Codec(String),

    /// A generic I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An internal logic error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// A validation error (e.g., schema validation).
    #[error("Validation error: {0}")]
    Validation(String),

    /// An error from the function engine (compile, invoke, registry).
    #[error("Function error: {0}")]
    Function(String),

    /// One or more validators rejected the write with structured,
    /// field-bound errors (S3). The inner string is a JSON-encoded
    /// array of `{ "field": [...] | null, "code": "..." }` objects.
    #[error("Validator rejected: {0}")]
    ValidatorRejected(String),

    /// A validator could not be invoked (missing from registry, WASM
    /// trap, undecodable return). Fail-closed: operator/deploy fault.
    #[error("Validator invalid: {0}")]
    ValidatorInvalid(String),
}

impl From<CodecError> for DbError {
    fn from(err: CodecError) -> Self {
        DbError::Codec(err.to_string())
    }
}

pub type DbResult<T> = Result<T, DbError>;
