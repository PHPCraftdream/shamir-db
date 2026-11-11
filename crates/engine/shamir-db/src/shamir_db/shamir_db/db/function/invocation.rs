use std::sync::Arc;

use async_trait::async_trait;
use shamir_engine::function::{
    BatchContext, FnBatch, FnCtx, FnResult, FunctionError, FunctionMeta, Params,
    ResolvedStoredCall, StoredCallResolver,
};
use shamir_types::access::{Actor, ResourceMeta};
use shamir_types::types::value::QueryValue;

use crate::engine::function::Security;
use crate::shamir_db::shamir_db::{ArtifactKind, ShamirDb};
use crate::{DbError, DbResult};

use super::super::db_gateway::FacadeDbGateway;
use super::artifact::artifact_id_from_record;

pub(in crate::shamir_db::shamir_db) struct ShamirDbCallResolver {
    shamir: ShamirDb,
    db_name: Option<String>,
    default_repo: Option<String>,
}

impl ShamirDbCallResolver {
    pub(in crate::shamir_db::shamir_db) fn new(
        shamir: ShamirDb,
        db_name: Option<String>,
        default_repo: Option<String>,
    ) -> Self {
        Self {
            shamir,
            db_name,
            default_repo,
        }
    }

    async fn resolve_with_signature(
        &self,
        name: &str,
        parent: &FnCtx,
        expected_signature: Option<&[u8]>,
    ) -> FnResult<ResolvedStoredCall> {
        let registry = self.shamir.functions().clone();
        let binding = registry
            .capture(name)
            .ok_or_else(|| FunctionError::NotFound(name.to_string()))?;
        #[cfg(test)]
        if let Some(hook) = self.shamir.function_resolution_hook.get() {
            hook(self.shamir.clone(), name.to_string(), true).await;
        }
        let record = self
            .shamir
            .system_store()
            .load_function(name)
            .await
            .map_err(|error| {
                FunctionError::Compute(format!("function catalogue lookup failed: {error}"))
            })?;
        let current_binding = registry
            .capture(name)
            .ok_or_else(|| FunctionError::Compute("function changed during resolution".into()))?;
        if !binding.same_as(&current_binding) {
            return Err(FunctionError::Compute(
                "function changed during resolution".into(),
            ));
        }
        let record = record.ok_or_else(|| {
            FunctionError::Compute("nested target is missing from the catalogue".into())
        })?;
        let row_artifact_id = artifact_id_from_record(&record).map_err(|error| {
            FunctionError::Compute(format!("invalid nested target artifact id: {error}"))
        })?;
        if row_artifact_id.as_deref() != binding.artifact_id() {
            return Err(FunctionError::Compute(
                "function changed during resolution".into(),
            ));
        }
        let meta = strict_nested_meta(&record)?;
        let owner = ResourceMeta::owner_field(&record).ok_or_else(|| {
            FunctionError::Compute("nested target owner is missing or invalid".into())
        })?;
        let (actor, target_owner_guard) = self
            .shamir
            .authorize_invocation(name, parent.actor(), Some(&record))
            .await
            .map_err(|error| FunctionError::Compute(error.to_string()))?;
        if let Some(expected) = expected_signature {
            let row_hash = record
                .get("module_hash")
                .and_then(|value| value.as_str())
                .and_then(|value| shamir_engine::function::ModuleHash::from_hex(value).ok());
            let Some(binding_hash) = binding.module_hash() else {
                return Err(FunctionError::Compute(
                    "checked target is not a coherent stored shamir WASM artifact".into(),
                ));
            };
            if record.get("lang").and_then(|value| value.as_str()) != Some("shamir")
                || ArtifactKind::from_record(&record) != ArtifactKind::Wasm
                || row_hash != Some(binding_hash)
            {
                return Err(FunctionError::Compute(
                    "checked target is not a coherent stored shamir WASM artifact".into(),
                ));
            }
            if !matches!(record.get("call_signature"), Some(QueryValue::Bin(signature)) if signature.as_slice() == expected)
            {
                return Err(FunctionError::Compute(
                    "checked target signature is missing or does not match".into(),
                ));
            }
        }
        if matches!(meta.security, Security::Invoker)
            && parent
                .definer_owner()
                .is_some_and(|parent_owner| parent_owner != &owner)
        {
            return Err(FunctionError::Compute(
                "Definer cannot call Invoker function owned by a different actor".into(),
            ));
        }
        let definer_owner = target_owner_guard.or_else(|| parent.definer_owner().cloned());
        let mut target = parent
            .clone()
            .without_target_capabilities()
            .with_actor(actor)
            .with_secret_grants(meta.secret_grants.clone())
            .with_net(self.shamir.net_allowlist.gateway(Some(&meta.net_grants)))
            .with_registry(registry.clone())
            .with_stored_call_resolver(Arc::new(ShamirDbCallResolver::new(
                self.shamir.clone(),
                self.db_name.clone(),
                self.default_repo.clone(),
            )));
        if let (Some(db_name), Some(repo)) = (&self.db_name, &self.default_repo) {
            let gateway = Arc::new(FacadeDbGateway {
                shamir: self.shamir.clone(),
                db_name: db_name.clone(),
                actor: target.actor().clone(),
            });
            target = target.with_db(gateway, repo.clone());
        } else if self.db_name.is_some() || self.default_repo.is_some() {
            return Err(FunctionError::Compute(
                "nested target database context is incomplete".into(),
            ));
        }
        Ok(ResolvedStoredCall::new(&binding, target, definer_owner))
    }
}

#[async_trait]
impl StoredCallResolver for ShamirDbCallResolver {
    async fn resolve(&self, name: &str, parent: &FnCtx) -> FnResult<ResolvedStoredCall> {
        self.resolve_with_signature(name, parent, None).await
    }

    async fn resolve_checked(
        &self,
        name: &str,
        parent: &FnCtx,
        expected_signature: &[u8],
    ) -> FnResult<ResolvedStoredCall> {
        self.resolve_with_signature(name, parent, Some(expected_signature))
            .await
    }
}

pub(super) fn strict_nested_meta(record: &QueryValue) -> FnResult<FunctionMeta> {
    let QueryValue::Map(fields) = record else {
        return Err(FunctionError::Compute(
            "nested target catalogue row is not a map".into(),
        ));
    };
    if !matches!(fields.get("metadata_version"), Some(QueryValue::Int(1)))
        || !fields.contains_key("security")
        || !fields.contains_key("secret_grants")
        || !fields.contains_key("net_grants")
    {
        return Err(FunctionError::Compute(
            "nested target security metadata is missing".into(),
        ));
    }
    FunctionMeta::try_from_record(record)
        .map_err(|error| FunctionError::Compute(format!("invalid target metadata: {error}")))
}

impl ShamirDb {
    pub(super) async fn invoke_function_root(
        &self,
        name: &str,
        params: Params,
        batch: Option<&Arc<BatchContext>>,
        caller: Actor,
        db: Option<(&str, &str)>,
    ) -> DbResult<QueryValue> {
        let binding = self
            .functions()
            .capture(name)
            .ok_or_else(|| DbError::Function(format!("function '{name}' not found")))?;
        #[cfg(test)]
        if let Some(hook) = self.function_resolution_hook.get() {
            hook(self.clone(), name.to_string(), false).await;
        }
        let record = self.system_store().load_function(name).await?;
        let current_binding = self
            .functions()
            .capture(name)
            .ok_or_else(|| DbError::Function("function changed during resolution".into()))?;
        if !binding.same_as(&current_binding) {
            return Err(DbError::Function(
                "function changed during resolution".into(),
            ));
        }
        if record.is_none() && !binding.is_database_builtin() {
            return Err(DbError::Function(
                "catalogue-managed function is missing from the catalogue".to_string(),
            ));
        }
        let row_artifact_id = match record.as_ref() {
            Some(record) => artifact_id_from_record(record)
                .map_err(|error| DbError::Function(format!("invalid root artifact id: {error}")))?,
            None => None,
        };
        if row_artifact_id.as_deref() != binding.artifact_id() {
            return Err(DbError::Function(
                "function changed during resolution".into(),
            ));
        }
        let (actor, root_owner) = self
            .authorize_invocation(name, &caller, record.as_ref())
            .await
            .map_err(|error| DbError::Function(error.to_string()))?;
        let meta = record
            .as_ref()
            .map(FunctionMeta::try_from_record)
            .transpose()
            .map_err(|error| DbError::Function(format!("invalid root metadata: {error}")))?;
        let resolver = Arc::new(ShamirDbCallResolver::new(
            self.clone(),
            db.map(|(db_name, _)| db_name.to_string()),
            db.map(|(_, repo)| repo.to_string()),
        ));
        let metadata = meta;
        if matches!(
            metadata.as_ref().map(|meta| meta.security),
            Some(Security::Definer)
        ) && root_owner.is_none()
        {
            return Err(DbError::Function(
                "root Definer owner is missing or invalid".into(),
            ));
        }
        let mut ctx = self.build_invoke_ctx(actor, resolver, root_owner, metadata.as_ref());
        if let Some((db_name, repo)) = db {
            let gateway = Arc::new(FacadeDbGateway {
                shamir: self.clone(),
                db_name: db_name.to_string(),
                actor: ctx.actor().clone(),
            });
            ctx = ctx.with_db(gateway, repo.to_string());
        }
        let batch = batch
            .map(|batch| FnBatch::with_context(batch.clone()))
            .unwrap_or_default();
        self.functions()
            .invoke_captured(&binding, &ctx, &batch, &params)
            .await
            .map_err(|error| DbError::Function(error.to_string()))
    }
}
