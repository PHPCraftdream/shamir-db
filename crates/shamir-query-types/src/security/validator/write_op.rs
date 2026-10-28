//! The kind of write operation a validator may fire on.

use serde::{Deserialize, Serialize};

/// The kind of write operation a validator may fire on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteOp {
    Insert,
    Update,
    Upsert,
    Delete,
}
