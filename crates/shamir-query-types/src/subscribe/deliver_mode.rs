use serde::{Deserialize, Serialize};

use crate::batch::SubBatchOp;
use crate::call::CallOp;

/// How matching events are delivered to the subscriber.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // dispatch DTO; rarely on the stack
pub enum DeliverMode {
    /// Send the affected records as-is.
    #[default]
    Records,
    /// Send only the record keys (no values).
    Keys,
    /// Execute a reactive sub-batch and send its result.
    Batch(SubBatchOp),
    /// Call a stored function and send its result.
    Call(CallOp),
}
