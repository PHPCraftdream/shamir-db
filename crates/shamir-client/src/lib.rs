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
pub mod cursor_stream;
mod error;
pub mod interner_cache;
mod interner_cache_ops;
pub mod subscription;
mod wire_frames;

#[cfg(test)]
mod tests;

pub use client::{Client, ConnectOptions, ResumeOptions};
pub use cursor_stream::CursorStream;
pub use error::ClientError;
pub use interner_cache::{FieldMap, InternerCacheRegistry};
pub use subscription::SubscriptionHandle;

// Re-export the wire payloads so callers don't need to depend on
// `shamir-query-types` directly.
pub use shamir_query_types::batch::{BatchRequest, BatchResponse};
pub use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

// Re-export the fluent query builder under `shamir_client::builder` so a
// single `shamir-client` dependency gives callers both the transport and the
// request/response ergonomics:
//
//   use shamir_client::builder::{query::Query, batch::Batch};
//   use shamir_client::builder::{filter::*, val::*, response::BatchResponseExt};
//   let req = Batch::new().query("u", Query::from("users")).build();
//   let resp = client.execute("db", req).await?;
//   let users: Vec<User> = resp.rows_as("u")?;
pub use shamir_query_builder as builder;
