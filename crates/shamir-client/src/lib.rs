//! ShamirDB Rust client SDK.
//!
//! Wraps the production handshake (`shamir-connect`) and TLS transport
//! (`shamir-transport-tcp`) into a small ergonomic surface:
//!
//! ```no_run
//! # use shamir_client::{Client, ConnectOptions};
//! # use zeroize::Zeroizing;
//! # async fn demo() -> Result<(), shamir_client::ClientError> {
//! let client = Client::connect(ConnectOptions {
//!     addr: "127.0.0.1:3742".parse().unwrap(),
//!     server_name: "localhost".into(),
//!     username: "admin".into(),
//!     password: Zeroizing::new(b"correct horse battery staple".to_vec()),
//!     accept_new_host: true,
//!     trusted_pin: None,
//! })
//! .await?;
//!
//! client.ping().await?;
//! client.close().await;
//! # Ok(()) }
//! ```
//!
//! The SDK is the same code path the reference Rust integration tests
//! exercise; the napi-rs Node binding (`shamir-client-node`) wraps this
//! surface 1:1, so all language clients share one implementation of
//! TLS+SCRAM+envelope handling.

mod client;
mod error;
mod wire_frames;

pub use client::{Client, ConnectOptions};
pub use error::ClientError;

// Re-export the wire payloads so callers don't need to depend on
// `shamir-query-types` directly.
pub use shamir_query_types::batch::{BatchRequest, BatchResponse};
pub use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
