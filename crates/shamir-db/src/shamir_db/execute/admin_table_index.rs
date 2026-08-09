//! Admin handlers: CreateTable, DropTable, CreateIndex, DropIndex.

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::{DdlOpKind, DdlOpState, DdlOpStatus, QueryResult};
use crate::shamir_db::shamir_db::schema_management::SCHEMA_FIELD;
use crate::types::value::QueryValue;
use shamir_engine::table::ddl_op_log;
use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, admin_result_with_op_id, apply_table_retention};

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_table(
        &self,
        op: &crate::query::admin::CreateTableOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Auth runs BEFORE the if_not_exists existence probe (#995): an
        // unauthorized caller must not be able to learn whether a table they
        // have no Create right on already exists, by toggling if_not_exists
        // (or relying on the duplicate-error path) and observing the
        // distinguishable outcomes (silent {"existed": true} no-op / "exists"
        // error vs access_denied). This is a pre-auth existence oracle
        // otherwise. Mirrors #989's fix for handle_drop_index/handle_rename_index.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.repo.clone()),
                Action::Create,
            )
            .await
            .map_err(err_access)?;

        // Check existence for if_not_exists / duplicate guard.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            if db.has_table(&op.repo, &op.create_table) {
                if op.if_not_exists {
                    return Ok(admin_result(mpack!({
                        "created_table": @(QueryValue::Str(op.create_table.clone())),
                        "repo": @(QueryValue::Str(op.repo.clone())),
                        "created": false,
                        "existed": true,
                    })));
                }
                return Err(err_code(
                    "exists",
                    format!(
                        "Table '{}' already exists in repository '{}'",
                        op.create_table, op.repo
                    ),
                ));
            }
        }
        // Route through ShamirDb so the table is persisted to the
        // catalogue and survives a restart (I.2).
        self.shamir
            .add_table_as(
                &self.db_name,
                &op.repo,
                &op.create_table,
                false,
                self.actor.clone(),
            )
            .await
            .map_err(|e| err(e.to_string()))?;

        // T3: apply per-table history retention at creation time.
        if let Some(ref dto) = op.retention {
            dto.validate().map_err(err)?;
            apply_table_retention(
                &self.shamir,
                &self.db_name,
                &op.repo,
                &op.create_table,
                crate::engine::repo::to_mvcc_retention(dto),
            )
            .await?;
        }

        Ok(admin_result(mpack!({
            "created_table": @(QueryValue::Str(op.create_table.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "created": true,
            "existed": false,
        })))
    }

    pub(super) async fn handle_drop_table(
        &self,
        op: &crate::query::admin::DropTableOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Auth runs BEFORE the if_exists existence probe (#995): an
        // unauthorized caller must not be able to learn whether a table they
        // have no Delete right on exists, by toggling if_exists and observing
        // the distinguishable outcomes (silent {"existed": false} no-op vs
        // access_denied). This is a pre-auth existence oracle otherwise.
        // Mirrors #989's fix for handle_drop_index/handle_rename_index.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.drop_table.clone()),
                Action::Delete,
            )
            .await
            .map_err(err_access)?;

        // if_exists early-exit: missing db or missing table → no-op.
        if op.if_exists {
            let exists = self
                .shamir
                .get_db(&self.db_name)
                .is_some_and(|db| db.has_table(&op.repo, &op.drop_table));
            if !exists {
                return Ok(admin_result(mpack!({
                    "dropped_table": @(QueryValue::Str(op.drop_table.clone())),
                    "existed": false,
                })));
            }
        }

        // Phase D.3 — reverse-FK drop guard.
        //
        // Refuse to drop a table that is still referenced by another table's
        // foreign key (any action — Restrict, Cascade, SetNull, NoAction).
        // Dropping a referenced parent would orphan the child FK and leave
        // dangling references.
        //
        // Discovery reads the PERSISTED catalogue schema from the system-store
        // (not the in-memory `TableManager` binding cache). The admin path's
        // `DbInstance` and the engine execute-path instance keep independent
        // in-memory buffers, so a validator binding just written through the
        // execute-path compile step is not reliably visible on the admin
        // handle. The catalogue is the coherent source of truth — every
        // `set_table_schema` commits the FK there before this guard runs.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            let table_names = db.list_tables(&op.repo).unwrap_or_default();
            for name in &table_names {
                if name == &op.drop_table {
                    continue;
                }
                let rec = match self
                    .shamir
                    .system_store()
                    .load_table_record(&self.db_name, &op.repo, name)
                    .await
                {
                    Ok(Some(r)) => r,
                    _ => continue,
                };
                let rules = match rec.get(SCHEMA_FIELD) {
                    Some(QueryValue::List(rules)) => rules,
                    _ => continue,
                };
                for rule in rules {
                    let refs_drop = rule
                        .get("foreign_key")
                        .and_then(|fk| fk.get("ref_table"))
                        .and_then(|v| v.as_str())
                        .is_some_and(|rt| rt == op.drop_table);
                    if refs_drop {
                        return Err(err_code(
                            "drop_refused_fk",
                            format!(
                                "cannot drop table '{}': still referenced by a foreign key in '{}'",
                                op.drop_table, name
                            ),
                        ));
                    }
                }
            }
        }

        // cascade: explicitly drop the table's own indexes (regular,
        // unique, sorted, index2) before removing validators and the
        // table itself.  Without cascade, indexes are orphaned in
        // storage (harmless — the catalogue entry is gone so they
        // will never be loaded again).
        if op.cascade {
            if let Some(db) = self.shamir.get_db(&self.db_name) {
                if let Ok(table) = db.get_table(&op.repo, &op.drop_table).await {
                    // F-3 (#1030): hold `ddl_admission` (via `begin_write_barrier`)
                    // for this WHOLE block of direct index-registry mutations.
                    // Pre-fix, these manager-level primitives
                    // (`index_manager_ref().drop_index`/`drop_unique_index`,
                    // `sorted_indexes().drop_index`, `index2_registry().remove_by_id`)
                    // bypassed the barrier `TableManager`'s own
                    // `drop_index`/`drop_unique_index`/`drop_sorted_index`/
                    // `drop_index2` wrappers take — violating the precondition
                    // `IndexRegistry::insert`'s doc comment states every
                    // registry-mutating DDL op must uphold (see
                    // `crates/shamir-index/src/registry.rs`, "# Precondition"):
                    // a concurrent `CREATE INDEX` could race this cascade's
                    // direct drops to the same next generation-counter value,
                    // un-serialized. One `begin_write_barrier` acquisition
                    // covering the entire block (not four separate ones, which
                    // would only serialize against itself) restores that
                    // guarantee. Bit choice: `REGULAR_INDEX_CREATE` — the same
                    // single `ddl_admission` mutex backs every bit
                    // (`begin_write_barrier`'s Step 0), so any one existing bit
                    // is sufficient for the admission serialization this fix
                    // needs; no new bit is minted since the table is about to
                    // be removed entirely regardless of which writers see the
                    // slow path via this specific bit. The raw manager
                    // primitives called below do NOT themselves acquire
                    // `ddl_admission`/`unique_write_lock` (confirmed by reading
                    // `TableManager::drop_index`/`drop_unique_index`/
                    // `drop_sorted_index`/`drop_index2`, which call these exact
                    // primitives AFTER already taking the barrier) — so
                    // acquiring it here once cannot re-enter the non-reentrant
                    // `tokio::sync::Mutex`.
                    let (_barrier, _uwl_guard) = table
                        .begin_write_barrier(
                            shamir_engine::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                        )
                        .await;
                    // base_index regular indexes.
                    let regular_ids: Vec<u64> = table
                        .index_manager_ref()
                        .iter_indexes()
                        .map(|d| d.name_interned)
                        .collect();
                    for id in regular_ids {
                        let _ = table.index_manager_ref().drop_index(id).await;
                    }
                    // base_index unique indexes.
                    let unique_ids: Vec<u64> = table
                        .index_manager_ref()
                        .iter_unique_indexes()
                        .map(|d| d.name_interned)
                        .collect();
                    for id in unique_ids {
                        let _ = table.index_manager_ref().drop_unique_index(id).await;
                    }
                    // Sorted indexes.
                    let sorted_ids: Vec<u64> = table
                        .sorted_indexes()
                        .iter_indexes()
                        .iter()
                        .map(|d| d.name_interned)
                        .collect();
                    for id in sorted_ids {
                        let _ = table.sorted_indexes().drop_index(id).await;
                    }
                    // index2 registry — remove all backends.
                    let backends = table.index2_registry().all_backends().await;
                    for b in &backends {
                        let _ = table
                            .index2_registry()
                            .remove_by_id(b.descriptor().id)
                            .await;
                    }
                }
            }
        }

        let removed = self
            .shamir
            .drop_table_cleaning_validators(&self.db_name, &op.repo, &op.drop_table)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "dropped_table": @(QueryValue::Str(op.drop_table.clone())),
            "existed": @(QueryValue::Bool(removed)),
        })))
    }

    pub(super) async fn handle_rename_table(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameTable(op) = batch_op else {
            unreachable!("handle_rename_table called with non-RenameTable op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Auth: Write on the source table (rename mutates the table's
        // identity). Mirrors the function/validator rename auth path.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.rename_table.clone(),
                ),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        self.shamir
            .rename_table_as(
                &self.db_name,
                &op.repo,
                &op.rename_table,
                &op.to,
                self.actor.clone(),
            )
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "renamed_table": @(QueryValue::Str(op.rename_table.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
        })))
    }

    pub(super) async fn handle_create_index(
        &self,
        op: &crate::query::admin::CreateIndexOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.table)
            .await
            .map_err(|e| err(e.to_string()))?;
        // Inject the per-DB scalar resolver so create_index_v2 can validate
        // user-registered trusted_pure scalars for functional indexes.
        table
            .set_scalar_resolver(shamir_funclib::scalar_resolver::ScalarResolver::new(
                std::sync::Arc::clone(db.scalars()),
            ))
            .await;

        // Check if the index already exists (for if_not_exists / dup guard).
        //
        // Checked across ALL FOUR index families (regular, unique, sorted,
        // index2) via the shared cross-family helper — not just the
        // base_index family `op.unique` selects. Before F-4 (#1029) this
        // probed only `unique_index_exists`/`index_exists`, so sorted/fts/
        // vector/functional CREATE never observed an existing SAME-family
        // duplicate here; control fell through into
        // `create_sorted_index_with_include`/`create_index_v2`, where R0-C's
        // own cross-family preflight (`any_index_exists`) unconditionally
        // errors regardless of `if_not_exists`. Using the same helper here
        // makes `IF NOT EXISTS` a correct no-op for every family.
        let already_exists = table.any_index_exists(&op.create_index).await;
        if already_exists {
            if op.if_not_exists {
                return Ok(admin_result(mpack!({
                    "created_index": @(QueryValue::Str(op.create_index.clone())),
                    "table": @(QueryValue::Str(op.table.clone())),
                    "created": false,
                    "existed": true,
                })));
            }
            return Err(err_code(
                "exists",
                format!(
                    "Index '{}' already exists on table '{}'",
                    op.create_index, op.table
                ),
            ));
        }

        let field_strs: Vec<Vec<&str>> = op
            .fields
            .iter()
            .map(|f| f.iter().map(|s| s.as_str()).collect())
            .collect();
        // For single-segment paths, join as dot-separated for create_index API
        let paths: Vec<String> = field_strs
            .iter()
            .map(|segments| segments.join("."))
            .collect();
        let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

        // P1-6 (#970): cross-type validation that runs for EVERY index_type.
        // The original 3 checks below (sorted&&unique, include&&!sorted,
        // sorted+multi-field) only run in the btree branch — i.e. AFTER the
        // non-btree early return below — so a non-btree index_type silently
        // ignored `.unique()`, `.sorted()`, and cross-family options (e.g.
        // `.unique().index_type("vector")` silently created a non-unique
        // vector index). These new checks close that gap.
        //
        // The exact wording is mirrored in
        // `CreateIndexBuildError::Display` (Rust try_build) and the TS
        // `createIndex()` builder so a caller sees the same explanation
        // everywhere.
        let itype = op.index_type.as_deref();
        let non_btree = matches!(itype, Some("vector") | Some("fts") | Some("functional"));

        // 1. At least one field for ANY index type.
        if op.fields.is_empty() {
            return Err(err("CREATE INDEX requires at least one field".to_string()));
        }
        // 2. `unique` is only meaningful for btree/hash indexes.
        if op.unique && non_btree {
            return Err(err(format!(
                "`unique` is not supported for '{}' indexes; only btree/hash indexes can be unique",
                itype.unwrap()
            )));
        }
        // 3. `sorted` is only meaningful for btree indexes.
        if op.sorted && non_btree {
            return Err(err(format!(
                "`sorted` is not supported for '{}' indexes",
                itype.unwrap()
            )));
        }
        // 4. Vector index requires a positive dimension.
        if itype == Some("vector") && (op.vector_dim.is_none() || op.vector_dim == Some(0)) {
            return Err(err("vector index requires `vector_dim` > 0".to_string()));
        }
        // 5. Vector metric must be a recognized value.
        if itype == Some("vector") {
            if let Some(m) = op.vector_metric.as_deref() {
                if !matches!(m, "l2" | "dot" | "cosine") {
                    return Err(err(format!(
                        "unknown vector_metric '{}'; expected 'l2', 'dot', or 'cosine'",
                        m
                    )));
                }
            }
        }
        // 6. Vector-specific options are only valid for vector indexes.
        if itype != Some("vector")
            && (op.vector_dim.is_some()
                || op.vector_metric.is_some()
                || op.vector_quantization.is_some())
        {
            return Err(err(
                "vector_dim/vector_metric/vector_quantization are only valid for 'vector' indexes"
                    .to_string(),
            ));
        }
        // 7. FTS-specific options are only valid for FTS indexes.
        if itype != Some("fts") && (op.fts_tokenizer.is_some() || op.fts_language.is_some()) {
            return Err(err(
                "fts_tokenizer/fts_language are only valid for 'fts' indexes".to_string(),
            ));
        }
        // 8. Functional-specific options are only valid for functional indexes.
        if itype != Some("functional")
            && (op.functional_op.is_some() || op.functional_args.is_some())
        {
            return Err(err(
                "functional_op/functional_args are only valid for 'functional' indexes".to_string(),
            ));
        }
        // 9. `include` (covering index) is only meaningful for sorted btree indexes.
        if !op.include.is_empty() && non_btree {
            return Err(err(format!(
                "`include` is not supported for '{}' indexes; covering fields are \
                 only valid for sorted indexes",
                itype.unwrap()
            )));
        }

        if op.index_type.as_deref().is_some_and(|t| t != "btree") {
            table
                .create_index_v2(op)
                .await
                .map_err(|e| err(e.to_string()))?;
            return Ok(admin_result(mpack!({
                "created_index": @(QueryValue::Str(op.create_index.clone())),
                "table": @(QueryValue::Str(op.table.clone())),
                "index_type": @(op.index_type.as_deref().map(|t| QueryValue::Str(t.to_string())).unwrap_or(QueryValue::Null)),
            })));
        }

        if op.sorted && op.unique {
            return Err(err("Index cannot be both sorted and unique".to_string()));
        }
        if !op.include.is_empty() && !op.sorted {
            return Err(err("include is only valid for sorted indexes".to_string()));
        }
        if op.sorted {
            if op.fields.len() != 1 {
                return Err(err(
                    "Sorted index requires exactly one field (composite TBD)".to_string(),
                ));
            }
            table
                .create_sorted_index_with_include(&op.create_index, &path_refs, op.include.clone())
                .await
                .map_err(|e| err(e.to_string()))?;
        } else if op.unique {
            table
                .create_unique_index(&op.create_index, &path_refs)
                .await
                .map_err(|e| err(e.to_string()))?;
        } else {
            table
                .create_index(&op.create_index, &path_refs)
                .await
                .map_err(|e| err(e.to_string()))?;
        }

        Ok(admin_result(mpack!({
            "created_index": @(QueryValue::Str(op.create_index.clone())),
            "table": @(QueryValue::Str(op.table.clone())),
            "unique": @(QueryValue::Bool(op.unique)),
            "sorted": @(QueryValue::Bool(op.sorted)),
        })))
    }

    pub(super) async fn handle_drop_index(
        &self,
        op: &crate::query::admin::DropIndexOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Auth runs BEFORE the if_exists existence probe: an unauthorized
        // caller must not be able to learn whether an index/table/db they have
        // no rights to query exists, by toggling if_exists and observing the
        // distinguishable outcomes (silent {"existed": false} no-op vs
        // access_denied). This is a pre-auth existence oracle otherwise.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // if_exists early-exit: missing db, table, or index → no-op.
        //
        // Existence is checked across ALL FOUR index mechanisms (base_index
        // regular, base_index unique, sorted, index2) — `DROP INDEX <name>` has
        // no `index_type` hint on the wire (see `DropIndexOp`), so the name
        // alone must be resolved. Before this, an index2 / sorted index of
        // the same name would be reported as "does not exist" and silently
        // no-op'd even though it does.
        if op.if_exists {
            let db_opt = self.shamir.get_db(&self.db_name);
            let table_opt = match &db_opt {
                Some(db) => db.get_table(&op.repo, &op.table).await.ok(),
                None => None,
            };
            let index_exists = match &table_opt {
                Some(table) => {
                    table.index_exists(&op.drop_index).await
                        || table.unique_index_exists(&op.drop_index).await
                        || table.sorted_index_exists(&op.drop_index).await
                        || table.index2_exists(&op.drop_index).await
                }
                None => false,
            };
            if !index_exists {
                return Ok(admin_result(mpack!({
                    "dropped_index": @(QueryValue::Str(op.drop_index.clone())),
                    "existed": false,
                })));
            }
        }

        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.table)
            .await
            .map_err(|e| err(e.to_string()))?;

        // R0-C (#1010, #1025): refuse instead of silently resolving when
        // `op.drop_index` is a PRE-EXISTING cross-family collision. Index names
        // are now globally unique per table across all four families (regular
        // hash, unique hash, sorted, index2), so a name can exist in at most one
        // family. This guard detects and refuses legacy collisions (only
        // reachable on tables that acquired them before #1010's CREATE-time
        // preflight landed). Mirrors TableManager::rename_index's exact
        // classification pattern.
        let is_regular = table.index_exists(&op.drop_index).await;
        let is_unique = table.unique_index_exists(&op.drop_index).await;
        let is_sorted = table.sorted_index_exists(&op.drop_index).await;
        let is_index2 = table.index2_exists(&op.drop_index).await;
        let matching_families = [is_regular, is_unique, is_sorted, is_index2]
            .iter()
            .filter(|&&m| m)
            .count();
        if matching_families > 1 {
            return Err(err_code(
                "cross_family_collision",
                format!(
                    "index '{}' exists in {matching_families} different index families \
                     on table '{}' (a pre-existing cross-family name collision) — DROP \
                     INDEX cannot safely resolve which one to drop. Run \
                     TableManager::verify() to see the affected families, then drop the \
                     colliding sibling(s) individually.",
                    op.drop_index, op.table
                ),
            ));
        }

        // Dispatch to the ONE matching family's drop call — the server now
        // resolves the index family from the catalog, not from the client's
        // `op.unique` hint. This matches TableManager::rename_index's pattern.
        // Safe to short-circuit: the guard above already refused if MORE THAN
        // ONE family matched, so at most one of these can be true.
        let removed = if is_regular {
            table
                .drop_index(&op.drop_index)
                .await
                .map_err(|e| err(e.to_string()))?
        } else if is_unique {
            table
                .drop_unique_index(&op.drop_index)
                .await
                .map_err(|e| err(e.to_string()))?
        } else if is_sorted {
            table
                .drop_sorted_index(&op.drop_index)
                .await
                .map_err(|e| err(e.to_string()))?
        } else if is_index2 {
            table
                .drop_index2(&op.drop_index)
                .await
                .map_err(|e| err(e.to_string()))?
        } else {
            // No index exists in any family — return existed:false (no-op).
            // This matches the pre-existing behavior when the client's unique
            // hint didn't match any index.
            false
        };

        // Mint an op_id for recoverable DDL ops (hash DROP and index2 DROP are in scope)
        let op_id = RecordId::system(&format!("ddl_drop_index_{}", op.drop_index));

        // Write the Succeeded status to the DDL op log for polling.
        // Use the ACTUAL resolved family (from the catalog), not the client's
        // `op.unique` hint, so the log reflects the truth even when a caller's
        // hint was wrong. Only write when we actually dropped an index.
        // Sorted-family drops have no dedicated `DdlOpKind` variant yet (never
        // wired to op-id status logging, same pre-existing scope gap as
        // before #1025) — falls back to `DropHashIndex` like the rest of this
        // status-log mechanism does for untracked families.
        if removed {
            let kind = if is_unique {
                DdlOpKind::DropUniqueHashIndex {
                    index_name: op.drop_index.clone(),
                }
            } else if is_index2 {
                DdlOpKind::DropIndex2 {
                    index_name: op.drop_index.clone(),
                }
            } else {
                // is_regular or is_sorted
                DdlOpKind::DropHashIndex {
                    index_name: op.drop_index.clone(),
                }
            };
            let status = DdlOpStatus {
                op_id,
                kind,
                state: DdlOpState::Succeeded {
                    completed_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                },
            };
            if let Err(e) = ddl_op_log::write_op_status(table.info_store(), &status).await {
                // Log write failure but don't fail the operation — the client still
                // got the op_id in the QueryResult, and polling will return Unknown.
                eprintln!("Failed to write DDL op status: {}", e);
            }
        }

        Ok(admin_result_with_op_id(
            mpack!({
                "dropped_index": @(QueryValue::Str(op.drop_index.clone())),
                "existed": @(QueryValue::Bool(removed)),
            }),
            op_id,
        ))
    }

    pub(super) async fn handle_rename_index(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameIndex(op) = batch_op else {
            unreachable!("handle_rename_index called with non-RenameIndex op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Auth runs BEFORE the if_exists existence probe: an unauthorized
        // caller must not be able to learn whether an index/table/db they have
        // no rights to query exists, by toggling if_exists and observing the
        // distinguishable outcomes (silent {"existed": false} no-op vs
        // access_denied). This is a pre-auth existence oracle otherwise.
        // Write on the parent table (rename mutates the index's identity).
        // Mirrors the index create/drop auth path.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // if_exists early-exit: missing db, table, or source index → no-op.
        //
        // Mirrors handle_drop_index's early-exit guard.
        // Existence is checked across ALL FOUR index mechanisms (base_index
        // regular, base_index unique, sorted, index2) because RenameIndexOp
        // carries no index_type hint — the source name alone must be
        // resolved before deciding to short-circuit.
        if op.if_exists {
            let db_opt = self.shamir.get_db(&self.db_name);
            let table_opt = match &db_opt {
                Some(db) => db.get_table(&op.repo, &op.table).await.ok(),
                None => None,
            };
            let index_exists = match &table_opt {
                Some(table) => {
                    table.index_exists(&op.rename_index).await
                        || table.unique_index_exists(&op.rename_index).await
                        || table.sorted_index_exists(&op.rename_index).await
                        || table.index2_exists(&op.rename_index).await
                }
                None => false,
            };
            if !index_exists {
                return Ok(admin_result(mpack!({
                    "renamed_index": @(QueryValue::Str(op.rename_index.clone())),
                    "existed": false,
                })));
            }
        }

        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.table)
            .await
            .map_err(|e| err(e.to_string()))?;

        // Mint an op_id for recoverable DDL ops (hash RENAME is in scope)
        let op_id = RecordId::system(&format!(
            "ddl_rename_index_{}_to_{}",
            op.rename_index, op.to
        ));

        table
            .rename_index(&op.rename_index, &op.to, Some(op_id))
            .await
            .map_err(|e| err_code("rename_index_failed", e.to_string()))?;

        // Write the Succeeded status to the DDL op log for polling.
        // Determine if this is a unique index by checking which rename succeeded.
        // (We already know the source exists; the rename call would have failed otherwise.)
        let is_unique = table.unique_index_exists(&op.to).await;
        let kind = if is_unique {
            DdlOpKind::RenameUniqueHashIndex {
                old_name: op.rename_index.clone(),
                new_name: op.to.clone(),
            }
        } else {
            DdlOpKind::RenameHashIndex {
                old_name: op.rename_index.clone(),
                new_name: op.to.clone(),
            }
        };
        let status = DdlOpStatus {
            op_id,
            kind,
            state: DdlOpState::Succeeded {
                completed_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            },
        };
        if let Err(e) = ddl_op_log::write_op_status(table.info_store(), &status).await {
            // Log write failure but don't fail the operation — the client still
            // got the op_id in the QueryResult, and polling will return Unknown.
            eprintln!("Failed to write DDL op status: {}", e);
        }

        Ok(admin_result_with_op_id(
            mpack!({
                "renamed_index": @(QueryValue::Str(op.rename_index.clone())),
                "to": @(QueryValue::Str(op.to.clone())),
                "table": @(QueryValue::Str(op.table.clone())),
                "repo": @(QueryValue::Str(op.repo.clone())),
                "existed": @(QueryValue::Bool(true)),
            }),
            op_id,
        ))
    }
}
