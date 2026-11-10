use std::sync::Arc;

use crate::shamir_db::shamir_db::ShamirDb;
use crate::DbError;

use super::super::types::{FunctionState, UnavailableFunction};

impl ShamirDb {
    /// Record a hash mismatch: mark the function corrupt and swap in a
    /// fail-closed placeholder. Returns the error to surface to the caller.
    pub(super) fn mark_function_corrupt(
        &self,
        name: &str,
        expected: String,
        actual: String,
    ) -> DbError {
        let state = FunctionState::Corrupt { expected, actual };
        let msg = state
            .unavailable_message(name)
            .unwrap_or_else(|| format!("function '{name}' is unavailable"));
        let artifact_id = self
            .functions
            .capture(name)
            .and_then(|binding| binding.artifact_id().map(str::to_string));
        self.functions.replace_with_artifact_id(
            name,
            Arc::new(UnavailableFunction::new(name, state.clone())),
            artifact_id,
        );
        self.function_state.insert(name.to_string(), state);
        DbError::Function(msg)
    }
}
