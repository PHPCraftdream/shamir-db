use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Duration;

pub(super) async fn raw_response(addr: SocketAddr, request: &str) -> (String, Vec<u8>) {
    let mut stream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    tokio::time::timeout(
        Duration::from_secs(15),
        stream.write_all(request.as_bytes()),
    )
    .await
    .expect("write timeout")
    .expect("write request");
    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let count = stream.read(&mut buffer).await.expect("read response");
            assert_ne!(count, 0, "response closed before headers");
            response.extend_from_slice(&buffer[..count]);
            if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
                break end;
            }
            assert!(response.len() < 8192, "oversized response headers");
        }
    })
    .await
    .expect("response timeout");
    let headers = String::from_utf8_lossy(&response[..header_end]).into_owned();
    let mut body = response[header_end + 4..].to_vec();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);
    while body.len() < content_length {
        let count = tokio::time::timeout(Duration::from_secs(15), stream.read(&mut buffer))
            .await
            .expect("body timeout")
            .expect("read body");
        assert_ne!(count, 0, "response ended early");
        body.extend_from_slice(&buffer[..count]);
    }
    (headers, body)
}

pub(super) fn upgrade(path: &str, host: &str, origin: Option<&str>, extra: &str) -> String {
    let origin = origin.map_or(String::new(), |value| format!("Origin: {value}\r\n"));
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{origin}Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n{extra}\r\n")
}
