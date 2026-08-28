use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;

use crate::IpcListener;

/// Unique-enough per-test pipe name — Windows named pipes share one
/// process-wide namespace (`\\.\pipe\...`), so a fixed literal name would
/// collide across tests run in the same nextest process.
fn test_pipe_name(tag: &str) -> String {
    format!(
        r"\\.\pipe\shamir-transport-ipc-test-{tag}-{}",
        std::process::id()
    )
}

#[tokio::test]
async fn accept_and_round_trip_bytes() {
    let name = test_pipe_name("roundtrip");
    let mut listener = IpcListener::bind(&name).await.expect("bind");
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept");
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("write");
    });

    let mut client = ClientOptions::new()
        .open(&client_name)
        .expect("open client");
    client.write_all(b"hello").await.expect("write");
    let mut buf = [0u8; 5];
    client.read_exact(&mut buf).await.expect("read");
    assert_eq!(&buf, b"world");

    server.await.expect("server task");
}

#[tokio::test]
async fn listener_serves_a_second_client_after_the_first_disconnects() {
    let name = test_pipe_name("second-client");
    let mut listener = IpcListener::bind(&name).await.expect("bind");
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let mut stream = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.expect("read");
            assert_eq!(&buf, b"ping");
        }
    });

    for _ in 0..2 {
        let mut client = ClientOptions::new()
            .open(&client_name)
            .expect("open client");
        client.write_all(b"ping").await.expect("write");
        drop(client);
    }

    server.await.expect("server task");
}
