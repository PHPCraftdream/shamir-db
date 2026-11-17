use serde::{Deserialize, Serialize};

use super::SchemaMigrationTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaMigrationChecksumValidation {
    Valid,
    MissingBody,
    Mismatch,
    UnknownVersion,
    NotChecked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaMigrationInFlight {
    pub migration_id: String,
    pub ordinal: u64,
    #[serde(default)]
    pub operation_index: Option<u32>,
    pub fence_generation: u64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationStatusRequest {
    pub protocol_version: u16,
    pub target: SchemaMigrationTarget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaMigrationStatusResult {
    pub protocol_version: u16,
    pub target: SchemaMigrationTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_id: Option<String>,
    pub last_applied_ordinal: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_checksum: Option<[u8; 32]>,
    pub checksum_validation: SchemaMigrationChecksumValidation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<SchemaMigrationInFlight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<super::SchemaMigrationErrorCode>,
}
