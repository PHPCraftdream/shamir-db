use std::sync::Arc;

use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::shamir_db::shamir_db::{ArtifactKind, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::{AdmissionPolicy, TierMode, WasmFunction, WasmLimits};
use shamir_types::types::value::QueryValue;

use super::super::boot::{decode_stored_bytes, verify_stored_hash, HashVerdict};
use super::super::types::{optimize_name, FunctionState};

impl ShamirDb {
    /// Change a function's optimize mode as the system actor.
    pub async fn alter_function_optimize(&self, name: &str, mode: TierMode) -> DbResult<()> {
        self.alter_function_optimize_as(name, mode, Actor::System)
            .await
    }

    /// Rebuild the stored module under `mode` and persist only `optimize`.
    /// The row is saved before the registry swap, so a failed save changes
    /// nothing live.
    pub async fn alter_function_optimize_as(
        &self,
        name: &str,
        mode: TierMode,
        actor: Actor,
    ) -> DbResult<()> {
        self.authorize_access(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Write,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let existing = self
            .system_store
            .load_function(name)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("function '{name}' not found")))?;
        if ArtifactKind::from_record(&existing) == ArtifactKind::Native {
            return Err(DbError::Function(
                "native functions have no module to rebuild".to_string(),
            ));
        }
        let state = self
            .function_state
            .get(name)
            .map(|s| s.value().clone())
            .unwrap_or(FunctionState::Loaded);
        if let Some(msg) = state.unavailable_message(name) {
            return Err(DbError::Function(msg));
        }
        let bytes = decode_stored_bytes(&existing)
            .ok_or_else(|| {
                DbError::Function(format!("function '{name}' has no exportable module"))
            })?
            .map_err(DbError::Function)?;
        if let HashVerdict::Mismatch { expected, actual } = verify_stored_hash(&existing, &bytes) {
            return Err(self.mark_function_corrupt(name, expected, actual));
        }
        let (wf, _report) = WasmFunction::from_binary_admitted_blocking(
            self.wasm_engine.clone(),
            &bytes,
            WasmLimits::default(),
            mode,
            AdmissionPolicy::Admission(*self.wasm_engine.admission_limits()),
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;

        let mut rec = existing.clone();
        if let QueryValue::Map(ref mut m) = rec {
            m.insert(
                "optimize".to_string(),
                QueryValue::Str(optimize_name(mode).to_string()),
            );
        }
        self.system_store
            .save_function(name, &rec, &ResourceMeta::from_record(&existing))
            .await?;
        self.functions.replace(name, Arc::new(wf));
        self.function_state
            .insert(name.to_string(), FunctionState::Loaded);
        Ok(())
    }
}
