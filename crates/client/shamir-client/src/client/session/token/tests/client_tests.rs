use std::future::Future;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;

use shamir_collections::TMap;
use shamir_connect::server::token::{encode_token, generate_token_parts};
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_types::types::value::QueryValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::builder::batch::Batch;
use crate::builder::query::Query;
use crate::{ApiToken, ClientError, CloseCause, TokenEndpoint, TokenHttpClient, TokenHttpOptions};

fn pong_body() -> Vec<u8> {
    rmp_serde::encode::to_vec_named(&DbResponse::Pong).expect("encode pong")
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn content_length(head: &[u8]) -> usize {
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    text.lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// cancel-safe: NO — cancellation drops the test socket and any partial reply.
async fn serve_connection(mut socket: TcpStream, status: &'static str, body: Vec<u8>, close: bool) {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let head_end = loop {
            if let Some(end) = find_head_end(&buf) {
                break end;
            }
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let total = head_end + content_length(&buf[..head_end]);
        while buf.len() < total {
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        buf.drain(..total);
        let connection = if close { "Connection: close\r\n" } else { "" };
        let head = format!(
            "HTTP/1.1 {status}\r\n{connection}Content-Type: application/msgpack\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        if socket.write_all(head.as_bytes()).await.is_err()
            || socket.write_all(&body).await.is_err()
            || close
        {
            return;
        }
    }
}

async fn spawn_fake(status: &'static str, body: Vec<u8>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(serve_connection(socket, status, body.clone(), false));
        }
    });
    addr
}

fn options(addr: SocketAddr) -> TokenHttpOptions {
    let (id, secret) = generate_token_parts().expect("token parts");
    let token = ApiToken::new(encode_token(&id, &secret).to_string()).expect("token");
    let endpoint = TokenEndpoint::plain_loopback(addr).expect("endpoint");
    TokenHttpOptions::new(endpoint, token)
}

async fn connect(addr: SocketAddr) -> TokenHttpClient {
    TokenHttpClient::connect(options(addr))
        .await
        .expect("connect")
}

async fn with_closing_peer<T, F: Future<Output = T>>(
    body: Vec<u8>,
    run: impl FnOnce(SocketAddr) -> F,
) -> (T, usize) {
    tokio::time::timeout(Duration::from_secs(120), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut accepts = 0;
        let result = tokio::select! {
            result = run(addr) => result,
            err = async {
                loop {
                    match listener.accept().await {
                        Ok((socket, _)) => {
                            accepts += 1;
                            serve_connection(socket, "200 OK", body.clone(), true).await;
                        }
                        Err(err) => break err,
                    }
                }
            } => panic!("accept: {err}"),
        };

        // The client has finished. Count any queued dials even if they sent no request.
        let listener = listener.into_std().expect("convert listener");
        loop {
            match listener.accept() {
                Ok(_) => accepts += 1,
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) => panic!("drain accepts: {err}"),
            }
        }
        (result, accepts)
    })
    .await
    .expect("client and peer finish within 120 seconds")
}

#[tokio::test]
async fn connect_once_ping_rejects_typed_non_pong_on_200() {
    let body = rmp_serde::encode::to_vec_named(&DbResponse::DdlOpStatus { status: None })
        .expect("encode DDL status");
    let (err, _) = with_closing_peer(body, |addr| async move {
        let client = TokenHttpClient::connect_once(options(addr))
            .await
            .expect("connect once");
        let err = client.ping().await.expect_err("non-Pong response");
        client.close().await;

        let client = connect(addr).await;
        client
            .ping()
            .await
            .expect("default SDK ping accepts HTTP 200");
        client.close().await;
        err
    })
    .await;
    assert!(matches!(err, ClientError::Protocol(message) if message == "expected Pong response"));
}

#[tokio::test]
async fn connect_once_ping_does_not_redial_after_connection_close() {
    let (second_ping, accepts) = with_closing_peer(pong_body(), |addr| async move {
        let client = TokenHttpClient::connect_once(options(addr))
            .await
            .expect("connect once");
        client.ping().await.expect("typed Pong");
        let second_ping = client.ping().await;
        client.close().await;
        second_ping
    })
    .await;
    assert_eq!(
        accepts, 1,
        "connect_once must not dial a replacement socket"
    );
    assert!(matches!(
        second_ping,
        Err(ClientError::ConnectionClosed(CloseCause::PeerEof))
    ));
}

#[tokio::test]
async fn ping_succeeds_on_200() {
    let client = connect(spawn_fake("200 OK", pong_body()).await).await;
    client.ping().await.expect("ping");
    client.close().await;
}

#[tokio::test]
async fn request_ping_returns_pong() {
    let client = connect(spawn_fake("200 OK", pong_body()).await).await;
    let response = client.request(&DbRequest::Ping).await.expect("request");
    assert!(matches!(response, DbResponse::Pong));
    client.close().await;
}

#[tokio::test]
async fn unauthorized_maps_to_http_error() {
    let client = connect(spawn_fake("401 Unauthorized", Vec::new()).await).await;
    let err = client.request(&DbRequest::Ping).await.expect_err("401");
    assert!(matches!(
        err,
        ClientError::Http { status: 401, code: Some(ref code), .. } if code == "unauthorized"
    ));
}

#[tokio::test]
async fn forbidden_maps_to_db_error() {
    let body = rmp_serde::encode::to_vec_named(&DbResponse::Error {
        code: "token_read_only".into(),
        message: "read only".into(),
    })
    .expect("encode error");
    let client = connect(spawn_fake("403 Forbidden", body).await).await;
    let err = client.request(&DbRequest::Ping).await.expect_err("403");
    assert!(matches!(err, ClientError::Db { ref code, .. } if code == "token_read_only"));
}

#[tokio::test]
async fn execute_returns_batch_response() {
    let expected = BatchResponse {
        id: QueryValue::Null,
        results: TMap::default(),
        execution_plan: Vec::new(),
        edge_provenance: TMap::default(),
        execution_time_us: 7,
        transaction: None,
        interner_delta: TMap::default(),
    };
    let body = rmp_serde::encode::to_vec_named(&DbResponse::Batch {
        response: expected.clone(),
    })
    .expect("encode batch");
    let client = connect(spawn_fake("200 OK", body).await).await;
    let mut batch = Batch::new();
    batch.query("u", Query::from("users"));
    let response = client.execute("db", batch.build()).await.expect("execute");
    assert_eq!(response, expected);
    client.close().await;
}
