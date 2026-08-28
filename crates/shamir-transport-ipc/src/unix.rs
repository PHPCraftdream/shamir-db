//! Unix domain socket implementation of [`crate::IpcListener`] / [`crate::IpcStream`].
//!
//! Access boundary: the socket file is created `0600` (owner read/write
//! only) immediately after `bind`. There is a narrow window between the
//! `bind` syscall (which creates the path in the filesystem) and the
//! subsequent `chmod` in which the file carries whatever the default mode
//! would be — but no `.await` point falls between the two calls, so no
//! other tokio task on this process can interleave, and no external process
//! has more than a few CPU instructions' worth of wall-clock time to both
//! discover the new path and connect. This is the same accepted-risk
//! pattern used broadly for Unix-socket daemons (the alternative — flipping
//! the process-global `umask` around `bind` — is unsafe in a multi-threaded
//! tokio runtime, where unrelated concurrent tasks creating files would
//! observe the wrong umask).

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener as TokioUnixListener, UnixStream};

/// Owner-only (`0600`) Unix domain socket listener.
pub struct IpcListener {
    inner: TokioUnixListener,
    path: PathBuf,
}

/// A connected Unix domain socket — implements `AsyncRead + AsyncWrite`,
/// so `shamir_transport_tcp::framing`'s frame helpers work on it directly.
pub type IpcStream = UnixStream;

/// Client-side connection type. On Unix, sockets have no client/server
/// type distinction (unlike Windows Named Pipes, where the client and
/// server ends are genuinely different OS handle types) — same alias as
/// [`IpcStream`].
pub type IpcClientStream = UnixStream;

/// Connect to a Unix domain socket at `path` as a client.
pub async fn connect(path: impl AsRef<Path>) -> io::Result<IpcClientStream> {
    UnixStream::connect(path).await
}

impl IpcListener {
    /// Bind a Unix domain socket at `path` and restrict it to `0600`
    /// (owner read/write only) — this IS the authorization boundary for
    /// the `binding_mode = 0x00` plain-transport handshake that follows
    /// (spec TRANSPORT_UNIX.md).
    pub async fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let inner = TokioUnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self { inner, path })
    }

    /// Accept the next incoming connection.
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(stream)
    }

    /// The filesystem path this listener is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IpcListener {
    /// Remove the socket file on shutdown — `bind` fails on a stale path
    /// left behind by an unclean process exit otherwise (the classic
    /// "Address already in use" for Unix sockets, which — unlike TCP — the
    /// kernel does not clean up on its own once the owning process is gone
    /// AND the path still exists on disk).
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
