use super::identity::CallIdentity;
use crate::runtime::contract::ShamirFunction;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct RegistryEntry {
    pub(crate) identity: CallIdentity,
    pub(crate) artifact: Arc<dyn ShamirFunction>,
    pub(crate) artifact_id: Option<String>,
    pub(crate) database_builtin: bool,
}

impl RegistryEntry {
    pub(crate) fn same_binding(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.artifact_id == other.artifact_id
            && self.database_builtin == other.database_builtin
            && std::ptr::addr_eq(Arc::as_ptr(&self.artifact), Arc::as_ptr(&other.artifact))
    }
}

/// The caller uses this capture for authorization and invokes its artifact directly.
#[derive(Clone)]
pub struct RegistryBinding(pub(crate) RegistryEntry);

impl RegistryBinding {
    /// Compare captured entry identity, artifact pointer, artifact id, and provenance.
    pub fn same_as(&self, other: &Self) -> bool {
        self.0.same_binding(&other.0)
    }

    /// The catalogue artifact id captured with this registry binding, if present.
    pub fn artifact_id(&self) -> Option<&str> {
        self.0.artifact_id.as_deref()
    }

    /// Whether this entry was installed as a database built-in.
    pub fn is_database_builtin(&self) -> bool {
        self.0.database_builtin
    }

    pub(crate) fn identity(&self) -> &CallIdentity {
        &self.0.identity
    }

    pub(crate) fn artifact(&self) -> &Arc<dyn ShamirFunction> {
        &self.0.artifact
    }
}
