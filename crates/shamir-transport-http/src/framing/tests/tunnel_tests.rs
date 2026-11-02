use super::super::guarded_io::GuardedIo;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn tunnel_passes_arbitrary_binary_bytes_both_directions_after_101() {
    let (mut client, server) = duplex(1024);
    let (_hold_tx, holds) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, holds);
    let task = tokio::spawn(async move {
        let request = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        let mut head = vec![0_u8; request.len()];
        guarded
            .read_exact(&mut head)
            .await
            .expect("read complete request head");
        assert_eq!(head, request);
        guarded.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write upgrade response");
        let mut inbound = [0_u8; 4];
        guarded
            .read_exact(&mut inbound)
            .await
            .expect("read tunnel bytes");
        assert_eq!(inbound, [0, 0xff, 0x80, 7]);
        guarded
            .write_all(&[0xff, 0, 0x80, 9])
            .await
            .expect("write tunnel bytes");
    });
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write request");
    let mut response = [0_u8; 256];
    let mut collected = Vec::new();
    while !collected.ends_with(b"\r\n\r\n") {
        let n = client.read(&mut response).await.expect("read response");
        collected.extend_from_slice(&response[..n]);
    }
    client
        .write_all(&[0, 0xff, 0x80, 7])
        .await
        .expect("write raw bytes");
    let mut echoed = [0_u8; 4];
    client
        .read_exact(&mut echoed)
        .await
        .expect("read raw bytes");
    assert_eq!(echoed, [0xff, 0, 0x80, 9]);
    task.await.expect("server task");
}

#[tokio::test]
async fn bytes_before_101_remain_inside_the_http_scanner() {
    let (mut client, server) = duplex(1024);
    let (_hold_tx, holds) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, holds);
    let task = tokio::spawn(async move {
        let mut head = [0_u8; 27];
        guarded
            .read_exact(&mut head)
            .await
            .expect("read complete HTTP request head");
        assert_eq!(&head[..14], b"GET / HTTP/1.1");
        // Hyper owns request/response sequencing after the validated head. No WebSocket
        // bytes are admitted by GuardedIo before it has observed the written 101.
        let mut next = [0_u8; 1];
        guarded
            .read(&mut next)
            .await
            .expect("read remains parked until response");
    });
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write complete HTTP head");
    task.abort();
    let _ = task.await;
}
