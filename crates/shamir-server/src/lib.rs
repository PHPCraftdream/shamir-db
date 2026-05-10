//! ShamirDB production server library.
//!
//! Orchestrates `shamir-connect` (SCRAM-Argon2id auth protocol) with the
//! TCP/WS transport bindings and durable state stores into a runnable
//! server. Modules:
//!
//! - [`config`] — Ktav-based configuration schema.
//! - [`server_meta`] — durable storage for server secrets, Ed25519
//!   identity, ticket key, audit chain key, bootstrap state.
//! - [`user_directory`] — durable `UserDirectory` impl backed by redb.
//! - [`audit_appender`] — durable `AuditAppender` impl with HMAC-chained
//!   JSON-line log + checkpoint.
//! - [`connection`] — accept loop + per-connection orchestration.
//! - [`scheduler`] — background tasks (gc, checkpoint, identity finalize).
//! - [`db_handler`] — `RequestHandler` bridge to `ShamirDb` query layer.

pub mod audit_appender;
pub mod bootstrap;
pub mod config;
pub mod connection;
pub mod db_handler;
pub mod scheduler;
pub mod server;
pub mod server_meta;
pub mod tls;
pub mod user_directory;
pub mod version;
