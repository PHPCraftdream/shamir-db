use serde::{Deserialize, Serialize};

use super::{SchemaMigrationPredecessor, SchemaMigrationTarget};
use crate::admin::types::schema_migration::{
    SchemaMigrationBody, SchemaMigrationMetadata, SchemaMigrationSubmission,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaMigrationWarning {
    ServerGlobalRegistrationBlastRadius,
    MaintenanceFenceRequired,
    RecoveryRequiresPostcondition,
    DependencyListMustBeReviewed,
    DataDmlNotAdmitted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationPlanRequest {
    pub protocol_version: u16,
    pub target: SchemaMigrationTarget,
    pub expected_predecessor: SchemaMigrationPredecessor,
    pub migrations: Vec<SchemaMigrationSubmission>,
}

impl SchemaMigrationPlanRequest {
    pub fn from_bodies(
        target: SchemaMigrationTarget,
        expected_predecessor: SchemaMigrationPredecessor,
        bodies: &[SchemaMigrationBody],
        checksums: &[[u8; 32]],
    ) -> Result<Self, crate::admin::types::SchemaMigrationError> {
        if bodies.len() != checksums.len() {
            return Err(crate::admin::types::SchemaMigrationError::UnsafeOperation(
                "each planned migration requires a matching checksum",
            ));
        }
        crate::admin::types::schema_migration::validate_migration_set(bodies)?;
        let migrations = bodies
            .iter()
            .zip(checksums)
            .map(|(body, checksum)| SchemaMigrationSubmission::new(body, *checksum))
            .collect();
        Ok(Self {
            protocol_version: 1,
            target,
            expected_predecessor,
            migrations,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaMigrationPlannedOperation {
    pub operation_index: u32,
    pub operation_kind: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_bound: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependent_functions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_database_registrations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaMigrationPlanResult {
    pub protocol_version: u16,
    pub migration: SchemaMigrationMetadata,
    pub checksum: [u8; 32],
    pub predecessor: SchemaMigrationPredecessor,
    pub operations: Vec<SchemaMigrationPlannedOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_work_bound: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<SchemaMigrationWarning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_server_wide_fence: Option<bool>,
}
