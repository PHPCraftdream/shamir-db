//! [`SubBatchOp`] — a nested sub-batch with explicit parameter bindings.

use serde::{Deserialize, Serialize};

use crate::filter::FilterValue;
use shamir_collections::TMap;

use super::batch_request::BatchRequest;

/// A sub-batch — a nested BatchRequest with explicit parameter bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubBatchOp {
    pub batch: BatchRequest,
    #[serde(default, skip_serializing_if = "TMap::is_empty")]
    pub bind: TMap<String, FilterValue>,
}
