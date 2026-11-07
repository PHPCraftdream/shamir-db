use super::super::guarded_io::GuardedIo;
use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn tunnel_passes_arbitrary_binary_bytes_both_directions_after_101() {
    let (mut client, server) = duplex(1024);
    let (_hold_tx, holds) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, holds);
    let task = tokio::spawn(async move {
        let request = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        let mut head = vec![0_u8; request.len()];
        guarded.read_exact(&mut head).await.expect("read request");
        assert_eq!(head, request);
        guarded.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write 101");
        let mut inbound = [0_u8; 4];
        guarded.read_exact(&mut inbound).await.expect("read tunnel");
        assert_eq!(inbound, [0, 0xff, 0x80, 7]);
        guarded
            .write_all(&[0xff, 0, 0x80, 9])
            .await
            .expect("write tunnel");
    });
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write request");
    let mut collected = Vec::new();
    let mut byte = [0_u8; 1];
    tokio::time::timeout(Duration::from_secs(5), async {
        while !collected.ends_with(b"\r\n\r\n") {
            client.read_exact(&mut byte).await.expect("read response");
            collected.push(byte[0]);
            assert!(collected.len() <= 8192, "response head exceeds limit");
        }
    })
    .await
    .expect("response head timeout");
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
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("server task timeout")
        .expect("server task");
}

#[tokio::test]
async fn bytes_before_101_remain_inside_the_http_scanner() {
    let (mut client, server) = duplex(1024);
    let (_hold_tx, holds) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, holds);
    let task = tokio::spawn(async move {
        let mut head = [0_u8; 27];
        guarded.read_exact(&mut head).await.expect("read HTTP head");
        let mut next = [0_u8; 1];
        tokio::time::timeout(Duration::from_millis(200), guarded.read(&mut next))
            .await
            .expect_err("pre-101 bytes must not be released");
    });
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n\x82\x85")
        .await
        .expect("write request and early WS bytes");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("server task timeout")
        .expect("server task");
}

#[tokio::test(start_paused = true)]
async fn idle_tunnel_never_rearms_http_deadlines() {
    let (mut client, server) = duplex(1024);
    let (_hold_tx, holds) = tokio::sync::mpsc::channel(1);
    let mut guarded = GuardedIo::new(server, 8192, 64, false, false, holds).with_timeouts(
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(1),
    );
    let task = tokio::spawn(async move {
        let request = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        let mut head = vec![0; request.len()];
        guarded.read_exact(&mut head).await.expect("read head");
        guarded.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write 101");
        tokio::time::advance(Duration::from_secs(10)).await;
        let mut incoming = [0; 4];
        tokio::time::timeout(Duration::from_secs(1), guarded.read_exact(&mut incoming))
            .await
            .expect("tunnel read remains live")
            .expect("read bytes");
        guarded
            .write_all(b"pong")
            .await
            .expect("tunnel write remains live");
    });
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write head");
    let mut response = Vec::new();
    let mut byte = [0; 1];
    while !response.ends_with(b"\r\n\r\n") {
        client.read_exact(&mut byte).await.expect("read response");
        response.push(byte[0]);
    }
    client.write_all(b"ping").await.expect("write tunnel data");
    let mut reply = [0; 4];
    client
        .read_exact(&mut reply)
        .await
        .expect("read tunnel data");
    assert_eq!(&reply, b"pong");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("server task timeout")
        .expect("server task");
}
