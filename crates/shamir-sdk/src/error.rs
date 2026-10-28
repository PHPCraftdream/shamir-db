//! Guest-side error type for user-defined functions.

/// Category of guest SDK error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    User,
    InvalidArgument,
    Codec,
    Host,
}

/// Error type returned by user-defined functions.
///
/// Use [`Error::user`] to construct a deliberate, user-surfaced error.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    /// Create a user-facing error with the given message.
    pub fn user(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::User, msg)
    }

    /// Create an invalid-argument error.
    pub fn invalid_argument(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidArgument, msg)
    }

    /// Create a codec error.
    pub fn codec(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Codec, msg)
    }

    /// Create a host/infrastructure error.
    pub fn host(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Host, msg)
    }

    fn new(kind: ErrorKind, msg: impl Into<String>) -> Self {
        Self {
            kind,
            message: msg.into(),
        }
    }

    /// Error category.
    pub fn kind(&self) -> ErrorKind {
        self.kind
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
