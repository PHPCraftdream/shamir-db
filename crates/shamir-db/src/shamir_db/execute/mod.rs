//! Batch execution entry point for ShamirDb.
//!
//! All sub-modules are declared here with `#[path]` so their module paths
//! stay `crate::shamir_db::execute::<name>` regardless of the on-disk
//! grouping (topical folders: `admin_ddl`, `admin_db`, `admin_meta`, `core`).
//!
//! Families:
//!
//! - DDL & schema (`admin_ddl/`) — CreateTable/DropTable, indexes, users &
//!   roles, validators, functions, interners, SetTableSchema &
//!   schema-catalogue codec
//! - DB/repo lifecycle (`admin_db/`) — CreateDb, DropDb, CreateRepo,
//!   DropRepo, migrations, retention, replication, visibility sampling
//! - Misc admin ops (`admin_meta/`) — buffer config, List, DescribeTable,
//!   access (chmod/chown/groups)
//! - Core plumbing (`core/`) — dispatcher, tx ops, shared helpers, table
//!   resolver, function invoker, ambient interner sync
//! - `repo_admin/` — repo durability, db_execute, quarantine caches
//! - `replication_seed/` — replication seeding sub-system (public re-export)

// ── admin_ddl/ — DDL, schema, users/roles, validators, functions ───────────

#[path = "admin_ddl/admin_table_index.rs"]
mod admin_table_index;

#[path = "admin_ddl/admin_users_roles.rs"]
mod admin_users_roles;

#[path = "admin_ddl/admin_validator.rs"]
mod admin_validator;

#[path = "admin_meta/admin_function.rs"]
mod admin_function;

#[path = "admin_meta/admin_interner.rs"]
mod admin_interner;

#[path = "admin_ddl/admin_schema.rs"]
mod admin_schema;

#[path = "admin_ddl/schema_codec.rs"]
mod schema_codec;

// ── admin_db/ — DB/repo lifecycle, migration, retention, replication ───────

#[path = "admin_db/admin_db_repo.rs"]
mod admin_db_repo;

#[path = "admin_db/admin_migration.rs"]
mod admin_migration;

#[path = "admin_db/admin_retention.rs"]
mod admin_retention;

#[path = "admin_db/admin_replication.rs"]
mod admin_replication;

#[path = "admin_db/visibility_sample/mod.rs"]
mod visibility_sample;

// ── admin_meta/ — buffer config, list, describe, access ────────────────────

#[path = "admin_meta/admin_buffer.rs"]
mod admin_buffer;

#[path = "admin_meta/admin_list.rs"]
mod admin_list;

#[path = "admin_meta/admin_describe.rs"]
mod admin_describe;

#[path = "admin_meta/admin_access.rs"]
mod admin_access;

// ── core/ — dispatcher, tx plumbing, shared helpers ────────────────────────

#[path = "core/admin_dispatch.rs"]
mod admin_dispatch;

#[path = "core/db_tx.rs"]
mod db_tx;

#[path = "core/helpers.rs"]
mod helpers;

#[path = "core/table_resolver.rs"]
mod table_resolver;

#[path = "core/function_invoker.rs"]
mod function_invoker;

#[path = "core/ambient_interner.rs"]
mod ambient_interner;

// ── repo_admin/ — repo durability, db execute, quarantine ──────────────────

#[path = "repo_admin/admin_repo_durability.rs"]
mod admin_repo_durability;

#[path = "repo_admin/db_execute.rs"]
mod db_execute;

#[path = "repo_admin/quarantine_helpers/mod.rs"]
mod quarantine_helpers;

#[path = "repo_admin/quarantine_cache.rs"]
pub(crate) mod quarantine_cache;

// ── replication seeding & tests ────────────────────────────────────────────

pub(crate) mod replication_seed;

#[cfg(test)]
#[path = "core/tests/mod.rs"]
mod tests;

#[cfg(test)]
pub(crate) use quarantine_cache::SKIP_READ_QUARANTINE_FOR_TEST;
pub(crate) use quarantine_cache::{skip_read_quarantine_for_test, ResyncQuarantineCache};
