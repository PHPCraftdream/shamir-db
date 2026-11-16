use serde::{Deserialize, Serialize};

/// Logical row section item, preserving raw key bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordItem {
    pub repository: String,
    pub table: String,
    #[serde(with = "serde_bytes")]
    pub raw_key: Vec<u8>,
    pub value: super::value::LogicalValue,
}
