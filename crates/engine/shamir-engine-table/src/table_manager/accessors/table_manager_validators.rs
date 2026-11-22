use std::sync::atomic::Ordering;
use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult};
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use shamir_engine_repo::RepoWritePermit;
use shamir_engine_validator::validator::{
    record_fields::{OwnedFields, ViewFields},
    record_validator::ValidatorCtx,
    TransformSpec, ValidatorFailure, WriteOp,
};

use crate::validator_bound::{ValidatorDb, ValidatorFkCtx};
use shamir_engine_validator::validator::record_fields::RecordFields;
use shamir_engine_validator::validator::schema::ForeignKeyRef;
use shamir_engine_validator::validator::{ValidatorBinding, ValidatorRegistry};

use crate::table_manager::TableManager;

/// Fixed refusal text for a table whose validator binding record is IN DOUBT.
/// Deliberately carries NO table path: the doubt may be reported to an actor
/// that is not allowed to read the table, and the state to report is that, not
/// a machine path. Round 7: this is the ONE text — `binding_in_doubt_error()` is
/// its only construction site, so the table's own bind/unbind, the admitted
/// validator replace/rename/drop and the facade refusals (schema activation,
/// table rename) cannot drift apart.
///
/// The flag behind it expresses UNCERTAINTY, not guaranteed exclusion: it is
/// raised when the forward `set` fails (before any flush), and when the future
/// is dropped at the forward `set`, at the forward flush or at a compensation
/// step — cancellation is itself a source of doubt. There is no suspension
/// point between a successful flush result and the publication that follows, so
/// a completed flush cannot be cancelled before the live snapshot catches up;
/// the cancellable case is a backing flush that took effect while the future was
/// still pending.
const BINDING_IN_DOUBT: &str =
    "validator binding state in doubt: a failed binding write could not be rolled back to the previous persisted record; reads and writes continue, validator binding DDL on this table is refused until the process restarts and re-reads the persisted binding record";

/// Attempts for the compensating `set` + `flush` pair that restores the
/// previous logical envelope after a failed binding write. Deliberately no
/// sleep between attempts: a delay would only widen the window in which the
/// durable record is unknown. Three immediate attempts are a POLICY, not
/// evidence about the fault: they prove neither that a flush fault is
/// persistent nor that it is already gone, which is exactly why a still-failing
/// compensation leaves the table in doubt instead of returning the original
/// error.
const COMPENSATION_ATTEMPTS: usize = 3;

impl TableManager {
    /// Cheap snapshot of the current validator bindings for this table.
    /// The S3 write path reads this on every write.
    pub fn validator_bindings(&self) -> Arc<Vec<ValidatorBinding>> {
        self.validator_bindings.load_full()
    }

    /// Borrow the global validator registry wired into this table, if any.
    /// `None` for system tables / tables without validators. Used by Phase D.2
    /// (SET NULL nullable enforcement) to inspect `SchemaValidator` rules.
    pub fn validator_registry_ref(&self) -> Option<&Arc<ValidatorRegistry>> {
        self.validator_registry.as_ref()
    }

    /// Append (or replace if same `validator_id`) a validator binding,
    /// persist to the info-twin, and swap the in-memory snapshot.
    ///
    /// Idempotent. Rare administrative metadata writes are flushed before
    /// return so independently opened read handles observe the new binding.
    ///
    /// Guarantee after round 6 (F1/F2), wording tightened in round 7: on
    /// success the live snapshot and the FLUSHED durable envelope both carry
    /// the new binding. On a failed write or flush the previous LOGICAL
    /// envelope — the FULL binding list, ops and priority included, under a new
    /// timestamp, not the old bytes — is written back AND flushed, with a
    /// bounded number of immediate attempts, and the bind is reported as not
    /// done. A record that did not exist before is restored as a PRESENT-EMPTY
    /// record, not as an absent one. If that compensation cannot be written or
    /// flushed (either failure is enough after the last attempt; the attempt
    /// count is policy, not evidence), the table is left IN DOUBT: the caller
    /// gets the fixed in-doubt error, this table's bind/unbind refuse, the
    /// holder scan treats the table as holding every validator identity, and the
    /// facade refusals that consult it (validator replace/rename/drop, schema
    /// activation on this table, rename of this table) refuse too — until a
    /// restart re-reads the envelope, which RESETS the flag but does not repair
    /// the records.
    ///
    /// cancel-safe: NO — cancellation inside the write → flush → publish
    /// window leaves the in-doubt flag raised for the rest of the process, so
    /// the table fails closed rather than trusting a durable record it could
    /// not reconcile.
    pub async fn add_validator_binding(
        &self,
        binding: ValidatorBinding,
    ) -> shamir_storage::error::DbResult<()> {
        // S5c-X: repo admission BEFORE `bindings_lock` (lock order: repo
        // permit FIRST → terminal `bindings_lock`).
        let permit = self.admit().await?;
        self.add_validator_binding_admitted(binding, &permit).await
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn add_validator_binding_admitted(
        &self,
        binding: ValidatorBinding,
        permit: &RepoWritePermit,
    ) -> shamir_storage::error::DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        self.ensure_attached()?;
        let _guard = self.bindings_lock.lock().await;
        self.refuse_bindings_in_doubt()?;
        let previous = self.validator_bindings.load_full();
        let mut bindings = (*previous).clone();
        if let Some(pos) = bindings
            .iter()
            .position(|b| b.validator_id == binding.validator_id)
        {
            bindings[pos] = binding;
        } else {
            bindings.push(binding);
        }
        // Round 6 (F2): the durable envelope and the live snapshot must never
        // be observed disagreeing. The shared in-doubt flag is raised for the
        // whole write → flush → publish window, so the window declares itself:
        // if this future is cancelled anywhere inside it, or the compensation
        // below cannot restore the previous record, the flag stays set and the
        // table fails closed until a restart re-reads the envelope. It is
        // cleared only after the live snapshot matches the flushed record.
        self.bindings_in_doubt.store(true, Ordering::Release);
        if let Err(error) = self.persist_bindings(&bindings).await {
            // The live snapshot never changed, so no write ran against a
            // half-applied binding. Validator admission scans may open the same
            // info store through a different repository wrapper, so the
            // compensating write below is FLUSHED here, not left to a later
            // drain, and its result is never discarded.
            return Err(self.compensate_failed_binding_write(&previous, error).await);
        }
        self.validator_bindings.store(Arc::new(bindings));
        self.bindings_in_doubt.store(false, Ordering::Release);
        Ok(())
    }

    /// Remove a validator binding by `validator_id`. Persists, flushes and
    /// then swaps the in-memory snapshot. Returns whether the binding existed.
    ///
    /// Ordering after round 6 (F3): the live removal is published ONLY after
    /// the durable record is saved AND flushed. Publishing first let a
    /// concurrent write run without the validator for the whole flush window,
    /// and a failed flush cannot un-validate a write that already ran. The
    /// round-4 cancellation safety is preserved in the conservative direction:
    /// cancellation before the flush leaves the live binding in place (only
    /// the durable record may be ahead), and cancellation after the flush but
    /// before the publication leaves a live binding whose durable record is
    /// gone — the in-doubt flag raised for this window makes the holder scan
    /// treat the table as holding EVERY identity, so no foreign replace/drop
    /// can act on that split state, and a restart re-reads the envelope.
    ///
    /// On a failed write or flush the previous LOGICAL envelope — the FULL
    /// binding list, ops and priority included; a record that did not exist
    /// before is restored as a PRESENT-EMPTY record — is written back and
    /// flushed (bounded immediate attempts) and the unbind is reported as not
    /// done; a compensation that cannot be written or flushed leaves the table
    /// IN DOUBT with the fixed in-doubt error, as for a failed bind.
    pub async fn remove_validator_binding(
        &self,
        validator_id: &RecordId,
    ) -> shamir_storage::error::DbResult<bool> {
        // S5c-X: repo admission BEFORE `bindings_lock`.
        let permit = self.admit().await?;
        self.remove_validator_binding_admitted(validator_id, &permit)
            .await
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn remove_validator_binding_admitted(
        &self,
        validator_id: &RecordId,
        permit: &RepoWritePermit,
    ) -> shamir_storage::error::DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        self.ensure_attached()?;
        let _guard = self.bindings_lock.lock().await;
        self.refuse_bindings_in_doubt()?;
        let previous = self.validator_bindings.load_full();
        let mut bindings = (*previous).clone();
        let before = bindings.len();
        bindings.retain(|b| b.validator_id != *validator_id);
        let removed = bindings.len() < before;
        if !removed {
            return Ok(false);
        }
        // See the doc comment: publish only after a successful flush, and
        // declare the window in doubt so cancellation inside it cannot hide a
        // durable/live split from the holder scan.
        self.bindings_in_doubt.store(true, Ordering::Release);
        if let Err(error) = self.persist_bindings(&bindings).await {
            return Err(self.compensate_failed_binding_write(&previous, error).await);
        }
        self.validator_bindings.store(Arc::new(bindings));
        self.bindings_in_doubt.store(false, Ordering::Release);
        Ok(true)
    }

    /// Whether this table's persisted validator binding record is IN DOUBT
    /// (round 6): set while a binding write is unreconciled, and left set when
    /// its compensation could not be written and flushed. Read by bind/unbind
    /// (refusal), by the holder scan and by the facade refusals (schema
    /// activation, table rename) via the repository; never on the write path.
    ///
    /// It expresses UNCERTAINTY, not guaranteed exclusion: the backing record
    /// may be the previous one, the new one or a partial compensating write, and
    /// ordinary validated reads and writes do NOT consult it — they run against
    /// the live snapshot, which is unchanged on this path, so a concurrent or
    /// later write is neither blocked nor re-validated.
    pub fn validator_bindings_in_doubt(&self) -> bool {
        self.bindings_in_doubt.load(Ordering::Acquire)
    }

    /// The fixed, path-free in-doubt refusal as a typed error (round 7). One
    /// construction site for every caller that must refuse on a doubtful binding
    /// state — this table's own bind/unbind, the admitted validator holder
    /// decisions (replace/rename/drop, built by `BindingHolders::refuse_in_doubt`
    /// in `shamir-db`) and the facade DDL that runs before touching a table
    /// (schema activation, table rename) — so the text cannot drift between
    /// them.
    pub fn binding_in_doubt_error() -> DbError {
        DbError::Conflict(BINDING_IN_DOUBT.to_string())
    }

    /// Fixed in-doubt refusal (round 6, F2a): bind/unbind on a doubtful table
    /// stay refused until a restart re-reads the persisted record.
    fn refuse_bindings_in_doubt(&self) -> DbResult<()> {
        if self.validator_bindings_in_doubt() {
            Err(Self::binding_in_doubt_error())
        } else {
            Ok(())
        }
    }

    /// Test-only: raise the in-doubt flag directly, so facade-level tests can
    /// prove the refusal paths without injecting a storage fault through the
    /// whole database. Production has exactly one writer of this flag: the
    /// binding DDL window of this manager.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn mark_validator_bindings_in_doubt(&self) {
        self.bindings_in_doubt.store(true, Ordering::Release);
    }

    /// The durable half of a binding DDL step: write the envelope, then flush
    /// it. One attempt; the caller decides what a failure means.
    async fn persist_bindings(&self, bindings: &[ValidatorBinding]) -> DbResult<()> {
        crate::validator_bound::persistence::save_validators_metadata(bindings, &self.info_store)
            .await?;
        self.info_store.flush().await
    }

    /// Restore the previous logical envelope and FLUSH it, up to
    /// [`COMPENSATION_ATTEMPTS`] times. The result is never ignored.
    async fn restore_bindings(&self, previous: &[ValidatorBinding]) -> DbResult<()> {
        let mut last = None;
        for _ in 0..COMPENSATION_ATTEMPTS {
            match self.persist_bindings(previous).await {
                Ok(()) => return Ok(()),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| {
            DbError::Internal("binding compensation retry exhausted".to_string())
        }))
    }

    /// Compensation after a failed binding write (round 6, F1/F2). The caller
    /// keeps the live snapshot unchanged; this restores and flushes the
    /// previous logical envelope. Success reports the ORIGINAL error — the
    /// operation did not happen and the table stays coherent. Failure leaves
    /// the in-doubt flag SET (the caller raised it before writing) and reports
    /// the fixed in-doubt refusal, so neither this manager nor a holder scan
    /// can mistake the binding state for known.
    async fn compensate_failed_binding_write(
        &self,
        previous: &Arc<Vec<ValidatorBinding>>,
        error: DbError,
    ) -> DbError {
        if self.restore_bindings(previous).await.is_ok() {
            self.bindings_in_doubt.store(false, Ordering::Release);
            error
        } else {
            Self::binding_in_doubt_error()
        }
    }

    /// Inject the global validator registry (S3). Called by the facade
    /// when user tables are created/opened. System tables leave this at
    /// `None` (validators disabled).
    pub fn set_validator_registry(&mut self, registry: Arc<ValidatorRegistry>) {
        self.validator_registry = Some(registry);
    }

    /// Inject the per-DB scalar resolver (user + builtin layers). Called
    /// by `DbTableResolver::resolve` so that `create_index_v2` can access
    /// user-registered scalars for the `.trusted_pure()` index-safety gate,
    /// AND so that already-constructed `FunctionalBackend` instances (e.g.
    /// those rebuilt during reopen) can resolve user scalars at eval time.
    pub async fn set_scalar_resolver(
        &self,
        resolver: shamir_funclib::scalar_resolver::ScalarResolver,
    ) {
        self.scalar_resolver
            .store(std::sync::Arc::new(resolver.clone()));
        // Push the resolver down to every index2 backend that may need it
        // (FunctionalBackend with IndexExpr::Scalar). Other backends
        // (FTS, Vector, Btree) have a no-op default impl.
        let backends = self.index2_registry.all_backends().await;
        for b in &backends {
            b.update_scalar_resolver(&resolver);
        }
    }

    /// QueryValue-input entry point for the INSERT/UPDATE/UPSERT write paths.
    ///
    /// All write paths feed `QueryValue` directly: INSERT passes `resolved_values`
    /// from `write_exec.rs`; UPDATE/UPSERT merge at the `QueryValue` level in
    /// `write_exec.rs` before calling here. DELETE goes through
    /// `run_validators_view` (RecordView lens).
    ///
    /// Algorithm (per the VALIDATORS.md spec):
    /// 1. If no registry is set, return `Ok(())` (validators disabled).
    /// 2. Load bindings snapshot; filter to those whose `ops` contains `op`.
    /// 3. Delegate to `run_validators_loop` with `OwnedFields` backings.
    ///
    /// Phase C1: `tx` and `resolver` are threaded so a [`ValidatorDb`] can be
    /// built and attached to [`ValidatorCtx`], enabling relational validators
    /// (`foreign_key` C2, `unique` C3) to read database state on the tx's
    /// snapshot.  Both are optional (`None` on paths without a tx context —
    /// legacy callers, some tests).
    ///
    /// RI-7 (actor semantics): `actor` is the CALLER's identity, threaded from
    /// `execute_*_tx` (which in turn receives it from `QueryRunner.actor`).
    /// validator cross-table reads execute with the CALLER's privileges; a
    /// validator referencing a table the caller cannot read will reject the
    /// write. This is fail-closed (worst case: a legitimate write is rejected
    /// because the caller can't see a referenced table — a UX bug, not a
    /// security bug) and matches the codebase's "ambiguous actor → return
    /// caller, not System" convention.
    pub async fn run_validators_qv(
        &self,
        op: WriteOp,
        new_record: Option<&shamir_types::types::value::QueryValue>,
        old_record: Option<&shamir_types::types::value::QueryValue>,
        actor: &shamir_types::access::Actor,
        tx: Option<&shamir_tx::TxContext>,
        fk_ctx: ValidatorFkCtx<'_>,
    ) -> Result<(), ValidatorFailure> {
        // 1. No registry → validators disabled (system tables / tests).
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Ok(()),
        };

        // Fast skip: a short-lived guard (no Arc clone) for the common "no validators
        // bound" case. The snapshot used for invocation below is an owned Arc, because
        // the validators are awaited and an arc-swap guard must not live across `.await`.
        if self.validator_bindings.load().is_empty() {
            return Ok(());
        }
        let all_bindings = self.validator_bindings.load_full();

        // 2. Filter the snapshot to applicable ops.
        let applicable: Vec<&ValidatorBinding> = all_bindings
            .iter()
            .filter(|b| b.ops.contains(&op))
            .collect();

        if applicable.is_empty() {
            return Ok(());
        }

        // 3. Get the interner for ValidatorCtx.
        let interner = self
            .interner()
            .get()
            .await
            .map_err(|e| ValidatorFailure::Invocation {
                id: applicable[0].validator_id,
                reason: format!("interner load failed: {e}"),
            })?;

        // 4. Build OwnedFields backings (transitional — INSERT/UPDATE already
        //    have QueryValue; ViewFields will replace this when the write path
        //    exposes RecordView directly).
        let new_fields = new_record.map(|qv| OwnedFields { qv });
        let old_fields = old_record.map(|qv| OwnedFields { qv });

        let new_dyn: Option<&dyn RecordFields> =
            new_fields.as_ref().map(|f| f as &dyn RecordFields);
        let old_dyn: Option<&dyn RecordFields> =
            old_fields.as_ref().map(|f| f as &dyn RecordFields);

        // Phase B — attach the scalar resolver so scalar-bridge rules can
        // resolve registered scalars.  The resolver is loaded once per call
        // (cheap ArcSwap load_full → Arc clone).
        //
        // Phase C1 — attach a [`ValidatorDb`] when `tx` is available so
        // relational validators (`foreign_key`, `unique`) can read database
        // state on the tx's snapshot.  When `tx` is `None` (no transaction
        // context), `db` stays `None` and relational rules silently skip
        // (fail-open, matching the scalar-bridge precedent).
        let scalar_res = self.scalar_resolver.load_full();
        if let Some(tx) = tx {
            let vdb = match fk_ctx.fk_cache {
                Some(cache) => ValidatorDb::new_with_fk_cache(tx, self, fk_ctx.resolver, cache),
                None => ValidatorDb::new(tx, self, fk_ctx.resolver),
            };
            let ctx = ValidatorCtx::with_db(actor, interner, scalar_res.as_ref(), &vdb);
            self.run_validators_loop(&applicable, reg, &ctx, new_dyn, old_dyn)
                .await
        } else {
            let ctx = ValidatorCtx::with_scalars(actor, interner, scalar_res.as_ref());
            self.run_validators_loop(&applicable, reg, &ctx, new_dyn, old_dyn)
                .await
        }
    }

    /// `RecordView`-input entry point for the DELETE write path (W6-delete).
    ///
    /// The delete path holds the raw storage bytes from the scan — no
    /// `InnerValue` tree is decoded on the index-planner path. This entry
    /// builds a [`ViewFields`] lens over `old_view` so native validators can
    /// probe fields by name WITHOUT a full de-intern.  WASM validators
    /// materialise the full record internally via `RecordFields::to_query_value`.
    ///
    /// Overlay-key resolution: the deleted record's fields are already in the
    /// base interner — they were committed when the record was inserted,
    /// before this delete. The tx overlay holds only brand-new field names
    /// staged in the CURRENT tx; a DELETE never introduces new fields.
    /// Base-only resolution via `ViewFields` is therefore correct and complete.
    ///
    /// `new_record` is `None` for Delete.
    ///
    /// Phase C1: `tx` (previously `_tx`) is now used to build a [`ValidatorDb`]
    /// for relational validators.
    pub(crate) async fn run_validators_view(
        &self,
        op: WriteOp,
        new_record: Option<&QueryValue>,
        old_view: Option<&RecordView<'_>>,
        actor: &shamir_types::access::Actor,
        tx: &shamir_tx::TxContext,
        fk_ctx: ValidatorFkCtx<'_>,
    ) -> Result<(), ValidatorFailure> {
        // 1. No registry → validators disabled.
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Ok(()),
        };

        // Fast skip: a short-lived guard (no Arc clone) for the common "no validators
        // bound" case. The snapshot used for invocation below is an owned Arc, because
        // the validators are awaited and an arc-swap guard must not live across `.await`.
        if self.validator_bindings.load().is_empty() {
            return Ok(());
        }
        let all_bindings = self.validator_bindings.load_full();

        // 2. Filter the snapshot to applicable ops.
        let applicable: Vec<&ValidatorBinding> = all_bindings
            .iter()
            .filter(|b| b.ops.contains(&op))
            .collect();

        if applicable.is_empty() {
            return Ok(());
        }

        let interner = self
            .interner()
            .get()
            .await
            .map_err(|e| ValidatorFailure::Invocation {
                id: applicable[0].validator_id,
                reason: format!("interner load failed: {e}"),
            })?;

        // 3. Build ViewFields lens (zero-copy over raw msgpack bytes) for the
        //    old record.  No full de-intern — native validators probe by name
        //    lazily; WASM materialises internally via to_query_value().
        let old_fields = old_view.map(|view| ViewFields::new(view, interner));
        let new_fields = new_record.map(|qv| OwnedFields { qv });

        let new_dyn: Option<&dyn RecordFields> =
            new_fields.as_ref().map(|f| f as &dyn RecordFields);
        let old_dyn: Option<&dyn RecordFields> =
            old_fields.as_ref().map(|f| f as &dyn RecordFields);

        // Phase B — attach the scalar resolver (same as the QV path).
        // Phase C1 — attach a [`ValidatorDb`] from `tx` for relational
        // validators (DELETE may have FK-referenced records, etc.).
        let scalar_res = self.scalar_resolver.load_full();
        let vdb = match fk_ctx.fk_cache {
            Some(cache) => ValidatorDb::new_with_fk_cache(tx, self, fk_ctx.resolver, cache),
            None => ValidatorDb::new(tx, self, fk_ctx.resolver),
        };
        let ctx = ValidatorCtx::with_db(actor, interner, scalar_res.as_ref(), &vdb);

        self.run_validators_loop(&applicable, reg, &ctx, new_dyn, old_dyn)
            .await
    }

    /// Per-validator invocation loop shared by `run_validators_qv` and
    /// [`run_validators_view`](Self::run_validators_view).
    ///
    /// Resolves each binding to a `RecordValidator`, calls `.validate(...)`,
    /// accumulates errors, and applies the stop-flag semantics.
    async fn run_validators_loop(
        &self,
        applicable: &[&ValidatorBinding],
        reg: &ValidatorRegistry,
        ctx: &ValidatorCtx<'_>,
        new: Option<&dyn RecordFields>,
        old: Option<&dyn RecordFields>,
    ) -> Result<(), ValidatorFailure> {
        // Sort by priority ascending, stable tie-break by id.
        let mut sorted = applicable.to_vec();
        sorted.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.validator_id.cmp(&b.validator_id))
        });

        let mut all_errors: Vec<shamir_query_types::validator::ValidationError> =
            Vec::with_capacity(sorted.len());

        for binding in &sorted {
            // Resolve validator_id → RecordValidator.
            let validator =
                reg.get_by_id(&binding.validator_id)
                    .ok_or(ValidatorFailure::Missing {
                        id: binding.validator_id,
                    })?;

            // Invoke the validator (async — WASM needs await for guest call).
            let result = validator.validate(new, old, ctx).await;

            // Detect WASM invocation errors encoded as sentinel codes.
            // WasmRecordValidator encodes errors as `__wasm_err:<reason>`.
            if result.stop && result.errors.len() == 1 {
                let code = &result.errors[0].code;
                if let Some(reason) = code.strip_prefix("__wasm_err:") {
                    return Err(ValidatorFailure::Invocation {
                        id: binding.validator_id,
                        reason: reason.to_string(),
                    });
                }
            }

            // Accumulate errors.
            all_errors.extend(result.errors);

            // On stop → break the loop.
            if result.stop {
                break;
            }
        }

        if all_errors.is_empty() {
            Ok(())
        } else {
            Err(ValidatorFailure::Failed(all_errors))
        }
    }

    /// Flush all metadata blobs (interner + counter) in one call.
    ///
    /// Replaces the repeated `self.interner().persist().await?` /
    /// `self.counter().persist().await?` pairs that used to appear after
    /// every write operation. Items that are not dirty short-circuit
    /// immediately; only genuinely changed blobs pay the I/O cost.
    pub async fn flush_metadata(&self) -> DbResult<()> {
        self.persist_registry.flush_all().await
    }

    /// Collect all foreign-key references declared by this table's validators.
    ///
    /// Iterates the table's validator bindings, resolves each to a compiled
    /// `RecordValidator` via the registry, and calls `fk_refs()` on each.
    /// Returns `(field_path, ForeignKeyRef)` pairs.
    ///
    /// Used by Phase D reverse-FK discovery (the RESTRICT gate needs to find
    /// which child tables reference a given parent table).
    pub fn collect_fk_refs(&self) -> Vec<(Vec<String>, ForeignKeyRef)> {
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Vec::new(),
        };
        let bindings = self.validator_bindings.load_full();
        let mut refs = Vec::new();
        for binding in bindings.iter() {
            if let Some(validator) = reg.get_by_id(&binding.validator_id) {
                refs.extend(validator.fk_refs());
            }
        }
        refs
    }

    /// Collect all literal-default rules declared by this table's bound
    /// validators.
    ///
    /// Mirrors [`collect_fk_refs`](Self::collect_fk_refs): iterates bindings,
    /// resolves each via the registry, calls `defaults()` on the compiled
    /// `RecordValidator`, and aggregates the `(field_path, default_value)`
    /// pairs.  Returns an empty vec when there is no registry or no bound
    /// validator declares a default (the common case — fast-skip on the
    /// insert hot path).
    ///
    /// Used by Phase ②.4c — `execute_insert_tx` / `execute_set_tx` call this
    /// once per batch and stamp each absent field with its literal default
    /// BEFORE encode + validation.
    pub fn schema_defaults(&self) -> Vec<(Vec<String>, QueryValue)> {
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Vec::new(),
        };
        let bindings = self.validator_bindings.load_full();
        let mut defaults = Vec::new();
        for binding in bindings.iter() {
            if let Some(validator) = reg.get_by_id(&binding.validator_id) {
                defaults.extend(validator.defaults());
            }
        }
        defaults
    }

    /// Collect all declarative transform rules declared by this table's bound
    /// validators.
    ///
    /// Close twin of [`schema_defaults`](Self::schema_defaults): iterates
    /// bindings, resolves each via the registry, calls `transforms()` on the
    /// compiled `RecordValidator`, and aggregates the
    /// `(field_path, TransformSpec)` pairs.  Returns an empty vec when there
    /// is no registry or no bound validator declares a transform (the common
    /// case — fast-skip on the insert hot path).
    ///
    /// Used by ③.2b — `execute_insert_tx` calls this once per batch and
    /// applies each transform (computed-default / timestamp stamping) to each
    /// record AFTER `apply_defaults` and BEFORE encode + CHECK-validators.
    /// The wiring is inert until `SchemaValidator::transforms()` returns
    /// non-empty entries (arriving in #281/#282 with the schema-surface
    /// fields `auto_now` / `auto_now_add` / expression-default).
    pub fn schema_transforms(&self) -> Vec<(Vec<String>, TransformSpec)> {
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Vec::new(),
        };
        let bindings = self.validator_bindings.load_full();
        let mut transforms = Vec::new();
        for binding in bindings.iter() {
            if let Some(validator) = reg.get_by_id(&binding.validator_id) {
                transforms.extend(validator.transforms());
            }
        }
        transforms
    }
}
