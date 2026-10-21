use serde::{Deserialize, Serialize};

use crate::validator::ValidatorBinding;

/// Persisted per-table validator bindings (mirrors `PersistedIndexes`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedValidators {
    pub bindings: Vec<ValidatorBinding>,
}
