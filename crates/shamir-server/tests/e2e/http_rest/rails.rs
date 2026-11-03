use std::net::SocketAddr;
use std::time::Duration;

use shamir_connect::server::token::TokenMode;
use shamir_query_builder::{batch::Batch, ddl, Query};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

use super::common::{request, Harness};

async fn raw_response(addr: SocketAddr, request: &str) -> (String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut stream = TcpStream::connect(addr).await.expect("connect HTTP socket");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write HTTP request");

        let mut response = Vec::new();
        let header_end = loop {
            if let Some(index) = response.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = index + 4;
                assert!(end <= 8192, "HTTP response headers exceed 8192 bytes");
                break end;
            }
            assert!(
                response.len() < 8192,
                "HTTP response headers exceed 8192 bytes"
            );
            let mut chunk = [0; 1024];
            let count = stream.read(&mut chunk).await.expect("read HTTP response");
            assert_ne!(count, 0, "connection closed before HTTP headers");
            response.extend_from_slice(&chunk[..count]);
        };

        let headers =
            String::from_utf8(response[..header_end].to_vec()).expect("HTTP headers are UTF-8");
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("valid Content-Length"))
            })
            .unwrap_or(0);
        assert!(
            content_length <= 16 * 1024 * 1024,
            "HTTP response body exceeds 16 MiB"
        );

        let body_start = header_end;
        while response.len() - body_start < content_length {
            let remaining = content_length - (response.len() - body_start);
            let mut chunk = vec![0; remaining.min(8192)];
            let count = stream
                .read(&mut chunk)
                .await
                .expect("read HTTP response body");
            assert_ne!(count, 0, "connection closed before HTTP response body");
            response.extend_from_slice(&chunk[..count]);
        }
        (
            headers,
            response[body_start..body_start + content_length].to_vec(),
        )
    })
    .await
    .expect("HTTP response timed out")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_remote_plaintext_opt_in_allows_executor_after_write_rail() {
    let mut settings = shamir_server::config::HttpListenerSettings {
        tls: Some(false),
        allow_plaintext_remote: true,
        ..Default::default()
    };
    let addr = "0.0.0.0:0";
    let harness = Harness::launch_with(addr, settings.clone()).await;
    let token = harness
        .token("remote-write-rail", TokenMode::ReadWrite)
        .await;
    let write_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch.insert(
                "insert",
                shamir_query_builder::write::insert("items").row(shamir_query_builder::doc! {
                    "name" => "remote-rail"
                }),
            );
            batch.build()
        },
    };
    let write_body = rmp_serde::to_vec_named(&write_request).expect("encode write");
    let (status, _, body) = request(harness.addr(), Some(&token), &write_body).await;
    assert_eq!(status, 403);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode remote write denial"),
        DbResponse::Error { code, .. } if code == "token_plaintext_write_denied"
    ));

    let browser_request = "GET /shamir/v1/ws/browser HTTP/1.1\r\nHost: localhost\r\nOrigin: https://app.example\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\n\r\n";
    let (browser_headers, _) = raw_response(harness.addr(), browser_request).await;
    assert_eq!(browser_headers.split_whitespace().nth(1), Some("404"));
    let native = format!(
        "GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    let (native_headers, native_body) = raw_response(harness.addr(), &native).await;
    assert_eq!(native_headers.split_whitespace().nth(1), Some("403"));
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&native_body).expect("decode remote write denial"),
        DbResponse::Error { code, .. } if code == "token_plaintext_write_denied"
    ));

    let read_only_token = harness.token("remote-read-only", TokenMode::ReadOnly).await;
    let native_read_only = format!(
        "GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: shamir-v1\r\nAuthorization: Bearer {read_only_token}\r\n\r\n"
    );
    let (native_headers, _) = raw_response(harness.addr(), &native_read_only).await;
    assert_eq!(native_headers.split_whitespace().nth(1), Some("101"));

    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "shutdown report: {report:?}");

    settings.allow_plaintext_write = true;
    let harness = Harness::launch_with_native_seed_settings(addr, settings).await;
    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(super::common::ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("admin TLS-exporter connect");
    let mut create = Batch::new();
    create.create_table("items", ddl::create_table("items"));
    client
        .execute("default", create.build())
        .await
        .expect("create table");
    drop(client);
    let token = harness
        .token("remote-write-enabled", TokenMode::ReadWrite)
        .await;
    let write_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = shamir_query_builder::batch::Batch::new();
            batch.insert(
                "insert",
                shamir_query_builder::write::insert("items").row(shamir_query_builder::doc! {
                    "name" => "remote-opt-in"
                }),
            );
            batch.build()
        },
    };
    let write_body = rmp_serde::to_vec_named(&write_request).expect("encode opted-in write");
    let (status, _, body) = request(harness.addr(), Some(&token), &write_body).await;
    assert_eq!(status, 200);
    let response = rmp_serde::from_slice::<DbResponse>(&body).expect("decode opted-in write");
    let DbResponse::Batch { response } = response else {
        panic!("expected successful insert batch, got {response:?}");
    };
    assert!(response.results.contains_key("insert"));
    let read_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.query("read", Query::from("items"));
            batch.build()
        },
    };
    let read_body = rmp_serde::to_vec_named(&read_request).expect("encode opted-in read");
    let (status, _, body) = request(harness.addr(), Some(&token), &read_body).await;
    assert_eq!(status, 200);
    let response = rmp_serde::from_slice::<DbResponse>(&body).expect("decode opted-in read");
    let DbResponse::Batch { response } = response else {
        panic!("expected successful read batch, got {response:?}");
    };
    let records = &response.results.get("read").expect("read alias").records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].get_value_str("name"), Some("remote-opt-in"));
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "shutdown report: {report:?}");
}
