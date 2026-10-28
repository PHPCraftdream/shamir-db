use thiserror::Error;

/// Typed error for path-navigation and coercion operations on [`QueryValue`].
///
/// [`QueryValue`]: crate::types::value::QueryValue
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValueError {
    /// The value at the given path has the wrong type.
    #[error("type mismatch at '{path}': expected {expected}, got {got}")]
    TypeMismatch {
        /// Dotted path where the mismatch occurred (empty string = root).
        path: String,
        /// Human-readable name of the expected type.
        expected: &'static str,
        /// Human-readable name of the actual type.
        got: &'static str,
    },

    #[error("intermediate node at '{path}' is not a map")]
    NotAMap {
        /// The dotted path prefix up to (and including) the non-Map node.
        path: String,
    },
}
