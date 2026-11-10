use super::identity::CallIdentity;
use crate::runtime::context::FnCtx;
use crate::runtime::contract::ShamirFunction;
use crate::runtime::error::FnResult;
use shamir_types::access::Actor;
use std::sync::Arc;

/// A target captured and authorized by the database catalogue resolver.
/// The artifact and identity must come from one registry capture; the supplied
/// context must contain only the target's effective actor and capabilities.
pub struct ResolvedStoredCall {
    pub(crate) identity: CallIdentity,
    pub(crate) artifact: Arc<dyn ShamirFunction>,
    pub(crate) target_context: FnCtx,
    pub(crate) definer_owner: Option<Actor>,
}

impl ResolvedStoredCall {
    /// Target contexts should be created with `parent.without_target_capabilities()`
    /// before deriving only target-authorized resources and actor.
    pub fn new(
        binding: &super::binding::RegistryBinding,
        target_context: FnCtx,
        definer_owner: Option<Actor>,
    ) -> Self {
        Self {
            identity: binding.identity().clone(),
            artifact: Arc::clone(binding.artifact()),
            target_context,
            definer_owner,
        }
    }
}

/// Catalogue-backed authorization and target-context construction for a guest stored call.
/// The resolver must return the target's exact captured binding and a context rebuilt from its authorized metadata.
#[async_trait::async_trait]
pub trait StoredCallResolver: Send + Sync {
    async fn resolve(&self, name: &str, parent: &FnCtx) -> FnResult<ResolvedStoredCall>;
}
