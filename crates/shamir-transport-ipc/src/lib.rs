//! ShamirDB local-IPC transport binding (spec TRANSPORT_UNIX.md).
//!
//! Two OS primitives, one API: Unix domain sockets (POSIX) and Windows
//! Named Pipes are exposed under the SAME public names — [`IpcListener`],
//! [`IpcStream`] (server-accepted connections), [`IpcClientStream`] and
//! [`connect`] (client-side) — so callers (`shamir-server`, `shamir-client`)
//! never branch on `cfg(unix)`/`cfg(windows)` themselves. Exactly one of the
//! two implementation modules is compiled per target (per the workspace's
//! single-binary-per-OS convention — see RUNTIME_MODES.md), so this is a
//! compile-time dispatch, not a runtime enum.
//!
//! Framing is NOT reimplemented here: both `UnixStream` and `NamedPipeServer`
//! implement `AsyncRead + AsyncWrite`, so `shamir_transport_tcp::framing`'s
//! `[u32_be length][msgpack]` helpers work directly against [`IpcStream`]
//! without modification.
//!
//! Security boundary: `binding_mode = 0x00` (no TLS, no channel binding) —
//! per TRANSPORT_UNIX.md, the OS-level access boundary (Unix file
//! permissions / Windows DACL, both restricted to the connecting process's
//! own user account) stands in for the transport encryption that TCP+TLS
//! listeners rely on. [`IpcListener::bind`] enforces this boundary itself;
//! it is not an opt-in the caller can forget.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{connect, IpcClientStream, IpcListener, IpcStream};
#[cfg(windows)]
pub use windows::{connect, IpcClientStream, IpcListener, IpcStream};

#[cfg(test)]
mod tests;
