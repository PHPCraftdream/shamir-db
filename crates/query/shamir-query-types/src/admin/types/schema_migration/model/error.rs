use std::fmt;

/// Validation failures for the model-only schema migration representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaMigrationError {
    EmptyId,
    OrdinalMustBePositive,
    UnknownCanonicalVersion(u16),
    OrdinalGap { expected: u64, actual: u64 },
    DuplicateOrdinal(u64),
    DuplicateId(String),
    UnsupportedOperation(&'static str),
    UnsafeOperation(&'static str),
}

impl fmt::Display for SchemaMigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => f.write_str("schema migration ID must not be empty"),
            Self::OrdinalMustBePositive => {
                f.write_str("schema migration ordinal must be one-based")
            }
            Self::UnknownCanonicalVersion(version) => {
                write!(f, "unknown schema migration canonical version {version}")
            }
            Self::OrdinalGap { expected, actual } => {
                write!(
                    f,
                    "schema migration ordinal gap: expected {expected}, got {actual}"
                )
            }
            Self::DuplicateOrdinal(ordinal) => {
                write!(f, "duplicate schema migration ordinal {ordinal}")
            }
            Self::DuplicateId(id) => write!(f, "duplicate schema migration ID {id:?}"),
            Self::UnsupportedOperation(kind) => {
                write!(
                    f,
                    "{kind} is not admitted in an application schema migration"
                )
            }
            Self::UnsafeOperation(reason) => {
                write!(f, "unsafe application schema migration operation: {reason}")
            }
        }
    }
}

impl std::error::Error for SchemaMigrationError {}
