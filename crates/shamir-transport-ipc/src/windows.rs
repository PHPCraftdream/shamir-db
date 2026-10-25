//! Windows Named Pipe implementation of [`crate::IpcListener`] / [`crate::IpcStream`].
//!
//! Access boundary: every pipe instance is created with an explicit
//! security descriptor restricting access to the CURRENT process token's
//! user SID only (`D:P(A;;GA;;;<sid>)` — protected DACL, Generic-All to
//! that one SID), built by the shared `shamir-private-dir` crate's
//! [`OwnerOnlySecurityDescriptor`]. This is the Windows analogue of the Unix listener's
//! `chmod 0600`: parity is "the OS account this server runs as", not "the
//! current logon session" (a service account has no stable logon SID
//! across restarts, so the user SID — not `OWNER RIGHTS`/`OW` and not the
//! logon SID — is the correct, stable anchor for a long-lived daemon).
//! `ServerOptions::reject_remote_clients` (tokio's own default: `true`) is
//! left at its default, which additionally blocks the pipe from being
//! reachable over SMB from a remote host at all, regardless of DACL.
//!
//! A `NamedPipeServer` handle serves exactly ONE client, unlike a socket
//! listener — after a client connects, [`IpcListener::accept`] must create
//! a fresh pending instance before the next `accept` call, or a second
//! connecting client would find no listener present.

use std::ffi::c_void;
use std::io;

use shamir_private_dir::OwnerOnlySecurityDescriptor;
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions,
};

/// A connected Named Pipe instance — implements `AsyncRead + AsyncWrite`,
/// so `shamir_transport_tcp::framing`'s frame helpers work on it directly.
pub type IpcStream = NamedPipeServer;

/// Client-side connection type. A Named Pipe's client and server ends are
/// genuinely different OS handle types (unlike a Unix socket, where both
/// ends share one type) — `NamedPipeClient`, not `NamedPipeServer`.
pub type IpcClientStream = NamedPipeClient;

/// `ERROR_PIPE_BUSY` (Win32 231): the server has no pending pipe instance
/// right now. It replenishes one at the start of every `accept`, so a
/// bounded yield-and-retry loop closes the rotation window instead of
/// surfacing the raw error to the caller (RV-TRANSPORT-IPC-466D0307).
const ERROR_PIPE_BUSY: i32 = 231;
const CONNECT_PIPE_BUSY_RETRIES: usize = 200;

/// Connect to the Named Pipe server at `name` as a client. No explicit
/// security attributes needed here — `CreateFile` (which `ClientOptions::open`
/// wraps) checks the CALLING process's token against the pipe's own DACL
/// (set at server creation, see [`OwnerOnlySecurityDescriptor`]) automatically;
/// there is nothing analogous to configure on the client's open call.
///
/// Retries a bounded number of times on `ERROR_PIPE_BUSY` (no pending server
/// instance during accept rotation) instead of surfacing the raw error.
pub async fn connect(name: &str) -> io::Result<IpcClientStream> {
    let mut attempts = 0;
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                attempts += 1;
                if attempts >= CONNECT_PIPE_BUSY_RETRIES {
                    return Err(e);
                }
                tokio::task::yield_now().await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Owner-only-DACL Named Pipe listener. See module docs for the security
/// model.
pub struct IpcListener {
    name: String,
    sec_desc: OwnerOnlySecurityDescriptor,
    /// The next pending (unconnected) pipe instance. `Some` between
    /// construction / the end of one `accept` and the start of the next.
    next: Option<NamedPipeServer>,
}

impl IpcListener {
    /// Create a Named Pipe server at `name` (e.g. `\\.\pipe\shamir-db`),
    /// restricted to the current process's own user account.
    pub async fn bind(name: impl Into<String>) -> io::Result<Self> {
        let name = name.into();
        let sec_desc = OwnerOnlySecurityDescriptor::for_current_user()?;
        let first = create_instance(&name, &sec_desc, /* first_pipe_instance */ true)?;
        Ok(Self {
            name,
            sec_desc,
            next: Some(first),
        })
    }

    /// Accept the next incoming connection. Internally rotates in a fresh
    /// pending instance so a subsequent `accept` call can serve the NEXT
    /// client — a single `NamedPipeServer` handle is one-client-only,
    /// unlike a socket listener.
    ///
    /// Cancellation-safe: the replacement pending instance is stored in
    /// `self.next` BEFORE awaiting the connect, so dropping this future at
    /// any await point leaves the listener usable for the next `accept`.
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        self.accept_with(|name, sec_desc| create_instance(name, sec_desc, false))
            .await
    }

    /// Testable core of [`IpcListener::accept`]: `create_replacement` is the
    /// hook that rotates in the next pending instance (the real one creates
    /// a fresh pipe); tests inject failures here to drive `accept` through
    /// its error path.
    pub(crate) async fn accept_with(
        &mut self,
        create_replacement: impl Fn(&str, &OwnerOnlySecurityDescriptor) -> io::Result<NamedPipeServer>,
    ) -> io::Result<IpcStream> {
        let pending = self
            .next
            .take()
            .expect("IpcListener::accept: no pending instance — invariant violated");
        match create_replacement(&self.name, &self.sec_desc) {
            Ok(replacement) => self.next = Some(replacement),
            Err(e) => {
                self.next = Some(pending);
                return Err(e);
            }
        }
        pending.connect().await?;
        Ok(pending)
    }

    /// The pipe name this listener is bound to (e.g. `\\.\pipe\shamir-db`).
    pub fn path(&self) -> &str {
        &self.name
    }
}

fn create_instance(
    name: &str,
    sec_desc: &OwnerOnlySecurityDescriptor,
    first_pipe_instance: bool,
) -> io::Result<NamedPipeServer> {
    // SAFETY: `sec_desc.as_security_attributes()` returns a pointer to a
    // `SECURITY_ATTRIBUTES` whose `lpSecurityDescriptor` points at a valid,
    // still-owned `SECURITY_DESCRIPTOR` (owned by `sec_desc`, which
    // outlives this call — it is a field of `self`, not a temporary). Byte
    // framing / pipe mode are unaffected by the security descriptor.
    unsafe {
        ServerOptions::new()
            .pipe_mode(PipeMode::Byte)
            .first_pipe_instance(first_pipe_instance)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                sec_desc.as_security_attributes() as *mut c_void,
            )
    }
}
