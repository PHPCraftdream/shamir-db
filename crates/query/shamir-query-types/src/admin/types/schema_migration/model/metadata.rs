use serde::{Deserialize, Serialize};

/// Stable, content-independent identity and ordering for an application migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationMetadata {
    pub id: String,
    pub ordinal: u64,
    pub display_name: String,
    pub canonical_version: u16,
}

impl SchemaMigrationMetadata {
    pub fn validate(&self) -> Result<(), super::SchemaMigrationError> {
        if self.id.trim().is_empty() {
            return Err(super::SchemaMigrationError::EmptyId);
        }
        if self.ordinal == 0 {
            return Err(super::SchemaMigrationError::OrdinalMustBePositive);
        }
        if self.canonical_version != 1 {
            return Err(super::SchemaMigrationError::UnknownCanonicalVersion(
                self.canonical_version,
            ));
        }
        Ok(())
    }
}
