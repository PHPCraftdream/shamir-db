use shamir_query_types::admin::types::schema_migration::SchemaMigrationError;
#[cfg(feature = "crypto")]
use shamir_query_types::admin::types::schema_migration::{
    canonical_body, canonical_body_checksum, SchemaMigrationCanonicalError,
};

/// An ordinal-ordered, locally validated set of migration bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaMigrationSet {
    bodies: Vec<shamir_query_types::admin::types::schema_migration::SchemaMigrationBody>,
}

impl SchemaMigrationSet {
    /// Validate unique IDs and the contiguous one-based ordinal sequence.
    pub fn new(
        bodies: Vec<shamir_query_types::admin::types::schema_migration::SchemaMigrationBody>,
    ) -> Result<Self, SchemaMigrationError> {
        shamir_query_types::admin::types::schema_migration::validate_migration_set(&bodies)?;
        Ok(Self { bodies })
    }

    /// Read migration bodies without allowing the set to be mutated.
    pub fn bodies(
        &self,
    ) -> &[shamir_query_types::admin::types::schema_migration::SchemaMigrationBody] {
        &self.bodies
    }

    /// Canonical version-1 body bytes for each migration, in set order.
    #[cfg(feature = "crypto")]
    pub fn canonical_bytes(&self) -> Result<Vec<Vec<u8>>, SchemaMigrationCanonicalError> {
        self.bodies.iter().map(canonical_body).collect()
    }

    /// SHA-256 checksums for each migration, in set order.
    #[cfg(feature = "crypto")]
    pub fn checksums(&self) -> Result<Vec<[u8; 32]>, SchemaMigrationCanonicalError> {
        self.bodies.iter().map(canonical_body_checksum).collect()
    }
}
