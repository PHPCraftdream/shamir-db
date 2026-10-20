use std::io;

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("seed I/O: {0}")]
    Io(#[from] io::Error),
    #[error("seed repository: {0}")]
    Database(#[from] shamir_db::DbError),
    #[error("invalid seed: {0}")]
    Invalid(String),
}

pub(crate) fn invalid(message: impl Into<String>) -> SeedError {
    SeedError::Invalid(message.into())
}
