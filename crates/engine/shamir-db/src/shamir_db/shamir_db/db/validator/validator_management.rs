use base64::Engine;
use std::sync::Arc;

use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::query::batch::BatchError;
use crate::shamir_db::execute::admin_db_schema_migration::{
    schema_migration_lifecycle_refusal_message, SchemaMigrationLifecycleGuard,
};
use crate::shamir_db::shamir_db::ReplicationControlGuard;
use crate::shamir_db::system_store::DatabaseIdentity;
use crate::{DbError, DbResult};
use shamir_engine::function::{
    compile_rust_source_blocking, create_shamir_source, AdmissionPolicy, CompileOptions,
    FunctionError, ModuleReport, TierMode, WasmFunction, WasmLimits,
};
use shamir_engine::validator::{
    NativeRecordValidator, RecordFields, RecordValidator, Validation, ValidatorBinding,
    ValidatorCtx, ValidatorRegistry, WasmRecordValidator, WriteOp,
};
use shamir_types::types::record_id::RecordId;

use crate::shamir_db::shamir_db::{ArtifactKind, FunctionSource, ShamirDb};

/// Target-admission mode of a validator registration replace or rename
/// (review finding 3). Every mode holds the replication-control lock from
/// the authoritative binding snapshot through persistence and the registry
/// swap, so binding additions/removals and registry name swaps share one
/// exclusion domain; the mode only chooses whether the migration fence of
/// the binding holders is reserved and, for migration-owned replay, which
/// identity is exempt from it.
pub(in crate::shamir_db) enum ValidatorTargetAdmission {
    /// Ordinary wire mutation: reserve EVERY binding holder.
    Fence,
    /// Migration-owned replay: reserve every holder except the migration's
    /// own database, whose fence the runner already holds (`None` exempts
    /// nothing — e.g. when the migration database has no cached identity).
    FenceExcept(Option<DatabaseIdentity>),
    /// Direct embedder API: exclusion without the fence, mirroring
    /// `bind_validator` / `unbind_validator`, which also skip the fence.
    ExclusionOnly,
}

/// Error of an admitted registration replace / rename: a fence refusal
/// naming the binding holder (stable code, same shape as bind/unbind), or
/// a plain database error.
pub(in crate::shamir_db) enum ValidatorRegistrationError {
    Fence {
        db: String,
        code: Option<&'static str>,
    },
    Db(DbError),
}

impl From<DbError> for ValidatorRegistrationError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl ValidatorRegistrationError {
    /// Shared wire mapping (one place): bind/unbind's refusal shape for the
    /// fence, the caller's `DbError` mapper otherwise.
    pub(in crate::shamir_db) fn into_batch_error(
        self,
        map_db_error: impl FnOnce(DbError) -> BatchError,
    ) -> BatchError {
        match self {
            Self::Fence {
                db,
                code: Some(code),
            } => BatchError::query_coded(
                "",
                code,
                format!(
                    "an application-schema migration holds the maintenance fence for \
                     database '{db}'; writes, DDL and stored-procedure calls are refused \
                     until it completes"
                ),
            ),
            Self::Fence { db, code: None } => BatchError::QueryError {
                alias: String::new(),
                message: format!("another database operation is in progress for '{db}'"),
                code: None,
            },
            Self::Db(error) => map_db_error(error),
        }
    }

    /// Shared direct-call mapping; `op` names the operation in the message.
    pub(in crate::shamir_db) fn into_db_error(self, op: &str) -> DbError {
        match self {
            Self::Fence { db, .. } => {
                DbError::Validation(schema_migration_lifecycle_refusal_message(op, &db))
            }
            Self::Db(error) => error,
        }
    }
}

/// A registration whose candidate artifact is fully built (compiled,
/// gate-checked, record fields known) and only awaits the locked commit.
struct PreparedValidatorRegistration {
    name: String,
    lang: &'static str,
    source: Option<String>,
    wasm_b64: Option<String>,
    module_hash: Option<String>,
    kind: ArtifactKind,
    adapter: Arc<dyn RecordValidator>,
    actor: Actor,
    replace: bool,
}

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
        let adapter = Arc::new(NativeRecordValidator::new(validator)) as Arc<dyn RecordValidator>;
        let prepared = PreparedValidatorRegistration {
            name: name.to_string(),
            lang: "rust",
            source: None,
            wasm_b64: None,
            module_hash: None,
            kind: ArtifactKind::Native,
            adapter,
            actor,
            replace,
        };
        self.commit_validator_registration(prepared, ValidatorTargetAdmission::ExclusionOnly)
            .await
            .map_err(|e| e.into_db_error("register"))
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

    /// Compile `shamir` language source and register the validator.
    pub async fn create_validator_from_shamir_source(
        &self,
        name: &str,
        source: &str,
        replace: bool,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Shamir(source), replace, Actor::System)
            .await
    }

    /// Like [`create_validator_from_shamir_source`] but with an explicit [`Actor`].
    pub async fn create_validator_from_shamir_source_as(
        &self,
        name: &str,
        source: &str,
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Shamir(source), replace, actor)
            .await
    }

    /// Internal: compile/validate WASM (no exclusion held), then commit the
    /// registration under one replication-control acquisition.
    async fn create_validator_inner(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        self.create_validator_admitted(
            name,
            source,
            replace,
            actor,
            ValidatorTargetAdmission::ExclusionOnly,
        )
        .await
        .map_err(|e| e.into_db_error("register"))
    }

    /// Admission-carrying registration used by the wire handlers (review
    /// finding 3): the candidate artifact is fully built BEFORE any lock is
    /// taken, then ONE replication-control acquisition spans the
    /// authoritative binding snapshot, the target reservations and the
    /// persist + registry swap, so a concurrent bind/unbind cannot slip
    /// between snapshot and swap.
    pub(in crate::shamir_db) async fn create_validator_admitted(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        replace: bool,
        actor: Actor,
        admission: ValidatorTargetAdmission,
    ) -> Result<RecordId, ValidatorRegistrationError> {
        let prepared = self
            .prepare_validator_registration(name, source, actor, replace)
            .await?;
        let control = self.lock_replication_control().await;
        self.commit_validator_registration_locked(prepared, &admission, &control)
            .await
    }

    /// Compile/validate the candidate without any exclusion held.
    async fn prepare_validator_registration(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        actor: Actor,
        replace: bool,
    ) -> DbResult<PreparedValidatorRegistration> {
        // `validator_ready` is Some for shamir sources: the service already
        // built and gate-checked the validator function, so rebuilding from
        // bytes would compile twice under two gate acquisitions.
        let mut validator_ready: Option<(WasmFunction, ModuleReport)> = None;
        let (wasm, lang_tag, source_str) = match source {
            FunctionSource::Wasm(bytes) => (bytes.to_vec(), "wasm", None),
            FunctionSource::Source(src) => {
                // Compiling Rust source runs a host compiler process — gate it
                // (mirrors the function_management WasmCompiler check).
                self.authorize_access(&actor, &ResourcePath::WasmCompiler, Action::Execute)
                    .await
                    .map_err(|e| DbError::Validation(e.to_string()))?;
                // Reject oversize source before spawning the compiler.
                let limit = self.wasm_engine.admission_limits().max_source_bytes;
                if src.len() > limit {
                    return Err(DbError::Validation(format!(
                        "source is {} bytes, limit is {}",
                        src.len(),
                        limit
                    )));
                }
                let compiled = compile_rust_source_blocking(src)
                    .await
                    .map_err(|e| match e {
                        FunctionError::ToolchainUnavailable(msg) => {
                            DbError::Validation(format!("toolchain unavailable: {}", msg))
                        }
                        other => DbError::Validation(other.to_string()),
                    })?;
                (compiled, "rust", Some(src.to_string()))
            }
            FunctionSource::Shamir(src) => {
                // Compiling shamir source is in-process, but the same
                // WasmCompiler Execute gate applies (mirrors the
                // function_management check).
                self.authorize_access(&actor, &ResourcePath::WasmCompiler, Action::Execute)
                    .await
                    .map_err(|e| DbError::Validation(e.to_string()))?;
                // Reject oversize source before compiling.
                let limit = self.wasm_engine.admission_limits().max_source_bytes;
                if src.len() > limit {
                    return Err(DbError::Validation(format!(
                        "source is {} bytes, limit is {}",
                        src.len(),
                        limit
                    )));
                }
                let (wf, wasm, report, _signature) = create_shamir_source(
                    self.wasm_engine.clone(),
                    src.to_string(),
                    CompileOptions::default(),
                    WasmLimits::default(),
                    TierMode::Auto,
                )
                .await
                .map_err(|e| DbError::Validation(e.to_string()))?;
                validator_ready = Some((wf, report));
                (wasm, "shamir", Some(src.to_string()))
            }
        };

        // Validate the wasm by compiling it (on the blocking thread pool).
        let (wf, report) = match validator_ready {
            Some(pair) => pair,
            None => WasmFunction::from_binary_admitted_blocking(
                self.wasm_engine.clone(),
                &wasm,
                WasmLimits::default(),
                TierMode::Auto,
                AdmissionPolicy::Admission(*self.wasm_engine.admission_limits()),
            )
            .await
            .map_err(|e| DbError::Validation(e.to_string()))?,
        };

        let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm);

        // Wrap the compiled WASM function in WasmRecordValidator so the
        // registry stores Arc<dyn RecordValidator> uniformly.
        let adapter = Arc::new(WasmRecordValidator::new(Arc::new(wf))) as Arc<dyn RecordValidator>;
        Ok(PreparedValidatorRegistration {
            name: name.to_string(),
            lang: lang_tag,
            source: source_str,
            wasm_b64: Some(wasm_b64),
            module_hash: Some(report.hash.to_hex()),
            kind: ArtifactKind::Wasm,
            adapter,
            actor,
            replace,
        })
    }

    /// Take the replication-control lock and run the locked commit. The
    /// caller never holds the lock across candidate compilation.
    async fn commit_validator_registration(
        &self,
        prepared: PreparedValidatorRegistration,
        admission: ValidatorTargetAdmission,
    ) -> Result<RecordId, ValidatorRegistrationError> {
        let control = self.lock_replication_control().await;
        self.commit_validator_registration_locked(prepared, &admission, &control)
            .await
    }

    /// Locked commit: authoritative replacement snapshot, target admission
    /// of every binding holder, then persist and registry swap — all under
    /// the replication-control lock the caller holds (`_control` proves it;
    /// nothing inside reacquires).
    async fn commit_validator_registration_locked(
        &self,
        prepared: PreparedValidatorRegistration,
        admission: &ValidatorTargetAdmission,
        _control: &ReplicationControlGuard<'_>,
    ) -> Result<RecordId, ValidatorRegistrationError> {
        // Namespace gate (brief 84): refuse while a validator-rename
        // migration is in progress. Sole exemption: a migration-owned
        // FRESH create (its `FenceExcept` admission proves it); a replace
        // never bypasses the gate (HMAC replace stays forbidden upstream).
        match admission {
            ValidatorTargetAdmission::FenceExcept(_) if !prepared.replace => {}
            _ => self.refuse_concurrent_validator_namespace_mutation("create")?,
        }
        // Under the caller's lock: a non-replace must not overwrite an
        // existing durable/live name with a fresh id (the unlocked native /
        // wasm prechecks were moved here).
        if !prepared.replace && self.validators.id_for_name(&prepared.name).is_some() {
            return Err(DbError::Validation(format!(
                "validator '{}' already exists",
                prepared.name
            ))
            .into());
        }
        let (id, bound_in, restore_pending) = self
            .validator_replacement_identity(&prepared.name, prepared.replace)
            .await?;

        let _guards = match admission {
            ValidatorTargetAdmission::ExclusionOnly => None,
            mode => {
                if !prepared.replace {
                    // A fresh ID has no bindings to consult.
                    None
                } else {
                    let exempt = match mode {
                        ValidatorTargetAdmission::FenceExcept(exempt) => *exempt,
                        _ => None,
                    };
                    // AUTHORITATIVE (round 3, findings 1+2): derive the
                    // holder set from the tables' own persisted bindings,
                    // never from the best-effort reverse snapshot in
                    // `bound_in`.
                    let holders = self.validator_binding_holders(&id, _control).await?;
                    Some(self.reserve_validator_target_identities_locked(
                        holders.dbs(),
                        exempt,
                        _control,
                    )?)
                }
            }
        };

        // Build the catalogue row from the authoritative snapshot; the
        // prepared fields are moved in (no wasm byte clones).
        let PreparedValidatorRegistration {
            name,
            lang,
            source,
            wasm_b64,
            module_hash,
            kind,
            adapter,
            actor,
            replace,
        } = prepared;
        let mut m = shamir_types::types::common::new_map();
        m.insert(
            "name".to_string(),
            shamir_types::types::value::QueryValue::Str(name.clone()),
        );
        m.insert(
            "_id".to_string(),
            shamir_types::types::value::QueryValue::Str(id.to_string()),
        );
        if let Some(wasm_b64) = wasm_b64 {
            m.insert(
                "wasm_b64".to_string(),
                shamir_types::types::value::QueryValue::Str(wasm_b64),
            );
        }
        if let Some(module_hash) = module_hash {
            m.insert(
                "module_hash".to_string(),
                shamir_types::types::value::QueryValue::Str(module_hash),
            );
        }
        m.insert(
            "lang".to_string(),
            shamir_types::types::value::QueryValue::Str(lang.to_string()),
        );
        m.insert(
            "source".to_string(),
            match source {
                Some(s) => shamir_types::types::value::QueryValue::Str(s),
                None => shamir_types::types::value::QueryValue::Null,
            },
        );
        m.insert(
            "bound_in".to_string(),
            shamir_types::types::value::QueryValue::List(
                bound_in
                    .iter()
                    .cloned()
                    .map(shamir_types::types::value::QueryValue::Str)
                    .collect(),
            ),
        );
        // Phase 0 parity plumbing: tag the artifact origin. The field
        // defaults to `Wasm` on read for any pre-existing row without it
        // (see `ArtifactKind::from_record`).
        m.insert(
            crate::shamir_db::shamir_db::KIND_FIELD.to_string(),
            kind.as_query_value(),
        );
        let record = shamir_types::types::value::QueryValue::Map(m);
        // Persist before registering so a crash can't leave a live entry
        // without a catalogue record.
        self.system_store
            .save_validator(&name, &record, &ResourceMeta::owned_enforced(actor))
            .await?;

        if replace {
            // Restore pending catalogue bindings before publishing the
            // artifact, not after it is live.
            if restore_pending {
                for table in &bound_in {
                    self.validators.add_binding(&id, table);
                }
            }
            self.validators.replace(id, &name, adapter)
        } else {
            self.validators.register(id, &name, adapter)
        }
        .map_err(|e| DbError::Validation(e.to_string()))?;

        Ok(id)
    }

    /// Reserve the migration fence of every named binding holder — caller
    /// holds replication control (no reacquire), identities are captured,
    /// reserved and re-verified under it, one at a time, fail-fast in the
    /// caller's stable sorted order.
    /// `pub(super)`: shared with `migration_admission.rs` (brief 84 item 3
    /// preflight) and `validator_drop.rs` — one reservation/error-mapping
    /// implementation, no drift.
    pub(in crate::shamir_db) fn reserve_validator_target_identities_locked<'a>(
        &'a self,
        dbs: &[String],
        exempt: Option<DatabaseIdentity>,
        _control: &ReplicationControlGuard<'_>,
    ) -> Result<Vec<SchemaMigrationLifecycleGuard<'a>>, ValidatorRegistrationError> {
        let mut guards = Vec::with_capacity(dbs.len());
        for db in dbs {
            let Some(id) = self.database_identity_for_fence(db) else {
                // Stale binding ref (e.g. a pre-fix rename): fail closed.
                return Err(ValidatorRegistrationError::Db(DbError::NotFound(format!(
                    "Database '{db}' not found"
                ))));
            };
            if Some(id) == exempt {
                continue;
            }
            let guard = self.reserve_admin_migration_target(id).map_err(|code| {
                ValidatorRegistrationError::Fence {
                    db: (*db).clone(),
                    code,
                }
            })?;
            if self.database_identity_for_fence(db) != Some(id) {
                return Err(ValidatorRegistrationError::Db(DbError::NotFound(format!(
                    "Database '{db}' not found"
                ))));
            }
            guards.push(guard);
        }
        Ok(guards)
    }

    /// Reuse live identity/bindings, or the pending native catalogue row on
    /// restart. Live bindings are authoritative; do not resurrect stale durable
    /// bindings after a live unbind. Only an absent-at-capture artifact needs
    /// binding restoration; live snapshots must never be replayed after await.
    /// This is not an async catalogue transaction: concurrent materialisation,
    /// unbinding or catalogue changes after an absent-id capture can still race
    /// pending restoration. Live catalogue rows can also retain a stale binding
    /// snapshot if binding DDL overlaps the save; live bindings are not replayed.
    async fn validator_replacement_identity(
        &self,
        name: &str,
        replace: bool,
    ) -> DbResult<(RecordId, Vec<String>, bool)> {
        if !replace {
            return Ok((RecordId::new(), Vec::new(), false));
        }
        if let Some(id) = self.validators.id_for_name(name) {
            return Ok((id, self.validators.bound_tables(&id), false));
        }
        if let Some(rec) = self.system_store.load_validator(name).await? {
            let id = rec
                .get("_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<RecordId>().ok())
                .ok_or_else(|| {
                    DbError::Validation(
                        format!("validator '{}' has an invalid catalogue id", name,),
                    )
                })?;
            let mut bound_in = self.validators.bound_tables(&id);
            if let Some(bound) = rec.get("bound_in").and_then(|v| v.as_array()) {
                for table in bound.iter().filter_map(|v| v.as_str()) {
                    if !bound_in.iter().any(|existing| existing == table) {
                        bound_in.push(table.to_string());
                    }
                }
            }
            return Ok((id, bound_in, true));
        }
        Ok((RecordId::new(), Vec::new(), false))
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
        let records = self.system_store.load_validators().await?;
        let mut kinds = shamir_types::types::common::new_map();
        for record in &records {
            if let Some(name) = record.get("name").and_then(|value| value.as_str()) {
                kinds
                    .entry(name.to_string())
                    .or_insert_with(|| ArtifactKind::from_record(record));
            }
        }
        let mut out = Vec::with_capacity(entries.len());
        for (id, name) in entries {
            let kind = kinds.get(&name).copied().unwrap_or(ArtifactKind::Wasm);
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
        // Review finding 3: binding additions share the replication-control
        // exclusion domain with the replace/rename locked commit, so a
        // concurrent registration commit can never snapshot a half-landed
        // binding.
        let _control = self.lock_replication_control().await;
        // Namespace gate (brief 84): a NEW bind must refuse while a
        // validator-rename migration is in progress (it would add a holder
        // the preflight never reserved); direct binds check it too — a
        // conservative conflict. Unbind deliberately does not.
        self.refuse_concurrent_validator_namespace_mutation("bind")?;
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
        // Review finding 3: see bind_validator_as — one exclusion domain for
        // binding removals and registration commits.
        let _control = self.lock_replication_control().await;
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
    /// Scoped to the moved `db` module tree: only the bind/unbind paths
    /// here and `drop_table_cleaning_validators_locked` in
    /// `table_management.rs` call it.
    pub(in crate::shamir_db::shamir_db::db) async fn persist_validator_bound_in(
        &self,
        name: &str,
        id: &RecordId,
    ) {
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
