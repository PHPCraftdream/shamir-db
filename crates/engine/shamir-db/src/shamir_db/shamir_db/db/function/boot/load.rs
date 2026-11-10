use std::sync::Arc;

use shamir_engine::function::{AdmissionPolicy, ShamirFunction, WasmFunction, WasmLimits};
use shamir_types::types::value::QueryValue;

use super::super::artifact::artifact_id_from_record;
use super::super::types::{FunctionState, UnavailableFunction};
use super::{decode_stored_bytes, stored_optimize, verify_stored_hash, HashVerdict};
use crate::shamir_db::shamir_db::catalog::with_checked_function_metadata;
use crate::shamir_db::shamir_db::{ArtifactKind, ShamirDb};
use crate::{DbError, DbResult};

impl ShamirDb {
    /// Register every catalogue function at boot.
    ///
    /// Stored modules are compiled under `AdmissionPolicy::Load` (structural
    /// checks only, no size/source caps) with their stored `optimize` mode. The
    /// stored hash is verified first; a mismatch marks the function `Corrupt`
    /// without compiling. A module that fails to decode or compile is
    /// registered as `Unloadable`. Unavailable functions stay visible,
    /// describable, replaceable and droppable; calling them fails closed.
    pub(in crate::shamir_db::shamir_db) async fn load_functions_at_boot(
        &self,
        fn_records: &[QueryValue],
    ) -> DbResult<()> {
        // Decode all metadata before registering recovered functions.
        with_checked_function_metadata(fn_records, |metadata| async {
            for (rec, meta) in fn_records.iter().zip(metadata) {
                let name = match rec["name"].as_str() {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if self.functions.contains(&name) {
                    return Err(DbError::Function(format!(
                        "catalogue function '{}' collides with a builtin or registered function",
                        name
                    )));
                }
                let artifact_id = artifact_id_from_record(rec).map_err(|error| {
                    DbError::Function(format!("invalid function artifact id in catalogue: {error}"))
                })?;
                // Dispatch on artifact kind: Native rows have no WASM bytes —
                // skip materialisation. The embedder re-registers the native
                // artifact at startup; until then the function is absent and any
                // invocation fails closed ("function not found").
                if ArtifactKind::from_record(rec) == ArtifactKind::Native {
                    log::info!(
                        "shamir_db::init: skipping native function '{}' — re-register at startup",
                        name
                    );
                    continue;
                }
                let decoded = match decode_stored_bytes(rec) {
                    Some(d) => d,
                    None => {
                        log::warn!(
                            "shamir_db::init: skipping function '{}' — no wasm_b64 field",
                            name
                        );
                        continue;
                    }
                };
                let (function, state): (Arc<dyn ShamirFunction>, FunctionState) = match decoded {
                    Err(e) => {
                        log::warn!(
                            "shamir_db::init: function '{}' has undecodable module: {}",
                            name,
                            e
                        );
                        let state = FunctionState::Unloadable {
                            reason: format!("stored module is not valid base64: {e}"),
                        };
                        (
                            Arc::new(UnavailableFunction::new(&name, state.clone())),
                            state,
                        )
                    }
                    Ok(bytes) => match verify_stored_hash(rec, &bytes) {
                        HashVerdict::Mismatch { expected, actual } => {
                            log::warn!(
                                "shamir_db::init: function '{}' is corrupt (expected hash {}, actual {})",
                                name,
                                expected,
                                actual
                            );
                            let state = FunctionState::Corrupt { expected, actual };
                            (
                                Arc::new(UnavailableFunction::new(&name, state.clone())),
                                state,
                            )
                        }
                        HashVerdict::Match(_) | HashVerdict::Unknown => {
                            match WasmFunction::from_binary_admitted_blocking(
                                self.wasm_engine.clone(),
                                &bytes,
                                WasmLimits::default(),
                                stored_optimize(rec),
                                AdmissionPolicy::Load,
                            )
                            .await
                            {
                                Ok((wf, _report)) => (Arc::new(wf), FunctionState::Loaded),
                                Err(e) => {
                                    log::warn!(
                                        "shamir_db::init: failed to compile WASM for function '{}': {}",
                                        name,
                                        e
                                    );
                                    let state = FunctionState::Unloadable {
                                        reason: e.to_string(),
                                    };
                                    (
                                        Arc::new(UnavailableFunction::new(&name, state.clone())),
                                        state,
                                    )
                                }
                            }
                        }
                    },
                };
                if self
                    .functions
                    .register_with_artifact_id(&name, function, artifact_id)
                    .is_err()
                {
                    return Err(DbError::Function(format!(
                        "catalogue function '{}' collides with a builtin or registered function",
                        name
                    )));
                }
                // Publish metadata and state after registration succeeds.
                let _ = self.function_meta.insert(name.clone(), meta);
                let _ = self.function_state.insert(name, state);
            }
            Ok::<(), DbError>(())
        })
        .await??;
        Ok(())
    }
}
