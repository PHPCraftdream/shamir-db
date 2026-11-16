use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;

use crate::admin::types::schema_migration::model::{
    SchemaMigrationFunctionDependencies, SchemaMigrationMetadata,
};

/// Database-scoped application-schema address, keyed by stable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationTarget {
    #[serde(with = "crate::wire::record_id_string")]
    pub database_identity: RecordId,
}

/// Identity of the applied predecessor expected by a plan/apply request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationPredecessor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_id: Option<String>,
    pub ordinal: u64,
    pub checksum: [u8; 32],
}

/// Typed body plus its server-recomputed digest; not trusted until validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationSubmission {
    pub metadata: SchemaMigrationMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub function_dependencies: Vec<SchemaMigrationFunctionDependencies>,
    pub operations: Vec<crate::admin::types::schema_migration::SchemaMigrationOp>,
    pub checksum: [u8; 32],
}

impl SchemaMigrationSubmission {
    pub fn new(
        body: &crate::admin::types::schema_migration::SchemaMigrationBody,
        checksum: [u8; 32],
    ) -> Self {
        Self {
            metadata: body.metadata.clone(),
            function_dependencies: body.function_dependencies.clone(),
            operations: body.operations.clone(),
            checksum,
        }
    }
}
