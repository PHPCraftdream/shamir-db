//! Shared types and primitives used by both client and server.

pub mod auth_message;
pub mod bootstrap_message;
pub mod changepw;
pub mod crypto;
pub mod domain_tags;
pub mod envelope;
pub mod error;
pub mod fake_blob;
pub mod identity;
pub mod kdf_params;
pub mod rotation;
pub mod scram;
pub mod time;
pub mod types;
pub mod username;

#[cfg(test)]
mod tests;

pub use auth_message::AuthMessage;
pub use error::{Error, Result};
pub use kdf_params::KdfParams;
pub use types::{BindingMode, ProtocolVersion, TransportKind};
