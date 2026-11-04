//! Transport axis of the client: the write sink, the dial helpers and the
//! channel binding handed to the handshake.

mod binding;
mod dial;
mod dial_ws;
pub mod token;
mod write_sink;
mod ws_stream;

// Re-exports: the mux/lifecycle/reader code lives in `client/session/mux/`
// but `crate::client::transport::{...}` paths must keep resolving for the
// subscription registry and the guard tests.
#[cfg(test)]
pub(crate) use crate::client::handshake::wire::{
    WireAuthInit, WireAuthOk, WireChallenge, WireClientProof, WireResumeInit, WireResumeOk,
    WireResumeProofOk,
};
#[cfg(test)]
pub(crate) use crate::client::session::mux::hooks::HookPoint;
pub(crate) use crate::client::session::mux::lifecycle::{
    ClosedState, Lifecycle, LifecycleHooks, ShutdownGuard,
};
pub(crate) use crate::client::session::mux::reader::{await_pending_response, reader_task};

pub(crate) use binding::ChannelBinding;
pub(crate) use dial::{dial_ipc, dial_tcp_plain, dial_tcp_tls, Dialed};
pub(crate) use dial_ws::dial_ws_native;
pub(crate) use write_sink::WriteSink;

#[cfg(test)]
pub(crate) use ws_stream::WsByteStream;

#[cfg(test)]
pub(crate) use dial::connect_with_timeout;
