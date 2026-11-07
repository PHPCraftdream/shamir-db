use crate::access::{Action, Actor, ResourcePath};
use crate::shamir_db::shamir_db::{FunctionSource, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::{check_module, AdmissionPolicy, ModuleReport, TierMode};

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
        let (bytes, _, _) = self.materialize_function_source(source, &actor).await?;
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
