//! Repl transports and factories: the `ReplSource` trait (`source`, with
//! the `seed` sub-module), the wire transport, the in-process transport
//! used by tests, and the production factory. Re-exports only.

pub mod in_process;
pub mod prod_factory;
pub mod source;
pub mod wire_source;
