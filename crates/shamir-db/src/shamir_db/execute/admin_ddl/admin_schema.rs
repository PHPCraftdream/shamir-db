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
use crate::query::admin::{
    AddSchemaRuleOp, FieldRuleDto, FkAction, GetTableSchemaOp, RemoveSchemaRuleOp, SetTableSchemaOp,
};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::shamir_db::shamir_db::schema_management::{
    parse_schema, SCHEMA_FIELD, SCHEMA_VALIDATOR_ID_FIELD, SCHEMA_VERSION_FIELD,
};
use crate::shamir_db::ShamirDb;
use crate::types::value::QueryValue;
use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;
use super::schema_codec::{
    dto_list_from_catalogue, map_insert, serialise_rules, serialise_rules_flat,
};

#[path = "../repo_admin/admin_schema_admission.rs"]
mod admission;
use admission::{begin_schema_activation_barrier, err_code, schema_lock_key, schema_repo_permit};

fn err_access(e: shamir_types::access::AccessError) -> BatchError {
    err_code("access_denied", e.to_string())
}

// ── per-table RMW lock ─────────────────────────────────────────────────────

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
// with no further check. The barrier acquisition therefore also drains every
// such in-flight fast-path writer before the count-proof proceeds. See
// `shamir-engine::table::writer_drain_barrier` for the reusable drain
// primitive.
//
// F-70 (#897, P0): acquired via `TableManager::begin_write_barrier` — raise
// `SCHEMA_ACTIVATION`, drain, THEN take `unique_write_lock` (drain-then-lock),
// NOT lock-then-drain (this file's original order, mirroring F-57's DDL
// shape, which this task found deadlocks against `pre_commit_prelock`'s
// drain-guard-then-lock shape on a second table). See
// `shamir-engine::table::writer_drain_barrier`'s "F-70 — THE canonical
// lock-order hierarchy" doc section for the full derivation; this DDL uses
// the SAME canonical entry point as every `shamir-engine` CREATE INDEX path,
// so the hierarchy cannot drift between the two crates.
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
    permit: &shamir_engine::repo::RepoWritePermit,
    db: &str,
    repo: &str,
    table: &str,
    rules: &[FieldRuleDto],
) -> Result<(), BatchError> {
    for rule in rules {
        if rule.constraints.unique == Some(true) {
            // The unique field lives on the SAME table being constrained.
            let self_table = match shamir.get_table_admitted(db, repo, table, permit).await {
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
            let has_index = self_table
                .info_store_read_view()
                .interned_field_id(field_name)
                .await
                .map_err(|e| err_code("internal_error", e.to_string()))?
                .is_some_and(|field_id| {
                    let field_path = [field_id];
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
    permit: &shamir_engine::repo::RepoWritePermit,
    db: &str,
    repo: &str,
    rules: &[FieldRuleDto],
) -> Result<(), BatchError> {
    for rule in rules {
        if let Some(fk) = &rule.constraints.foreign_key {
            // Resolve the referenced table.
            let ref_table = match shamir
                .get_table_admitted(db, repo, &fk.ref_table, permit)
                .await
            {
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
            let has_index = ref_table
                .info_store_read_view()
                .interned_field_id(&fk.ref_field)
                .await
                .map_err(|e| err_code("internal_error", e.to_string()))?
                .is_some_and(|field_id| {
                    let field_path = [field_id];
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
    permit: &shamir_engine::repo::RepoWritePermit,
    db: &str,
    repo: &str,
    table: &str,
    prev_rules: &[FieldRuleDto],
    new_rules: &mut [FieldRuleDto],
) -> Result<(), BatchError> {
    let table_handle = match shamir.get_table_admitted(db, repo, table, permit).await {
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
        let permit = schema_repo_permit(&self.shamir, db, repo).await?;

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
        validate_fk_indexes(&self.shamir, &permit, db, repo, &op.schema).await?;

        // Phase C3 — validate unique index requirements before persisting.
        validate_unique_indexes(&self.shamir, &permit, db, repo, table, &op.schema).await?;

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
        // row history was never actually proven homogeneous. Both RAII
        // guards are held for the whole sequence — on EVERY exit path
        // (success or any `?` error below) the lock releases first, then
        // the barrier bit clears (F-87, #915: a `let (a, b, c) = ...` tuple
        // drops its bindings in REVERSE declaration order, so `_uwl_guard`
        // — declared last — drops FIRST; the prior comment here claimed
        // the opposite order). Benign either way: a writer arriving in the
        // gap between lock-release and bit-clear still reads the bit as
        // set and takes the slow (locked) path, simply finding the lock
        // already free.
        //
        // F-70 (#897, P0): `begin_schema_activation_barrier` acquires via the
        // canonical drain-then-lock path (raise → drain → lock) — see that
        // function's doc.
        let (_barrier_handle, _barrier, _uwl_guard) =
            begin_schema_activation_barrier(&self.shamir, db, repo, table, &permit).await?;

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
            &permit,
            db,
            repo,
            table,
            &prev_rules,
            &mut stamped_rules,
        )
        .await?;

        // Serialise the DTO rules into the catalogue form (interning paths).
        // RU8: propagate the typed quota code from a base-intern over cap.
        let schema_qv = serialise_rules(&stamped_rules, interner)
            .map_err(|e| err_code(e.code().unwrap_or("internal_error"), e.to_string()))?;

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
            .compile_table_schema_admitted(db, repo, table, schema_validator_id, rules, &permit)
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
        let permit = schema_repo_permit(&self.shamir, db, repo).await?;

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
        validate_fk_indexes(
            &self.shamir,
            &permit,
            db,
            repo,
            std::slice::from_ref(new_rule),
        )
        .await?;

        // Phase C3 — validate unique index requirements for the new/updated rule.
        validate_unique_indexes(
            &self.shamir,
            &permit,
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
        // writer race. RAII: the barrier bit clears and the lock releases on
        // every exit path (success or `?` error), reverse declaration drop
        // order.
        //
        // F-70 (#897, P0): acquired via the canonical drain-then-lock path
        // (see `begin_schema_activation_barrier`'s doc).
        let (_barrier_handle, _barrier, _uwl_guard) =
            begin_schema_activation_barrier(&self.shamir, db, repo, table, &permit).await?;

        // F-17 (#810) — stamp the server-computed `keyset_safe` proof. Only
        // the upserted rule (new path or type-changed) gets a fresh
        // `table.count() == 0` check; all other rules preserve their prior
        // proof from the catalogue.
        stamp_keyset_safe(
            &self.shamir,
            &permit,
            db,
            repo,
            table,
            &prev_rules,
            &mut rules,
        )
        .await?;

        // Re-serialise + persist. RU8: propagate the typed quota code.
        let schema_qv = serialise_rules(&rules, interner)
            .map_err(|e| err_code(e.code().unwrap_or("internal_error"), e.to_string()))?;
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
            .compile_table_schema_admitted(
                db,
                repo,
                table,
                schema_validator_id,
                compiled_rules,
                &permit,
            )
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
        let permit = schema_repo_permit(&self.shamir, db, repo).await?;

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

        // RU8: propagate the typed quota code.
        let schema_qv = serialise_rules(&rules, interner)
            .map_err(|e| err_code(e.code().unwrap_or("internal_error"), e.to_string()))?;
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
            .compile_table_schema_admitted(
                db,
                repo,
                table,
                schema_validator_id,
                compiled_rules,
                &permit,
            )
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
