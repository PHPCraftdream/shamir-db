use shamir_types::types::record_id::RecordId;

use super::ValidationError;

/// Aggregated result of running one validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationOutcome {
    /// Field-bound errors returned by the validator.
    pub errors: Vec<ValidationError>,
    /// If `true`, the engine should skip remaining validators.
    pub stop: bool,
}

/// Failure modes for the S3 validator pass on the write path.
///
/// - `Failed` — one or more validators returned field-bound errors.
/// - `Missing` — a binding references a validator id that is not
///   registered (fail-closed: operator/deploy fault).
/// - `Invocation` — the validator trapped, returned an undecodable
///   result, or produced any other runtime error (fail-closed).
#[derive(Debug)]
pub enum ValidatorFailure {
    /// The validator(s) produced field-bound errors — the write is
    /// rejected with a structured error list.
    Failed(Vec<ValidationError>),
    /// A binding references a validator id that is not in the registry.
    Missing { id: RecordId },
    /// The validator invocation failed (WASM trap, undecodable return,
    /// function error).
    Invocation { id: RecordId, reason: String },
}

impl std::fmt::Display for ValidatorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(errors) => {
                write!(f, "validator rejected: ")?;
                for (i, e) in errors.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    if let Some(ref field) = e.field {
                        write!(f, "{}: {}", field.join("."), e.code)?;
                    } else {
                        write!(f, "{}", e.code)?;
                    }
                }
                Ok(())
            }
            Self::Missing { id } => write!(f, "validator {id} not found in registry (fail-closed)"),
            Self::Invocation { id, reason } => {
                write!(f, "validator {id} invocation failed: {reason}")
            }
        }
    }
}
