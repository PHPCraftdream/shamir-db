use std::net::SocketAddr;

use shamir_collections::TMap;
use shamir_connect::server::token::{encode_token, generate_token_parts};
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_types::types::value::QueryValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::builder::batch::Batch;
use crate::builder::query::Query;
use crate::{ApiToken, ClientError, TokenEndpoint, TokenHttpClient, TokenHttpOptions};

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

async fn serve_connection(mut socket: TcpStream, status: &'static str, body: Vec<u8>) {
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
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/msgpack\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        if socket.write_all(head.as_bytes()).await.is_err()
            || socket.write_all(&body).await.is_err()
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
            tokio::spawn(serve_connection(socket, status, body.clone()));
        }
    });
    addr
}

async fn connect(addr: SocketAddr) -> TokenHttpClient {
    let (id, secret) = generate_token_parts().expect("token parts");
    let token = ApiToken::new(encode_token(&id, &secret).to_string()).expect("token");
    let endpoint = TokenEndpoint::plain_loopback(addr).expect("endpoint");
    TokenHttpClient::connect(TokenHttpOptions::new(endpoint, token))
        .await
        .expect("connect")
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
