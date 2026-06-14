use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;

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

    /// Run all applicable validators for the given write operation on a
    /// single record. This is the S3 validator pass.
    ///
    /// Algorithm (per the VALIDATORS.md spec):
    /// 1. If no registry is set, return `Ok(())` (validators disabled).
    /// 2. Load bindings snapshot; filter to those whose `ops` contains `op`.
    /// 3. Sort by `priority` ascending, stable tie-break by `validator_id`.
    /// 4. For each binding, resolve `validator_id` → compiled function.
    ///    Missing → `Err(ValidatorFailure::Missing)` (fail-closed).
    /// 5. Build `Params` with `record` + `old_record`, build `FnCtx`
    ///    with actor. Invoke `validator.call(...)`.
    /// 6. Accumulate errors. On `stop` → break.
    /// 7. If errors → `Err(ValidatorFailure::Failed(...))`.
    pub async fn run_validators(
        &self,
        op: crate::validator::WriteOp,
        new_record: Option<&shamir_types::types::value::InnerValue>,
        old_record: Option<&shamir_types::types::value::InnerValue>,
        actor: &shamir_types::access::Actor,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        // Direct (non-tx) path: all keys live in the base interner.
        self.run_validators_resolved(op, new_record, old_record, actor, None)
            .await
    }

    /// tx-aware variant of [`run_validators`](Self::run_validators).
    ///
    /// The tx write path interns brand-new field names into the per-tx
    /// LAYERED interner overlay (ids ≥ `OVERLAY_ID_BASE`) that the global
    /// interner cannot yet resolve (the overlay merges into base only at
    /// commit, AFTER validators run). Passing the tx's overlay through here
    /// lets a just-staged record with new-field keys resolve at validation
    /// time. Without this, validating a tx insert that introduces a brand-new
    /// field fails with `interned key … not found`.
    pub async fn run_validators_tx(
        &self,
        op: crate::validator::WriteOp,
        new_record: Option<&shamir_types::types::value::InnerValue>,
        old_record: Option<&shamir_types::types::value::InnerValue>,
        actor: &shamir_types::access::Actor,
        tx: &shamir_tx::TxContext,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        self.run_validators_resolved(op, new_record, old_record, actor, Some(tx))
            .await
    }

    /// Shared implementation backing [`run_validators`] and
    /// [`run_validators_tx`]. When `tx` is `Some`, interned keys are
    /// resolved through a layered view (overlay first, then base) so
    /// just-staged new-field keys resolve before commit.
    async fn run_validators_resolved(
        &self,
        op: crate::validator::WriteOp,
        new_record: Option<&shamir_types::types::value::InnerValue>,
        old_record: Option<&shamir_types::types::value::InnerValue>,
        actor: &shamir_types::access::Actor,
        tx: Option<&shamir_tx::TxContext>,
    ) -> Result<(), crate::validator::ValidatorFailure> {
        use crate::function::{FnBatch, FnCtx, Params};
        use crate::validator::{
            decode_validation_result, inner_to_query_value_with, ValidatorFailure,
        };
        use shamir_types::core::interner::InternerKey;
        use shamir_types::types::value::QueryValue;

        // 1. No registry → validators disabled (system tables / tests).
        let reg = match &self.validator_registry {
            Some(r) => r,
            None => return Ok(()),
        };

        // 2. Load bindings snapshot; filter to applicable ops.
        let all_bindings = self.validator_bindings.load_full();
        let mut applicable: Vec<&crate::validator::ValidatorBinding> = all_bindings
            .iter()
            .filter(|b| b.ops.contains(&op))
            .collect();

        if applicable.is_empty() {
            return Ok(());
        }

        // 3. Sort by priority ascending, stable tie-break by id.
        applicable.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.validator_id.cmp(&b.validator_id))
        });

        // Pre-convert records to QueryValue (string-keyed) once.
        let interner = self
            .interner()
            .get()
            .await
            .map_err(|e| ValidatorFailure::Invocation {
                id: applicable[0].validator_id,
                reason: format!("interner load failed: {e}"),
            })?;

        // Key resolver: base interner first; for the tx path fall back to
        // the tx's interner overlay so brand-new field keys (ids ≥
        // OVERLAY_ID_BASE, not yet merged into base) resolve at validation
        // time. The closure borrows `interner` + `tx` for the conversion.
        let resolve = |key: &InternerKey| -> Option<String> {
            if let Some(s) = interner.with_str(key, |s| s.to_string()) {
                return Some(s);
            }
            // Overlay is keyed by name → id, so reverse-lookup by id.
            tx.and_then(|t| {
                let mut found: Option<String> = None;
                t.interner_overlay.scan(|name: &String, v: &u64| {
                    if *v == key.id() {
                        found = Some(name.clone());
                    }
                });
                found
            })
        };

        let qv_new: Option<QueryValue> = match new_record {
            Some(r) => Some(inner_to_query_value_with(r, &resolve).map_err(|e| {
                ValidatorFailure::Invocation {
                    id: applicable[0].validator_id,
                    reason: format!("record conversion failed: {e}"),
                }
            })?),
            None => None,
        };

        let qv_old: Option<QueryValue> = match old_record {
            Some(r) => Some(inner_to_query_value_with(r, &resolve).map_err(|e| {
                ValidatorFailure::Invocation {
                    id: applicable[0].validator_id,
                    reason: format!("old_record conversion failed: {e}"),
                }
            })?),
            None => None,
        };

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
            if let Some(ref rec) = qv_new {
                params.set("record", rec.clone());
            } else {
                params.set("record", QueryValue::Null);
            }
            if let Some(ref old) = qv_old {
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
