//! Batch execution entry point for ShamirDb.
//!
//! This module is split into focused sub-modules:
//!
//! - [`helpers`]           — shared utility fns (admin_result, validate_name_component, …)
//! - [`table_resolver`]    — `impl TableResolver for DbTableResolver`
//! - [`function_invoker`]  — `impl FunctionInvoker for ShamirFunctionInvoker`
//! - [`admin_dispatch`]    — `impl AdminExecutor for ShamirAdminExecutor` (thin dispatcher)
//! - [`admin_db_repo`]     — CreateDb, DropDb, CreateRepo, DropRepo
//! - [`admin_table_index`] — CreateTable, DropTable, CreateIndex, DropIndex
//! - [`admin_buffer`]      — GetBufferConfig, SetBufferConfig, AlterBufferConfig
//! - [`admin_list`]        — List
//! - [`admin_users_roles`] — CreateUser, DropUser, GrantRole, RevokeRole
//! - [`admin_migration`]   — StartMigration, CommitMigration, RollbackMigration, MigrationStatus
//! - [`admin_access`]      — Chmod, Chown, Chgrp, CreateGroup, DropGroup, AddGroupMember, RemoveGroupMember, AccessTree
//! - [`admin_function`]    — CreateFunction, DropFunction, RenameFunction, CreateFunctionFolder
//! - [`admin_validator`]   — CreateValidator, DropValidator, RenameValidator, BindValidator, UnbindValidator, ListValidators
//! - [`admin_schema`]      — SetTableSchema, AddSchemaRule, RemoveSchemaRule, GetTableSchema
//! - [`admin_describe`]    — DescribeTable
//! - [`admin_repo_durability`] — AlterRepo, DescribeRepo
//! - [`admin_retention`]   — SetRetention, PurgeHistory, ChangesSince
//! - [`admin_interner`]    — InternerDump, InternerTouch
//! - [`ambient_interner`]  — ambient epoch-delta sync (Part A)
//! - [`db_execute`]        — `impl ShamirDb { execute, execute_as }`
//! - [`db_tx`]             — `impl ShamirDb { tx_begin, tx_begin_as, tx_execute, tx_execute_as, tx_commit, tx_commit_as }`

mod admin_access;
mod admin_buffer;
mod admin_db_repo;
mod admin_describe;
mod admin_dispatch;
mod admin_function;
mod admin_interner;
mod admin_list;
mod admin_migration;
mod admin_replication;
#[path = "repo_admin/admin_repo_durability.rs"]
mod admin_repo_durability;
mod admin_retention;
mod admin_schema;
mod admin_table_index;
mod admin_users_roles;
mod admin_validator;
mod ambient_interner;
#[path = "repo_admin/db_execute.rs"]
mod db_execute;
mod db_tx;
mod function_invoker;
mod helpers;
pub(crate) mod replication_seed;
mod table_resolver;
mod visibility_sample;

#[cfg(test)]
mod tests;

// S5c-X PR8b: the read-quarantine cache + its negative-control seam live in
// the private `repo_admin::db_execute` module but are consumed by
// `shamir_db::shamir_db::{core, changelog}`; re-export for the crate tree.
pub(crate) use db_execute::{ResyncQuarantineCache, SKIP_READ_QUARANTINE_FOR_TEST};
