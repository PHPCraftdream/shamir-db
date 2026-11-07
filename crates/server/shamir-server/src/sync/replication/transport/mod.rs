//! Repl transports and factories: the `ReplSource` trait (`source`, with
//! the `seed` sub-module), the wire transport, the in-process transport
//! used by tests, and the production factory. Re-exports only.

pub mod in_process;
pub mod prod_factory;
#[path = "config/replication_config.rs"]
pub(crate) mod replication_config;
#[path = "config/replicator_password.rs"]
pub(crate) mod replicator_password;
pub mod source;
pub mod wire_source;
