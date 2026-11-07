//! Shared types and primitives used by both client and server.
//!
//! The modules are grouped into topical subfolders; every module is
//! re-exported here so its public path stays `crate::common::<module>`.

mod messages;
mod model;
mod primitives;
mod runtime;
mod user;

#[cfg(test)]
mod tests;

pub use messages::{auth_message, bootstrap_message, changepw, envelope, push_envelope, wire};
pub use model::{admit, error, types};
pub use primitives::{crypto, domain_tags, kdf_params, password, scram};
pub use runtime::{fake_blob, latency, rotation, time};
pub use user::{identity, username};

pub use auth_message::AuthMessage;
pub use error::{Error, Result};
pub use kdf_params::KdfParams;
pub use push_envelope::{PushEnvelope, PushKind};
pub use types::{BindingMode, ProtocolVersion, TransportKind};
