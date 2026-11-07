//! Browser-route Origin/Host policy, pre-auth permits and first-frame parsing.

pub(crate) mod first_frame;
pub(crate) mod policy;
mod pre_auth;

pub(crate) use pre_auth::{PreAuthAdmission, PreAuthPermit};
