//! Shared types and primitives used by both client and server.

pub mod auth_message;
pub mod domain_tags;
pub mod error;
pub mod kdf_params;
pub mod time;
pub mod types;
pub mod username;

#[cfg(test)]
mod tests;

pub use auth_message::AuthMessage;
pub use error::{Error, Result};
pub use kdf_params::KdfParams;
pub use types::{BindingMode, ProtocolVersion, TransportKind};
