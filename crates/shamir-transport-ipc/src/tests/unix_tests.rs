use std::io;
use std::os::unix::fs::PermissionsExt;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::{validate_endpoint, IpcListener};

#[tokio::test]
async fn rv_transport_ipc_e0b387b3_refuses_untrusted_parent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("socket.sock");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777))
        .expect("make parent writable");
    let error = match IpcListener::bind(path.to_str().expect("utf8 path")).await {
        Ok(_) => panic!("writable parent unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!path.exists());
}

#[tokio::test]
async fn rv_transport_ipc_e0b387b3_recovers_stale_socket_but_preserves_files_and_live_sockets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale = dir.path().join("stale.sock");
    let old = std::os::unix::net::UnixListener::bind(&stale).expect("create stale socket");
    drop(old);
    let listener = IpcListener::bind(stale.to_str().expect("utf8 path"))
        .await
        .expect("reclaim stale socket");
    drop(listener);

    let regular = dir.path().join("regular");
    std::fs::write(&regular, b"preserve").expect("write file");
    assert!(IpcListener::bind(regular.to_str().expect("utf8 path"))
        .await
        .is_err());
    assert_eq!(std::fs::read(regular).expect("file remains"), b"preserve");

    let live_path = dir.path().join("live.sock");
    let live = IpcListener::bind(live_path.to_str().expect("utf8 path"))
        .await
        .expect("live bind");
    assert!(IpcListener::bind(live_path.to_str().expect("utf8 path"))
        .await
        .is_err());
    drop(live);
}

#[tokio::test]
async fn rv_transport_ipc_dd5d7b60_drop_preserves_replacement_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("owned.sock");
    let old = IpcListener::bind(path.to_str().expect("utf8 path"))
        .await
        .expect("first bind");
    std::fs::remove_file(&path).expect("replace path");
    let replacement = IpcListener::bind(path.to_str().expect("utf8 path"))
        .await
        .expect("replacement bind");
    drop(old);
    assert!(
        path.exists(),
        "old listener must not unlink replacement socket"
    );
    let mut replacement = replacement;
    let server =
        tokio::spawn(async move { replacement.accept().await.expect("replacement accepts") });
    let client = crate::connect(path.to_str().expect("utf8 path"))
        .await
        .expect("replacement remains reachable");
    drop(client);
    drop(server.await.expect("accepted connection"));
}

#[test]
fn rv_transport_ipc_bd4ebef7_unix_endpoint_validation_rules_and_boundaries() {
    assert!(validate_endpoint("/tmp/socket").is_ok());
    assert!(validate_endpoint("relative").is_err());
    assert!(validate_endpoint("/tmp/../socket").is_err());
    let max = if cfg!(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )) {
        104
    } else {
        108
    };
    assert!(validate_endpoint(&format!("/{}", "x".repeat(max - 2))).is_ok());
    assert!(validate_endpoint(&format!("/{}", "x".repeat(max - 1))).is_err());
    assert!(validate_endpoint("/tmp/with\0nul").is_err());
}

#[tokio::test]
async fn rv_transport_ipc_bd4ebef7_entrypoints_reject_invalid_addresses_before_io() {
    let err = match crate::connect("relative").await {
        Ok(_) => panic!("relative path unexpectedly connected"),
        Err(error) => error,
    };
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    let dir = tempfile::tempdir().expect("tempdir");
    let relative = format!("shamir-ipc-relative-{}.sock", std::process::id());
    let relative_path = std::env::current_dir()
        .expect("current directory")
        .join(&relative);
    let relative_error = match IpcListener::bind(&relative).await {
        Ok(_) => panic!("relative path unexpectedly bound"),
        Err(error) => error,
    };
    assert_eq!(relative_error.kind(), io::ErrorKind::InvalidInput);
    assert!(
        !relative_path.exists(),
        "invalid relative bind creates no entry"
    );
    std::fs::create_dir(dir.path().join("x")).expect("create traversal target directory");
    let traversal = dir.path().join("x").join("..").join("traversal.sock");
    let traversal = traversal.to_string_lossy().into_owned();
    assert!(traversal.contains(".."));
    let error = match IpcListener::bind(&traversal).await {
        Ok(_) => panic!("parent traversal unexpectedly bound"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    let connect_error = match crate::connect(&traversal).await {
        Ok(_) => panic!("parent traversal unexpectedly connected"),
        Err(error) => error,
    };
    assert_eq!(connect_error.kind(), io::ErrorKind::InvalidInput);
    assert!(!dir.path().join("traversal.sock").exists());
}
#[tokio::test]
async fn bind_restricts_socket_file_to_owner_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-test.sock");
    let listener = IpcListener::bind(path.to_str().expect("utf8 path"))
        .await
        .expect("bind");

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
    let mut listener = IpcListener::bind(path.to_str().expect("utf8 path"))
        .await
        .expect("bind");
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
    let listener = IpcListener::bind(path.to_str().expect("utf8 path"))
        .await
        .expect("bind");
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
    let rebound = IpcListener::bind(path.to_str().expect("utf8 path")).await;
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
    let mut listener = IpcListener::bind(path.to_str().expect("utf8 path"))
        .await
        .expect("first bind");

    let second = IpcListener::bind(path.to_str().expect("utf8 path")).await;
    assert!(second.is_err());
    assert!(
        path.exists(),
        "the failed bind must not remove the original socket file"
    );

    // The original listener still serves clients after the collision. The
    // failed bind's liveness probe is queued first: accept it, expect EOF.
    let server = tokio::spawn(async move {
        let mut probe = listener.accept().await.expect("accept probe");
        let mut rest = Vec::new();
        probe.read_to_end(&mut rest).await.expect("probe read");
        assert!(rest.is_empty(), "the liveness probe sends no bytes");
        let mut stream = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
    });
    let mut client = crate::connect(path.to_str().expect("utf8 path"))
        .await
        .expect("client connect");
    client.write_all(b"ping").await.expect("client write");
    server.await.expect("server task");
}
