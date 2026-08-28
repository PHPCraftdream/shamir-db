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
