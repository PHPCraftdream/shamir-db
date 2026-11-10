use std::sync::Arc;

/// Opaque runtime identity of one registry entry: replacement/rename retain it;
/// remove followed by a fresh registration mints a new one.
#[derive(Clone, Debug)]
pub(crate) struct CallIdentity(Arc<()>);

impl CallIdentity {
    pub(crate) fn new() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for CallIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CallIdentity {}
