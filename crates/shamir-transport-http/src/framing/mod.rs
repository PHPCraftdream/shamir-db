//! Framing scanner and guarded I/O for validating HTTP request heads.

/// I/O wrapper that validates framing before forwarding request bytes.
pub(crate) mod guarded_io;
/// Incremental request-head scanner and verdicts.
pub mod scanner;
#[cfg(test)]
mod tests;
