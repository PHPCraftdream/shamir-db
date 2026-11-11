use crate::access::{Action, Actor, ResourcePath};
use crate::shamir_db::shamir_db::{FunctionSource, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::{
    check_module, check_shamir_source, AdmissionPolicy, CompileOptions, ModuleReport, ShamirStage,
    TierMode, WasmLimits,
};

impl ShamirDb {
    /// Dry-run a function creation as the system actor.
    pub async fn check_function(
        &self,
        source: FunctionSource<'_>,
        optimize: TierMode,
    ) -> DbResult<ModuleReport> {
        self.check_function_as(Actor::System, source, optimize)
            .await
    }

    /// Admit and compile `source` exactly as create would, persisting and
    /// registering nothing.
    pub async fn check_function_as(
        &self,
        actor: Actor,
        source: FunctionSource<'_>,
        optimize: TierMode,
    ) -> DbResult<ModuleReport> {
        self.authorize_access(&actor, &ResourcePath::FunctionNamespace, Action::Create)
            .await
            .map_err(|e| DbError::Function(e.to_string()))?;
        match source {
            FunctionSource::Shamir(src) => {
                // Route shamir through the check-only service path so the
                // source compiles exactly once; materialize would compile it
                // again from bytes. Nothing is persisted.
                self.authorize_shamir_source(src, &actor).await?;
                let mut options = CompileOptions {
                    stage: ShamirStage::Procedures,
                    ..CompileOptions::default()
                };
                options.callable_catalog = self.callable_catalog_snapshot(&actor, src).await?;
                check_shamir_source(
                    self.wasm_engine.clone(),
                    src.to_string(),
                    options,
                    WasmLimits::default(),
                    optimize,
                )
                .await
                .map_err(|e| DbError::Function(e.to_string()))
            }
            other => {
                let (bytes, _, _, _, _) = self
                    .materialize_function_source(other, &actor, optimize)
                    .await?;
                check_module(
                    self.wasm_engine.clone(),
                    &bytes,
                    AdmissionPolicy::Admission(*self.wasm_engine.admission_limits()),
                    optimize,
                )
                .await
                .map_err(|e| DbError::Function(e.to_string()))
            }
        }
    }
}
