//! Transaction isolation level for transactional batches.

use serde::{Deserialize, Serialize};

/// Transaction isolation level (query-language v4).
///
/// Deserialization is strict: unknown wire strings (including the legacy
/// `"level3"` alias, dropped in v4) are rejected instead of silently
/// falling back to snapshot isolation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    /// Snapshot Isolation (default): reads see a consistent snapshot;
    /// writes use last-writer-wins.
    #[default]
    Snapshot,
    /// Serializable Snapshot Isolation: read-set validated at commit;
    /// concurrent write conflict → abort.
    Serializable,
    /// Pessimistic (level-3) isolation: locks guard concurrent writers.
    Pessimistic,
}

impl IsolationLevel {
    /// The wire name of this isolation level.
    pub fn as_str(self) -> &'static str {
        match self {
            IsolationLevel::Snapshot => "snapshot",
            IsolationLevel::Serializable => "serializable",
            IsolationLevel::Pessimistic => "pessimistic",
        }
    }
}
