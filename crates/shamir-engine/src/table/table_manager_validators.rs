use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::validator::{
    record_fields::{OwnedFields, ViewFields},
    record_validator::ValidatorCtx,
    ValidatorFailure,
};

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
    /// 3. Delegate to `run_validators_loop` with `OwnedFields` backings.
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

        let new_dyn: Option<&dyn crate::validator::record_fields::RecordFields> = new_fields
            .as_ref()
            .map(|f| f as &dyn crate::validator::record_fields::RecordFields);
        let old_dyn: Option<&dyn crate::validator::record_fields::RecordFields> = old_fields
            .as_ref()
            .map(|f| f as &dyn crate::validator::record_fields::RecordFields);

        let ctx = ValidatorCtx { actor, interner };

        self.run_validators_loop(&applicable, reg, &ctx, new_dyn, old_dyn)
            .await
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

        // 3. Build ViewFields lens (zero-copy over raw msgpack bytes) for the
        //    old record.  No full de-intern — native validators probe by name
        //    lazily; WASM materialises internally via to_query_value().
        let old_fields = old_view.map(|view| ViewFields { view, interner });
        let new_fields = new_record.map(|qv| OwnedFields { qv });

        let new_dyn: Option<&dyn crate::validator::record_fields::RecordFields> = new_fields
            .as_ref()
            .map(|f| f as &dyn crate::validator::record_fields::RecordFields);
        let old_dyn: Option<&dyn crate::validator::record_fields::RecordFields> = old_fields
            .as_ref()
            .map(|f| f as &dyn crate::validator::record_fields::RecordFields);

        let ctx = ValidatorCtx { actor, interner };

        self.run_validators_loop(&applicable, reg, &ctx, new_dyn, old_dyn)
            .await
    }

    /// Per-validator invocation loop shared by [`run_validators_qv`] and
    /// [`run_validators_view`](Self::run_validators_view).
    ///
    /// Resolves each binding to a `RecordValidator`, calls `.validate(...)`,
    /// accumulates errors, and applies the stop-flag semantics.
    async fn run_validators_loop(
        &self,
        applicable: &[&crate::validator::ValidatorBinding],
        reg: &crate::validator::ValidatorRegistry,
        ctx: &ValidatorCtx<'_>,
        new: Option<&dyn crate::validator::record_fields::RecordFields>,
        old: Option<&dyn crate::validator::record_fields::RecordFields>,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        use crate::validator::ValidatorFailure;

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
}
