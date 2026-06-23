use base64::Engine;
use std::sync::Arc;

use crate::access::{Actor, ResourceMeta};
use crate::{DbError, DbResult};
use shamir_engine::function::{compile_rust_source, FunctionError, WasmFunction, WasmLimits};
use shamir_engine::validator::{
    NativeRecordValidator, RecordFields, RecordValidator, Validation, ValidatorBinding,
    ValidatorCtx, ValidatorRegistry, WasmRecordValidator, WriteOp,
};
use shamir_types::types::record_id::RecordId;

use super::{ArtifactKind, FunctionSource, ShamirDb};

impl ShamirDb {
    // ========================================================================
    // Validator lifecycle API (S1)
    // ========================================================================

    /// Access the live validator registry.
    pub fn validators(&self) -> &Arc<ValidatorRegistry> {
        &self.validators
    }

    /// Register a **native** (in-process) validator by closure — no WASM
    /// required, no `replace_artifact` dance.
    ///
    /// The closure receives `(new, old, ctx)` where `new` / `old` are
    /// `Option<&dyn RecordFields>` (by-name, zero-copy access) and `ctx`
    /// carries the actor and interner.  Returns a [`Validation`].
    ///
    /// Internally the closure is wrapped in [`NativeRecordValidator`] which
    /// implements [`RecordValidator`] — no `ShamirFunction` overhead, no
    /// de-interning on the native hot path.
    ///
    /// A catalogue row with `kind = Native` and **no** `wasm_b64` is
    /// persisted so the binding survives restarts. On reopen the boot loop
    /// skips wasm materialisation for `Native` rows; the embedder must
    /// re-register the native artifact at startup. Until then the binding is
    /// remembered and any use fails closed ("validator not found").
    ///
    /// Returns the `RecordId` assigned to the new validator.
    pub async fn register_native_validator<F>(
        &self,
        name: &str,
        replace: bool,
        validator: F,
    ) -> DbResult<RecordId>
    where
        F: Fn(
                Option<&dyn RecordFields>,
                Option<&dyn RecordFields>,
                &ValidatorCtx<'_>,
            ) -> Validation
            + Send
            + Sync
            + 'static,
    {
        self.register_native_validator_as(name, replace, validator, Actor::System)
            .await
    }

    /// Like [`register_native_validator`] but with an explicit [`Actor`].
    pub async fn register_native_validator_as<F>(
        &self,
        name: &str,
        replace: bool,
        validator: F,
        actor: Actor,
    ) -> DbResult<RecordId>
    where
        F: Fn(
                Option<&dyn RecordFields>,
                Option<&dyn RecordFields>,
                &ValidatorCtx<'_>,
            ) -> Validation
            + Send
            + Sync
            + 'static,
    {
        if !replace && self.validators.id_for_name(name).is_some() {
            return Err(DbError::Validation(format!(
                "validator '{}' already exists",
                name
            )));
        }

        // Determine the RecordId: on replace, try the live registry first,
        // then fall back to the persisted catalogue (the boot loop skips
        // Native rows, so the registry won't have them after a restart).
        // The catalogue is keyed by `name`, so the fallback uses the O(1)
        // `load_validator(name)` lookup rather than a full-catalogue scan.
        let id = if replace {
            if let Some(existing) = self.validators.id_for_name(name) {
                existing
            } else if let Some(rec) = self.system_store.load_validator(name).await? {
                rec.get("_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<RecordId>().ok())
                    .unwrap_or_default()
            } else {
                RecordId::default()
            }
        } else {
            RecordId::new()
        };

        // Build the catalogue row — kind=Native, no wasm_b64.
        let mut m = shamir_types::types::common::new_map();
        m.insert(
            "name".to_string(),
            shamir_types::types::value::QueryValue::Str(name.to_string()),
        );
        m.insert(
            "_id".to_string(),
            shamir_types::types::value::QueryValue::Str(id.to_string()),
        );
        // Native rows have no wasm bytes; omit wasm_b64 / wasm_hash entirely.
        m.insert(
            "lang".to_string(),
            shamir_types::types::value::QueryValue::Str("rust".to_string()),
        );
        m.insert(
            "source".to_string(),
            shamir_types::types::value::QueryValue::Null,
        );
        m.insert(
            "bound_in".to_string(),
            shamir_types::types::value::QueryValue::List(vec![]),
        );
        m.insert(
            super::KIND_FIELD.to_string(),
            ArtifactKind::Native.as_query_value(),
        );
        let record = shamir_types::types::value::QueryValue::Map(m);

        // Persist before registering so a crash can't leave a live entry
        // without a catalogue record.
        self.system_store
            .save_validator(name, &record, &ResourceMeta::owned_by(actor))
            .await?;

        if replace {
            if let Some(old_id) = self.validators.id_for_name(name) {
                self.validators.remove(&old_id);
            }
        }

        let adapter = Arc::new(NativeRecordValidator::new(validator)) as Arc<dyn RecordValidator>;
        self.validators
            .register(id, name, adapter)
            .map_err(|e| DbError::Validation(e.to_string()))?;

        Ok(id)
    }

    /// Register a WASM validator from pre-compiled binary bytes.
    ///
    /// Returns the `RecordId` assigned to the new validator.
    /// If `replace` is false and a validator with the same name already
    /// exists, returns [`DbError::Validation`].
    pub async fn create_validator_from_wasm(
        &self,
        name: &str,
        wasm: &[u8],
        replace: bool,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Wasm(wasm), replace, Actor::System)
            .await
    }

    /// Like [`create_validator_from_wasm`] but with an explicit [`Actor`].
    pub async fn create_validator_from_wasm_as(
        &self,
        name: &str,
        wasm: &[u8],
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Wasm(wasm), replace, actor)
            .await
    }

    /// Compile a Rust source string to WASM and register the validator.
    ///
    /// Returns the `RecordId` assigned to the new validator.
    pub async fn create_validator_from_source(
        &self,
        name: &str,
        source: &str,
        replace: bool,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Source(source), replace, Actor::System)
            .await
    }

    /// Like [`create_validator_from_source`] but with an explicit [`Actor`].
    pub async fn create_validator_from_source_as(
        &self,
        name: &str,
        source: &str,
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Source(source), replace, actor)
            .await
    }

    /// Internal: compile/validate WASM, persist, register.
    async fn create_validator_inner(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        let (wasm, lang_tag, source_str) = match source {
            FunctionSource::Wasm(bytes) => (bytes.to_vec(), "wasm", None),
            FunctionSource::Source(src) => {
                let compiled = compile_rust_source(src).map_err(|e| match e {
                    FunctionError::ToolchainUnavailable(msg) => {
                        DbError::Validation(format!("toolchain unavailable: {}", msg))
                    }
                    other => DbError::Validation(other.to_string()),
                })?;
                (compiled, "rust", Some(src.to_string()))
            }
        };

        // Validate the wasm by compiling it.
        let wf = WasmFunction::from_binary(self.wasm_engine.clone(), &wasm, WasmLimits::default())
            .map_err(|e| DbError::Validation(e.to_string()))?;

        let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm);
        let wasm_hash = format!("{:016x}", fxhash::hash64(&wasm));

        if !replace && self.validators.id_for_name(name).is_some() {
            return Err(DbError::Validation(format!(
                "validator '{}' already exists",
                name
            )));
        }

        // Determine the RecordId: on replace, reuse existing; otherwise new.
        let id = if replace {
            self.validators.id_for_name(name).unwrap_or_default()
        } else {
            RecordId::new()
        };

        let mut m = shamir_types::types::common::new_map();
        m.insert(
            "name".to_string(),
            shamir_types::types::value::QueryValue::Str(name.to_string()),
        );
        m.insert(
            "_id".to_string(),
            shamir_types::types::value::QueryValue::Str(id.to_string()),
        );
        m.insert(
            "wasm_b64".to_string(),
            shamir_types::types::value::QueryValue::Str(wasm_b64),
        );
        m.insert(
            "wasm_hash".to_string(),
            shamir_types::types::value::QueryValue::Str(wasm_hash),
        );
        m.insert(
            "lang".to_string(),
            shamir_types::types::value::QueryValue::Str(lang_tag.to_string()),
        );
        m.insert(
            "source".to_string(),
            match source_str {
                Some(s) => shamir_types::types::value::QueryValue::Str(s),
                None => shamir_types::types::value::QueryValue::Null,
            },
        );
        m.insert(
            "bound_in".to_string(),
            shamir_types::types::value::QueryValue::List(vec![]),
        );
        // Phase 0 parity plumbing: tag the artifact origin. Today every row
        // persisted here is WASM; later phases introduce `Native` rows. The
        // field defaults to `Wasm` on read for any pre-existing row without
        // it (see `ArtifactKind::from_record`), so this is a no-op for
        // behaviour — purely additive catalogue metadata.
        m.insert(
            super::KIND_FIELD.to_string(),
            ArtifactKind::Wasm.as_query_value(),
        );
        let record = shamir_types::types::value::QueryValue::Map(m);
        // Persist before registering so a crash can't leave a live entry
        // without a catalogue record.
        self.system_store
            .save_validator(name, &record, &ResourceMeta::owned_by(actor))
            .await?;

        if replace {
            // Remove old entry if it exists; ignore errors.
            if let Some(old_id) = self.validators.id_for_name(name) {
                self.validators.remove(&old_id);
            }
        }
        // Wrap the compiled WASM function in WasmRecordValidator so the
        // registry stores Arc<dyn RecordValidator> uniformly.
        let wasm_rv = Arc::new(WasmRecordValidator::new(Arc::new(wf))) as Arc<dyn RecordValidator>;
        self.validators
            .register(id, name, wasm_rv)
            .map_err(|e| DbError::Validation(e.to_string()))?;

        Ok(id)
    }

    /// Drop a validator by name.
    ///
    /// Returns `Ok(true)` if the validator existed and was removed.
    /// Returns `Err` if the validator is still bound to tables.
    pub async fn drop_validator(&self, name: &str) -> DbResult<bool> {
        self.drop_validator_as(name, Actor::System).await
    }

    /// Like [`drop_validator`] but with an explicit [`Actor`].
    pub async fn drop_validator_as(&self, name: &str, _actor: Actor) -> DbResult<bool> {
        let id = match self.validators.id_for_name(name) {
            Some(id) => id,
            None => return Ok(false),
        };

        // Refuse if bound.
        if self.validators.is_bound(&id) {
            let tables = self.validators.bound_tables(&id);
            return Err(DbError::Validation(format!(
                "cannot drop validator '{}': still bound to tables: {}",
                name,
                tables.join(", ")
            )));
        }

        let existed = self.validators.remove(&id);
        // Best-effort catalogue removal.
        if let Err(e) = self.system_store.remove_validator(name).await {
            log::warn!(
                "shamir_db::drop_validator: failed to remove '{}' from catalogue: {}",
                name,
                e
            );
        }
        Ok(existed)
    }

    /// Rename a validator. The underlying WASM module is not recompiled.
    /// Id and bindings are unchanged.
    pub async fn rename_validator(&self, from: &str, to: &str) -> DbResult<()> {
        self.rename_validator_as(from, to, Actor::System).await
    }

    /// Like [`rename_validator`] but with an explicit [`Actor`].
    pub async fn rename_validator_as(&self, from: &str, to: &str, _actor: Actor) -> DbResult<()> {
        // Load the old catalogue record to re-key it.
        let old_record = self.system_store.load_validator(from).await?;

        // Rename in the live registry first.
        self.validators
            .rename(from, to)
            .map_err(|e| DbError::Validation(e.to_string()))?;

        // If there was a durable record, re-key it, preserving the
        // existing owner/group/mode.
        if let Some(mut rec) = old_record {
            let existing_meta = ResourceMeta::from_record(&rec);
            self.system_store.remove_validator(from).await?;
            if let shamir_types::types::value::QueryValue::Map(ref mut map) = rec {
                map.insert(
                    "name".to_string(),
                    shamir_types::types::value::QueryValue::Str(to.to_string()),
                );
            }
            self.system_store
                .save_validator(to, &rec, &existing_meta)
                .await?;
        }

        Ok(())
    }

    /// Resolve a validator name to its `RecordId`.
    pub fn validator_id(&self, name: &str) -> Option<RecordId> {
        self.validators.id_for_name(name)
    }

    /// List all registered validators as `(id, name)` pairs.
    pub fn list_validators(&self) -> Vec<(RecordId, String)> {
        self.validators.list()
    }

    /// List all registered validators as `(id, name, kind)` triples.
    ///
    /// The `kind` is resolved per-validator against the persisted catalogue
    /// record (keyed by `name`): a validator whose row carries `kind = Native`
    /// reports [`ArtifactKind::Native`]; everything else reports
    /// [`ArtifactKind::Wasm`] (the historical default for any pre-Phase-0 row
    /// without an explicit `kind` field — see [`ArtifactKind::from_record`]).
    ///
    /// This is the introspection accessor behind the `list validators` wire
    /// surface; the catalogue is the source of truth for `kind`. If a
    /// catalogue lookup fails transiently the validator still appears, with
    /// `kind` defaulting to `Wasm` — the listing never silently drops entries.
    pub async fn list_validators_with_kind(
        &self,
    ) -> DbResult<Vec<(RecordId, String, ArtifactKind)>> {
        let entries = self.validators.list();
        let mut out = Vec::with_capacity(entries.len());
        for (id, name) in entries {
            let kind = match self.system_store.load_validator(&name).await? {
                Some(rec) => ArtifactKind::from_record(&rec),
                None => ArtifactKind::Wasm,
            };
            out.push((id, name, kind));
        }
        Ok(out)
    }

    // ========================================================================
    // Validator binding API (S2)
    // ========================================================================

    /// Bind a validator to a table on specified write operations.
    ///
    /// The validator must already exist in the registry (created via
    /// `create_validator_from_wasm` / `create_validator_from_source`).
    /// `priority` must be in `[1000, 9999]`, `ops` must be non-empty.
    /// Bind is idempotent: re-binding the same validator updates its
    /// `ops` and `priority`.
    pub async fn bind_validator(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
        ops: Vec<WriteOp>,
        priority: u16,
    ) -> DbResult<()> {
        self.bind_validator_as(
            db_name,
            repo_name,
            table_name,
            validator_name,
            ops,
            priority,
            Actor::System,
        )
        .await
    }

    /// Like [`bind_validator`] but with an explicit [`Actor`].
    #[allow(clippy::too_many_arguments)]
    pub async fn bind_validator_as(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
        ops: Vec<WriteOp>,
        priority: u16,
        _actor: Actor,
    ) -> DbResult<()> {
        // 1. Resolve name → id.
        let validator_id = self.validators.id_for_name(validator_name).ok_or_else(|| {
            DbError::Validation(format!("validator '{}' not found", validator_name))
        })?;

        // 2. Validate priority range.
        if !(1000..=9999).contains(&priority) {
            return Err(DbError::Validation(format!(
                "validator priority must be in [1000, 9999], got {}",
                priority
            )));
        }

        // 3. Validate ops non-empty.
        if ops.is_empty() {
            return Err(DbError::Validation(
                "validator binding ops must be non-empty".to_string(),
            ));
        }

        // 4. Get the TableManager.
        let table = self.get_table(db_name, repo_name, table_name).await?;

        // 5. Add the binding to the table's info-twin.
        let binding = ValidatorBinding {
            validator_id,
            ops: ops.into(),
            priority,
        };
        table.add_validator_binding(binding).await?;

        // 6. Update the global registry's bound_in tracking.
        let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
        self.validators.add_binding(&validator_id, &table_ref);

        // 7. Persist bound_in in the validator catalogue record.
        self.persist_validator_bound_in(validator_name, &validator_id)
            .await;

        Ok(())
    }

    /// Unbind a validator from a table.
    ///
    /// Returns `Ok(true)` if the binding existed and was removed.
    pub async fn unbind_validator(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
    ) -> DbResult<bool> {
        self.unbind_validator_as(
            db_name,
            repo_name,
            table_name,
            validator_name,
            Actor::System,
        )
        .await
    }

    /// Like [`unbind_validator`] but with an explicit [`Actor`].
    pub async fn unbind_validator_as(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
        _actor: Actor,
    ) -> DbResult<bool> {
        let validator_id = self.validators.id_for_name(validator_name).ok_or_else(|| {
            DbError::Validation(format!("validator '{}' not found", validator_name))
        })?;

        let table = self.get_table(db_name, repo_name, table_name).await?;
        let removed = table.remove_validator_binding(&validator_id).await?;

        if removed {
            let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
            self.validators.remove_binding(&validator_id, &table_ref);

            // Persist bound_in update in the validator catalogue.
            self.persist_validator_bound_in(validator_name, &validator_id)
                .await;
        }

        Ok(removed)
    }

    /// List the validator bindings for a specific table.
    pub async fn list_validator_bindings(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<Vec<ValidatorBinding>> {
        let table = self.get_table(db_name, repo_name, table_name).await?;
        Ok((*table.validator_bindings()).clone())
    }

    /// Best-effort update of the `bound_in` array in the validator's
    /// catalogue record. Logged on failure — does not propagate errors
    /// (the live registry is the source of truth; the catalogue is
    /// durability insurance for `init` reload).
    pub(super) async fn persist_validator_bound_in(&self, name: &str, id: &RecordId) {
        let tables = self.validators.bound_tables(id);
        let bound_list: Vec<shamir_types::types::value::QueryValue> = tables
            .into_iter()
            .map(shamir_types::types::value::QueryValue::Str)
            .collect();

        if let Ok(Some(mut rec)) = self.system_store.load_validator(name).await {
            let existing_meta = ResourceMeta::from_record(&rec);
            if let shamir_types::types::value::QueryValue::Map(ref mut map) = rec {
                map.insert(
                    "bound_in".to_string(),
                    shamir_types::types::value::QueryValue::List(bound_list),
                );
            }
            if let Err(e) = self
                .system_store
                .save_validator(name, &rec, &existing_meta)
                .await
            {
                log::warn!(
                    "shamir_db::persist_validator_bound_in: failed to update '{}': {}",
                    name,
                    e
                );
            }
        }
    }
}
