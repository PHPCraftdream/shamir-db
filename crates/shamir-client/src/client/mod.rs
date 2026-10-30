//! Async TLS+SCRAM client with rid-demux multiplexer.
//!
//! After the handshake a background reader task owns the `ReadHalf` and
//! routes every incoming frame to the caller that is waiting for its
//! `request_id` via a `oneshot` channel stored in a pending map.
//!
//! Concurrent callers can issue multiple requests in flight simultaneously:
//! each `execute`/`ping` call registers its oneshot **before** writing to
//! the socket, sends, then awaits the oneshot independently. Responses arrive in
//! completion order (not send order); the reader task matches each by `rid`.

mod api;
mod connect;
mod options;
pub(crate) mod transport;

pub mod cursor_stream;
pub mod subscription;

pub use connect::Client;
pub use options::{
    bound_until, resolve_host, ConnectLocalOptions, ConnectOptions, ResumeOptions,
    SecureResumeOptions,
};

#[cfg(test)]
pub(crate) use api::should_advertise_interner_epochs;
#[cfg(test)]
pub(crate) use connect::resume_security::{
    read_resume_frame, serialize_resume, ProtectedResumeBuffer,
};
#[cfg(test)]
pub(crate) use connect::{resume_wire_exchange, take_resume_wipes, verify_resume_identity_proof};

#[cfg(test)]
pub(crate) use transport::{
    await_pending_response, reader_task, HookPoint, Lifecycle, LifecycleHooks, ShutdownGuard,
};
