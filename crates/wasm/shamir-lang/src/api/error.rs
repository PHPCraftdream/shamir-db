use shamir_wasm_admission::AdmissionError;
use thiserror::Error;

use super::{Diagnostic, Span};

/// Public compile error surface (design section 6).
#[derive(Debug, Error)]
pub enum CompileError {
    /// One or more located diagnostics; rendered one per line.
    #[error("{}", joined(.0))]
    Diagnostics(Vec<Diagnostic>),
    /// The emitted module failed the self-admission scan.
    #[error("self-admission rejected the emitted module: {0}")]
    Admission(AdmissionError),
    /// A compiler resource limit was exceeded.
    #[error(
        "{code}:{}:{}: limit {} exceeded: {} observed",
        .span.start.line,
        .span.start.column,
        .limit,
        .observed
    )]
    LimitExceeded {
        /// Stable diagnostic code of the limit.
        code: &'static str,
        /// Source location charged with the overflow.
        span: Span,
        /// The applied ceiling.
        limit: u64,
        /// The observed usage.
        observed: u64,
    },
    /// An internal invariant was violated; always a compiler bug.
    #[error("internal invariant: {0}")]
    InternalInvariant(String),
}

/// Joins diagnostics with one rendered line each, in order.
fn joined(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Builds the `B001` limit error for the compile pipeline.
pub(crate) fn limit_error(span: Span, limit: u64, observed: u64) -> CompileError {
    CompileError::LimitExceeded {
        code: "B001",
        span,
        limit,
        observed,
    }
}
