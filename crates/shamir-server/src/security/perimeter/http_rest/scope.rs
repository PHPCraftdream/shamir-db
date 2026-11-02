use shamir_connect::server::token::TokenMode;
use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::{BatchOp, BatchTreeVisitor};
use shamir_query_types::wire::DbRequest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AllowedOperation {
    Ping,
    WaitForVisibility,
    GetDdlOpStatus,
    Execute,
}

impl AllowedOperation {
    /// Fixed-vocabulary label for logs and metrics.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::WaitForVisibility => "wait_for_visibility",
            Self::GetDdlOpStatus => "get_ddl_op_status",
            Self::Execute => "execute",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Allowed(AllowedOperation),
    Denied(&'static str),
    NotSupported(&'static str),
}

pub(crate) fn classify_request(
    request: &DbRequest,
    mode: TokenMode,
    databases: Option<&[String]>,
) -> Decision {
    let db = match request {
        DbRequest::Execute { db, .. }
        | DbRequest::TxBegin { db, .. }
        | DbRequest::TxExecute { db, .. }
        | DbRequest::TxCommit { db, .. }
        | DbRequest::TxRollback { db, .. }
        | DbRequest::WaitForVisibility { db, .. }
        | DbRequest::GetDdlOpStatus { db, .. }
        | DbRequest::CreateCursor { db, .. } => Some(db.as_str()),
        DbRequest::Ping
        | DbRequest::FetchNext { .. }
        | DbRequest::CancelCursor { .. }
        | DbRequest::CreateScramUser { .. }
        | DbRequest::SetSuperuser { .. }
        | DbRequest::SetReplicator { .. }
        | DbRequest::Repl(_)
        | DbRequest::ChangePasswordChallenge { .. }
        | DbRequest::ChangePasswordVerify(_) => None,
    };
    if let (Some(allow), Some(db)) = (databases, db) {
        if !allow.iter().any(|allowed| allowed == db) {
            return Decision::Denied("token_database_denied");
        }
    }
    match request {
        DbRequest::Ping => Decision::Allowed(AllowedOperation::Ping),
        DbRequest::WaitForVisibility { .. } => {
            Decision::Allowed(AllowedOperation::WaitForVisibility)
        }
        DbRequest::GetDdlOpStatus { .. } => Decision::Allowed(AllowedOperation::GetDdlOpStatus),
        DbRequest::Execute { batch, .. } => match classify_batch(batch, mode, databases) {
            Decision::Allowed(_) => Decision::Allowed(AllowedOperation::Execute),
            denial => denial,
        },
        DbRequest::TxBegin { .. }
        | DbRequest::TxExecute { .. }
        | DbRequest::TxCommit { .. }
        | DbRequest::TxRollback { .. }
        | DbRequest::CreateCursor { .. }
        | DbRequest::FetchNext { .. }
        | DbRequest::CancelCursor { .. } => Decision::NotSupported("not_supported_over_rest"),
        DbRequest::CreateScramUser { .. }
        | DbRequest::SetSuperuser { .. }
        | DbRequest::SetReplicator { .. }
        | DbRequest::Repl(_)
        | DbRequest::ChangePasswordChallenge { .. }
        | DbRequest::ChangePasswordVerify(_) => Decision::Denied("token_request_denied"),
    }
}

fn classify_batch(
    batch: &shamir_query_types::batch::BatchRequest,
    mode: TokenMode,
    databases: Option<&[String]>,
) -> Decision {
    for visit in BatchTreeVisitor::new(&batch.queries) {
        let top = visit.depth == 0;
        let nested = visit.depth > 0;
        match &visit.entry.op {
            BatchOp::Read(_) => {}
            BatchOp::Insert(_)
            | BatchOp::Update(_)
            | BatchOp::Set(_)
            | BatchOp::Delete(_)
            | BatchOp::Call(_) => {
                if mode == TokenMode::ReadOnly {
                    return Decision::Denied("token_read_only");
                }
            }
            BatchOp::Batch(_) | BatchOp::ForEach(_) => {}
            BatchOp::List(op) => {
                if top && databases.is_some() && matches!(op, ListOp::Databases | ListOp::Users) {
                    return Decision::Denied("token_database_denied");
                }
                if nested {
                    return Decision::Denied("token_request_denied");
                }
            }
            BatchOp::AccessTree(op) => {
                if top {
                    if let Some(allowed) = databases {
                        if op
                            .db
                            .as_deref()
                            .is_none_or(|db| !allowed.iter().any(|item| item == db))
                        {
                            return Decision::Denied("token_database_denied");
                        }
                    }
                } else {
                    return Decision::Denied("token_request_denied");
                }
            }
            BatchOp::DescribeTable(_) | BatchOp::GetTableSchema(_) => {
                if nested {
                    return Decision::Denied("token_request_denied");
                }
            }
            BatchOp::Subscribe(_) | BatchOp::Unsubscribe(_) => {
                return Decision::NotSupported("not_supported_over_rest")
            }
            BatchOp::CreateDb(_)
            | BatchOp::DropDb(_)
            | BatchOp::CreateRepo(_)
            | BatchOp::DropRepo(_)
            | BatchOp::RenameRepo(_)
            | BatchOp::AlterRepo(_)
            | BatchOp::RenameDb(_)
            | BatchOp::CreateTable(_)
            | BatchOp::DropTable(_)
            | BatchOp::RenameTable(_)
            | BatchOp::CreateIndex(_)
            | BatchOp::DropIndex(_)
            | BatchOp::RenameIndex(_)
            | BatchOp::SetBufferConfig(_)
            | BatchOp::GetBufferConfig(_)
            | BatchOp::AlterBufferConfig(_)
            | BatchOp::StartMigration(_)
            | BatchOp::CommitMigration(_)
            | BatchOp::RollbackMigration(_)
            | BatchOp::MigrationStatus(_)
            | BatchOp::CreateUser(_)
            | BatchOp::DropUser(_)
            | BatchOp::GrantRole(_)
            | BatchOp::RevokeRole(_)
            | BatchOp::Chmod(_)
            | BatchOp::Chown(_)
            | BatchOp::Chgrp(_)
            | BatchOp::CreateGroup(_)
            | BatchOp::DropGroup(_)
            | BatchOp::RenameGroup(_)
            | BatchOp::AddGroupMember(_)
            | BatchOp::RemoveGroupMember(_)
            | BatchOp::CreateFunction(_)
            | BatchOp::DropFunction(_)
            | BatchOp::RenameFunction(_)
            | BatchOp::CreateValidator(_)
            | BatchOp::DropValidator(_)
            | BatchOp::RenameValidator(_)
            | BatchOp::BindValidator(_)
            | BatchOp::UnbindValidator(_)
            | BatchOp::ListValidators(_)
            | BatchOp::SetTableSchema(_)
            | BatchOp::AddSchemaRule(_)
            | BatchOp::RemoveSchemaRule(_)
            | BatchOp::DescribeRepo(_)
            | BatchOp::CreateFunctionFolder(_)
            | BatchOp::RenameFunctionFolder(_)
            | BatchOp::InternerDump(_)
            | BatchOp::InternerTouch(_)
            | BatchOp::PurgeHistory(_)
            | BatchOp::SetRetention(_)
            | BatchOp::ChangesSince(_)
            | BatchOp::CreateReplicationProfile(_)
            | BatchOp::DropReplicationProfile(_)
            | BatchOp::CreatePublication(_)
            | BatchOp::DropPublication(_)
            | BatchOp::CreateSubscription(_)
            | BatchOp::DropSubscription(_)
            | BatchOp::AlterSubscription(_)
            | BatchOp::ListPublications(_)
            | BatchOp::ListSubscriptions(_)
            | BatchOp::ReplicationStatus(_) => return Decision::Denied("token_request_denied"),
        }
    }
    Decision::Allowed(AllowedOperation::Execute)
}
