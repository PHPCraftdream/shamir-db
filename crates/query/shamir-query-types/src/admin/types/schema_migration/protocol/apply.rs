use serde::{Deserialize, Serialize};

use super::{SchemaMigrationPredecessor, SchemaMigrationSubmission, SchemaMigrationTarget};
use crate::admin::types::schema_migration::SchemaMigrationBody;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationApplyRequest {
    pub protocol_version: u16,
    pub target: SchemaMigrationTarget,
    pub expected_predecessor: SchemaMigrationPredecessor,
    pub expected_fence_generation: u64,
    pub migration: SchemaMigrationSubmission,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaMigrationApplyResult {
    pub protocol_version: u16,
    pub migration_id: String,
    pub ordinal: u64,
    pub checksum: [u8; 32],
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at_unix_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<super::SchemaMigrationErrorCode>,
}

impl SchemaMigrationApplyRequest {
    pub fn from_body(
        target: SchemaMigrationTarget,
        expected_predecessor: SchemaMigrationPredecessor,
        expected_fence_generation: u64,
        body: &SchemaMigrationBody,
        checksum: [u8; 32],
    ) -> Self {
        Self {
            protocol_version: 1,
            target,
            expected_predecessor,
            expected_fence_generation,
            migration: SchemaMigrationSubmission::new(body, checksum),
        }
    }
}
