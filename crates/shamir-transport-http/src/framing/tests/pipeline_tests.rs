use crate::framing::guarded_io::GuardedIo;
use std::future::Future;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

#[tokio::test(start_paused = true)]
async fn malformed_second_head_waits_for_first_response_flush_and_then_is_canned() {
    let invalid = b"GET /shamir/v1/ping HTTP/1.1\n\n";
    for invalid in [
        invalid.as_slice(),
        b"GET / HTTP/1.1\r\n folded: x\r\n\r\n",
        b"GET / HTTP/1.1\r\nX: bare\rvalue\r\n\r\n",
        b"GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
        b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"GET / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"POST / HTTP/1.1\r\nContent-Length: 18446744073709551616\r\n\r\n",
        b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
        b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked, gzip\r\n\r\n",
    ] {
        for split in 0..=invalid.len() {
            let (client, server) = duplex(8192);
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let mut guard = GuardedIo::new(server, 8192, 64, false, false, rx);
            let first = b"GET /shamir/v1/ping HTTP/1.1\r\nHost: x\r\n\r\n";
            let prefix = invalid[..split].to_vec();
            let suffix = invalid[split..].to_vec();
            let writer = tokio::spawn(async move {
                let mut client = client;
                client.write_all(first).await.unwrap();
                client.write_all(&prefix).await.unwrap();
                if let Err(error) = client.write_all(&suffix).await {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::NotConnected
                    ));
                }
                let mut response = Vec::new();
                client.read_to_end(&mut response).await.unwrap();
                response
            });
            let mut head = [0; 512];
            let n = guard.read(&mut head).await.unwrap();
            assert_eq!(n, first.len());
            assert!(head[..n].ends_with(b"\r\n\r\n"));
            let mut first_response = [0; 128];
            let mut first_response_read = Box::pin(guard.read(&mut first_response));
            assert!(
                std::future::poll_fn(|cx| {
                    std::task::Poll::Ready(first_response_read.as_mut().poll(cx).is_pending())
                })
                .await
            );
            drop(first_response_read);
            guard
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            guard.flush().await.unwrap();
            let mut second = [0; 128];
            assert_eq!(guard.read(&mut second).await.unwrap(), 0);
            drop(guard);
            let response = String::from_utf8_lossy(&writer.await.unwrap()).to_string();
            assert!(
                response.starts_with("HTTP/1.1 200 OK\r\n"),
                "split {split}: {response}"
            );
            assert_eq!(
                response.strip_prefix("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap(),
                "HTTP/1.1 400 Bad Request\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: application/msgpack\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                "split {split}: {response}"
            );
            drop(tx);
        }
    }
}

#[tokio::test]
async fn oversized_second_head_gets_431_and_closes_after_first_response() {
    for split in 0..=b"GET /this-path-is-too-long HTTP/1.1\r\n\r\n".len() {
        let (client, server) = duplex(4096);
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        let mut guard = GuardedIo::new(server, 32, 64, false, false, rx);
        let prefix = b"GET /this-path-is-too-long HTTP/1.1\r\n\r\n"[..split].to_vec();
        let suffix = b"GET /this-path-is-too-long HTTP/1.1\r\n\r\n"[split..].to_vec();
        let client_task = tokio::spawn(async move {
            let mut client = client;
            client.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
            client.write_all(&prefix).await.unwrap();
            client.write_all(&suffix).await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            response
        });
        let mut head = [0; 64];
        assert_eq!(
            guard.read(&mut head).await.unwrap(),
            b"GET / HTTP/1.1\r\n\r\n".len()
        );
        guard
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        guard.flush().await.unwrap();
        assert_eq!(guard.read(&mut head).await.unwrap(), 0);
        drop(guard);
        let response = client_task.await.unwrap();
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "split {split}");
        assert_eq!(
            response.split_once("HTTP/1.1 431 Request Header Fields Too Large\r\n").unwrap().1,
            "Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: application/msgpack\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            "split {split}"
        );
    }
}

#[tokio::test]
async fn fixed_and_chunked_bodies_do_not_scan_payload_as_next_head() {
    let fixed_head = b"POST / HTTP/1.1\r\nContent-Length: 18\r\n\r\n";
    let fixed_body = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_head = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    let chunked_body = b"12\r\nGET / HTTP/1.1\r\n\r\n\r\n0\r\nX-Trailer: yes\r\n\r\n";
    for (head, body) in [
        (fixed_head.as_slice(), fixed_body.as_slice()),
        (chunked_head.as_slice(), chunked_body.as_slice()),
    ] {
        for split in 0..=head.len() {
            let (mut client, server) = duplex(4096);
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            let mut guard = GuardedIo::new(server, 8192, 64, false, false, rx);
            client.write_all(&head[..split]).await.unwrap();
            let rest = head[split..].to_vec();
            let body_and_next = [body, b"GET /next HTTP/1.1\r\n\r\n".as_slice()].concat();
            let writer = tokio::spawn(async move {
                let mut client = client;
                client.write_all(&rest).await.unwrap();
                client.write_all(&body_and_next).await.unwrap();
                client
            });
            let mut actual_head = vec![0; head.len()];
            guard.read_exact(&mut actual_head).await.unwrap();
            assert_eq!(actual_head, head);
            let mut actual_body = vec![0; body.len()];
            guard.read_exact(&mut actual_body).await.unwrap();
            assert_eq!(actual_body, body);
            let mut client = writer.await.unwrap();
            let mut next = [0; 64];
            let mut next_read = Box::pin(guard.read(&mut next));
            assert!(
                std::future::poll_fn(|cx| {
                    std::task::Poll::Ready(next_read.as_mut().poll(cx).is_pending())
                })
                .await
            );
            drop(next_read);
            guard
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            guard.flush().await.unwrap();
            let n = guard.read(&mut next).await.unwrap();
            assert_eq!(&next[..n], b"GET /next HTTP/1.1\r\n\r\n");
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let mut actual_response = vec![0; response.len()];
            client.read_exact(&mut actual_response).await.unwrap();
            assert_eq!(actual_response, response);
            drop(client);
        }
    }
}

#[tokio::test]
async fn hold_survives_body_eof_until_final_flush() {
    let (client, server) = duplex(4096);
    let (hold_tx, rx) = tokio::sync::mpsc::channel::<Box<dyn Send + Sync>>(1);
    let mut guard = GuardedIo::new(server, 8192, 64, false, false, rx);
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    struct Flag(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let hold = Box::new(Flag(dropped.clone())) as Box<dyn Send + Sync>;
    hold_tx.send(hold).await.unwrap();
    let mut peer = client;
    peer.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut request = [0; 64];
    let n = guard.read(&mut request).await.unwrap();
    assert_eq!(&request[..n], b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    guard
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
        .await
        .unwrap();
    assert!(!dropped.load(std::sync::atomic::Ordering::SeqCst));
    let mut response = [0; 128];
    let n = peer.read(&mut response).await.unwrap();
    assert!(n > 0);
    guard.flush().await.unwrap();
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
}
