use super::identity::CallIdentity;
use crate::runtime::contract::ShamirFunction;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct RegistryEntry {
    pub(crate) identity: CallIdentity,
    pub(crate) artifact: Arc<dyn ShamirFunction>,
}

impl RegistryEntry {
    pub(crate) fn same_binding(&self, other: &Self) -> bool {
        self.identity == other.identity
            && std::ptr::addr_eq(Arc::as_ptr(&self.artifact), Arc::as_ptr(&other.artifact))
    }
}

/// The caller uses this capture for authorization and invokes its artifact directly.
#[derive(Clone)]
pub struct RegistryBinding(pub(crate) RegistryEntry);

impl RegistryBinding {
    /// Compare captured entry identity and artifact pointer, independent of its registry name.
    pub fn same_as(&self, other: &Self) -> bool {
        self.0.same_binding(&other.0)
    }

    pub(crate) fn identity(&self) -> &CallIdentity {
        &self.0.identity
    }

    pub(crate) fn artifact(&self) -> &Arc<dyn ShamirFunction> {
        &self.0.artifact
    }
}
