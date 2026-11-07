use shamir_query_builder::{ddl, token_admin};
use shamir_query_types::admin::{
    BufferConfigDto, BufferConfigPatch, InternerDumpOp, InternerTouchOp,
};
use shamir_query_types::auth::{CreateUserOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::wire::repl::CURRENT_REPL_PROTO_VER;
use shamir_query_types::wire::{ChangePasswordSecrets, DbRequest, ReplRequest};

pub(super) fn admin_request_cases() -> Vec<(DbRequest, &'static str)> {
    vec![
        (
            DbRequest::CreateScramUser {
                name: "blocked".into(),
                password: "blocked-password".into(),
                roles: vec![],
                hmac: None,
            },
            "CreateScramUser",
        ),
        (
            DbRequest::Repl(ReplRequest::Hello {
                proto_ver: CURRENT_REPL_PROTO_VER,
                node_id: "test-node".into(),
                capabilities: None,
            }),
            "Repl",
        ),
        (
            DbRequest::ChangePasswordChallenge {
                client_nonce_cp: vec![1; 32],
            },
            "ChangePasswordChallenge",
        ),
        (
            DbRequest::ChangePasswordVerify(ChangePasswordSecrets {
                client_proof_old: vec![0; 32],
                new_salt: vec![1; 16],
                new_stored_key: vec![2; 32],
                new_server_key: vec![3; 32],
            }),
            "ChangePasswordVerify",
        ),
        (
            DbRequest::SetSuperuser {
                user: "admin".into(),
                on: false,
                hmac: None,
            },
            "SetSuperuser",
        ),
        (
            DbRequest::SetReplicator {
                user: "admin".into(),
                on: false,
                hmac: None,
            },
            "SetReplicator",
        ),
        (
            token_admin::create_api_token("denied").build(),
            "CreateApiToken",
        ),
        (token_admin::list_api_tokens().build(), "ListApiTokens"),
        (
            token_admin::revoke_api_token("AAAAAAAAAAAAAAAAAAAAAA").build(),
            "RevokeApiToken",
        ),
        (
            token_admin::revoke_user_tokens().build(),
            "RevokeUserTokens",
        ),
    ]
}

pub(super) fn admin_batch_cases() -> Vec<(BatchOp, &'static str)> {
    vec![
        (ddl::create_db("deny").build(), "CreateDb"),
        (
            ddl::drop_db("deny")
                .cascade()
                .hmac("invalid")
                .build()
                .expect("typed DropDb"),
            "DropDb",
        ),
        (
            ddl::create_repo("deny").tables(["table"]).build(),
            "CreateRepo",
        ),
        (
            ddl::drop_repo("deny")
                .cascade()
                .hmac("invalid")
                .build()
                .expect("typed DropRepo"),
            "DropRepo",
        ),
        (ddl::rename_repo("main", "other").build(), "RenameRepo"),
        (ddl::alter_repo("main").build(), "AlterRepo"),
        (ddl::rename_db("default", "other").build(), "RenameDb"),
        (
            ddl::create_table("deny").if_not_exists().build(),
            "CreateTable",
        ),
        (
            ddl::drop_table("deny")
                .cascade()
                .hmac("invalid")
                .build()
                .expect("typed DropTable"),
            "DropTable",
        ),
        (ddl::rename_table("missing", "other").build(), "RenameTable"),
        (
            ddl::create_index("idx_denied", "missing")
                .field("x")
                .build(),
            "CreateIndex",
        ),
        (
            ddl::drop_index("idx_denied", "missing")
                .if_exists()
                .hmac("invalid")
                .build()
                .expect("typed DropIndex"),
            "DropIndex",
        ),
        (
            ddl::rename_index("missing", "old", "new").build(),
            "RenameIndex",
        ),
        (
            ddl::set_buffer_config(
                "missing",
                BufferConfigDto {
                    max_bytes: 1,
                    max_entries: 1,
                    ttl_ms: None,
                    flush_interval_ms: 1,
                    flush_batch_size: 1,
                },
            )
            .build(),
            "SetBufferConfig",
        ),
        (ddl::get_buffer_config("missing").build(), "GetBufferConfig"),
        (
            ddl::alter_buffer_config("missing", BufferConfigPatch::default()).build(),
            "AlterBufferConfig",
        ),
        (
            ddl::start_migration("missing", "other", "fjall")
                .repo("main")
                .hmac("invalid")
                .build()
                .expect("typed StartMigration"),
            "StartMigration",
        ),
        (
            ddl::commit_migration("missing")
                .hmac("invalid")
                .build()
                .expect("typed CommitMigration"),
            "CommitMigration",
        ),
        (
            ddl::rollback_migration("missing")
                .hmac("invalid")
                .build()
                .expect("typed RollbackMigration"),
            "RollbackMigration",
        ),
        (ddl::migration_status("missing"), "MigrationStatus"),
        (
            BatchOp::CreateUser(CreateUserOp {
                create_user: "denied".into(),
                password: "blocked-password".into(),
                roles: vec![],
                database: None,
                hmac: None,
            }),
            "CreateUser",
        ),
        (
            BatchOp::DropUser(DropUserOp {
                drop_user: "denied".into(),
                hmac: Some("invalid".into()),
                if_exists: false,
            }),
            "DropUser",
        ),
        (
            BatchOp::GrantRole(GrantRoleOp {
                grant_role: "denied".into(),
                user: "admin".into(),
                hmac: None,
            }),
            "GrantRole",
        ),
        (
            BatchOp::RevokeRole(RevokeRoleOp {
                revoke_role: "denied".into(),
                user: "admin".into(),
                hmac: None,
            }),
            "RevokeRole",
        ),
        (
            ddl::access_control::chmod(
                shamir_query_types::admin::ResourceRef::Database {
                    database: "default".into(),
                },
                0o700,
            )
            .hmac("invalid")
            .build()
            .expect("typed Chmod"),
            "Chmod",
        ),
        (
            ddl::access_control::chown(
                shamir_query_types::admin::ResourceRef::Database {
                    database: "default".into(),
                },
                1,
            )
            .hmac("invalid")
            .build()
            .expect("typed Chown"),
            "Chown",
        ),
        (
            ddl::access_control::chgrp(
                shamir_query_types::admin::ResourceRef::Database {
                    database: "default".into(),
                },
                None,
            )
            .hmac("invalid")
            .build()
            .expect("typed Chgrp"),
            "Chgrp",
        ),
        (
            ddl::access_control::create_group("denied")
                .hmac("invalid")
                .build()
                .expect("typed CreateGroup"),
            "CreateGroup",
        ),
        (
            ddl::access_control::drop_group(shamir_query_types::admin::GroupRef::Name {
                name: "denied".into(),
            })
            .hmac("invalid")
            .build()
            .expect("typed DropGroup"),
            "DropGroup",
        ),
        (
            ddl::access_control::rename_group(
                shamir_query_types::admin::GroupRef::Name {
                    name: "denied".into(),
                },
                "other",
            )
            .hmac("invalid")
            .build()
            .expect("typed RenameGroup"),
            "RenameGroup",
        ),
        (
            ddl::access_control::add_group_member(
                shamir_query_types::admin::GroupRef::Name {
                    name: "denied".into(),
                },
                1,
            )
            .hmac("invalid")
            .build()
            .expect("typed AddGroupMember"),
            "AddGroupMember",
        ),
        (
            ddl::access_control::remove_group_member(
                shamir_query_types::admin::GroupRef::Name {
                    name: "denied".into(),
                },
                1,
            )
            .hmac("invalid")
            .build()
            .expect("typed RemoveGroupMember"),
            "RemoveGroupMember",
        ),
        (ddl::set_table_schema("missing").build(), "SetTableSchema"),
        (
            ddl::add_schema_rule("missing")
                .rule(ddl::schema::field(["x"]).string().build())
                .build()
                .expect("typed AddSchemaRule"),
            "AddSchemaRule",
        ),
        (
            ddl::remove_schema_rule("missing", ["x"]).build(),
            "RemoveSchemaRule",
        ),
        (
            ddl::create_validator("denied")
                .source("pub fn validate() {}")
                .build()
                .expect("typed CreateValidator"),
            "CreateValidator",
        ),
        (
            ddl::drop_validator("denied")
                .if_exists()
                .hmac("invalid")
                .build()
                .expect("typed DropValidator"),
            "DropValidator",
        ),
        (ddl::rename_validator("denied", "other"), "RenameValidator"),
        (
            ddl::bind_validator("denied", "missing")
                .db("default")
                .build(),
            "BindValidator",
        ),
        (
            ddl::unbind_validator("denied", "missing")
                .db("default")
                .build(),
            "UnbindValidator",
        ),
        (
            ddl::list_validators("missing").db("default").build(),
            "ListValidators",
        ),
        (
            BatchOp::InternerDump(InternerDumpOp {
                interner_dump: "main".into(),
                since: None,
            }),
            "InternerDump",
        ),
        (
            BatchOp::InternerTouch(InternerTouchOp {
                interner_touch: "main".into(),
                names: vec!["x".into()],
            }),
            "InternerTouch",
        ),
        (
            ddl::purge_history(
                "missing",
                shamir_query_types::admin::PurgeScope::OlderThanAge { age_secs: 1 },
            )
            .hmac("invalid")
            .build()
            .expect("typed PurgeHistory"),
            "PurgeHistory",
        ),
        (
            ddl::set_retention("missing", shamir_query_types::admin::Retention::default())
                .hmac("invalid")
                .build()
                .expect("typed SetRetention"),
            "SetRetention",
        ),
        (ddl::changes_since(0).build(), "ChangesSince"),
        (ddl::describe_repo("default").build(), "DescribeRepo"),
        (
            ddl::create_function_folder(["denied"]),
            "CreateFunctionFolder",
        ),
        (
            ddl::rename_function_folder(["denied"], ["other"]),
            "RenameFunctionFolder",
        ),
        (
            ddl::create_function("denied")
                .source("pub fn denied() {}")
                .build()
                .expect("typed CreateFunction"),
            "CreateFunction",
        ),
        (
            ddl::drop_function("denied")
                .hmac("invalid")
                .build()
                .expect("typed DropFunction"),
            "DropFunction",
        ),
        (ddl::rename_function("denied", "other"), "RenameFunction"),
        (
            ddl::replication_profile("denied").build(),
            "CreateReplicationProfile",
        ),
        (
            ddl::drop_replication_profile("denied")
                .hmac("invalid")
                .build()
                .expect("typed DropReplicationProfile"),
            "DropReplicationProfile",
        ),
        (ddl::publication("denied").build(), "CreatePublication"),
        (
            ddl::drop_publication("denied")
                .hmac("invalid")
                .build()
                .expect("typed DropPublication"),
            "DropPublication",
        ),
        (
            ddl::subscription("denied", "loopback", "denied", "denied"),
            "CreateSubscription",
        ),
        (
            ddl::drop_subscription("denied")
                .hmac("invalid")
                .build()
                .expect("typed DropSubscription"),
            "DropSubscription",
        ),
        (
            ddl::alter_subscription("denied")
                .pause()
                .build()
                .expect("typed AlterSubscription"),
            "AlterSubscription",
        ),
        (ddl::list_publications(), "ListPublications"),
        (ddl::list_subscriptions(), "ListSubscriptions"),
        (ddl::replication_status(), "ReplicationStatus"),
    ]
}
