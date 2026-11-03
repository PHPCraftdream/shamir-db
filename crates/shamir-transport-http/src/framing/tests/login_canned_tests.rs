use crate::framing::guarded_io::GuardedIo;
use std::sync::{atomic::AtomicU64, Arc};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn enabled_canned_rejection_uses_msgpack_error_and_pragma() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, rx).with_login_errors(true);
    let task = tokio::spawn(async move {
        let mut request = [0; 64];
        let _ = guarded.read(&mut request).await;
    });
    client.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.read_to_end(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    task.await.unwrap();
    assert!(bytes
        .windows(b"Pragma: no-cache".len())
        .any(|w| w == b"Pragma: no-cache"));
    assert!(bytes.ends_with(b"\x81\xa5error\xafinvalid_request"));
}

#[tokio::test]
async fn bearer_canned_rejection_remains_unchanged_without_pragma() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, rx);
    let task = tokio::spawn(async move {
        let mut request = [0; 64];
        let _ = guarded.read(&mut request).await;
    });
    client.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.read_to_end(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    task.await.unwrap();
    assert!(!bytes
        .windows(b"Pragma: no-cache".len())
        .any(|w| w == b"Pragma: no-cache"));
    assert!(bytes.ends_with(b"Content-Length: 0\r\n\r\n"));
}

#[tokio::test(start_paused = true)]
async fn shared_write_timeout_expires_stalled_canned_rejection() {
    let (mut client, server) = duplex(8);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let override_ms = Arc::new(AtomicU64::new(5));
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_login_errors(true)
        .with_write_timeout_override(override_ms.clone());
    let mut request = [0; 64];
    let task = tokio::spawn(async move { guarded.read(&mut request).await });
    client.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    tokio::time::advance(std::time::Duration::from_millis(5)).await;
    assert!(task.await.unwrap().is_err());
    let mut output = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.read_to_end(&mut output),
    )
    .await
    .unwrap()
    .unwrap();
}
