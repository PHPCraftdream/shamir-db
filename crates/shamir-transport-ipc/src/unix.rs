//! Unix domain socket implementation of [`crate::IpcListener`] / [`crate::IpcStream`].
//!
//! Socket parents are checked for effective-UID ownership and absence of
//! group/world write bits before bind. This pathname check does not prevent
//! same-UID races or replacement of an ancestor directory; identity-guarded
//! cleanup protects against a replaced final socket entry, not those races.

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;

use tokio::net::{UnixListener as TokioUnixListener, UnixStream};

// POSIX uid_t is an unsigned int on supported Unix targets.
unsafe extern "C" {
    fn geteuid() -> u32;
}

/// Owner-only (`0600`) Unix domain socket listener.
pub struct IpcListener {
    inner: TokioUnixListener,
    path: String,
    identity: (u64, u64),
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
pub async fn connect(path: &str) -> io::Result<IpcClientStream> {
    crate::validate_endpoint(path)?;
    UnixStream::connect(path).await
}

impl IpcListener {
    /// Bind a Unix domain socket at `path` and restrict it to `0600`
    /// (owner read/write only) — this IS the authorization boundary for
    /// the `binding_mode = 0x00` plain-transport handshake that follows
    /// (spec TRANSPORT_UNIX.md).
    pub async fn bind(path: &str) -> io::Result<Self> {
        crate::validate_endpoint(path)?;
        Self::bind_with(Path::new(path), |p| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
        })
        .await
    }

    /// Testable core of [`IpcListener::bind`]: `set_perms` is the hook that
    /// restricts the socket file (the real one applies `0600`); tests inject
    /// failures here.
    pub(crate) async fn bind_with(
        path: &Path,
        set_perms: impl Fn(&Path) -> io::Result<()>,
    ) -> io::Result<Self> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("/"));
        let parent_metadata = std::fs::metadata(parent)?;
        // SAFETY: geteuid has no preconditions and returns the effective UID of this process.
        let effective_uid = unsafe { geteuid() };
        if parent_metadata.uid() != effective_uid || parent_metadata.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unix IPC socket parent must be owned by the effective user and not group/world writable",
            ));
        }
        let inner = match TokioUnixListener::bind(path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                reclaim_stale_socket(path).await?;
                TokioUnixListener::bind(path)?
            }
            Err(error) => return Err(error),
        };
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            drop(inner);
            return Err(io::Error::other(
                "bound IPC endpoint was replaced before ownership was recorded",
            ));
        }
        let identity = (metadata.dev(), metadata.ino());
        let parent_after = match std::fs::metadata(parent) {
            Ok(metadata) => metadata,
            Err(e) => {
                remove_if_identity(path, identity);
                drop(inner);
                return Err(e);
            }
        };
        if parent_after.dev() != parent_metadata.dev()
            || parent_after.ino() != parent_metadata.ino()
            || parent_after.uid() != effective_uid
            || parent_after.mode() & 0o022 != 0
        {
            remove_if_identity(path, identity);
            drop(inner);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unix IPC socket parent changed or is no longer trusted",
            ));
        }
        if let Err(e) = set_perms(path) {
            // Cleanup only the socket created by this bind; never unlink a replacement.
            remove_if_identity(path, identity);
            drop(inner);
            return Err(e);
        }
        Ok(Self {
            inner,
            path: path.to_string_lossy().into_owned(),
            identity,
        })
    }

    /// Accept the next incoming connection.
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(stream)
    }

    /// The filesystem path this listener is bound to.
    pub fn path(&self) -> &str {
        &self.path
    }
}

fn remove_if_identity(path: &Path, identity: (u64, u64)) {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() && (metadata.dev(), metadata.ino()) == identity {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn reclaim_stale_socket(path: &Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "IPC endpoint exists and is not a socket",
        ));
    }
    // Non-blocking probe: a full accept backlog reports WouldBlock instead of
    // stalling the runtime; only a refused connect proves the socket is stale.
    match UnixStream::connect(path).await {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "IPC socket is live",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "IPC socket is live",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(error) => return Err(error),
    }
    let current = std::fs::symlink_metadata(path)?;
    if !current.file_type().is_socket()
        || (current.dev(), current.ino()) != (metadata.dev(), metadata.ino())
    {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "IPC socket changed during stale-endpoint check",
        ));
    }
    std::fs::remove_file(path)
}

impl Drop for IpcListener {
    fn drop(&mut self) {
        remove_if_identity(Path::new(&self.path), self.identity);
    }
}
