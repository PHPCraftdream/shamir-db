//! Session-scoped connection state: `ConnectionContext`, the in-flight
//! guard and the handshake wire types.

pub(crate) mod auth_ok_hints;
pub(crate) mod conn_tasks;
pub(crate) mod connection_context;
pub(crate) mod in_flight_guard;
pub(crate) mod resume;
pub(crate) mod wire;

pub use conn_tasks::ConnTasks;
pub use connection_context::ConnectionContext;
