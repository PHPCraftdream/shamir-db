use serde::{Deserialize, Serialize};

use super::{
    SchemaMigrationApplyRequest, SchemaMigrationPlanRequest, SchemaMigrationStatusRequest,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "payload", rename_all = "snake_case")]
pub enum SchemaMigrationAdminOp {
    ApplicationSchemaStatus(SchemaMigrationStatusRequest),
    ApplicationSchemaPlan(SchemaMigrationPlanRequest),
    ApplyApplicationSchemaMigration(SchemaMigrationApplyRequest),
}
