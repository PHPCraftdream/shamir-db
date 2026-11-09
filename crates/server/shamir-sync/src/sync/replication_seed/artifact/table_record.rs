use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableRecord {
    pub name: String,
    pub source_enable_indexes: bool,
    pub follower_enable_indexes: bool,
    pub schema_version: u64,
    pub schema: QueryValue,
    pub excluded_indexes: Vec<(String, Vec<u8>)>,
    pub live_records: u64,
}
