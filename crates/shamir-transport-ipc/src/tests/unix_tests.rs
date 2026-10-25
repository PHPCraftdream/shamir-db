use std::io;
use std::os::unix::fs::PermissionsExt;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::IpcListener;

#[tokio::test]
async fn bind_restricts_socket_file_to_owner_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-test.sock");
    let listener = IpcListener::bind(&path).await.expect("bind");

    let mode = std::fs::metadata(listener.path())
        .expect("stat socket file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "socket file must be owner-rw-only");
}

#[tokio::test]
async fn accept_and_round_trip_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-test.sock");
    let mut listener = IpcListener::bind(&path).await.expect("bind");
    let client_path = path.clone();

    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept");
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("write");
    });

    let mut client = UnixStream::connect(&client_path).await.expect("connect");
    client.write_all(b"hello").await.expect("write");
    let mut buf = [0u8; 5];
    client.read_exact(&mut buf).await.expect("read");
    assert_eq!(&buf, b"world");

    server.await.expect("server task");
}

#[tokio::test]
async fn drop_removes_the_socket_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-test.sock");
    let listener = IpcListener::bind(&path).await.expect("bind");
    assert!(path.exists());
    drop(listener);
    assert!(!path.exists(), "Drop must unlink the socket path");
}

/// RV-TRANSPORT-IPC-3C21F8FA: if restricting the socket file's permissions
/// fails after `bind` created the file, the socket file must be removed
/// before the error propagates — otherwise it leaks with default (wide)
/// permissions and poisons the next start ("address in use").
#[tokio::test]
async fn rv_transport_ipc_3c21f8fa_failed_chmod_removes_socket_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-test.sock");

    let result = crate::unix::IpcListener::bind_with(&path, |_| {
        Err(io::Error::other("injected chmod failure"))
    })
    .await;

    assert!(result.is_err(), "bind_with must propagate the perms error");
    assert!(
        !path.exists(),
        "socket file must be removed on set_permissions failure"
    );

    // Verify safe rebinding after cleanup: the real `bind` (real chmod)
    // must succeed on the same path now that the failed socket file is gone.
    let rebound = IpcListener::bind(&path).await;
    assert!(rebound.is_ok(), "re-bind after cleanup must succeed");
    drop(rebound);
}

/// RV-TRANSPORT-IPC-07D10AFD: bind-on-existing-path semantics pin — a
/// second `bind` on a LIVE listener's path must fail, must NOT clobber the
/// existing socket file, and the original listener must keep serving.
#[tokio::test]
async fn rv_transport_ipc_07d10afd_bind_existing_path_fails_without_clobbering() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-test.sock");
    let mut listener = IpcListener::bind(&path).await.expect("first bind");

    let second = IpcListener::bind(&path).await;
    assert!(
        second.is_err(),
        "bind on an existing live socket path must fail"
    );
    assert!(
        path.exists(),
        "the failed bind must not remove the original socket file"
    );

    // The original listener still serves clients after the collision.
    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
    });
    let mut client = crate::connect(&path).await.expect("client connect");
    client.write_all(b"ping").await.expect("client write");
    server.await.expect("server task");
}
