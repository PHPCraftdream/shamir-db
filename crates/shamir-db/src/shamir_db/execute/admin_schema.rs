//! Admin handlers: SetTableSchema, AddSchemaRule, RemoveSchemaRule, GetTableSchema.
//!
//! All mutating schema ops are gated by `Action::Write` on the table
//! resource (same as ALTER TABLE). `GetTableSchema` is gated by
//! `Action::Read` (introspection).
//!
//! # Catalogue contract
//!
//! The schema is persisted into the table's catalogue record (the row in
//! the `__tables` system table keyed by `(db_name, repo_name, table_name)`)
//! under three fields:
//!
//! - `schema` — `List[Map]` of rule entries; each map has
//!   `"path": List[Int]` (interned ids), `"type": Str`, plus optional
//!   constraint fields (`required`, `nullable`, `unsigned`, `min`, `max`,
//!   `len`, `max_len`, `min_len`, `one_of`, `array_of`, `scalar`, `format`,
//!   `compare`). This is the exact shape [`parse_schema`] reads back.
//! - `schema_validator_id` — `Str` form of the [`RecordId`] of the
//!   compiled `SchemaValidator`. Reused across ALTERs so the validator
//!   registry entry is replaced in place (RCU) rather than churned.
//! - `schema_version` — `Int` monotonic counter, bumped on every mutation.
//!   Used for optimistic-concurrency checks (`expected_version`).
//!
//! Each mutating op runs under a per-table RMW lock (keyed in
//! `admin_user_locks` under `"schema:{db}/{repo}/{table}"`) so two
//! concurrent `SetTableSchema` calls cannot lose an update.
//!
//! [`parse_schema`]: shamir_db::shamir_db::schema_management::parse_schema

use crate::access::{Action, ResourcePath};
use crate::engine::table::TableManager;
use crate::query::admin::{
    AddSchemaRuleOp, CompareDto, ConstraintsDto, FieldRuleDto, FkAction, ForeignKeyDto,
    GetTableSchemaOp, NumDto, RemoveSchemaRuleOp, SetTableSchemaOp,
};
use crate::query::batch::BatchError;
use crate::query::filter::{
    filter_value_to_query_value as fv_to_qv_literal,
    query_value_to_filter_value as qv_to_fv_literal, FilterValue,
};
use crate::query::read::QueryResult;
use crate::shamir_db::shamir_db::schema_management::{
    parse_schema, SCHEMA_FIELD, SCHEMA_VALIDATOR_ID_FIELD, SCHEMA_VERSION_FIELD,
};
use crate::shamir_db::ShamirDb;
use crate::types::common::{new_map, TMap};
use crate::types::value::QueryValue;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

// ── error helpers ──────────────────────────────────────────────────────────

fn err_code(code: &str, msg: impl Into<String>) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: msg.into(),
        code: Some(code.to_string()),
    }
}

fn err_access(e: shamir_types::access::AccessError) -> BatchError {
    err_code("access_denied", e.to_string())
}

// ── per-table RMW lock ─────────────────────────────────────────────────────

/// Build the lock key for a per-table schema RMW op.
///
/// The lock lives in `ShamirDb::admin_user_locks` (a `DashMap<String,
/// Arc<Mutex<()>>>`); we reuse that map with a schema-specific key prefix
/// so schema mutations serialise against each other without colliding with
/// user-lifecycle locks (which key on the bare username).
fn schema_lock_key(db: &str, repo: &str, table: &str) -> String {
    format!("schema:{db}/{repo}/{table}")
}

/// Acquire the per-table schema RMW lock using the canonical pattern from
/// `admin_users_roles.rs`: clone the Arc out of the DashMap, then lock it.
/// The returned [`OwnedMutexGuard`] owns its reference to the Mutex via the
/// cloned Arc, so it is independent of the DashMap entry's lifetime.
async fn lock_schema_rmw(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    table: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let key = schema_lock_key(db, repo, table);
    let arc = shamir
        .admin_user_locks()
        .entry(key)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    arc.lock_owned().await
}

// ── F-37 (#845): schema-activation write barrier ───────────────────────────
//
// `lock_schema_rmw` above only serializes schema DDL against OTHER schema DDL
// (a separate `admin_user_locks` map). It does NOT intersect with the actual
// write path (`TableManager::insert`/`set`/etc in `table_manager_crud.rs`),
// which knows nothing about `admin_user_locks`. So a concurrent writer could
// land between `stamp_keyset_safe`'s `count() == 0` read and the schema's
// persist+activate, writing a row under the about-to-be-activated rule before
// `keyset_safe = true` is stamped — undermining the cursor gate that trusts it.
//
// The fix mirrors the existing unique-index DDL write-barrier pattern exactly
// (see `TableManager::index2_create_barrier` + `create_index_v2`): hold the
// table's shared `unique_write_lock` across the ENTIRE count→persist→activate
// sequence AND raise a sibling `schema_activation_barrier` flag so every
// writer consulting `needs_write_barrier()` serializes on that same lock.
// `lock_schema_rmw` (DDL-vs-DDL) stays as-is — these are two independent
// serialization axes.
//
// F-48 (#859): the flag + lock alone are a check-then-act, not a drain — a
// writer that read `needs_write_barrier() == false` a moment BEFORE the flag
// went up proceeds lock-free through its whole validate→write→index sequence
// with no further check. `SchemaActivationBarrierGuard::raise` therefore also
// calls `TableManager::drain_writers()` after raising the flag (while still
// holding `unique_write_lock`), genuinely waiting for any such in-flight
// fast-path writer to exit before the count-proof proceeds. See
// `shamir-engine::table::writer_drain_barrier` for the reusable drain
// primitive (F-50 will wire the SAME call into `create_index_v2`).

/// RAII guard that raises a table's `schema_activation_barrier` flag while
/// held and clears it on drop — including every `?` early-return path inside
/// a schema-activation DDL. Mirrors the engine's `Index2CreateBarrierGuard`
/// (`table_manager_index_mgmt.rs`): the flag is `Release`-stored here, pairing
/// with the writer's `Acquire` load in `needs_write_barrier`.
///
/// **Must be created while the caller already holds `unique_write_lock`**
/// (see [`begin_schema_activation_barrier`]) — exactly as the engine's
/// `Index2CreateBarrierGuard` is raised under `_uwl_guard`. The clear-on-drop
/// then runs while the lock is still held (Rust drops locals in reverse
/// declaration order: the barrier guard, declared AFTER the lock guard, drops
/// FIRST), matching `create_index_v2`'s own clear-flag-then-release-lock
/// sequence.
///
/// F-48 (#859): [`raise`](Self::raise) is `async` because it ALSO calls
/// `TableManager::drain_writers()` after the flag store — the genuine drain
/// that catches any fast-path writer that read `false` before the flag went
/// up. The flag + lock alone were a check-then-act, not a drain; the drain
/// closes that gap. Callers MUST `.await` the result.
struct SchemaActivationBarrierGuard<'a> {
    table: &'a TableManager,
}

impl<'a> SchemaActivationBarrierGuard<'a> {
    /// Raise the flag (`Release`) and drain every in-flight fast-path writer
    /// that read `needs_write_barrier() == false` before the flag went up.
    /// The caller MUST already hold `unique_write_lock` (acquired by
    /// [`begin_schema_activation_barrier`]). See the type-level doc for the
    /// F-48 drain rationale.
    async fn raise(table: &'a TableManager) -> Self {
        table.set_schema_activation_barrier(true);
        // F-48 (#859): the genuine drain. After the flag's `Release` store and
        // while the caller holds `unique_write_lock`, wait for any fast-path
        // writer that already read `false` to finish its validate→write→index
        // sequence. This closes the check-then-act gap the flag + lock alone
        // leave open. Cheap (one `Acquire` load) when no writer is in flight.
        table.drain_writers().await;
        Self { table }
    }
}

impl Drop for SchemaActivationBarrierGuard<'_> {
    fn drop(&mut self) {
        self.table.set_schema_activation_barrier(false);
    }
}

/// Begin the schema-activation write barrier for `table`: take the table's
/// shared `unique_write_lock` (the SAME lock the non-tx writer path takes
/// when `needs_write_barrier()` is true) and return the handle + lock guard.
/// The caller then constructs a [`SchemaActivationBarrierGuard`] against the
/// returned handle so all three locals (`_barrier`, `_uwl_guard`, `_handle`)
/// share one scope and drop in the right order (barrier clears first, then
/// the lock releases) — the exact shape of `create_index_v2`'s
/// `_uwl_guard` + `Index2CreateBarrierGuard` pair.
///
/// Call this immediately BEFORE the `stamp_keyset_safe` count-proof read so
/// the subsequent count→persist→activate sequence is a genuine snapshot no
/// concurrent writer can invalidate.
async fn begin_schema_activation_barrier(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    table: &str,
) -> Result<(TableManager, tokio::sync::OwnedMutexGuard<()>), BatchError> {
    let handle = shamir
        .get_table(db, repo, table)
        .await
        .map_err(|e| err_code("internal_error", e.to_string()))?;
    let uwl_guard = handle.unique_write_lock().lock_owned().await;
    Ok((handle, uwl_guard))
}

/// Validate that all unique-constrained fields in the rule set have a
/// backing index on the SAME table.  Returns an error
/// (`unique_requires_index`) if any unique field lacks a single-field
/// index.
///
/// Called at DDL time (set_table_schema, add_schema_rule) — fail-closed so
/// a unique constraint without an index is never persisted (would be O(n)
/// scan per insert).
async fn validate_unique_indexes(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    table: &str,
    rules: &[FieldRuleDto],
) -> Result<(), BatchError> {
    for rule in rules {
        if rule.constraints.unique == Some(true) {
            // The unique field lives on the SAME table being constrained.
            let self_table = match shamir.get_table(db, repo, table).await {
                Ok(t) => t,
                Err(_) => {
                    return Err(err_code(
                        "unique_requires_index",
                        format!("unique on {:?}: table '{}' not found", rule.path, table),
                    ));
                }
            };

            // The field path must be a single segment for a single-field index.
            if rule.path.len() != 1 {
                return Err(err_code(
                    "unique_requires_index",
                    format!(
                        "unique on {:?}: only single-segment field paths are \
                         supported for unique constraints",
                        rule.path
                    ),
                ));
            }

            let field_name = &rule.path[0];
            let interner = self_table
                .interner()
                .get()
                .await
                .map_err(|e| err_code("internal_error", e.to_string()))?;
            let has_index = interner.get_ind(field_name).is_some_and(|field_id| {
                let field_path = [field_id.id()];
                self_table.find_single_field_index(&field_path).is_some()
            });
            if !has_index {
                return Err(err_code(
                    "unique_requires_index",
                    format!(
                        "unique on {:?}: no index on '{}.{}' — \
                         create an index before adding a unique constraint",
                        rule.path, table, field_name
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Validate that no `default` / `auto_now` / `auto_now_add` rule uses a
/// multi-segment path. Returns an error
/// (`nested_path_transform_not_supported`) if any such rule has
/// `path.len() != 1`.
///
/// Mirrors `validate_unique_indexes`'s DDL-time guard pattern: the write path
/// (`apply_defaults` / `apply_transforms` in `write_helpers.rs`) silently skips
/// multi-segment paths (MVP single-segment scope — see the `match path.first()`
/// guards there). Without this DDL-time check a multi-segment
/// `default`/`auto_now`/`auto_now_add` rule would be silently accepted at DDL
/// time and then silently dropped on every insert/update forever — an
/// asymmetry with `validate_unique_indexes`, which already rejects a
/// multi-segment `unique` at DDL time. This function closes that asymmetry by
/// rejecting the rule up-front instead of letting it be silently ignored at
/// write time.
///
/// Called at DDL time (set_table_schema, add_schema_rule) — the same entry
/// points that call `validate_unique_indexes`.
fn validate_nested_path_transforms(rules: &[FieldRuleDto]) -> Result<(), BatchError> {
    for rule in rules {
        let has_transform = rule.constraints.default.is_some()
            || rule.constraints.auto_now
            || rule.constraints.auto_now_add;
        if has_transform && rule.path.len() != 1 {
            return Err(err_code(
                "nested_path_transform_not_supported",
                format!(
                    "default/auto_now/auto_now_add on {:?}: only single-segment field paths \
                     are supported for default and transform rules",
                    rule.path
                ),
            ));
        }
    }
    Ok(())
}

/// Validate that no foreign-key reference declares a self-referential
/// `on_delete: Cascade`. Returns an error
/// (`self_referential_cascade_not_supported`) if any FK has
/// `ref_table == table` (the table being defined) and
/// `on_delete == FkAction::Cascade`.
///
/// The DELETE-path cascade planner (`fk_actions.rs::plan_cascade_recursive`)
/// uses a table-NAME-based per-path cycle guard (`CascadePathGuard`). A
/// self-referential CASCADE would re-enter the same table name on the very
/// first recursion level — which the guard cannot distinguish from a genuine
/// cross-table cycle — and is therefore silently skipped at runtime as
/// defense-in-depth. This DDL-time check converts that silent skip into an
/// explicit, honest error so an operator never declares an action the engine
/// cannot honor.
///
/// Self-referential `SetNull` / `Restrict` are safe (single-level, no
/// recursion) and are NOT rejected here.
///
/// Called at DDL time (set_table_schema, add_schema_rule) — the same entry
/// points that call `validate_unique_indexes` / `validate_nested_path_transforms`.
fn validate_no_self_referential_cascade(
    table: &str,
    rules: &[FieldRuleDto],
) -> Result<(), BatchError> {
    for rule in rules {
        if let Some(fk) = &rule.constraints.foreign_key {
            if fk.ref_table == table && fk.on_delete == FkAction::Cascade {
                return Err(err_code(
                    "self_referential_cascade_not_supported",
                    format!(
                        "foreign_key on {:?}: self-referential ON DELETE CASCADE is not \
                         supported (the cascade cycle-guard is table-name based and cannot \
                         resolve a table cascading into itself); use SET NULL or RESTRICT instead",
                        rule.path
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Validate that all FK references in the rule set have a backing index on
/// the parent table. Returns an error (`fk_requires_index`) if any FK
/// reference lacks a single-field index on `(ref_table, ref_field)`.
///
/// Called at DDL time (set_table_schema, add_schema_rule) — fail-closed so
/// an FK without an index is never persisted.
async fn validate_fk_indexes(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    rules: &[FieldRuleDto],
) -> Result<(), BatchError> {
    for rule in rules {
        if let Some(fk) = &rule.constraints.foreign_key {
            // Resolve the referenced table.
            let ref_table = match shamir.get_table(db, repo, &fk.ref_table).await {
                Ok(t) => t,
                Err(_) => {
                    return Err(err_code(
                        "fk_requires_index",
                        format!(
                            "foreign_key on {:?}: referenced table '{}' not found",
                            rule.path, fk.ref_table
                        ),
                    ));
                }
            };

            // Resolve the ref_field to an interner id and check for a
            // single-field index.
            let interner = ref_table
                .interner()
                .get()
                .await
                .map_err(|e| err_code("internal_error", e.to_string()))?;
            let has_index = interner.get_ind(&fk.ref_field).is_some_and(|field_id| {
                let field_path = [field_id.id()];
                ref_table.find_single_field_index(&field_path).is_some()
            });
            if !has_index {
                return Err(err_code(
                    "fk_requires_index",
                    format!(
                        "foreign_key on {:?}: no index on '{}.{}' — \
                         create an index before adding a foreign_key constraint",
                        rule.path, fk.ref_table, fk.ref_field
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// F-17 (#810) — Stamp the server-computed `keyset_safe` proof onto each
/// rule in `new_rules` right before persisting.
///
/// For each rule:
/// - If a rule with the **same path AND same type tag** existed in
///   `prev_rules`, preserve its previously-recorded `keyset_safe` value
///   (an unchanged rule's proof doesn't expire just because the table is
///   now non-empty — it was proven at its original bind time).
/// - Otherwise (genuinely new path, or type-changed), compute a fresh
///   `table.count().await? == 0` check.  This is O(1) (stored counter),
///   and captures the table state at the exact moment the rule is bound.
///
/// **Client-supplied `keyset_safe` is always ignored** — the server
/// overwrites it unconditionally here.
async fn stamp_keyset_safe(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    table: &str,
    prev_rules: &[FieldRuleDto],
    new_rules: &mut [FieldRuleDto],
) -> Result<(), BatchError> {
    let table_handle = match shamir.get_table(db, repo, table).await {
        Ok(t) => t,
        Err(_) => {
            return Err(err_code(
                "internal_error",
                format!("table '{db}/{repo}/{table}' not found for keyset_safe check"),
            ));
        }
    };
    let count = table_handle
        .count()
        .await
        .map_err(|e| err_code("internal_error", e.to_string()))?;
    let table_empty = count == 0;

    for rule in new_rules.iter_mut() {
        let preserved = prev_rules
            .iter()
            .find(|prev| prev.path == rule.path && prev.r#type == rule.r#type);
        rule.keyset_safe = match preserved {
            // Unchanged rule — preserve its prior proof.
            Some(prev) => prev.keyset_safe,
            // New or type-changed rule — fresh emptiness proof.
            None => table_empty,
        };
    }
    Ok(())
}

impl ShamirAdminExecutor {
    pub(super) async fn handle_set_table_schema(
        &self,
        op: &SetTableSchemaOp,
    ) -> Result<QueryResult, BatchError> {
        let table = &op.set_table_schema;
        let repo = &op.repo;
        let db = &self.db_name;

        // Authz: Action::Write on the table.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(db.clone(), repo.clone(), table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // Per-table RMW lock.
        let _guard = lock_schema_rmw(&self.shamir, db, repo, table).await;

        // Read the current catalogue record.
        let mut rec = self
            .shamir
            .system_store()
            .load_table_record(db, repo, table)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?
            .ok_or_else(|| err_code("not_found", format!("table '{db}/{repo}/{table}'")))?;

        // Optimistic-concurrency check.
        let current_version = rec
            .get(SCHEMA_VERSION_FIELD)
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as u64;
        if let Some(expected) = op.expected_version {
            if expected != current_version {
                return Err(err_code(
                    "version_conflict",
                    format!("expected schema_version {expected}, found {current_version}"),
                ));
            }
        }

        // Resolve the repo interner (for path interning).
        let interner_mgr = self
            .shamir
            .resolve_repo_interner(db, repo)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;
        let interner = interner_mgr
            .get()
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // Phase C2 — validate FK index requirements before persisting.
        validate_fk_indexes(&self.shamir, db, repo, &op.schema).await?;

        // Phase C3 — validate unique index requirements before persisting.
        validate_unique_indexes(&self.shamir, db, repo, table, &op.schema).await?;

        // Reject nested-path default/auto_now/auto_now_add rules at DDL time
        // (the write path silently skips multi-segment paths — convert that
        // silent skip into an explicit error, mirroring validate_unique_indexes).
        validate_nested_path_transforms(&op.schema)?;

        // Reject self-referential ON DELETE CASCADE at DDL time (the cascade
        // cycle-guard cannot resolve a table cascading into itself — convert
        // the silent runtime skip into an explicit error).
        validate_no_self_referential_cascade(table, &op.schema)?;

        // F-37 (#845) — acquire the schema-activation write barrier across the
        // ENTIRE stamp_keyset_safe (count() == 0 proof) → persist catalogue →
        // activate validator sequence. Without this a concurrent INSERT/UPDATE
        // could land between the count proof and the schema's activation,
        // writing a row under whatever schema was active a moment ago —
        // `keyset_safe = true` would then be stamped for a table whose full
        // row history was never actually proven homogeneous. The barrier
        // mirrors the unique-index DDL's pattern (`create_index_v2`): hold the
        // shared `unique_write_lock` and raise `schema_activation_barrier` so
        // every writer consulting `needs_write_barrier()` serializes on this
        // lock. All three locals are RAII — on EVERY exit path (success or any
        // `?` error below) the barrier flag clears first, then the lock
        // releases (reverse declaration drop order), matching `create_index_v2`.
        //
        // F-48 (#859): `raise` is `async` — it also drains in-flight
        // fast-path writers (see `SchemaActivationBarrierGuard::raise`).
        let (_barrier_handle, _uwl_guard) =
            begin_schema_activation_barrier(&self.shamir, db, repo, table).await?;
        let _barrier = SchemaActivationBarrierGuard::raise(&_barrier_handle).await;

        // F-17 (#810) — stamp the server-computed `keyset_safe` proof onto
        // each rule. Read the PREVIOUS rules from the catalogue so that
        // unchanged rules (same path + same type) preserve their prior
        // proof, and only genuinely new/type-changed rules get a fresh
        // `table.count() == 0` check. Client-supplied `keyset_safe` is
        // ignored unconditionally.
        let prev_rules: Vec<FieldRuleDto> = match rec.get(SCHEMA_FIELD) {
            Some(qv) => dto_list_from_catalogue(qv, interner),
            None => Vec::new(),
        };
        let mut stamped_rules = op.schema.clone();
        stamp_keyset_safe(
            &self.shamir,
            db,
            repo,
            table,
            &prev_rules,
            &mut stamped_rules,
        )
        .await?;

        // Serialise the DTO rules into the catalogue form (interning paths).
        let schema_qv = serialise_rules(&stamped_rules, interner);

        // schema_validator_id: reuse if present (ALTER), else mint a new one.
        let schema_validator_id = match rec.get(SCHEMA_VALIDATOR_ID_FIELD).and_then(|v| v.as_str())
        {
            Some(id_str) => id_str
                .parse::<RecordId>()
                .unwrap_or_else(|_| RecordId::new()),
            None => RecordId::new(),
        };

        let new_version = current_version + 1;

        // F-4 — precompile gate: prove the serialised schema round-trips into
        // `Vec<FieldRule>` BEFORE we touch the catalogue. If this fails,
        // nothing has been persisted yet, so there is nothing to roll back —
        // return the error immediately (validate → precompile → persist →
        // activate). Previously this ran AFTER `save_table_meta`, so a
        // schema that failed to parse left the catalogue holding an
        // unparseable (never-activated) schema.
        let rules = parse_schema(&schema_qv, interner)
            .map_err(|e: shamir_storage::error::DbError| err_code("bad_schema", e.to_string()))?;

        // Snapshot the pre-mutation catalogue record so we can roll back to it
        // if activation (compile_table_schema) fails after we persist.
        let rec_prev = rec.clone();

        // Mutate the catalogue record in place.
        map_insert(&mut rec, SCHEMA_FIELD, schema_qv);
        map_insert(
            &mut rec,
            SCHEMA_VALIDATOR_ID_FIELD,
            QueryValue::Str(schema_validator_id.to_string()),
        );
        map_insert(
            &mut rec,
            SCHEMA_VERSION_FIELD,
            QueryValue::Int(new_version as i64),
        );

        // Persist the catalogue (now that the schema is proven well-formed).
        self.shamir
            .system_store()
            .save_table_meta(&rec)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // Durably persist the repo interner so the schema path ids survive a
        // reopen — `boot_compile_schemas` de-interns them on restart. Mirrors
        // the `interner().persist()` other DDL ops perform. If this fails
        // after a successful catalogue write, roll the catalogue back to the
        // pre-mutation record (best-effort compensating write) — otherwise
        // the catalogue would durably reference field-path ids the interner
        // never learned. The ORIGINAL persist error is returned to the
        // caller; a rollback-write failure is secondary information and only
        // logged (matches the activation-rollback pattern below).
        if let Err(persist_err) = interner_mgr.persist().await {
            if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await {
                log::warn!(
                    "admin_schema::handle_set_table_schema: catalogue rollback \
                     for '{db}/{repo}/{table}' FAILED after interner persist error \
                     ({persist_err}): {rollback_err}. The catalogue may now \
                     hold a schema that was never activated."
                );
            }
            return Err(err_code("internal_error", persist_err.to_string()));
        }

        // Activate: compile + register + auto-bind the validator (live,
        // in-process). If this fails after a successful catalogue write, roll
        // the catalogue back to the pre-mutation record (best-effort
        // compensating write) so the persisted state reflects the still-working
        // old schema rather than a new one that was never activated. The
        // ORIGINAL activation error is returned to the caller; a rollback-write
        // failure is secondary information and only logged (matches the
        // best-effort cleanup pattern used elsewhere, e.g. restore.rs).
        if let Err(activation_err) = self
            .shamir
            .compile_table_schema(db, repo, table, schema_validator_id, rules)
            .await
        {
            if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await {
                log::warn!(
                    "admin_schema::handle_set_table_schema: catalogue rollback \
                     for '{db}/{repo}/{table}' FAILED after activation error \
                     ({activation_err}): {rollback_err}. The catalogue may now \
                     hold a schema that was never activated."
                );
            }
            return Err(err_code("internal_error", activation_err.to_string()));
        }

        Ok(admin_result(mpack!({
            "set_table_schema": @(QueryValue::Str(table.clone())),
            "repo": @(QueryValue::Str(repo.clone())),
            "ok": true,
            "schema_version": @(QueryValue::Int(new_version as i64)),
        })))
    }

    pub(super) async fn handle_add_schema_rule(
        &self,
        op: &AddSchemaRuleOp,
    ) -> Result<QueryResult, BatchError> {
        let table = &op.add_schema_rule;
        let repo = &op.repo;
        let db = &self.db_name;

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(db.clone(), repo.clone(), table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        let _guard = lock_schema_rmw(&self.shamir, db, repo, table).await;

        let mut rec = self
            .shamir
            .system_store()
            .load_table_record(db, repo, table)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?
            .ok_or_else(|| err_code("not_found", format!("table '{db}/{repo}/{table}'")))?;

        let interner_mgr = self
            .shamir
            .resolve_repo_interner(db, repo)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;
        let interner = interner_mgr
            .get()
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // Read the current schema list (de-interned for path matching).
        let mut rules: Vec<FieldRuleDto> = match rec.get(SCHEMA_FIELD) {
            Some(qv) => dto_list_from_catalogue(qv, interner),
            None => Vec::new(),
        };

        // F-17 (#810) — snapshot the previous rules for keyset_safe
        // preservation (unchanged rules keep their prior proof).
        let prev_rules = rules.clone();

        // Upsert by path: replace if a rule with the same path exists.
        let new_rule = &op.rule;
        if let Some(pos) = rules.iter().position(|r| r.path == new_rule.path) {
            rules[pos] = new_rule.clone();
        } else {
            rules.push(new_rule.clone());
        }

        // Phase C2 — validate FK index requirements for the new/updated rule.
        validate_fk_indexes(&self.shamir, db, repo, std::slice::from_ref(new_rule)).await?;

        // Phase C3 — validate unique index requirements for the new/updated rule.
        validate_unique_indexes(
            &self.shamir,
            db,
            repo,
            table,
            std::slice::from_ref(new_rule),
        )
        .await?;

        // Reject nested-path default/auto_now/auto_now_add rules at DDL time.
        validate_nested_path_transforms(std::slice::from_ref(new_rule))?;

        // Reject self-referential ON DELETE CASCADE at DDL time.
        validate_no_self_referential_cascade(table, std::slice::from_ref(new_rule))?;

        // F-37 (#845) — acquire the schema-activation write barrier across the
        // stamp_keyset_safe → persist → activate sequence, exactly as in
        // `handle_set_table_schema`. AddSchemaRule shares the same
        // count-based `keyset_safe` proof (below) and therefore the same
        // writer race. RAII: the flag clears and the lock releases on every
        // exit path (success or `?` error), reverse declaration drop order.
        //
        // F-48 (#859): `raise` is `async` — it also drains in-flight
        // fast-path writers (see `SchemaActivationBarrierGuard::raise`).
        let (_barrier_handle, _uwl_guard) =
            begin_schema_activation_barrier(&self.shamir, db, repo, table).await?;
        let _barrier = SchemaActivationBarrierGuard::raise(&_barrier_handle).await;

        // F-17 (#810) — stamp the server-computed `keyset_safe` proof. Only
        // the upserted rule (new path or type-changed) gets a fresh
        // `table.count() == 0` check; all other rules preserve their prior
        // proof from the catalogue.
        stamp_keyset_safe(&self.shamir, db, repo, table, &prev_rules, &mut rules).await?;

        // Re-serialise + persist.
        let schema_qv = serialise_rules(&rules, interner);
        let schema_validator_id = match rec.get(SCHEMA_VALIDATOR_ID_FIELD).and_then(|v| v.as_str())
        {
            Some(id_str) => id_str
                .parse::<RecordId>()
                .unwrap_or_else(|_| RecordId::new()),
            None => RecordId::new(),
        };
        let current_version = rec
            .get(SCHEMA_VERSION_FIELD)
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as u64;
        let new_version = current_version + 1;

        // F-4 — precompile gate: prove the serialised schema round-trips into
        // `Vec<FieldRule>` BEFORE we touch the catalogue (see
        // handle_set_table_schema for the full rationale).
        let compiled_rules = parse_schema(&schema_qv, interner)
            .map_err(|e: shamir_storage::error::DbError| err_code("bad_schema", e.to_string()))?;

        // Snapshot the pre-mutation catalogue record for activation rollback.
        let rec_prev = rec.clone();

        map_insert(&mut rec, SCHEMA_FIELD, schema_qv);
        map_insert(
            &mut rec,
            SCHEMA_VALIDATOR_ID_FIELD,
            QueryValue::Str(schema_validator_id.to_string()),
        );
        map_insert(
            &mut rec,
            SCHEMA_VERSION_FIELD,
            QueryValue::Int(new_version as i64),
        );

        self.shamir
            .system_store()
            .save_table_meta(&rec)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // Durably persist the repo interner so the schema path ids survive a
        // reopen — `boot_compile_schemas` de-interns them on restart. Mirrors
        // the `interner().persist()` other DDL ops perform. Roll the
        // catalogue back to the pre-mutation record on failure (see
        // handle_set_table_schema for the full rationale).
        if let Err(persist_err) = interner_mgr.persist().await {
            if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await {
                log::warn!(
                    "admin_schema::handle_add_schema_rule: catalogue rollback \
                     for '{db}/{repo}/{table}' FAILED after interner persist error \
                     ({persist_err}): {rollback_err}. The catalogue may now \
                     hold a schema that was never activated."
                );
            }
            return Err(err_code("internal_error", persist_err.to_string()));
        }

        // Activate, rolling back the catalogue write on activation failure
        // (see handle_set_table_schema for the full rationale).
        if let Err(activation_err) = self
            .shamir
            .compile_table_schema(db, repo, table, schema_validator_id, compiled_rules)
            .await
        {
            if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await {
                log::warn!(
                    "admin_schema::handle_add_schema_rule: catalogue rollback \
                     for '{db}/{repo}/{table}' FAILED after activation error \
                     ({activation_err}): {rollback_err}. The catalogue may now \
                     hold a schema that was never activated."
                );
            }
            return Err(err_code("internal_error", activation_err.to_string()));
        }

        Ok(admin_result(mpack!({
            "add_schema_rule": @(QueryValue::Str(table.clone())),
            "repo": @(QueryValue::Str(repo.clone())),
            "ok": true,
            "schema_version": @(QueryValue::Int(new_version as i64)),
        })))
    }

    pub(super) async fn handle_remove_schema_rule(
        &self,
        op: &RemoveSchemaRuleOp,
    ) -> Result<QueryResult, BatchError> {
        let table = &op.remove_schema_rule;
        let repo = &op.repo;
        let db = &self.db_name;

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(db.clone(), repo.clone(), table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        let _guard = lock_schema_rmw(&self.shamir, db, repo, table).await;

        let mut rec = self
            .shamir
            .system_store()
            .load_table_record(db, repo, table)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?
            .ok_or_else(|| err_code("not_found", format!("table '{db}/{repo}/{table}'")))?;

        let interner_mgr = self
            .shamir
            .resolve_repo_interner(db, repo)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;
        let interner = interner_mgr
            .get()
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // Read current rules (DTO form, de-interned).
        let mut rules: Vec<FieldRuleDto> = match rec.get(SCHEMA_FIELD) {
            Some(qv) => dto_list_from_catalogue(qv, interner),
            None => Vec::new(),
        };

        // Remove by path.
        let before = rules.len();
        rules.retain(|r| r.path != op.path);
        let removed = rules.len() < before;

        let schema_qv = serialise_rules(&rules, interner);
        let schema_validator_id = match rec.get(SCHEMA_VALIDATOR_ID_FIELD).and_then(|v| v.as_str())
        {
            Some(id_str) => id_str
                .parse::<RecordId>()
                .unwrap_or_else(|_| RecordId::new()),
            None => RecordId::new(),
        };
        let current_version = rec
            .get(SCHEMA_VERSION_FIELD)
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as u64;
        let new_version = current_version + 1;

        // Recompile (even if empty — an empty schema validator accepts all).
        // F-4 — precompile gate BEFORE we touch the catalogue (see
        // handle_set_table_schema for the full rationale).
        let compiled_rules = parse_schema(&schema_qv, interner)
            .map_err(|e: shamir_storage::error::DbError| err_code("bad_schema", e.to_string()))?;

        // Snapshot the pre-mutation catalogue record for activation rollback.
        let rec_prev = rec.clone();

        map_insert(&mut rec, SCHEMA_FIELD, schema_qv);
        map_insert(
            &mut rec,
            SCHEMA_VALIDATOR_ID_FIELD,
            QueryValue::Str(schema_validator_id.to_string()),
        );
        map_insert(
            &mut rec,
            SCHEMA_VERSION_FIELD,
            QueryValue::Int(new_version as i64),
        );

        self.shamir
            .system_store()
            .save_table_meta(&rec)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // Durably persist the repo interner so the schema path ids survive a
        // reopen — `boot_compile_schemas` de-interns them on restart. Mirrors
        // the `interner().persist()` other DDL ops perform. Roll the
        // catalogue back to the pre-mutation record on failure (see
        // handle_set_table_schema for the full rationale).
        if let Err(persist_err) = interner_mgr.persist().await {
            if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await {
                log::warn!(
                    "admin_schema::handle_remove_schema_rule: catalogue rollback \
                     for '{db}/{repo}/{table}' FAILED after interner persist error \
                     ({persist_err}): {rollback_err}. The catalogue may now \
                     hold a schema that was never activated."
                );
            }
            return Err(err_code("internal_error", persist_err.to_string()));
        }

        // Activate, rolling back the catalogue write on activation failure
        // (see handle_set_table_schema for the full rationale).
        if let Err(activation_err) = self
            .shamir
            .compile_table_schema(db, repo, table, schema_validator_id, compiled_rules)
            .await
        {
            if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await {
                log::warn!(
                    "admin_schema::handle_remove_schema_rule: catalogue rollback \
                     for '{db}/{repo}/{table}' FAILED after activation error \
                     ({activation_err}): {rollback_err}. The catalogue may now \
                     hold a schema that was never activated."
                );
            }
            return Err(err_code("internal_error", activation_err.to_string()));
        }

        Ok(admin_result(mpack!({
            "remove_schema_rule": @(QueryValue::Str(table.clone())),
            "repo": @(QueryValue::Str(repo.clone())),
            "ok": true,
            "removed": @QueryValue::Bool(removed),
            "schema_version": @(QueryValue::Int(new_version as i64)),
        })))
    }

    pub(super) async fn handle_get_table_schema(
        &self,
        op: &GetTableSchemaOp,
    ) -> Result<QueryResult, BatchError> {
        let table = &op.get_table_schema;
        let repo = &op.repo;
        let db = &self.db_name;

        // Authz: Action::Read on the table (introspection).
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(db.clone(), repo.clone(), table.clone()),
                Action::Read,
            )
            .await
            .map_err(err_access)?;

        let rec = self
            .shamir
            .system_store()
            .load_table_record(db, repo, table)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?
            .ok_or_else(|| err_code("not_found", format!("table '{db}/{repo}/{table}'")))?;

        let interner_mgr = self
            .shamir
            .resolve_repo_interner(db, repo)
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;
        let interner = interner_mgr
            .get()
            .await
            .map_err(|e| err_code("internal_error", e.to_string()))?;

        // De-intern the catalogue schema into the wire DTO form (flat names).
        let schema_dto = match rec.get(SCHEMA_FIELD) {
            Some(qv) if !matches!(qv, QueryValue::Null) => dto_list_from_catalogue(qv, interner),
            _ => Vec::new(),
        };
        let schema_version = rec
            .get(SCHEMA_VERSION_FIELD)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // Serialise the DTO list to a QueryValue List of Maps (flat names).
        let schema_qv = serialise_rules_flat(&schema_dto);

        Ok(admin_result(mpack!({
            "get_table_schema": @(QueryValue::Str(table.clone())),
            "repo": @(QueryValue::Str(repo.clone())),
            "schema": @schema_qv,
            "schema_version": @QueryValue::Int(schema_version),
        })))
    }
}

/// Insert a `(key, value)` pair into a `QueryValue::Map` in place.
///
/// `QueryValue` exposes `as_object` (immutable) but no `as_object_mut`;
/// this helper does the in-place mutation via pattern matching.
fn map_insert(qv: &mut QueryValue, key: &str, value: QueryValue) {
    if let QueryValue::Map(m) = qv {
        m.insert(key.to_string(), value);
    }
}

// ── serialisation helpers ──────────────────────────────────────────────────

/// Serialise a list of DTO rules into the **catalogue form** (paths as
/// interned-id `List[Int]`, exactly the shape [`parse_schema`] reads).
fn serialise_rules(rules: &[FieldRuleDto], interner: &Interner) -> QueryValue {
    let items: Vec<QueryValue> = rules
        .iter()
        .map(|r| serialise_one_rule_catalogue(r, interner))
        .collect();
    QueryValue::List(items)
}

/// Serialise one rule into the catalogue Map form (interned path ids).
fn serialise_one_rule_catalogue(rule: &FieldRuleDto, interner: &Interner) -> QueryValue {
    let mut m = new_map();

    // path: List[Int] (interned ids).
    let path_ids: Vec<QueryValue> = rule
        .path
        .iter()
        .map(|seg| {
            let touch = interner.touch_ind(seg).expect("interner touch_ind");
            QueryValue::Int(touch.into_key().id() as i64)
        })
        .collect();
    m.insert("path".to_string(), QueryValue::List(path_ids));

    // type: Str.
    m.insert("type".to_string(), QueryValue::Str(rule.r#type.clone()));

    // Constraints (optional fields, only inserted when present).
    insert_constraint_fields(&mut m, &rule.constraints);

    // F-17 (#810) — persist `keyset_safe` only when true (omit when false
    // so legacy catalogue rows stay byte-identical).
    if rule.keyset_safe {
        m.insert("keyset_safe".to_string(), QueryValue::Bool(true));
    }

    QueryValue::Map(m)
}

/// Serialise a list of DTO rules into the **flat (de-interned) wire form**
/// — paths as `List[Str]`, suitable for `GetTableSchema` responses.
pub(super) fn serialise_rules_flat(rules: &[FieldRuleDto]) -> QueryValue {
    let items: Vec<QueryValue> = rules.iter().map(serialise_one_rule_flat).collect();
    QueryValue::List(items)
}

/// Serialise one rule into the flat wire form (string paths).
fn serialise_one_rule_flat(rule: &FieldRuleDto) -> QueryValue {
    let mut m = new_map();

    let path_strs: Vec<QueryValue> = rule
        .path
        .iter()
        .map(|s| QueryValue::Str(s.clone()))
        .collect();
    m.insert("path".to_string(), QueryValue::List(path_strs));
    m.insert("type".to_string(), QueryValue::Str(rule.r#type.clone()));

    insert_constraint_fields(&mut m, &rule.constraints);

    // F-17 (#810) — include `keyset_safe` when true (same omit-when-false
    // contract as the catalogue form).
    if rule.keyset_safe {
        m.insert("keyset_safe".to_string(), QueryValue::Bool(true));
    }

    QueryValue::Map(m)
}

/// Insert the optional constraint fields from a DTO into a catalogue/flat
/// Map. Shared by both serialisers — the constraint fields are identical in
/// both forms (only `path` differs).
fn insert_constraint_fields(m: &mut TMap<String, QueryValue>, c: &ConstraintsDto) {
    if let Some(v) = c.required {
        m.insert("required".to_string(), QueryValue::Bool(v));
    }
    if let Some(v) = c.nullable {
        m.insert("nullable".to_string(), QueryValue::Bool(v));
    }
    if let Some(v) = c.unsigned {
        m.insert("unsigned".to_string(), QueryValue::Bool(v));
    }
    if let Some(n) = &c.min {
        m.insert(
            "min".to_string(),
            match n {
                NumDto::Int(i) => QueryValue::Int(*i),
                NumDto::F64(f) => QueryValue::F64(*f),
            },
        );
    }
    if let Some(n) = &c.max {
        m.insert(
            "max".to_string(),
            match n {
                NumDto::Int(i) => QueryValue::Int(*i),
                NumDto::F64(f) => QueryValue::F64(*f),
            },
        );
    }
    if let Some(v) = c.len {
        m.insert("len".to_string(), QueryValue::Int(v as i64));
    }
    if let Some(v) = c.max_len {
        m.insert("max_len".to_string(), QueryValue::Int(v as i64));
    }
    if let Some(v) = c.min_len {
        m.insert("min_len".to_string(), QueryValue::Int(v as i64));
    }
    if let Some(items) = &c.one_of {
        m.insert("one_of".to_string(), QueryValue::List(items.clone()));
    }
    // ③.2c — default (literal or expression; extends ②.4b literal-only).
    // WRITE: FilterValue → QueryValue.
    // Literals use a direct match (no allocation, no msgpack).
    // Expression defaults ($fn/$ref/$expr/$cond/$param/$query) have no direct
    // QueryValue equivalent and are serialised via msgpack round-trip, which
    // preserves the untagged-serde shape faithfully.
    // On failure (malformed expression) we log a warning instead of silently
    // dropping — boot-resilience is preserved (the field is omitted rather
    // than aborting), but the failure is now visible in the log.
    if let Some(fv) = &c.default {
        let qv_opt: Option<QueryValue> = if let Some(qv) = fv_to_qv_literal(fv) {
            // Literal path: direct, zero-copy conversion.
            Some(qv)
        } else {
            // Expression path: msgpack round-trip.
            match rmp_serde::to_vec_named(fv)
                .ok()
                .and_then(|bytes| rmp_serde::from_slice::<QueryValue>(&bytes).ok())
            {
                Some(qv) => Some(qv),
                None => {
                    log::warn!(
                        "admin_schema::insert_constraint_fields: \
                         failed to serialise default FilterValue to QueryValue — \
                         field omitted from catalogue. value = {:?}",
                        fv
                    );
                    None
                }
            }
        };
        if let Some(qv) = qv_opt {
            m.insert("default".to_string(), qv);
        }
    }
    if let Some(s) = &c.array_of {
        m.insert("array_of".to_string(), QueryValue::Str(s.clone()));
    }
    // Phase B fields.
    if let Some(s) = &c.scalar {
        m.insert("scalar".to_string(), QueryValue::Str(s.clone()));
    }
    if let Some(s) = &c.format {
        m.insert("format".to_string(), QueryValue::Str(s.clone()));
    }
    if let Some(cmp) = &c.compare {
        let mut cmp_m = new_map();
        let other_list: Vec<QueryValue> = cmp
            .other
            .iter()
            .map(|s| QueryValue::Str(s.clone()))
            .collect();
        cmp_m.insert("other".to_string(), QueryValue::List(other_list));
        cmp_m.insert("op".to_string(), QueryValue::Str(cmp.op.clone()));
        m.insert("compare".to_string(), QueryValue::Map(cmp_m));
    }
    // Phase C3 — unique.
    if let Some(v) = c.unique {
        m.insert("unique".to_string(), QueryValue::Bool(v));
    }
    // ③.2d — server-stamping flags (omit when false — legacy rows stay unchanged).
    if c.auto_now {
        m.insert("auto_now".to_string(), QueryValue::Bool(true));
    }
    if c.auto_now_add {
        m.insert("auto_now_add".to_string(), QueryValue::Bool(true));
    }
    // Phase C2 — foreign_key.
    if let Some(fk) = &c.foreign_key {
        let mut fk_m = new_map();
        fk_m.insert(
            "ref_table".to_string(),
            QueryValue::Str(fk.ref_table.clone()),
        );
        fk_m.insert(
            "ref_field".to_string(),
            QueryValue::Str(fk.ref_field.clone()),
        );
        // Phase D — persist the ON DELETE action so reverse-FK discovery
        // (RESTRICT / CASCADE / SET NULL) survives the catalogue round-trip.
        // Mirrors `foreign_key_dto_from_qv` read mapping. NoAction is omitted
        // so legacy rows (and the common default) stay byte-identical.
        let on_delete = match fk.on_delete {
            FkAction::Restrict => Some("restrict"),
            FkAction::Cascade => Some("cascade"),
            FkAction::SetNull => Some("set_null"),
            FkAction::NoAction => None,
        };
        if let Some(action) = on_delete {
            fk_m.insert("on_delete".to_string(), QueryValue::Str(action.to_string()));
        }
        // Phase ②.2a — persist the ON UPDATE action (surface only; enforcement
        // lands in ②.2b). Symmetric to `on_delete` above; NoAction is omitted
        // so legacy rows (and the default) stay byte-identical.
        let on_update = match fk.on_update {
            FkAction::Restrict => Some("restrict"),
            FkAction::Cascade => Some("cascade"),
            FkAction::SetNull => Some("set_null"),
            FkAction::NoAction => None,
        };
        if let Some(action) = on_update {
            fk_m.insert("on_update".to_string(), QueryValue::Str(action.to_string()));
        }
        m.insert("foreign_key".to_string(), QueryValue::Map(fk_m));
    }
}

// ── deserialisation helper (catalogue → DTO, de-interned) ──────────────────

/// Parse a catalogue-form schema (`List[Map]` with interned path ids) back
/// into the wire DTO form (flat string paths). Used by add/remove/get.
pub(super) fn dto_list_from_catalogue(qv: &QueryValue, interner: &Interner) -> Vec<FieldRuleDto> {
    let items = match qv {
        QueryValue::List(l) => l,
        _ => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|item| dto_one_from_catalogue(item, interner))
        .collect()
}

/// Parse one catalogue rule Map into a DTO (de-interning the path).
fn dto_one_from_catalogue(item: &QueryValue, interner: &Interner) -> Option<FieldRuleDto> {
    let m = item.as_object()?;

    // path: List[Int] → List<String>.
    let path_ids = m.get("path")?.as_array()?;
    let path: Vec<String> = path_ids
        .iter()
        .map(|id_val| {
            let id = id_val.as_i64()?;
            let key = InternerKey::new(id as u64);
            interner.get_str(&key).map(|s| s.to_string())
        })
        .collect::<Option<Vec<_>>>()?;

    let r#type = m.get("type")?.as_str()?.to_string();

    let constraints = ConstraintsDto {
        required: m.get("required").and_then(|v| v.as_bool()),
        nullable: m.get("nullable").and_then(|v| v.as_bool()),
        unsigned: m.get("unsigned").and_then(|v| v.as_bool()),
        min: m.get("min").and_then(num_dto_from_qv),
        max: m.get("max").and_then(num_dto_from_qv),
        len: m.get("len").and_then(|v| v.as_i64()).map(|v| v as u64),
        max_len: m.get("max_len").and_then(|v| v.as_i64()).map(|v| v as u64),
        min_len: m.get("min_len").and_then(|v| v.as_i64()).map(|v| v as u64),
        one_of: m.get("one_of").and_then(|v| {
            if let QueryValue::List(items) = v {
                Some(items.clone())
            } else {
                None
            }
        }),
        // ③.2c — default (literal or expression; extends ②.4b literal-only).
        // READ: QueryValue → FilterValue.
        // Literals use a direct match (no msgpack). Map (expression defaults
        // like {"$fn":...}) fall back to msgpack. On genuine decode failure
        // we log a warning — the default is dropped (boot-resilience preserved),
        // but the failure is visible in the log instead of being silent.
        default: m
            .get("default")
            .and_then(|qv| catalogue_qv_to_filter_value(qv, "dto_one_from_catalogue", "default")),
        array_of: m.get("array_of").and_then(|v| v.as_str()).map(String::from),
        scalar: m.get("scalar").and_then(|v| v.as_str()).map(String::from),
        format: m.get("format").and_then(|v| v.as_str()).map(String::from),
        compare: m.get("compare").and_then(compare_dto_from_qv),
        foreign_key: m.get("foreign_key").and_then(foreign_key_dto_from_qv),
        unique: m.get("unique").and_then(|v| v.as_bool()),
        // ③.2d — server-stamping flags (absent = false for legacy rows).
        auto_now: m.get("auto_now").and_then(|v| v.as_bool()).unwrap_or(false),
        auto_now_add: m
            .get("auto_now_add")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    Some(FieldRuleDto {
        path,
        r#type,
        constraints,
        // F-17 (#810) — absent in pre-fix rows = false (unproven).
        keyset_safe: m
            .get("keyset_safe")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Extract a `NumDto` from a catalogue QueryValue.
fn num_dto_from_qv(v: &QueryValue) -> Option<NumDto> {
    match v {
        QueryValue::Int(i) => Some(NumDto::Int(*i)),
        QueryValue::F64(f) => Some(NumDto::F64(*f)),
        _ => None,
    }
}

/// Extract a `ForeignKeyDto` from a catalogue Map.
fn foreign_key_dto_from_qv(v: &QueryValue) -> Option<ForeignKeyDto> {
    let m = v.as_object()?;
    let ref_table = m.get("ref_table")?.as_str()?.to_string();
    let ref_field = m.get("ref_field")?.as_str()?.to_string();
    // Legacy catalogue rows written before on_delete exist — default to
    // NoAction (the serde/wire default) so they round-trip unchanged.
    let on_delete = match m.get("on_delete").and_then(|v| v.as_str()) {
        Some("no_action") => FkAction::NoAction,
        Some("restrict") => FkAction::Restrict,
        Some("cascade") => FkAction::Cascade,
        Some("set_null") => FkAction::SetNull,
        _ => FkAction::default(),
    };
    // Phase ②.2a — read on_update (surface only; enforcement lands in ②.2b).
    // Legacy rows without the field default to NoAction, mirroring on_delete.
    let on_update = match m.get("on_update").and_then(|v| v.as_str()) {
        Some("no_action") => FkAction::NoAction,
        Some("restrict") => FkAction::Restrict,
        Some("cascade") => FkAction::Cascade,
        Some("set_null") => FkAction::SetNull,
        _ => FkAction::default(),
    };
    Some(ForeignKeyDto {
        ref_table,
        ref_field,
        on_delete,
        on_update,
    })
}

/// Convert a catalogue [`QueryValue`] back to a [`FilterValue`].
///
/// Strategy (READ path):
/// 1. Literal variants (Null/Bool/Int/F64/Str/Bin/List) → direct match via
///    [`qv_to_fv_literal`] (no msgpack, zero-copy for common case).
/// 2. `Map` (expression defaults such as `{"$fn": ...}`) → msgpack round-trip,
///    which preserves the untagged-serde shape that encodes expression variants.
/// 3. Other exotic types (Dec/Big/Set) → msgpack round-trip as last resort.
/// 4. Genuine failure → `log::warn!` with context (caller + field name) and
///    `None` (default is dropped, boot-resilience preserved — no panic).
fn catalogue_qv_to_filter_value(qv: &QueryValue, caller: &str, field: &str) -> Option<FilterValue> {
    // Tier 1: direct literal match.
    if let Some(fv) = qv_to_fv_literal(qv) {
        return Some(fv);
    }
    // Tier 2: msgpack for Map (expression defaults) and exotic types.
    if let Some(fv) = rmp_serde::to_vec_named(qv)
        .ok()
        .and_then(|bytes| rmp_serde::from_slice::<FilterValue>(&bytes).ok())
    {
        return Some(fv);
    }
    // Tier 3: genuine decode failure — log visibly, drop gracefully.
    log::warn!(
        "admin_schema::{}: catalogue default decode failed for field '{}' — \
         dropped. qv = {:?}",
        caller,
        field,
        qv
    );
    None
}

/// Extract a `CompareDto` from a catalogue Map.
fn compare_dto_from_qv(v: &QueryValue) -> Option<CompareDto> {
    let m = v.as_object()?;
    let other_arr = m.get("other")?.as_array()?;
    let other: Vec<String> = other_arr
        .iter()
        .filter_map(|s| s.as_str().map(String::from))
        .collect();
    if other.is_empty() {
        return None;
    }
    let op = m.get("op")?.as_str()?.to_string();
    Some(CompareDto { other, op })
}
