mod diagnostic;
mod options;
mod span;

pub use diagnostic::{Diagnostic, RelatedSpan, MAX_DIAGNOSTIC_MESSAGE_BYTES};
pub use options::{CompileLimits, CompileOptions, LimitError, Stage};
pub use span::{Position, Span};

#[cfg(test)]
mod tests;
