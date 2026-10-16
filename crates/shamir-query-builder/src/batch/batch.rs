use super::batch_refs::{collect_filter_refs, collect_op_query_refs, split_path_tail};
use shamir_collections::{new_map, TMap};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, IndexVisibility as WireIndexVisibility,
    IsolationLevel as WireIsolationLevel, QueryEntry, RequestDurability as WireRequestDurability,
    ResultEncoding, SubBatchOp,
};
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::ReadQuery;
use shamir_query_types::subscribe::{SubscribeOp, UnsubscribeOp};
use shamir_types::types::value::QueryValue;

#[path = "op/into_batch_op/repo_admin.rs"]
mod repo_admin;

use super::build_error::BuildError;
use super::durability::Durability;
use super::handle::Handle;
use super::index_visibility::IndexVisibility;
use super::into_batch_op::IntoBatchOp;
use super::isolation::Isolation;
use super::try_into_batch_op::TryIntoBatchOp;
use crate::write::BuilderError;

/// Fluent batch assembler.
///
/// Accumulates query/write entries under string aliases and produces a
/// [`BatchRequest`].
#[derive(Debug, Clone)]
pub struct Batch {
    id: QueryValue,
    name: Option<String>,
    transactional: bool,
    isolation: Option<WireIsolationLevel>,
    visibility_wait: Option<shamir_query_types::batch::VisibilityWait>,
    durability: Option<WireRequestDurability>,
    index_visibility: Option<WireIndexVisibility>,
    pub(crate) queries: TMap<String, QueryEntry>,
    return_all: bool,
    return_only: Option<Vec<String>>,
    limits: BatchLimits,
    result_encoding: ResultEncoding,
    interner_epochs: TMap<String, u64>,
}

impl Default for Batch {
    fn default() -> Self {
        Self::new()
    }
}

impl Batch {
    /// Create an empty batch with default settings.
    pub fn new() -> Self {
        Self {
            id: QueryValue::Null,
            name: None,
            transactional: false,
            isolation: None,
            visibility_wait: None,
            durability: None,
            index_visibility: None,
            queries: new_map(),
            return_all: true,
            return_only: None,
            limits: BatchLimits::default(),
            result_encoding: ResultEncoding::default(),
            interner_epochs: new_map(),
        }
    }

    /// Create a named batch.
    pub fn named(name: impl Into<String>) -> Self {
        let mut b = Self::new();
        b.name = Some(name.into());
        b
    }

    // ── config (chainable) ─────────────────────────────────────────

    /// Set the batch name.
    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// Set the client-provided request id.
    pub fn id(&mut self, id: impl Into<QueryValue>) -> &mut Self {
        self.id = id.into();
        self
    }

    /// Enable transactional semantics (MVCC).
    pub fn transactional(&mut self) -> &mut Self {
        self.transactional = true;
        self
    }

    /// Set the isolation level.
    pub fn isolation(&mut self, iso: Isolation) -> &mut Self {
        self.isolation = Some(match iso {
            Isolation::Snapshot => WireIsolationLevel::Snapshot,
            Isolation::Serializable => WireIsolationLevel::Serializable,
            Isolation::Pessimistic => WireIsolationLevel::Pessimistic,
        });
        self
    }

    /// Carry a commit-only visibility policy (strict behaviour in VIS-1).
    pub fn visibility_wait(
        &mut self,
        wait: shamir_query_types::batch::VisibilityWait,
    ) -> &mut Self {
        self.visibility_wait = Some(wait);
        self
    }

    /// Set the durability level.
    pub fn durability(&mut self, d: Durability) -> &mut Self {
        self.durability = Some(match d {
            Durability::Buffered => WireRequestDurability::Buffered,
            Durability::Synced => WireRequestDurability::Synced,
        });
        self
    }

    /// Set the index-apply visibility for transactional commits (v4).
    pub fn index_visibility(&mut self, v: IndexVisibility) -> &mut Self {
        self.index_visibility = Some(match v {
            IndexVisibility::Sync => WireIndexVisibility::Sync,
            IndexVisibility::Async => WireIndexVisibility::Async,
        });
        self
    }

    /// Return all results (resets `return_only`).
    pub fn return_all(&mut self) -> &mut Self {
        self.return_all = true;
        self.return_only = None;
        self
    }

    /// Return only entries whose `return_result` flag is `true`.
    ///
    /// Sets `return_all = false` without specifying a `return_only` list.
    /// The executor will filter results to include only those aliases added
    /// via non-silent methods (or whose `return_result` was explicitly set
    /// to `true`), skipping entries added via `query_silent` / `op_silent`.
    pub fn return_flagged(&mut self) -> &mut Self {
        self.return_all = false;
        self.return_only = None;
        self
    }

    /// Return only the listed aliases.
    pub fn return_only(
        &mut self,
        aliases: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        self.return_only = Some(aliases.into_iter().map(Into::into).collect());
        self.return_all = false;
        self
    }

    /// Override default execution limits.
    pub fn limits(&mut self, limits: BatchLimits) -> &mut Self {
        self.limits = limits;
        self
    }

    /// Set the result encoding (Name vs Id).
    pub fn result_encoding(&mut self, enc: ResultEncoding) -> &mut Self {
        self.result_encoding = enc;
        self
    }

    /// Set the per-repo interner epochs the client has cached.
    pub fn interner_epochs(&mut self, epochs: TMap<String, u64>) -> &mut Self {
        self.interner_epochs = epochs;
        self
    }

    // ── entry insertion ────────────────────────────────────────────

    /// Add a read query (returned in the response).
    pub fn query(&mut self, alias: impl Into<String>, q: impl Into<ReadQuery>) -> Handle {
        self.add_entry(alias, BatchOp::Read(q.into()), true)
    }

    /// Add a read query (silent — not returned in the response).
    pub fn query_silent(&mut self, alias: impl Into<String>, q: impl Into<ReadQuery>) -> Handle {
        self.add_entry(alias, BatchOp::Read(q.into()), false)
    }

    /// Add a read query with explicit ordering dependencies, in one call.
    ///
    /// Equivalent to `query(...)` followed by `after(&handle, on)` for each
    /// entry in `after`, but registers the dependency at the same call site
    /// as the entry itself — no separate post-hoc statement, and no chance
    /// of transposing `dependent`/`on` (a real risk with the two-`&Handle`
    /// [`Batch::after`]).
    pub fn query_after(
        &mut self,
        alias: impl Into<String>,
        q: impl Into<ReadQuery>,
        after: &[&Handle],
    ) -> Handle {
        self.add_entry_after(alias, BatchOp::Read(q.into()), true, after)
    }

    /// Add an insert operation.
    pub fn insert(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an insert operation with explicit ordering dependencies.
    ///
    /// See [`Batch::query_after`] for the rationale (fluent, transposition-safe
    /// `after` at the registration call site).
    pub fn insert_after(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
        after: &[&Handle],
    ) -> Handle {
        self.add_entry_after(alias, op.into_batch_op(), true, after)
    }

    /// Add an update operation.
    pub fn update(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an update operation with explicit ordering dependencies.
    ///
    /// See [`Batch::query_after`] for the rationale.
    pub fn update_after(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
        after: &[&Handle],
    ) -> Handle {
        self.add_entry_after(alias, op.into_batch_op(), true, after)
    }

    /// Add an upsert (set) operation.
    pub fn upsert(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an upsert (set) operation with explicit ordering dependencies.
    ///
    /// See [`Batch::query_after`] for the rationale.
    pub fn upsert_after(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
        after: &[&Handle],
    ) -> Handle {
        self.add_entry_after(alias, op.into_batch_op(), true, after)
    }

    /// Add a delete operation.
    pub fn delete(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add a delete operation with explicit ordering dependencies.
    ///
    /// See [`Batch::query_after`] for the rationale.
    pub fn delete_after(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
        after: &[&Handle],
    ) -> Handle {
        self.add_entry_after(alias, op.into_batch_op(), true, after)
    }

    // ── fallible write-op insertion ───────────────────────────────
    //
    // Mirrors of the `update`/`upsert`/`delete` entry methods above that
    // surface a missing-required-field as a typed `BuilderError` instead of
    // panicking inside `IntoBatchOp`. Each calls the builder's own fallible
    // `build()` and delegates to the same internal `add_entry`/`add_entry_after`
    // its non-try sibling uses, so a well-formed op produces the identical wire
    // shape (see `try_into_batch_op_tests`).

    /// Fallible mirror of [`Batch::update`]: build `op` and add it, returning a
    /// typed error if [`crate::write::Update`] is missing its `.set()` payload.
    pub fn try_update(
        &mut self,
        alias: impl Into<String>,
        op: crate::write::Update,
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry(alias, BatchOp::Update(op.build()?), true))
    }

    /// Fallible mirror of [`Batch::update_after`].
    pub fn try_update_after(
        &mut self,
        alias: impl Into<String>,
        op: crate::write::Update,
        after: &[&Handle],
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry_after(alias, BatchOp::Update(op.build()?), true, after))
    }

    /// Fallible mirror of [`Batch::upsert`]: build `op` and add it, returning a
    /// typed error if [`crate::write::Upsert`] is missing its `.key()` /
    /// `.value()`.
    pub fn try_upsert(
        &mut self,
        alias: impl Into<String>,
        op: crate::write::Upsert,
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry(alias, BatchOp::Set(op.build()?), true))
    }

    /// Fallible mirror of [`Batch::upsert_after`].
    pub fn try_upsert_after(
        &mut self,
        alias: impl Into<String>,
        op: crate::write::Upsert,
        after: &[&Handle],
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry_after(alias, BatchOp::Set(op.build()?), true, after))
    }

    /// Fallible mirror of [`Batch::delete`]: build `op` and add it, returning a
    /// typed error if [`crate::write::Delete`] is missing its `.where_()` clause.
    pub fn try_delete(
        &mut self,
        alias: impl Into<String>,
        op: crate::write::Delete,
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry(alias, BatchOp::Delete(op.build()?), true))
    }

    /// Fallible mirror of [`Batch::delete_after`].
    pub fn try_delete_after(
        &mut self,
        alias: impl Into<String>,
        op: crate::write::Delete,
        after: &[&Handle],
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry_after(alias, BatchOp::Delete(op.build()?), true, after))
    }

    // ── DDL: database ──────────────────────────────────────────────

    /// Create a new database.
    pub fn create_db(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a database.
    pub fn drop_db(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: repository ───────────────────────────────────────────

    /// Create a new repository.
    pub fn create_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a repository.
    pub fn drop_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a repository.
    pub fn rename_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a database (pure catalogue re-key, no file move).
    pub fn rename_db(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: table ────────────────────────────────────────────────

    /// Create a table.
    pub fn create_table(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a table.
    pub fn drop_table(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a table.
    pub fn rename_table(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: index ────────────────────────────────────────────────

    /// Create an index on a table.
    pub fn create_index(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop an index from a table.
    pub fn drop_index(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename an index on a table (in-place rekey, no data loss).
    pub fn rename_index(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: function ─────────────────────────────────────────────

    /// Create (or replace) a stored function.
    pub fn create_function(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a stored function.
    pub fn drop_function(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a stored function.
    pub fn rename_function(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Create a function folder.
    pub fn create_function_folder(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
    ) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a function folder (and its descendant subtree).
    pub fn rename_function_folder(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
    ) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: validator ────────────────────────────────────────────

    /// Create (or replace) a validator.
    pub fn create_validator(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a validator.
    pub fn drop_validator(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a validator.
    pub fn rename_validator(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Bind a validator to a table.
    pub fn bind_validator(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Unbind a validator from a table.
    pub fn unbind_validator(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List validator bindings for a table.
    pub fn list_validators(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: auth (users + roles) ─────────────────────────────────

    /// Create a user.
    pub fn create_user(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a user.
    pub fn drop_user(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Grant a role to a user.
    pub fn grant_role(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Revoke a role from a user.
    pub fn revoke_role(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: access control ───────────────────────────────────────

    /// Change mode bits on a resource.
    pub fn chmod(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Change owner on a resource.
    pub fn chown(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Change group on a resource.
    pub fn chgrp(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: groups ───────────────────────────────────────────────

    /// Create a new group.
    pub fn create_group(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a group.
    pub fn drop_group(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a group.
    pub fn rename_group(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add a user to a group.
    pub fn add_group_member(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Remove a user from a group.
    pub fn remove_group_member(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
    ) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: buffer config ────────────────────────────────────────

    /// Set the full buffer config for a table.
    pub fn set_buffer_config(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Get the buffer config for a table.
    pub fn get_buffer_config(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Partially alter buffer config for a table.
    pub fn alter_buffer_config(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
    ) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: declarative schema ───────────────────────────────────

    /// Whole-replace a table's declarative schema.
    pub fn set_table_schema(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add (or replace by path) a single rule in a table's schema.
    pub fn add_schema_rule(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Fallible mirror of [`Batch::add_schema_rule`]: build `op` and add it,
    /// returning a typed error if [`crate::ddl::AddSchemaRuleBuilder`] is
    /// missing its `.rule(...)`.
    pub fn try_add_schema_rule(
        &mut self,
        alias: impl Into<String>,
        op: crate::ddl::AddSchemaRuleBuilder,
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry(alias, op.build()?, true))
    }

    /// Remove a rule from a table's schema by path.
    pub fn remove_schema_rule(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Read a table's declarative schema (introspection).
    pub fn get_table_schema(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Describe a table — full introspection in one response.
    pub fn describe_table(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: retention ─────────────────────────────────────────────

    /// Change a live table's history-retention policy.
    pub fn set_retention(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Imperative one-shot history purge (temporal T4).
    pub fn purge_history(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// One-shot "changes since version V" journal read (temporal T4-changes-since).
    pub fn changes_since(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: list operations ──────────────────────────────────────

    /// List databases.
    pub fn list_databases(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List repositories.
    pub fn list_repos(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List tables in a repository.
    pub fn list_tables(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List indexes on a table.
    pub fn list_indexes(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List users.
    pub fn list_users(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List all registered functions (catalogue-wide).
    pub fn list_functions(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List all registered validators (catalogue-wide).
    pub fn list_all_validators(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
    ) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// List explicitly created function folders.
    pub fn list_function_folders(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
    ) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: access tree ──────────────────────────────────────────

    /// Request the access-control tree.
    pub fn access_tree(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── DDL: migration ────────────────────────────────────────────

    /// Start an online table migration.
    pub fn start_migration(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Commit a running migration.
    pub fn commit_migration(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rollback a running migration.
    pub fn rollback_migration(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Query the status of a migration.
    pub fn migration_status(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    // ── stored procedure call ────────────────────────────────────

    /// Call a stored function with positional parameters.
    ///
    /// The function runs in the default repository (`"main"`).
    /// Each parameter is converted to a [`FilterValue`], so you can pass
    /// literals (`lit(1)`, `"hello"`) as well as `$query` references
    /// from other batch handles.
    pub fn call(
        &mut self,
        alias: impl Into<String>,
        name: impl Into<String>,
        params: impl IntoIterator<Item = impl Into<FilterValue>>,
    ) -> Handle {
        let op = CallOp {
            call: name.into(),
            params: params.into_iter().map(Into::into).collect(),
            repo: "main".to_string(),
        };
        self.add_entry(alias, BatchOp::Call(op), true)
    }

    /// Call a stored function in a specific repository.
    pub fn call_in_repo(
        &mut self,
        alias: impl Into<String>,
        name: impl Into<String>,
        repo: impl Into<String>,
        params: impl IntoIterator<Item = impl Into<FilterValue>>,
    ) -> Handle {
        let op = CallOp {
            call: name.into(),
            params: params.into_iter().map(Into::into).collect(),
            repo: repo.into(),
        };
        self.add_entry(alias, BatchOp::Call(op), true)
    }

    // ── nested sub-batch ──────────────────────────────────────────────

    /// Embed a nested [`BatchRequest`] with explicit parameter bindings.
    ///
    /// Registers a `BatchOp::Batch(SubBatchOp { batch, bind })` entry under
    /// `alias` and returns a [`Handle`] so outer queries can reference the
    /// sub-batch result via `$query` paths (e.g. `handle.first().field("id")`).
    ///
    /// `bind` maps parameter names to [`FilterValue`]s (typically `$query`
    /// references into the outer batch, or plain literals).  Inner queries
    /// reference these values via [`crate::val::param`].
    ///
    /// Use [`sub_batch_no_bind`][Self::sub_batch_no_bind] when no bindings
    /// are needed.
    pub fn sub_batch(
        &mut self,
        alias: impl Into<String>,
        inner: impl Into<BatchRequest>,
        bind: TMap<String, FilterValue>,
    ) -> Handle {
        let op = SubBatchOp {
            batch: inner.into(),
            bind,
        };
        self.add_entry(alias, BatchOp::Batch(op), true)
    }

    /// Embed a nested [`BatchRequest`] with no parameter bindings.
    ///
    /// Convenience wrapper over [`sub_batch`][Self::sub_batch] for the common
    /// case where the inner batch is self-contained and requires no
    /// outer-scope values.
    pub fn sub_batch_no_bind(
        &mut self,
        alias: impl Into<String>,
        inner: impl Into<BatchRequest>,
    ) -> Handle {
        self.sub_batch(alias, inner, new_map())
    }

    // ── for-each loop ─────────────────────────────────────────────────

    /// Register a data-dependent for-each loop (`BatchOp::ForEach`, Epic04).
    ///
    /// `over` resolves to a list EXACTLY ONCE before the loop starts (it may be
    /// a `$query` reference, an `$fn` call, or a literal array — anything
    /// convertible to `FilterValue`). The loop body (`inner`) is planned once
    /// and executed once per element, with the current element bound to the
    /// parameter named `bind_row` — reference it inside `inner` via
    /// [`crate::val::param`].
    ///
    /// See `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` for the
    /// primitive's full semantics (result shape, `max_iterations` limit, error
    /// abort behavior).
    pub fn for_each(
        &mut self,
        alias: impl Into<String>,
        over: impl Into<FilterValue>,
        bind_row: impl Into<String>,
        inner: impl Into<BatchRequest>,
    ) -> Handle {
        let op = ForEachOp {
            over: over.into(),
            bind_row: bind_row.into(),
            batch: inner.into(),
        };
        self.add_entry(alias, BatchOp::ForEach(op), true)
    }

    // ── subscriptions ─────────────────────────────────────────────

    /// Subscribe to table change events.
    pub fn subscribe(&mut self, alias: impl Into<String>, sub: SubscribeOp) -> Handle {
        self.add_entry(alias, BatchOp::Subscribe(sub), true)
    }

    /// Cancel an active subscription.
    pub fn unsubscribe(&mut self, alias: impl Into<String>, sub_id: u64) -> Handle {
        self.add_entry(
            alias,
            BatchOp::Unsubscribe(UnsubscribeOp {
                unsubscribe: sub_id,
            }),
            true,
        )
    }

    // ── escape hatches ────────────────────────────────────────────

    /// Escape hatch: add any `BatchOp` (returned in the response).
    pub fn op(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Escape hatch: add any `BatchOp` (silent).
    pub fn op_silent(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), false)
    }

    /// Escape hatch: add any `BatchOp` with explicit ordering dependencies —
    /// the typical case is DDL→DML ordering (e.g. `create_table` then
    /// `insert`) where there is no `$query` data-flow edge to imply it.
    ///
    /// See [`Batch::query_after`] for the rationale.
    pub fn op_after(
        &mut self,
        alias: impl Into<String>,
        op: impl IntoBatchOp,
        after: &[&Handle],
    ) -> Handle {
        self.add_entry_after(alias, op.into_batch_op(), true, after)
    }

    /// Fallible mirror of [`Batch::op`]: build `op` via [`TryIntoBatchOp`] and
    /// add it, returning a typed [`BuilderError`] when a required builder field
    /// was never set instead of panicking. This is the only fallible entry
    /// method for [`crate::ddl::AlterSubscriptionBuilder`] (which has no
    /// dedicated non-try method, mirroring how the panicking path uses plain
    /// `.op(...)` for it today).
    pub fn try_op(
        &mut self,
        alias: impl Into<String>,
        op: impl TryIntoBatchOp,
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry(alias, op.try_into_batch_op()?, true))
    }

    /// Fallible mirror of [`Batch::op_silent`]: build `op` via [`TryIntoBatchOp`]
    /// and add it silently, returning a typed [`BuilderError`] when a required
    /// builder field was never set instead of panicking.
    pub fn try_op_silent(
        &mut self,
        alias: impl Into<String>,
        op: impl TryIntoBatchOp,
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry(alias, op.try_into_batch_op()?, false))
    }

    /// Fallible mirror of [`Batch::op_after`]: build `op` via
    /// [`TryIntoBatchOp`] and add it with explicit ordering dependencies.
    pub fn try_op_after(
        &mut self,
        alias: impl Into<String>,
        op: impl TryIntoBatchOp,
        after: &[&Handle],
    ) -> Result<Handle, BuilderError> {
        Ok(self.add_entry_after(alias, op.try_into_batch_op()?, true, after))
    }
}
