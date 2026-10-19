//! Error type for the function engine.

use thiserror::Error;

/// Result alias for function-engine operations.
pub type FnResult<T> = Result<T, FunctionError>;

/// Failures from registering, looking up, or invoking a function.
#[derive(Debug, Error)]
pub enum FunctionError {
    /// No function registered under the given name.
    #[error("function not found: {0}")]
    NotFound(String),

    /// A function is already registered under the given name.
    #[error("function already registered: {0}")]
    AlreadyExists(String),

    /// A required call parameter was absent.
    #[error("missing parameter: {0}")]
    MissingParam(String),

    /// A call parameter was present but of the wrong shape/range.
    #[error("invalid parameter `{name}`: {reason}")]
    BadParam { name: String, reason: String },

    /// A deliberate, user-surfaced error raised by the function body.
    /// Rolls the enclosing batch transaction back.
    #[error("user error: {0}")]
    User(String),

    /// The function's computation failed (e.g. the KDF rejected its inputs).
    #[error("compute error: {0}")]
    Compute(String),

    /// The off-thread task carrying the computation was cancelled or panicked.
    #[error("function task cancelled")]
    Cancelled,

    /// The Rust→WASM toolchain (cargo + wasm32-unknown-unknown target) is not
    /// installed on this host.
    #[error("toolchain unavailable: {0}")]
    ToolchainUnavailable(String),
}
