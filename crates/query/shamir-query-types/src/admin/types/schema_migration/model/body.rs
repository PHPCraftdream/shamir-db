use super::{SchemaMigrationError, SchemaMigrationMetadata, SchemaMigrationOp};
use serde::{Deserialize, Serialize};
use shamir_collections::{TFxSet, THasher};

/// One ordered, typed migration body. Operation order is effect order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationBody {
    pub metadata: SchemaMigrationMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub function_dependencies: Vec<SchemaMigrationFunctionDependencies>,
    pub operations: Vec<SchemaMigrationOp>,
}

impl SchemaMigrationBody {
    pub fn validate(&self) -> Result<(), SchemaMigrationError> {
        self.metadata.validate()?;
        let mut function_names: TFxSet<&str> = TFxSet::with_hasher(THasher::default());
        for dependency in &self.function_dependencies {
            if dependency.function.trim().is_empty() {
                return Err(SchemaMigrationError::UnsafeOperation(
                    "dependent function name must not be empty",
                ));
            }
            if !function_names.insert(dependency.function.as_str()) {
                return Err(SchemaMigrationError::UnsafeOperation(
                    "duplicate function dependency metadata",
                ));
            }
            let mut dependencies: TFxSet<&str> = TFxSet::with_hasher(THasher::default());
            for name in &dependency.depends_on {
                if name.trim().is_empty() || !dependencies.insert(name.as_str()) {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "function dependency names must be nonempty and unique",
                    ));
                }
            }
        }
        for operation in &self.operations {
            operation.validate()?;
        }
        Ok(())
    }
}

/// Author-declared dependencies considered during function change planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaMigrationFunctionDependencies {
    pub function: String,
    pub depends_on: Vec<String>,
}

pub fn validate_migration_set(bodies: &[SchemaMigrationBody]) -> Result<(), SchemaMigrationError> {
    let mut ids: TFxSet<&str> = TFxSet::with_capacity_and_hasher(bodies.len(), THasher::default());
    let mut ordinals: TFxSet<u64> =
        TFxSet::with_capacity_and_hasher(bodies.len(), THasher::default());
    let mut expected = 1_u64;
    for body in bodies {
        body.validate()?;
        let metadata = &body.metadata;
        if !ids.insert(metadata.id.as_str()) {
            return Err(SchemaMigrationError::DuplicateId(metadata.id.clone()));
        }
        if !ordinals.insert(metadata.ordinal) {
            return Err(SchemaMigrationError::DuplicateOrdinal(metadata.ordinal));
        }
        if metadata.ordinal != expected {
            return Err(SchemaMigrationError::OrdinalGap {
                expected,
                actual: metadata.ordinal,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(SchemaMigrationError::OrdinalMustBePositive)?;
    }
    Ok(())
}
