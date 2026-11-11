mod compile;
mod diagnostic;
mod error;
mod options;
mod span;

#[cfg(feature = "testsupport")]
pub(crate) use compile::compile_with;
pub use compile::{compile, CompiledModule};
pub use diagnostic::{Diagnostic, RelatedSpan, MAX_DIAGNOSTIC_MESSAGE_BYTES};
pub use error::CompileError;
pub use options::{CompileLimits, CompileOptions, LimitError, Stage};
pub use span::{Position, Span};

#[cfg(test)]
mod tests;
