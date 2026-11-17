use serde::{Deserialize, Serialize};

/// Stable wire error identifiers from design section 5.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaMigrationErrorCode {
    SchemaMigrationInProgress,
    SchemaMigrationInDoubt,
    SchemaMigrationChecksumMismatch,
    SchemaMigrationGap,
    SchemaMigrationPredecessorMismatch,
    SchemaMigrationFollowerReadOnly,
    SchemaMigrationDurabilityUnavailable,
    SchemaMigrationPostconditionFailed,
    SchemaMigrationIdentityMismatch,
}
