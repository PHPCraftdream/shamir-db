//! Constructors for the typed application-schema admin batch operation
//! (`BatchOp::ApplicationSchema`, release-1 R1c step 2b-1).
//!
//! Only the read-only `ApplicationSchemaStatus` arm is wired end to end;
//! the plan and apply arms exist on the wire but the server still refuses
//! them with a temporary error. Builders never hand-write wire JSON: these
//! constructors produce the typed `SchemaMigrationAdminOp` directly.

use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationAdminOp, SchemaMigrationApplyRequest, SchemaMigrationPlanRequest,
    SchemaMigrationStatusRequest,
};
use shamir_query_types::batch::BatchOp;

/// Read-only application-schema status for the database identified by
/// `database_identity` (design §5.2: requires `Read` on the database).
pub fn application_schema_status(request: SchemaMigrationStatusRequest) -> BatchOp {
    BatchOp::ApplicationSchema(SchemaMigrationAdminOp::ApplicationSchemaStatus(request))
}

/// Application-schema plan request (temporary: the server refuses it until
/// R1c step 2b-2 lands).
pub fn application_schema_plan(request: SchemaMigrationPlanRequest) -> BatchOp {
    BatchOp::ApplicationSchema(SchemaMigrationAdminOp::ApplicationSchemaPlan(request))
}

/// Application-schema apply request (temporary: the server refuses it until
/// R1c step 2b-3 lands).
pub fn apply_application_schema_migration(request: SchemaMigrationApplyRequest) -> BatchOp {
    BatchOp::ApplicationSchema(SchemaMigrationAdminOp::ApplyApplicationSchemaMigration(
        request,
    ))
}
