use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationBody, SchemaMigrationError, SchemaMigrationFunctionDependencies,
    SchemaMigrationMetadata, SchemaMigrationOp,
};
use shamir_query_types::batch::BatchOp;

/// Fluent builder for one validated application-schema migration body.
pub struct SchemaMigration {
    metadata: SchemaMigrationMetadata,
    function_dependencies: Vec<SchemaMigrationFunctionDependencies>,
    operations: Vec<SchemaMigrationOp>,
}

impl SchemaMigration {
    /// Start a version-1 migration with stable identity, one-based ordinal and display name.
    pub fn new(id: impl Into<String>, ordinal: u64, name: impl Into<String>) -> Self {
        Self {
            metadata: SchemaMigrationMetadata {
                id: id.into(),
                ordinal,
                display_name: name.into(),
                canonical_version: 1,
            },
            function_dependencies: Vec::new(),
            operations: Vec::new(),
        }
    }

    /// Select the versioned canonical body format. Unknown versions fail at `build`.
    pub fn version(mut self, version: u16) -> Self {
        self.metadata.canonical_version = version;
        self
    }

    /// Declare function dependencies used by migration planning.
    pub fn dependencies(mut self, dependencies: Vec<SchemaMigrationFunctionDependencies>) -> Self {
        self.function_dependencies = dependencies;
        self
    }

    /// Add an existing typed DDL operation if it belongs to the closed admitted subset.
    pub fn op(mut self, operation: BatchOp) -> Result<Self, SchemaMigrationError> {
        self.operations
            .push(SchemaMigrationOp::try_from(operation)?);
        Ok(self)
    }

    /// Validate and produce the immutable typed migration body.
    pub fn build(self) -> Result<SchemaMigrationBody, SchemaMigrationError> {
        let body = SchemaMigrationBody {
            metadata: self.metadata,
            function_dependencies: self.function_dependencies,
            operations: self.operations,
        };
        body.validate()?;
        Ok(body)
    }
}
