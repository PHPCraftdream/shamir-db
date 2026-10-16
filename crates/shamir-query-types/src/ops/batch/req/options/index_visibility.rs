//! Index-apply visibility for a transactional batch commit.

use serde::{Deserialize, Serialize};

/// Index-apply visibility for a transactional batch commit
/// (query-language v4).
///
/// Replaces the old `durability: "async_index"` request string (removed in
/// v4): WAL durability and index visibility are orthogonal options now.
/// Only meaningful for `transactional: true` batches; ignored otherwise.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IndexVisibility {
    /// Index posting apply runs inline before ack (default).
    #[default]
    Sync,
    /// Ack after WAL fsync + data apply + MVCC publish; index posting
    /// apply, recovery markers, WAL cleanup, and HNSW promote run on a
    /// background task. Shortens the pre-ACK critical section while
    /// preserving WAL durability and read-your-own-writes on data.
    #[serde(rename = "async")]
    Async,
}

impl IndexVisibility {
    /// The wire name of this visibility mode.
    pub fn as_str(self) -> &'static str {
        match self {
            IndexVisibility::Sync => "sync",
            IndexVisibility::Async => "async",
        }
    }
}
