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

mod boot;
pub mod durability;
mod engine;
pub(crate) use shamir_server_security as security;
pub(crate) use shamir_sync as sync;

pub(crate) use boot::maintenance_lock;
pub use boot::tokens;
pub(crate) use boot::transactions_config;
pub use boot::{
    access_tree, bootstrap, config, observability, restore, runtime, scheduler, service, version,
};
pub use connection::conn_limiter;
pub use durability::{backup, tx_registry};
pub use engine::{
    cursor_registry, db_handler, server, server_meta, subscriptions, tables_registry,
};
pub use logging::audit_appender;
pub use observability::doctor;
pub use observability::logging;
pub use security::{byte_budget, http_login, http_rest, http_ws, tls, user_directory};
#[cfg(windows)]
pub use service::windows_service;
pub use sync::{connection, framer, ports, replication, replication_seed};

#[cfg(test)]
mod tests;
