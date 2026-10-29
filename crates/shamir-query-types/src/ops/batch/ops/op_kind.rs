//! Batch operation discriminator lookup.

macro_rules! define_op_kinds {
    ($($variant:ident => $key:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum OpKind { $($variant),* }

        pub(super) fn op_kind(key: &str) -> Option<OpKind> {
            match key { $($key => Some(OpKind::$variant),)* _ => None }
        }

        #[cfg(test)]
        const ALL: &[(&str, OpKind)] = &[$(($key, OpKind::$variant)),*];
    };
}

define_op_kinds! {
    Read => "from",
    Insert => "insert_into",
    Update => "update",
    Delete => "delete_from",
    CreateDb => "create_db",
    DropDb => "drop_db",
    CreateRepo => "create_repo",
    DropRepo => "drop_repo",
    RenameRepo => "rename_repo",
    AlterRepo => "alter_repo",
    RenameDb => "rename_db",
    CreateTable => "create_table",
    DropTable => "drop_table",
    RenameTable => "rename_table",
    CreateIndex => "create_index",
    DropIndex => "drop_index",
    RenameIndex => "rename_index",
    SetBufferConfig => "set_buffer_config",
    GetBufferConfig => "get_buffer_config",
    AlterBufferConfig => "alter_buffer_config",
    StartMigration => "start_migration",
    CommitMigration => "commit_migration",
    RollbackMigration => "rollback_migration",
    MigrationStatus => "migration_status",
    CreateUser => "create_user",
    DropUser => "drop_user",
    GrantRole => "grant_role",
    RevokeRole => "revoke_role",
    List => "list",
    Chmod => "chmod",
    Chown => "chown",
    Chgrp => "chgrp",
    CreateGroup => "create_group",
    DropGroup => "drop_group",
    RenameGroup => "rename_group",
    AddGroupMember => "add_group_member",
    RemoveGroupMember => "remove_group_member",
    AccessTree => "access_tree",
    CreateFunction => "create_function",
    DropFunction => "drop_function",
    RenameFunction => "rename_function",
    CreateValidator => "create_validator",
    DropValidator => "drop_validator",
    RenameValidator => "rename_validator",
    BindValidator => "bind_validator",
    UnbindValidator => "unbind_validator",
    ListValidators => "list_validators",
    SetTableSchema => "set_table_schema",
    AddSchemaRule => "add_schema_rule",
    RemoveSchemaRule => "remove_schema_rule",
    GetTableSchema => "get_table_schema",
    DescribeTable => "describe_table",
    DescribeRepo => "describe_repo",
    CreateFunctionFolder => "create_function_folder",
    RenameFunctionFolder => "rename_function_folder",
    InternerDump => "interner_dump",
    InternerTouch => "interner_touch",
    PurgeHistory => "purge_history",
    SetRetention => "set_retention",
    ChangesSince => "changes_since",
    CreateReplicationProfile => "create_replication_profile",
    DropReplicationProfile => "drop_replication_profile",
    CreatePublication => "create_publication",
    DropPublication => "drop_publication",
    CreateSubscription => "create_subscription",
    DropSubscription => "drop_subscription",
    AlterSubscription => "alter_subscription",
    ListPublications => "list_publications",
    ListSubscriptions => "list_subscriptions",
    ReplicationStatus => "replication_status",
    Call => "call",
    ForEach => "for_each",
    Batch => "batch",
    Subscribe => "subscribe",
    Unsubscribe => "unsubscribe",
    Set => "set",
}

#[cfg(test)]
#[path = "../tests/batch_types_tests/op_kind_tests.rs"]
mod tests;
