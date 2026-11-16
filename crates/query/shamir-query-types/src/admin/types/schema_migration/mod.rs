//! Typed application-schema migration models, protocol DTOs, and canonical body.

#[cfg(feature = "crypto")]
pub mod canonical;
pub mod model;
pub mod protocol;

#[cfg(feature = "crypto")]
pub use canonical::{canonical_body, canonical_body_checksum, SchemaMigrationCanonicalError};
pub use model::{
    validate_migration_set, validate_query_value, validate_query_values, validate_schema_rules,
    SchemaMigrationBody, SchemaMigrationError, SchemaMigrationFunctionDependencies,
    SchemaMigrationMetadata, SchemaMigrationOp,
};
pub use protocol::*;

#[cfg(test)]
mod tests;
