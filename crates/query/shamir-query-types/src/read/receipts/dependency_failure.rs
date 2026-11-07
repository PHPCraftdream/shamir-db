use serde::{Deserialize, Serialize};

/// Alias-local failure: the write committed, but this operation did not run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisibilityDependencyPending {
    pub code: VisibilityDependencyCode,
    pub commit_version: u64,
    pub visible_through: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityDependencyCode {
    VisibilityDependencyPending,
}
