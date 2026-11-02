use crate::framing::scanner::{Scanner, Verdict};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

#[test]
fn every_split_rejects_http10_http2_bare_cr_and_invalid_head() {
    let cases: &[&[u8]] = &[
        b"GET / HTTP/1.0\r\nHost: x\r\n\r\n",
        b"GET / HTTP/2.0\r\nHost: x\r\n\r\n",
        b"GET / HTTP/1.1\r\nX: bare\rvalue\r\n\r\n",
        b"GET / HTTP/1.1\n\n",
        b"GET / HTTP/1.1\r\n folded: x\r\n\r\n",
    ];
    for input in cases {
        for split in 0..=input.len() {
            let mut scanner = Scanner::new();
            let verdict = scanner.feed(&input[..split], 8192, 64);
            let verdict = if verdict == Verdict::NeedMore {
                scanner.feed(&input[split..], 8192, 64)
            } else {
                verdict
            };
            assert_eq!(verdict, Verdict::BadRequest, "{input:?} split {split}");
        }
    }
}

#[test]
fn transfer_encoding_ows_is_semantically_trimmed() {
    for suffix in [b"chunked ".as_slice(), b"\tchunked", b"chunked\t"] {
        let mut request = b"POST / HTTP/1.1\r\nTransfer-Encoding: ".to_vec();
        request.extend_from_slice(suffix);
        request.extend_from_slice(b"\r\n\r\n");
        let mut scanner = Scanner::new();
        assert_eq!(scanner.feed(&request, 8192, 64), Verdict::Complete);
    }
}

#[test]
fn sticky_scanner_verdict_cannot_be_rewritten() {
    let mut scanner = Scanner::new();
    assert_eq!(
        scanner.feed(b"GET / HTTP/1.0\r\n\r\n", 8192, 64),
        Verdict::BadRequest
    );
    assert_eq!(
        scanner.feed(b"GET / HTTP/1.1\r\n\r\n", 8192, 64),
        Verdict::BadRequest
    );
}

#[tokio::test]
async fn malformed_head_gets_complete_canned_response() {
    let (mut client, server) = duplex(4096);
    let (_hold_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut guarded = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx);
    let task = tokio::spawn(async move {
        let mut head = [0; 128];
        let eof = guarded.read(&mut head).await.unwrap();
        assert_eq!(eof, 0);
    });
    client
        .write_all(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(text.contains("Cache-Control: no-store\r\n"));
    assert!(text.contains("X-Content-Type-Options: nosniff\r\n"));
    assert!(text.contains("Content-Type: application/msgpack\r\n"));
    assert!(text.contains("Connection: close\r\n"));
    assert!(text.ends_with("Content-Length: 0\r\n\r\n"));
}
