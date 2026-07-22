//! ShamirDB production server library.
//!
//! Orchestrates `shamir-connect` (SCRAM-Argon2id auth protocol) with the
//! TCP/WS transport bindings and durable state stores into a runnable
//! server. Modules:
//!
//! - [`config`] — Ktav-based configuration schema.
//! - [`server_meta`] — durable storage for server secrets, Ed25519
//!   identity, ticket key, audit chain key, bootstrap state.
//! - [`user_directory`] — durable `UserDirectory` impl backed by fjall.
//! - [`audit_appender`] — durable `AuditAppender` impl with HMAC-chained
//!   tab-separated log + checkpoint.
//! - [`connection`] — accept loop + per-connection orchestration.
//! - [`scheduler`] — background tasks (gc, checkpoint, identity finalize).
//! - [`db_handler`] — `RequestHandler` bridge to `ShamirDb` query layer.

pub mod access_tree;
pub mod audit_appender;
pub mod backup;
pub mod bootstrap;
pub mod byte_budget;
pub mod config;
pub mod conn_limiter;
pub mod connection;
pub mod cursor_registry;
pub mod db_handler;
pub mod framer;
pub mod logging;
pub mod observability;
pub mod ports;
pub mod replication;
pub mod restore;
pub mod runtime;
pub mod scheduler;
pub mod server;
pub mod server_meta;
pub mod service;
pub mod subscriptions;
pub mod tables_registry;
pub mod tls;
pub mod tx_registry;
pub mod user_directory;
pub mod version;
#[cfg(windows)]
pub mod windows_service;

#[cfg(test)]
mod tests;
