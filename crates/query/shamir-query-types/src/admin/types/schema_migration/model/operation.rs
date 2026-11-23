use crate::batch::BatchOp;
use serde::{Deserialize, Serialize};

use super::SchemaMigrationError;

/// Closed admitted schema-migration operation subset; it intentionally does not store `BatchOp`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "payload", rename_all = "snake_case")]
pub enum SchemaMigrationOp {
    CreateTable(Box<crate::admin::types::table_ops::CreateTableOp>),
    DropTable(Box<crate::admin::types::table_ops::DropTableOp>),
    RenameTable(Box<crate::admin::types::table_ops::RenameTableOp>),
    CreateIndex(Box<crate::admin::types::index_ops::CreateIndexOp>),
    DropIndex(Box<crate::admin::types::index_ops::DropIndexOp>),
    RenameIndex(Box<crate::admin::types::index_ops::RenameIndexOp>),
    SetTableSchema(Box<crate::admin::types::schema_ops::SetTableSchemaOp>),
    AddSchemaRule(Box<crate::admin::types::schema_ops::AddSchemaRuleOp>),
    RemoveSchemaRule(Box<crate::admin::types::schema_ops::RemoveSchemaRuleOp>),
    CreateFunction(Box<crate::admin::types::function_ops::CreateFunctionOp>),
    DropFunction(Box<crate::admin::types::function_ops::DropFunctionOp>),
    RenameFunction(Box<crate::admin::types::function_ops::RenameFunctionOp>),
    CheckFunction(Box<crate::admin::types::function_ops::CheckFunctionOp>),
    AlterFunction(Box<crate::admin::types::function_ops::AlterFunctionOp>),
    CreateValidator(Box<crate::admin::types::validator_ops::CreateValidatorOp>),
    DropValidator(Box<crate::admin::types::validator_ops::DropValidatorOp>),
    RenameValidator(Box<crate::admin::types::validator_ops::RenameValidatorOp>),
    BindValidator(Box<crate::admin::types::validator_ops::BindValidatorOp>),
    UnbindValidator(Box<crate::admin::types::validator_ops::UnbindValidatorOp>),
}

impl SchemaMigrationOp {
    pub fn validate(&self) -> Result<(), SchemaMigrationError> {
        match self {
            Self::CreateTable(operation) => {
                if let Some(schema) = &operation.schema {
                    super::validate_schema_rules(schema)?;
                }
            }
            Self::SetTableSchema(operation) => super::validate_schema_rules(&operation.schema)?,
            Self::AddSchemaRule(operation) => {
                super::validate_schema_rules(std::slice::from_ref(&operation.rule))?;
            }
            Self::CreateIndex(operation) => {
                if operation.index_type.as_deref() != Some("functional")
                    && (operation.functional_op.is_some() || operation.functional_args.is_some())
                {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "functional_op/functional_args are only valid for 'functional' indexes",
                    ));
                }
                if operation.index_type.as_deref() == Some("functional") {
                    let Some(functional_op) = operation.functional_op.as_deref() else {
                        return Err(SchemaMigrationError::UnsafeOperation(
                            "functional index requires an explicit built-in operation",
                        ));
                    };
                    if !matches!(functional_op, "lower" | "upper" | "trim" | "length") {
                        return Err(SchemaMigrationError::UnsafeOperation(
                            "index function is outside the built-in functional-index allowlist",
                        ));
                    }
                    if operation.functional_args.is_some() {
                        return Err(SchemaMigrationError::UnsafeOperation(
                            "functional_args are not admitted because functional indexes do not use them",
                        ));
                    }
                }
                if operation
                    .index_type
                    .as_deref()
                    .is_some_and(|kind| !matches!(kind, "btree" | "fts" | "functional" | "vector"))
                {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "unsupported index type is not admitted",
                    ));
                }
                if operation
                    .vector_metric
                    .as_deref()
                    .is_some_and(|metric| !matches!(metric, "l2" | "cosine" | "dot"))
                {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "unsupported vector metric is not admitted",
                    ));
                }
                if operation
                    .vector_quantization
                    .as_deref()
                    .is_some_and(|value| !value.eq_ignore_ascii_case("sq8"))
                {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "unsupported vector quantization is not admitted",
                    ));
                }
            }
            Self::CheckFunction(operation) => validate_function_spec(&operation.check_function)?,
            Self::CreateFunction(operation) => {
                if operation.source.is_some() == operation.wasm.is_some() {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "function creation requires exactly one source or module",
                    ));
                }
                validate_function_code_size(
                    operation.source.as_deref(),
                    operation.wasm.as_deref(),
                )?;
                validate_function_defaults(
                    operation.lang.as_deref(),
                    operation.wasm.is_some(),
                    operation.optimize.as_deref(),
                    operation.security.as_deref(),
                    operation.visibility.as_deref(),
                )?;
            }
            Self::CreateValidator(operation) => {
                if operation.source.is_some() == operation.wasm.is_some() {
                    return Err(SchemaMigrationError::UnsafeOperation(
                        "validator creation requires exactly one source or module",
                    ));
                }
                validate_function_code_size(
                    operation.source.as_deref(),
                    operation.wasm.as_deref(),
                )?;
            }
            Self::DropTable(_)
            | Self::RenameTable(_)
            | Self::DropIndex(_)
            | Self::RenameIndex(_)
            | Self::RemoveSchemaRule(_)
            | Self::DropFunction(_)
            | Self::RenameFunction(_)
            | Self::AlterFunction(_)
            | Self::DropValidator(_)
            | Self::RenameValidator(_)
            | Self::BindValidator(_)
            | Self::UnbindValidator(_) => {}
        }
        Ok(())
    }
}

fn validate_function_spec(
    spec: &crate::admin::types::function_ops::CheckFunctionSpec,
) -> Result<(), SchemaMigrationError> {
    if spec.source.is_some() == spec.wasm.is_some() {
        return Err(SchemaMigrationError::UnsafeOperation(
            "function check requires exactly one source or module",
        ));
    }
    validate_function_code_size(spec.source.as_deref(), spec.wasm.as_deref())?;
    validate_function_defaults(
        spec.lang.as_deref(),
        spec.wasm.is_some(),
        spec.optimize.as_deref(),
        None,
        None,
    )
}

fn validate_function_code_size(
    source: Option<&str>,
    wasm: Option<&str>,
) -> Result<(), SchemaMigrationError> {
    if source.is_some_and(|value| value.len() > 16 * 1024 * 1024)
        || wasm.is_some_and(|value| value.len() > 16 * 1024 * 1024)
    {
        return Err(SchemaMigrationError::UnsafeOperation(
            "function source or module exceeds the 16 MiB canonical size bound",
        ));
    }
    Ok(())
}

fn validate_function_defaults(
    lang: Option<&str>,
    has_wasm: bool,
    optimize: Option<&str>,
    security: Option<&str>,
    visibility: Option<&str>,
) -> Result<(), SchemaMigrationError> {
    let valid_language = matches!(
        (lang, has_wasm),
        (None, _) | (Some("wasm"), true) | (Some("rust" | "shamir"), false)
    );
    if !valid_language {
        return Err(SchemaMigrationError::UnsafeOperation(
            "function language must match its source/module representation",
        ));
    }
    if optimize.is_some_and(|value| !matches!(value, "auto" | "now" | "never")) {
        return Err(SchemaMigrationError::UnsafeOperation(
            "unsupported function optimization mode is not admitted",
        ));
    }
    if security.is_some_and(|value| !matches!(value, "invoker" | "definer")) {
        return Err(SchemaMigrationError::UnsafeOperation(
            "unsupported function security mode is not admitted",
        ));
    }
    if visibility.is_some_and(|value| !matches!(value, "public" | "private")) {
        return Err(SchemaMigrationError::UnsafeOperation(
            "unsupported function visibility is not admitted",
        ));
    }
    Ok(())
}

impl TryFrom<crate::batch::BatchOp> for SchemaMigrationOp {
    type Error = SchemaMigrationError;

    fn try_from(operation: crate::batch::BatchOp) -> Result<Self, Self::Error> {
        let admitted = match operation {
            BatchOp::CreateTable(value) => Self::CreateTable(Box::new(value)),
            BatchOp::DropTable(value) => Self::DropTable(Box::new(value)),
            BatchOp::RenameTable(value) => Self::RenameTable(Box::new(value)),
            BatchOp::CreateIndex(value) => Self::CreateIndex(Box::new(value)),
            BatchOp::DropIndex(value) => Self::DropIndex(Box::new(value)),
            BatchOp::RenameIndex(value) => Self::RenameIndex(Box::new(value)),
            BatchOp::SetTableSchema(value) => Self::SetTableSchema(Box::new(value)),
            BatchOp::AddSchemaRule(value) => Self::AddSchemaRule(Box::new(value)),
            BatchOp::RemoveSchemaRule(value) => Self::RemoveSchemaRule(Box::new(value)),
            BatchOp::CreateFunction(value) => Self::CreateFunction(Box::new(value)),
            BatchOp::DropFunction(value) => Self::DropFunction(Box::new(value)),
            BatchOp::RenameFunction(value) => Self::RenameFunction(Box::new(value)),
            BatchOp::CheckFunction(value) => Self::CheckFunction(Box::new(value)),
            BatchOp::AlterFunction(value) => Self::AlterFunction(Box::new(value)),
            BatchOp::CreateValidator(value) => Self::CreateValidator(Box::new(value)),
            BatchOp::DropValidator(value) => Self::DropValidator(Box::new(value)),
            BatchOp::RenameValidator(value) => Self::RenameValidator(Box::new(value)),
            BatchOp::BindValidator(value) => Self::BindValidator(Box::new(value)),
            BatchOp::UnbindValidator(value) => Self::UnbindValidator(Box::new(value)),
            BatchOp::Read(_) => return Err(SchemaMigrationError::UnsupportedOperation("read")),
            BatchOp::Insert(_) => return Err(SchemaMigrationError::UnsupportedOperation("insert")),
            BatchOp::Update(_) => return Err(SchemaMigrationError::UnsupportedOperation("update")),
            BatchOp::Set(_) => return Err(SchemaMigrationError::UnsupportedOperation("set")),
            BatchOp::Delete(_) => return Err(SchemaMigrationError::UnsupportedOperation("delete")),
            BatchOp::CreateDb(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "create database",
                ))
            }
            BatchOp::DropDb(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("drop database"))
            }
            BatchOp::CreateRepo(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "create repository",
                ))
            }
            BatchOp::DropRepo(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "drop repository",
                ))
            }
            BatchOp::RenameRepo(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "rename repository",
                ))
            }
            BatchOp::AlterRepo(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "alter repository",
                ))
            }
            BatchOp::RenameDb(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "rename database",
                ))
            }
            BatchOp::SetBufferConfig(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "set buffer config",
                ))
            }
            BatchOp::GetBufferConfig(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "get buffer config",
                ))
            }
            BatchOp::AlterBufferConfig(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "alter buffer config",
                ))
            }
            BatchOp::List(_) => return Err(SchemaMigrationError::UnsupportedOperation("list")),
            BatchOp::StartMigration(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "online storage migration",
                ))
            }
            BatchOp::CommitMigration(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "online storage migration",
                ))
            }
            BatchOp::RollbackMigration(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "online storage migration",
                ))
            }
            BatchOp::MigrationStatus(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "online storage migration status",
                ))
            }
            BatchOp::CreateUser(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "user administration",
                ))
            }
            BatchOp::DropUser(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "user administration",
                ))
            }
            BatchOp::GrantRole(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "role administration",
                ))
            }
            BatchOp::RevokeRole(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "role administration",
                ))
            }
            BatchOp::Chmod(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("access control"))
            }
            BatchOp::Chown(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("access control"))
            }
            BatchOp::Chgrp(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("access control"))
            }
            BatchOp::CreateGroup(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "group administration",
                ))
            }
            BatchOp::DropGroup(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "group administration",
                ))
            }
            BatchOp::RenameGroup(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "group administration",
                ))
            }
            BatchOp::AddGroupMember(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "group administration",
                ))
            }
            BatchOp::RemoveGroupMember(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "group administration",
                ))
            }
            BatchOp::AccessTree(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "access-tree inspection",
                ))
            }
            BatchOp::DescribeFunction(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "function inspection",
                ))
            }
            BatchOp::ExportFunction(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "function export",
                ))
            }
            BatchOp::ListValidators(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "validator listing",
                ))
            }
            BatchOp::GetTableSchema(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "schema inspection",
                ))
            }
            BatchOp::DescribeTable(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "table inspection",
                ))
            }
            BatchOp::DescribeRepo(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "repository inspection",
                ))
            }
            BatchOp::CreateFunctionFolder(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "function-folder administration",
                ))
            }
            BatchOp::RenameFunctionFolder(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "function-folder administration",
                ))
            }
            BatchOp::InternerDump(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "interner administration",
                ))
            }
            BatchOp::InternerTouch(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "interner administration",
                ))
            }
            BatchOp::PurgeHistory(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("history purge"))
            }
            BatchOp::SetRetention(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "retention change",
                ))
            }
            BatchOp::ChangesSince(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("change feed"))
            }
            BatchOp::Call(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "arbitrary function invocation",
                ))
            }
            BatchOp::Batch(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("nested batch"))
            }
            BatchOp::ForEach(value) => {
                let crate::batch::ForEachOp {
                    over: _,
                    bind_row: _,
                    batch: _,
                } = value;
                return Err(SchemaMigrationError::UnsupportedOperation("ForEach"));
            }
            BatchOp::Subscribe(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation("subscription"))
            }
            BatchOp::Unsubscribe(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "subscription control",
                ))
            }
            BatchOp::CreateReplicationProfile(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::DropReplicationProfile(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::CreatePublication(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::DropPublication(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::CreateSubscription(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::DropSubscription(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::AlterSubscription(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication control",
                ))
            }
            BatchOp::ListPublications(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication inspection",
                ))
            }
            BatchOp::ListSubscriptions(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication inspection",
                ))
            }
            BatchOp::ReplicationStatus(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "replication inspection",
                ))
            }
            BatchOp::ApplicationSchema(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "application-schema admin operation",
                ))
            }
            BatchOp::StartBackup(_) | BatchOp::CancelBackup(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "online-backup admin operation",
                ))
            }
            BatchOp::BackupStatus(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "online-backup inspection",
                ))
            }
            BatchOp::ListActivity(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "activity listing",
                ))
            }
            BatchOp::CancelRequest(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "request cancellation",
                ))
            }
            BatchOp::CancelTx(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "transaction cancellation",
                ))
            }
            BatchOp::CancelCursorAdmin(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "cursor cancellation",
                ))
            }
            BatchOp::ListShapeStats(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "shape statistics listing",
                ))
            }
            BatchOp::ResetShapeStats(_) => {
                return Err(SchemaMigrationError::UnsupportedOperation(
                    "shape statistics reset",
                ))
            }
        };
        admitted.validate()?;
        Ok(admitted)
    }
}
