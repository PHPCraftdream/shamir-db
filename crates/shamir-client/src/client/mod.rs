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
mod handshake;
mod session;
pub(crate) mod transport;

pub mod cursor_stream;
pub mod subscription;

pub use handshake::{login, LoginChannels, LoginMode, LoginOptions, LoginOutcome, LoginScope};
pub use session::{ApiToken, AuthKind, TokenHttpClient, TokenHttpOptions, TokenWsOptions};
pub use transport::token::{TokenEndpoint, TokenTrust};

pub use session::{
    bound_until, resolve_host, BrowserResumeOptions, BrowserWsOptions, Client, ConnectLocalOptions,
    ConnectOptions, ResumeOptions, SecureResumeOptions,
};

#[cfg(test)]
pub(crate) use api::admin_request_tag_for_test as admin_request_tag;
#[cfg(test)]
pub(crate) use api::encode_named_into;
pub(crate) use api::should_advertise_interner_epochs;
#[cfg(test)]
pub(crate) use handshake::{
    resume_wire_exchange, run_scram, verify_resume_identity_proof, ScramRequest,
};
#[cfg(test)]
pub(crate) use session::entry::resume_security::{
    read_resume_frame, serialize_resume, ProtectedResumeBuffer,
};
#[cfg(test)]
pub(crate) use session::entry::take_resume_wipes;

#[cfg(test)]
pub(crate) use transport::{
    await_pending_response, reader_task, HookPoint, Lifecycle, LifecycleHooks, ShutdownGuard,
};
