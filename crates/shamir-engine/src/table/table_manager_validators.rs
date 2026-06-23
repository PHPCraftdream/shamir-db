use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::validator::ValidatorFailure;

use super::table_manager::TableManager;

impl TableManager {
    /// Cheap snapshot of the current validator bindings for this table.
    /// The S3 write path reads this on every write.
    pub fn validator_bindings(&self) -> Arc<Vec<crate::validator::ValidatorBinding>> {
        self.validator_bindings.load_full()
    }

    /// Append (or replace if same `validator_id`) a validator binding,
    /// persist to the info-twin, and swap the in-memory snapshot.
    ///
    /// Bind is idempotent: if a binding with the same `validator_id`
    /// already exists, it is replaced (updated `ops` + `priority`).
    pub async fn add_validator_binding(
        &self,
        binding: crate::validator::ValidatorBinding,
    ) -> shamir_storage::error::DbResult<()> {
        let mut bindings = (*self.validator_bindings.load_full()).clone();
        if let Some(pos) = bindings
            .iter()
            .position(|b| b.validator_id == binding.validator_id)
        {
            bindings[pos] = binding;
        } else {
            bindings.push(binding);
        }
        crate::validator::persistence::save_validators_metadata(&bindings, &self.info_store)
            .await?;
        self.validator_bindings.store(Arc::new(bindings));
        Ok(())
    }

    /// Remove a validator binding by `validator_id`. Persists and swaps
    /// the in-memory snapshot. Returns whether the binding existed.
    pub async fn remove_validator_binding(
        &self,
        validator_id: &RecordId,
    ) -> shamir_storage::error::DbResult<bool> {
        let mut bindings = (*self.validator_bindings.load_full()).clone();
        let before = bindings.len();
        bindings.retain(|b| b.validator_id != *validator_id);
        let removed = bindings.len() < before;
        if removed {
            crate::validator::persistence::save_validators_metadata(&bindings, &self.info_store)
                .await?;
            self.validator_bindings.store(Arc::new(bindings));
        }
        Ok(removed)
    }

    /// Inject the global validator registry (S3). Called by the facade
    /// when user tables are created/opened. System tables leave this at
    /// `None` (validators disabled).
    pub fn set_validator_registry(&mut self, registry: Arc<crate::validator::ValidatorRegistry>) {
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
    /// [`run_validators_view`](Self::run_validators_view) (RecordView lens).
    ///
    /// Algorithm (per the VALIDATORS.md spec):
    /// 1. If no registry is set, return `Ok(())` (validators disabled).
    /// 2. Load bindings snapshot; filter to those whose `ops` contains `op`.
    /// 3. Delegate to `run_validators_loop` (sort, resolve, invoke, accumulate, fail).
    pub async fn run_validators_qv(
        &self,
        op: crate::validator::WriteOp,
        new_record: Option<&shamir_types::types::value::QueryValue>,
        old_record: Option<&shamir_types::types::value::QueryValue>,
        actor: &shamir_types::access::Actor,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        // 1. No registry → validators disabled (system tables / tests).
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Ok(()),
        };

        // 2. Load bindings snapshot; filter to applicable ops.
        let all_bindings = self.validator_bindings.load_full();
        let applicable: Vec<&crate::validator::ValidatorBinding> = all_bindings
            .iter()
            .filter(|b| b.ops.contains(&op))
            .collect();

        if applicable.is_empty() {
            return Ok(());
        }

        self.run_validators_loop(op, &applicable, reg, actor, new_record, old_record)
            .await
    }

    /// `RecordView`-input entry point for the DELETE write path (W6-delete).
    ///
    /// The delete path holds the raw storage bytes from the scan — no
    /// `InnerValue` tree is decoded on the index-planner path. This entry
    /// de-interns `old_view` to a `QueryValue` via `RecordRef::to_query_value`
    /// (the O(N) lens walker proven byte-for-byte identical to the tree path
    /// by `deintern_parity_tests`), then routes through `run_validators_loop`.
    ///
    /// Overlay-key resolution: the deleted record's fields are already in the
    /// base interner — they were committed when the record was inserted,
    /// before this delete. The tx overlay holds only brand-new field names
    /// staged in the CURRENT tx; a DELETE never introduces new fields.
    /// Base-only resolution via `RecordRef::to_query_value` is therefore
    /// correct and complete for the old-record side.
    ///
    /// The `tx` parameter is reserved for forward compatibility (overlay
    /// reverse-lookup if a future path needs it). Currently unused on the
    /// delete path since `new_record` is always `None` and the old record
    /// is always in base.
    ///
    /// `new_record` is `None` for Delete. A non-`None` value is forwarded
    /// unchanged (future-proofing — not used by the delete path today).
    pub(super) async fn run_validators_view(
        &self,
        op: crate::validator::WriteOp,
        new_record: Option<&QueryValue>,
        old_view: Option<&RecordView<'_>>,
        actor: &shamir_types::access::Actor,
        _tx: &shamir_tx::TxContext,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        // 1. No registry → validators disabled.
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Ok(()),
        };

        // 2. Load bindings snapshot; filter to applicable ops.
        let all_bindings = self.validator_bindings.load_full();
        let applicable: Vec<&crate::validator::ValidatorBinding> = all_bindings
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

        // De-intern the old view via the zero-copy lens walker.
        // `RecordRef::to_query_value` on `RecordView` calls
        // `record_view_to_query_value` (base interner only). For a deleted
        // record, all field keys are in base (they were committed at insert
        // time), so this is equivalent to the full overlay-aware path in
        // `run_validators_resolved`.
        let qv_old: Option<QueryValue> = old_view.map(|v| v.to_query_value(interner));

        self.run_validators_loop(op, &applicable, reg, actor, new_record, qv_old.as_ref())
            .await
    }

    /// Per-validator invocation loop shared by [`run_validators_qv`] and
    /// [`run_validators_view`](Self::run_validators_view).
    ///
    /// Both `qv_new` / `qv_old` arrive already as `QueryValue`. This body
    /// is the spec'd VALIDATORS.md algorithm steps 3–7 (sort, resolve,
    /// invoke, accumulate, fail).
    async fn run_validators_loop(
        &self,
        _op: crate::validator::WriteOp,
        applicable: &[&crate::validator::ValidatorBinding],
        reg: &crate::validator::ValidatorRegistry,
        actor: &shamir_types::access::Actor,
        qv_new: Option<&shamir_types::types::value::QueryValue>,
        qv_old: Option<&shamir_types::types::value::QueryValue>,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        use crate::function::{FnBatch, FnCtx, Params};
        use crate::validator::{decode_validation_result, ValidatorFailure};
        use shamir_types::types::value::QueryValue;

        // The caller already filtered to a non-empty `applicable`, so
        // indexing [0] for error attribution is safe.
        let mut applicable = applicable.to_vec();

        // 3. Sort by priority ascending, stable tie-break by id.
        applicable.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.validator_id.cmp(&b.validator_id))
        });

        // Allocate with a known lower bound to avoid realloc on the common
        // single-validator path.
        let mut all_errors: Vec<shamir_query_types::validator::ValidationError> =
            Vec::with_capacity(applicable.len());

        // Build ctx and batch once — they carry no per-validator state and
        // are only borrowed by `call`. Hoisting saves applicable.len()-1
        // Arc allocs per run_validators invocation.
        let ctx = FnCtx::new().with_actor(actor.clone());
        let batch = FnBatch::new();

        for binding in &applicable {
            // 4. Resolve validator_id → compiled function.
            let validator =
                reg.get_by_id(&binding.validator_id)
                    .ok_or(ValidatorFailure::Missing {
                        id: binding.validator_id,
                    })?;

            // 5. Build Params per validator (each call gets its own map).
            let mut params = Params::new();
            if let Some(rec) = qv_new {
                params.set("record", rec.clone());
            } else {
                params.set("record", QueryValue::Null);
            }
            if let Some(old) = qv_old {
                params.set("old_record", old.clone());
            } else {
                params.set("old_record", QueryValue::Null);
            }

            // Invoke the validator.
            let result = validator.call(&ctx, &batch, &params).await;

            match result {
                Err(fn_err) => {
                    // Invocation failure (trap/cancel) → fail-closed.
                    return Err(ValidatorFailure::Invocation {
                        id: binding.validator_id,
                        reason: fn_err.to_string(),
                    });
                }
                Ok(value) => {
                    // Decode the return value.
                    let outcome = decode_validation_result(&value).map_err(|e| {
                        ValidatorFailure::Invocation {
                            id: binding.validator_id,
                            reason: format!("decode error: {e}"),
                        }
                    })?;

                    // 6. Accumulate errors.
                    all_errors.extend(outcome.errors);

                    // On stop → break the loop.
                    if outcome.stop {
                        break;
                    }
                }
            }
        }

        // 7. If errors accumulated → fail.
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
}
