use thiserror::Error;

/// A generic error type for all database and storage operations.
#[derive(Error, Debug)]
pub enum DbError {
    /// The requested item was not found.
    #[error("Item not found: {0}")]
    NotFound(String),

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
}

impl From<rmp_serde::encode::Error> for DbError {
    fn from(err: rmp_serde::encode::Error) -> Self {
        DbError::Codec(err.to_string())
    }
}

impl From<rmp_serde::decode::Error> for DbError {
    fn from(err: rmp_serde::decode::Error) -> Self {
        DbError::Codec(err.to_string())
    }
}

impl From<surrealkv::Error> for DbError {
    fn from(err: surrealkv::Error) -> Self {
        DbError::Storage(err.to_string())
    }
}

pub type DbResult<T> = Result<T, DbError>;
