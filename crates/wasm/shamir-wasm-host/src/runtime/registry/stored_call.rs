use super::resolver::ResolvedStoredCall;
use crate::runtime::context::{FnBatch, FnCtx};
use crate::runtime::error::{FnResult, FunctionError};
use crate::runtime::params::Params;
use shamir_types::types::value::QueryValue;

/// Registry fallback is unchecked and only valid for catalogue-free host-only contexts.
pub(crate) async fn dispatch_stored_call(
    parent: &FnCtx,
    name: &str,
    batch: &FnBatch,
    params: &Params,
) -> FnResult<QueryValue> {
    dispatch_stored_call_with_signature(parent, name, None, batch, params).await
}

/// Shared stored-call path for legacy and signature-checked dispatch.
pub(crate) async fn dispatch_stored_call_with_signature(
    parent: &FnCtx,
    name: &str,
    expected_signature: Option<&[u8]>,
    batch: &FnBatch,
    params: &Params,
) -> FnResult<QueryValue> {
    if parent.active_call_path().len() == 0 {
        return Err(FunctionError::Compute(
            "stored call requires an active root invocation".into(),
        ));
    }
    if parent
        .registry()
        .is_some_and(|registry| registry.is_catalogue_bound())
        && parent.stored_call_resolver().is_none()
    {
        return Err(FunctionError::Compute(
            "catalogue-bound stored call requires a stored-call resolver".into(),
        ));
    }
    let next_depth = parent.depth().saturating_add(1);
    if next_depth > parent.depth_limit() {
        return Err(FunctionError::Compute(format!(
            "call depth limit exceeded: {next_depth} > {}",
            parent.depth_limit()
        )));
    }

    let resolved = if let Some(expected_signature) = expected_signature {
        let resolver = parent.stored_call_resolver().ok_or_else(|| {
            FunctionError::Compute("checked stored call requires a catalogue resolver".into())
        })?;
        resolver
            .resolve_checked(name, parent, expected_signature)
            .await?
    } else if let Some(resolver) = parent.stored_call_resolver() {
        resolver.resolve(name, parent).await?
    } else {
        let registry = parent
            .registry()
            .ok_or_else(|| FunctionError::NotFound(name.to_string()))?;
        let binding = registry
            .capture(name)
            .ok_or_else(|| FunctionError::NotFound(name.to_string()))?;
        ResolvedStoredCall::new(&binding, parent.clone(), parent.definer_owner().cloned())
    };
    let child_path = parent.active_call_path().extend(resolved.identity)?;
    let child_ctx = resolved.target_context.with_call_child_runtime(
        parent,
        next_depth,
        child_path,
        resolved.definer_owner,
    );
    resolved.artifact.call(&child_ctx, batch, params).await
}
