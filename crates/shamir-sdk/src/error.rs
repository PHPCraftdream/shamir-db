//! Guest-side error type for user-defined functions.

/// Error type returned by user-defined functions.
///
/// Use [`Error::user`] to construct a deliberate, user-surfaced error.
#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    /// Create a user-facing error with the given message.
    pub fn user(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

/// Result alias for user-defined function returns.
pub type Result<T> = core::result::Result<T, Error>;
