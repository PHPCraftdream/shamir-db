//! Per-variant classification impls for [`BatchOp`].

use shamir_types::access::{Action, ResourcePath};

use super::super::ops::batch_op::BatchOp;
use crate::admin::types::schema_migration::SchemaMigrationAdminOp;
use crate::batch::tree_visitor::BatchTreeVisitor;

impl BatchOp {
    /// Returns the `(Action, ResourcePath)` this op must be authorized
    /// against, for ops with a `table_ref()`. `None` for admin/DDL ops
    /// (authorized separately in `execute_admin`) and read-only/
    /// introspection ops with no target table.
    ///
    /// This is the single source of truth for the per-op DML authorization
    /// mapping that was previously duplicated (byte-for-byte) in
    /// `shamir-db`'s `execute_as` and `tx_execute_as` — both now call this
    /// method instead of re-deriving the `Action` inline. Like
    /// [`is_write`](Self::is_write), this is an **exhaustive `match` with
    /// no wildcard arm**: adding a new `BatchOp` variant that carries a
    /// `table_ref()` will fail to compile here until the author explicitly
    /// classifies its `Action` — the old duplicated code's `_ =>
    /// Action::Write` wildcard silently swallowed that decision instead.
    ///
    /// The per-variant `Action` choice mirrors `is_write`'s existing
    /// read/write classification: `Read` → `Action::Read`; `Insert` →
    /// `Action::Create`; `Set`/`Update` → `Action::Write`; `Delete` →
    /// `Action::Delete`. Every other variant either has no `table_ref()`
    /// (admin/DDL, `Batch`, `Subscribe`, `Unsubscribe`) and is unreachable
    /// here, or is listed explicitly below for completeness even though
    /// `table_ref()` already returns `None` for it — this keeps the two
    /// matches mechanically in sync so a future change to `table_ref()`
    /// that starts returning `Some` for one of them is forced to pick an
    /// `Action` here too.
    pub fn required_access(&self, db: &str) -> Option<(Action, ResourcePath)> {
        let action = match self {
            // ----- data ops: the classification duplicated pre-fix -------
            BatchOp::Read(_) => Action::Read,
            BatchOp::Insert(_) => Action::Create,
            BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
            BatchOp::Delete(_) => Action::Delete,

            // ----- everything else has no table_ref(): unreachable below,
            // but listed explicitly (no wildcard) so a new BatchOp variant
            // forces a conscious choice here too. -------------------------
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
            | BatchOp::List(_)
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
            | BatchOp::AccessTree(_)
            | BatchOp::CreateFunction(_)
            | BatchOp::DropFunction(_)
            | BatchOp::RenameFunction(_)
            | BatchOp::DescribeFunction(_)
            | BatchOp::ExportFunction(_)
            | BatchOp::CheckFunction(_)
            | BatchOp::AlterFunction(_)
            | BatchOp::CreateValidator(_)
            | BatchOp::DropValidator(_)
            | BatchOp::RenameValidator(_)
            | BatchOp::BindValidator(_)
            | BatchOp::UnbindValidator(_)
            | BatchOp::ListValidators(_)
            | BatchOp::SetTableSchema(_)
            | BatchOp::AddSchemaRule(_)
            | BatchOp::RemoveSchemaRule(_)
            | BatchOp::GetTableSchema(_)
            | BatchOp::DescribeTable(_)
            | BatchOp::DescribeRepo(_)
            | BatchOp::CreateFunctionFolder(_)
            | BatchOp::RenameFunctionFolder(_)
            | BatchOp::InternerDump(_)
            | BatchOp::InternerTouch(_)
            | BatchOp::PurgeHistory(_)
            | BatchOp::SetRetention(_)
            | BatchOp::ChangesSince(_)
            | BatchOp::Call(_)
            | BatchOp::Batch(_)
            | BatchOp::ForEach(_)
            | BatchOp::Subscribe(_)
            | BatchOp::Unsubscribe(_)
            | BatchOp::CreateReplicationProfile(_)
            | BatchOp::DropReplicationProfile(_)
            | BatchOp::CreatePublication(_)
            | BatchOp::DropPublication(_)
            | BatchOp::CreateSubscription(_)
            | BatchOp::DropSubscription(_)
            | BatchOp::AlterSubscription(_)
            | BatchOp::ListPublications(_)
            | BatchOp::ListSubscriptions(_)
            | BatchOp::ReplicationStatus(_)
            | BatchOp::ApplicationSchema(_)
            | BatchOp::StartBackup(_)
            | BatchOp::BackupStatus(_)
            | BatchOp::CancelBackup(_)
            | BatchOp::ListActivity(_)
            | BatchOp::StartExport(_)
            | BatchOp::ExportStatus(_)
            | BatchOp::CancelExport(_)
            | BatchOp::CancelRequest(_)
            | BatchOp::CancelTx(_)
            | BatchOp::CancelCursorAdmin(_)
            | BatchOp::ListShapeStats(_)
            | BatchOp::ResetShapeStats(_) => return None,
        };
        let tref = self.table_ref()?;
        Some((
            action,
            ResourcePath::Table {
                db: db.to_string(),
                store: tref.repo.clone(),
                table: tref.table.clone(),
            },
        ))
    }

    /// Returns the table reference for data operations, None for admin ops.
    pub fn table_ref(&self) -> Option<&crate::TableRef> {
        match self {
            BatchOp::Read(q) => Some(&q.from),
            BatchOp::Insert(i) => Some(&i.insert_into),
            BatchOp::Update(u) => Some(&u.update),
            BatchOp::Set(s) => Some(&s.set),
            BatchOp::Delete(d) => Some(&d.delete_from),
            BatchOp::Batch(_)
            | BatchOp::ForEach(_)
            | BatchOp::Subscribe(_)
            | BatchOp::Unsubscribe(_) => None,
            _ => None,
        }
    }

    /// Returns true if this leaf or any descendant leaf is an admin (DDL) op.
    pub fn is_admin(&self) -> bool {
        if let Some(queries) = BatchTreeVisitor::body_queries(self) {
            return BatchTreeVisitor::new(queries).any(|visit| visit.entry.op.is_admin_leaf());
        }
        self.is_admin_leaf()
    }

    fn is_admin_leaf(&self) -> bool {
        matches!(
            self,
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
                | BatchOp::List(_)
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
                | BatchOp::AccessTree(_)
                | BatchOp::CreateFunction(_)
                | BatchOp::DropFunction(_)
                | BatchOp::RenameFunction(_)
                | BatchOp::DescribeFunction(_)
                | BatchOp::ExportFunction(_)
                | BatchOp::CheckFunction(_)
                | BatchOp::AlterFunction(_)
                | BatchOp::CreateValidator(_)
                | BatchOp::DropValidator(_)
                | BatchOp::RenameValidator(_)
                | BatchOp::BindValidator(_)
                | BatchOp::UnbindValidator(_)
                | BatchOp::ListValidators(_)
                | BatchOp::SetTableSchema(_)
                | BatchOp::AddSchemaRule(_)
                | BatchOp::RemoveSchemaRule(_)
                | BatchOp::GetTableSchema(_)
                | BatchOp::DescribeTable(_)
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
                | BatchOp::ReplicationStatus(_)
                | BatchOp::ApplicationSchema(_)
                | BatchOp::StartBackup(_)
                | BatchOp::BackupStatus(_)
                | BatchOp::CancelBackup(_)
                | BatchOp::ListActivity(_)
                | BatchOp::StartExport(_)
                | BatchOp::ExportStatus(_)
                | BatchOp::CancelExport(_)
                | BatchOp::CancelRequest(_)
                | BatchOp::CancelTx(_)
                | BatchOp::CancelCursorAdmin(_)
                | BatchOp::ListShapeStats(_)
                | BatchOp::ResetShapeStats(_)
        )
    }

    /// Returns true if this operation mutates data or persistent server state.
    ///
    /// This is the gate used by read-only follower nodes (REPLICATION §4.3)
    /// to reject writes: any op that is *not* a pure read/introspection is
    /// classified as a write. `Call` (WASM stored-procedure invocation) is
    /// conservatively treated as a write, since a host function may mutate
    /// state.
    ///
    /// Unlike [`is_admin`](Self::is_admin) (which uses `matches!`), this is
    /// an **exhaustive `match` with no wildcard arm** — adding a new
    /// `BatchOp` variant will break compilation here and force the author
    /// to explicitly classify it as read or write.
    pub fn is_write(&self) -> bool {
        if let Some(queries) = BatchTreeVisitor::body_queries(self) {
            return BatchTreeVisitor::new(queries).any(|visit| visit.entry.op.is_write_leaf());
        }
        self.is_write_leaf()
    }

    pub(crate) fn is_write_leaf(&self) -> bool {
        match self {
            // ----- pure reads / introspection (write = false) -------------
            BatchOp::Read(_) => false,
            BatchOp::List(_) => false,
            BatchOp::GetBufferConfig(_) => false,
            BatchOp::MigrationStatus(_) => false,
            BatchOp::AccessTree(_) => false,
            BatchOp::ListValidators(_) => false,
            BatchOp::GetTableSchema(_) => false,
            BatchOp::DescribeTable(_) => false,
            BatchOp::DescribeRepo(_) => false,
            BatchOp::DescribeFunction(_) => false,
            BatchOp::ExportFunction(_) => false,
            BatchOp::CheckFunction(_) => false,
            BatchOp::InternerDump(_) => false,
            BatchOp::ChangesSince(_) => false,
            BatchOp::Subscribe(_) => false,
            BatchOp::Unsubscribe(_) => false,
            BatchOp::ListPublications(_) => false,
            BatchOp::ListSubscriptions(_) => false,
            BatchOp::ReplicationStatus(_) => false,
            // Online-backup admin ops: status is pure introspection; start
            // and cancel change persistent node-side backup state (R2a 3a).
            BatchOp::BackupStatus(_) | BatchOp::ListActivity(_) => false,
            // Online logical-export admin ops (R3 step 2b): status is pure
            // introspection; start and cancel change export-job state.
            BatchOp::ExportStatus(_) => false,
            BatchOp::StartExport(_) => true,
            BatchOp::CancelExport(_) => true,
            // Cooperative cancellation only changes transient execution state.
            // Operator cancellation likewise only changes transient state.
            BatchOp::CancelRequest(_) => false,
            BatchOp::CancelTx(_) | BatchOp::CancelCursorAdmin(_) => false,
            // The shape registry is local in-memory telemetry; the listing is
            // pure introspection.
            BatchOp::ListShapeStats(_) => false,
            // The reset destroys local statistics only, never stored data
            // (audited as an admin op, see the server handler).
            BatchOp::ResetShapeStats(_) => false,
            BatchOp::StartBackup(_) => true,
            BatchOp::CancelBackup(_) => true,
            // Application-schema migrations: the read-only status and plan
            // arms do not mutate state; the apply arm does (design §5.2).
            BatchOp::ApplicationSchema(
                SchemaMigrationAdminOp::ApplicationSchemaStatus(_)
                | SchemaMigrationAdminOp::ApplicationSchemaPlan(_),
            ) => false,
            BatchOp::ApplicationSchema(
                SchemaMigrationAdminOp::ApplyApplicationSchemaMigration(_),
            ) => true,

            // ----- data mutations ----------------------------------------
            BatchOp::Insert(_) => true,
            BatchOp::Update(_) => true,
            BatchOp::Set(_) => true,
            BatchOp::Delete(_) => true,

            // ----- DDL (catalog / schema / index) ------------------------
            BatchOp::CreateDb(_) => true,
            BatchOp::DropDb(_) => true,
            BatchOp::CreateRepo(_) => true,
            BatchOp::DropRepo(_) => true,
            BatchOp::RenameRepo(_) => true,
            BatchOp::AlterRepo(_) => true,
            BatchOp::RenameDb(_) => true,
            BatchOp::CreateTable(_) => true,
            BatchOp::DropTable(_) => true,
            BatchOp::RenameTable(_) => true,
            BatchOp::CreateIndex(_) => true,
            BatchOp::DropIndex(_) => true,
            BatchOp::RenameIndex(_) => true,
            BatchOp::SetBufferConfig(_) => true,
            BatchOp::AlterBufferConfig(_) => true,

            // ----- migrations (engine swap) ------------------------------
            BatchOp::StartMigration(_) => true,
            BatchOp::CommitMigration(_) => true,
            BatchOp::RollbackMigration(_) => true,

            // ----- auth mutations ----------------------------------------
            BatchOp::CreateUser(_) => true,
            BatchOp::DropUser(_) => true,
            BatchOp::GrantRole(_) => true,
            BatchOp::RevokeRole(_) => true,

            // ----- access-control DDL ------------------------------------
            BatchOp::Chmod(_) => true,
            BatchOp::Chown(_) => true,
            BatchOp::Chgrp(_) => true,
            BatchOp::CreateGroup(_) => true,
            BatchOp::DropGroup(_) => true,
            BatchOp::RenameGroup(_) => true,
            BatchOp::AddGroupMember(_) => true,
            BatchOp::RemoveGroupMember(_) => true,

            // ----- function / validator DDL ------------------------------
            BatchOp::CreateFunction(_) => true,
            BatchOp::DropFunction(_) => true,
            BatchOp::RenameFunction(_) => true,
            BatchOp::AlterFunction(_) => true,
            BatchOp::CreateValidator(_) => true,
            BatchOp::DropValidator(_) => true,
            BatchOp::RenameValidator(_) => true,
            BatchOp::BindValidator(_) => true,
            BatchOp::UnbindValidator(_) => true,

            // ----- declarative schema DDL --------------------------------
            BatchOp::SetTableSchema(_) => true,
            BatchOp::AddSchemaRule(_) => true,
            BatchOp::RemoveSchemaRule(_) => true,

            // ----- function-folder DDL -----------------------------------
            BatchOp::CreateFunctionFolder(_) => true,
            BatchOp::RenameFunctionFolder(_) => true,

            // ----- interner / temporal mutations -------------------------
            BatchOp::InternerTouch(_) => true,
            BatchOp::PurgeHistory(_) => true,
            BatchOp::SetRetention(_) => true,

            // ----- replication DDL (REPLICATION.md §5.5) ------------------
            // Mutating ops: create/drop/alter — they change the system-store
            // catalog (profiles / publications / subscriptions) and thus the
            // persistent cluster topology.
            BatchOp::CreateReplicationProfile(_) => true,
            BatchOp::DropReplicationProfile(_) => true,
            BatchOp::CreatePublication(_) => true,
            BatchOp::DropPublication(_) => true,
            BatchOp::CreateSubscription(_) => true,
            BatchOp::DropSubscription(_) => true,
            BatchOp::AlterSubscription(_) => true,

            // ----- stored procedures -------------------------------------
            // Conservative: a WASM host function may mutate state.
            BatchOp::Call(_) => true,

            // Containers have no intrinsic policy; the iterative visitor
            // classifies the FULL static body, even for zero iterations.
            BatchOp::Batch(_) | BatchOp::ForEach(_) => false,
        }
    }
}
