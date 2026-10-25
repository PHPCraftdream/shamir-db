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

    /// A function's computation failed (e.g. the KDF rejected its inputs).
    #[error("compute error: {0}")]
    Compute(String),

    /// The guest trapped with `Trap::OutOfFuel` — its per-Store fuel grant
    /// was exhausted mid-execution. Typed (not message-classified) so the
    /// cause survives the `FunctionError → host_call → wasmtime::Error →
    /// outer FunctionError` round trip without stringification.
    #[error("fuel exhausted")]
    FuelExhausted,

    /// The invocation's AGGREGATE fuel budget (the shared free-capacity
    /// pool across all nested Stores) could not fund the reservation.
    #[error("aggregate fuel budget exhausted across nested calls")]
    AggregateFuelExhausted,

    /// The guest was interrupted by the epoch/wall-clock deadline
    /// (`Trap::Interrupt`). Typed for the same reason as
    /// [`FunctionError::FuelExhausted`].
    #[error("wasm guest interrupted (wall-clock deadline exceeded)")]
    Interrupted,

    /// The off-thread task carrying the computation was cancelled or panicked.
    #[error("function task cancelled")]
    Cancelled,

    /// The Rust→WASM toolchain (cargo + wasm32-unknown-unknown target) is not
    /// installed on this host.
    #[error("toolchain unavailable: {0}")]
    ToolchainUnavailable(String),
}
