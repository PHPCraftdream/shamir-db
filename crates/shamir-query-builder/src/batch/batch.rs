use shamir_collections::{new_map, TMap};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, SubBatchOp,
};
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::ReadQuery;
use shamir_query_types::subscribe::{SubscribeOp, UnsubscribeOp};
use shamir_types::types::value::QueryValue;

use super::build_error::BuildError;
use super::durability::Durability;
use super::handle::Handle;
use super::into_batch_op::IntoBatchOp;
use super::isolation::Isolation;

/// Fluent batch assembler.
///
/// Accumulates query/write entries under string aliases and produces a
/// [`BatchRequest`].
#[derive(Debug, Clone)]
pub struct Batch {
    id: QueryValue,
    name: Option<String>,
    transactional: bool,
    isolation: Option<String>,
    durability: Option<String>,
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
            durability: None,
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
        self.isolation = Some(iso.as_str().to_owned());
        self
    }

    /// Set the durability level.
    pub fn durability(&mut self, d: Durability) -> &mut Self {
        self.durability = Some(d.as_str().to_owned());
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

    /// Add an insert operation.
    pub fn insert(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an update operation.
    pub fn update(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add an upsert (set) operation.
    pub fn upsert(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add a delete operation.
    pub fn delete(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
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

    /// Create a role.
    pub fn create_role(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Drop a role.
    pub fn drop_role(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Rename a role.
    pub fn rename_role(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
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

    /// List roles.
    pub fn list_roles(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
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

    // ── wire encoding (build + encode in one step) ─────────────────

    /// Build and encode as msgpack (named fields) — primary wire format.
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec_named(&self.build())
    }

    /// Build, encode to msgpack, and decode back into a `BatchRequest`.
    ///
    /// Round-trips the request through the wire codec (named msgpack) — the
    /// same path a real client/server uses — so callers (notably tests)
    /// exercise the builder AND the codec in one step. Panics on a codec
    /// error (the builder always produces a serialisable request).
    pub fn to_request_via_msgpack(&self) -> BatchRequest {
        let bytes = self.to_msgpack().expect("msgpack encode");
        rmp_serde::from_slice(&bytes).expect("msgpack decode")
    }

    // ── build ──────────────────────────────────────────────────────

    /// Infallible build — clones accumulated state into a [`BatchRequest`].
    pub fn build(&self) -> BatchRequest {
        BatchRequest {
            id: self.id.clone(),
            name: self.name.clone(),
            transactional: self.transactional,
            isolation: self.isolation.clone(),
            durability: self.durability.clone(),
            queries: self.queries.clone(),
            return_all: self.return_all,
            return_only: self.return_only.clone(),
            limits: self.limits.clone(),
            interner_epochs: self.interner_epochs.clone(),
            result_encoding: self.result_encoding,
        }
    }

    /// Build with client-side validation.
    ///
    /// Serializes each entry's op to a [`QueryValue`] tree via msgpack, walks
    /// for `"$query"` map keys, normalizes the base alias (strip `@`, cut at
    /// `[`/`.`), and checks:
    /// - the base alias exists as a key in `queries`
    /// - the base alias is not the referencing entry's own alias
    ///
    /// Also validates `after` entries: each must reference a known alias
    /// and must not reference the entry's own alias.
    pub fn try_build(&self) -> Result<BatchRequest, BuildError> {
        for (alias, entry) in &self.queries {
            // Validate $query refs.
            //
            // Encode the op to msgpack and decode as QueryValue so we can
            // walk the tree as a plain QueryValue map.
            let bytes = rmp_serde::to_vec_named(&entry.op)
                .expect("BatchOp msgpack serialization is infallible");
            let qv: shamir_types::types::value::QueryValue =
                rmp_serde::from_slice(&bytes).expect("BatchOp→QueryValue round-trip is infallible");
            let mut refs = Vec::new();
            collect_query_refs(&qv, &mut refs);
            for raw_ref in &refs {
                let base = extract_base_alias(raw_ref);
                if base == *alias {
                    return Err(BuildError::SelfReference {
                        alias: alias.clone(),
                    });
                }
                if !self.queries.contains_key(&base) {
                    return Err(BuildError::UnknownAlias {
                        alias: base,
                        referenced_by: alias.clone(),
                    });
                }
            }

            // Validate `after` refs.
            for raw in &entry.after {
                let base = extract_base_alias(raw);
                if base == *alias {
                    return Err(BuildError::SelfReference {
                        alias: alias.clone(),
                    });
                }
                if !self.queries.contains_key(&base) {
                    return Err(BuildError::UnknownAlias {
                        alias: base,
                        referenced_by: alias.clone(),
                    });
                }
            }
        }
        Ok(self.build())
    }

    // ── internal ───────────────────────────────────────────────────

    /// Declare that `dependent` must execute AFTER `on` (ordering edge).
    /// Use for DDL→DML ordering, e.g. an insert after a create_table.
    pub fn after(&mut self, dependent: &Handle, on: &Handle) -> &mut Self {
        if let Some(entry) = self.queries.get_mut(dependent.alias()) {
            entry.after.push(on.alias().to_string());
        }
        self
    }

    fn add_entry(&mut self, alias: impl Into<String>, op: BatchOp, return_result: bool) -> Handle {
        let alias = alias.into();
        self.queries.insert(
            alias.clone(),
            QueryEntry {
                op,
                return_result,
                after: Vec::new(),
            },
        );
        Handle { alias }
    }
}

// ============================================================================
// $query ref walking helpers (mirrors planner.rs logic)
// ============================================================================

/// Collect all `$query` string values from a `QueryValue` tree.
///
/// The `FilterValue::QueryRef` variant serializes as `{"$query": "@alias", ...}`.
/// Walking the `QueryValue::Map` for the `"$query"` key mirrors the planner's
/// logic directly on the msgpack-decoded tree.
fn collect_query_refs(value: &shamir_types::types::value::QueryValue, out: &mut Vec<String>) {
    use shamir_types::types::value::QueryValue;
    match value {
        QueryValue::Map(map) => {
            if let Some(QueryValue::Str(s)) = map.get("$query") {
                out.push(s.clone());
            }
            for v in map.values() {
                collect_query_refs(v, out);
            }
        }
        QueryValue::List(arr) => {
            for v in arr {
                collect_query_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Strip leading `@` and cut at the first `[` or `.`.
fn extract_base_alias(s: &str) -> String {
    let s = s.strip_prefix('@').unwrap_or(s);
    s.find(['[', '.'])
        .map(|pos| s[..pos].to_string())
        .unwrap_or_else(|| s.to_string())
}
