use super::identity::CallIdentity;
use crate::runtime::error::{FnResult, FunctionError};
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct ActiveCallPath(Arc<Vec<CallIdentity>>);

impl ActiveCallPath {
    pub(crate) fn root(identity: CallIdentity) -> Self {
        Self(Arc::new(vec![identity]))
    }

    pub(crate) fn standalone_root() -> Self {
        Self::root(super::identity::CallIdentity::new())
    }

    pub(crate) fn extend(&self, identity: CallIdentity) -> FnResult<Self> {
        if self.0.iter().any(|active| active == &identity) {
            return Err(FunctionError::Compute(
                "stored call cycle detected: target is already active".into(),
            ));
        }
        if self.0.len() >= 32 {
            return Err(FunctionError::Compute(
                "stored call active path limit exceeded: maximum is 32".into(),
            ));
        }
        let mut path = self.0.as_ref().clone();
        path.push(identity);
        Ok(Self(Arc::new(path)))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}
