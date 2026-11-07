//! Runtime subsystem: boot orchestration, configuration schema, background
//! scheduling, transaction policy and versioning.

mod policy;

pub mod bootstrap;
pub mod config;
pub mod runtime;
pub mod scheduler;

pub(crate) use policy::transactions_config;
pub use policy::version;
