//! Wire-facing validator types: `WriteOp` and `ValidationError`.
//!
//! These are pure DTOs shared between client, server, and engine.

pub mod validation_error;
pub mod write_op;

pub use validation_error::ValidationError;
pub use write_op::WriteOp;

#[cfg(test)]
mod tests;
