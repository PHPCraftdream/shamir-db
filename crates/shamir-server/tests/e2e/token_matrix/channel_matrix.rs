use super::super::http_rest::common::{Harness, ADMIN_PASSWORD};
use super::support::{self, connect, send_http, status, DEADLINE, MAX_HEAD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::server::token::{TokenChannels, TokenMode};
use shamir_query_builder::{batch::Batch, ddl, Query};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION as CURRENT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};
use tokio_tungstenite::WebSocketStream;

async fn post(harness: &Harness, tls: bool, token: &str, body: &[u8]) -> (String, Vec<u8>) {
    let head = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/msgpack\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let mut stream = connect(harness, tls).await;
    tokio::time::timeout(DEADLINE, stream.write_all(head.as_bytes()))
        .await
        .expect("HTTP head write timeout")
        .expect("HTTP head write");
    tokio::time::timeout(DEADLINE, stream.write_all(body))
        .await
        .expect("HTTP body write timeout")
        .expect("HTTP body write");
    support::read_response(&mut stream).await
}

async fn websocket(
    harness: &Harness,
    tls: bool,
    token: &str,
) -> (WebSocketStream<support::Stream>, [u8; 32]) {
    #[derive(Deserialize)]
    struct Binding {
        ok: bool,
        #[serde(with = "serde_bytes")]
        sid: Vec<u8>,
        expires_at_ns: u64,
    }
    let scheme = if tls { "wss" } else { "ws" };
    let mut request = format!("{scheme}://{}/shamir/v1/ws", harness.addr())
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let (mut ws, response) = tokio::time::timeout(
        DEADLINE,
        tokio_tungstenite::client_async(request, connect(harness, tls).await),
    )
    .await
    .expect("upgrade timeout")
    .expect("upgrade");
    assert_eq!(response.status().as_u16(), 101);
    let frame = tokio::time::timeout(DEADLINE, ws.next())
        .await
        .expect("binding timeout")
        .expect("binding")
        .expect("binding frame");
    let Message::Binary(bytes) = frame else {
        panic!("expected binding")
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("binding");
    assert!(binding.ok && binding.expires_at_ns > 0);
    (ws, binding.sid.as_slice().try_into().expect("32-byte sid"))
}

async fn ws_data(harness: &Harness, tls: bool) {
    let make = |write: bool| {
        let mut batch = Batch::new();
        if write {
            batch.insert(
                "write",
                shamir_query_builder::write::insert("items")
                    .row(shamir_query_builder::doc! { "name" => "matrix-ws" }),
            );
        } else {
            batch.query("read", Query::from("items"));
        }
        DbRequest::Execute {
            query_version: CURRENT,
            db: "default".into(),
            batch: batch.build(),
        }
    };
    let token = harness.token("matrix-ws", TokenMode::ReadWrite).await;
    let (mut ws, sid) = websocket(harness, tls, &token).await;
    for (rid, write) in [(1, true), (2, false)] {
        let body = rmp_serde::to_vec_named(&make(write)).expect("request body");
        let request = RequestEnvelope::new(sid, Some(rid), body)
            .to_msgpack()
            .expect("request envelope");
        tokio::time::timeout(DEADLINE, ws.send(Message::Binary(request.into())))
            .await
            .expect("send timeout")
            .expect("send");
        let frame = tokio::time::timeout(DEADLINE, ws.next())
            .await
            .expect("reply timeout")
            .expect("reply")
            .expect("frame");
        let Message::Binary(frame) = frame else {
            panic!("expected response")
        };
        let response = ResponseEnvelope::from_msgpack(&frame).expect("response envelope");
        let response: DbResponse = rmp_serde::from_slice(&response.res).expect("DB response");
        let DbResponse::Batch { response } = response else {
            panic!("expected batch")
        };
        let alias = if write { "write" } else { "read" };
        assert!(response.results.contains_key(alias));
        if !write {
            assert!(response.results[alias]
                .records
                .iter()
                .any(|r| r.get_value_str("name") == Some("matrix-ws")));
        }
    }
    let ro = harness.token("matrix-ro", TokenMode::ReadOnly).await;
    let (mut ws, sid) = websocket(harness, tls, &ro).await;
    let body = rmp_serde::to_vec_named(&make(true)).expect("write request");
    let request = RequestEnvelope::new(sid, Some(3), body)
        .to_msgpack()
        .expect("request envelope");
    tokio::time::timeout(DEADLINE, ws.send(Message::Binary(request.into())))
        .await
        .expect("send timeout")
        .expect("send");
    let frame = tokio::time::timeout(DEADLINE, ws.next())
        .await
        .expect("reply timeout")
        .expect("reply")
        .expect("frame");
    let Message::Binary(frame) = frame else {
        panic!("expected denial")
    };
    let response = ResponseEnvelope::from_msgpack(&frame).expect("response envelope");
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&response.res).expect("denial"), DbResponse::Error { code, .. } if code == "token_read_only")
    );
    let scoped = harness
        .token_with_options(
            "matrix-scope",
            TokenMode::ReadOnly,
            3600,
            TokenChannels::Both,
            Some(vec!["outside".into()]),
        )
        .await
        .token
        .to_string();
    let (mut ws, sid) = websocket(harness, tls, &scoped).await;
    let body = rmp_serde::to_vec_named(&make(false)).expect("read request");
    let request = RequestEnvelope::new(sid, Some(4), body)
        .to_msgpack()
        .expect("request envelope");
    tokio::time::timeout(DEADLINE, ws.send(Message::Binary(request.into())))
        .await
        .expect("send timeout")
        .expect("send");
    let frame = tokio::time::timeout(DEADLINE, ws.next())
        .await
        .expect("reply timeout")
        .expect("reply")
        .expect("frame");
    let Message::Binary(frame) = frame else {
        panic!("expected scope denial")
    };
    let response = ResponseEnvelope::from_msgpack(&frame).expect("response envelope");
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&response.res).expect("scope denial"), DbResponse::Error { code, .. } if code == "token_database_denied")
    );
}

async fn https_rest_matrix(harness: &Harness) {
    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: zeroize::Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(DEADLINE),
        handshake_timeout: Some(DEADLINE),
        request_timeout: Some(DEADLINE),
    })
    .await
    .expect("admin TLS client");
    let mut create = Batch::new();
    create.create_table("items", ddl::create_table("items"));
    client
        .execute("default", create.build())
        .await
        .expect("create table");
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        shamir_query_builder::write::insert("items")
            .row(shamir_query_builder::doc! { "name" => "matrix-https" }),
    );
    client
        .execute("default", seed.build())
        .await
        .expect("seed data");
    drop(client);
    let token = harness.token("https-matrix", TokenMode::ReadWrite).await;
    for (write, alias) in [(true, "write"), (false, "read")] {
        let mut batch = Batch::new();
        if write {
            batch.insert(
                alias,
                shamir_query_builder::write::insert("items")
                    .row(shamir_query_builder::doc! { "name" => "tls-write" }),
            );
        } else {
            batch.query(alias, Query::from("items"));
        }
        let body = rmp_serde::to_vec_named(&DbRequest::Execute {
            query_version: CURRENT,
            db: "default".into(),
            batch: batch.build(),
        })
        .expect("request body");
        let (head, bytes) = post(harness, true, &token, &body).await;
        assert_eq!(status(&head), 200);
        let DbResponse::Batch { response } =
            rmp_serde::from_slice(&bytes).expect("HTTP DB response")
        else {
            panic!("expected batch")
        };
        assert!(response.results.contains_key(alias));
        if !write {
            assert!(response.results[alias]
                .records
                .iter()
                .any(|r| r.get_value_str("name") == Some("tls-write")));
        }
    }
    let readonly = harness.token("https-ro", TokenMode::ReadOnly).await;
    let mut batch = Batch::new();
    batch.insert(
        "write",
        shamir_query_builder::write::insert("items")
            .row(shamir_query_builder::doc! { "name" => "denied" }),
    );
    let body = rmp_serde::to_vec_named(&DbRequest::Execute {
        query_version: CURRENT,
        db: "default".into(),
        batch: batch.build(),
    })
    .expect("write body");
    let (head, bytes) = post(harness, true, &readonly, &body).await;
    assert_eq!(status(&head), 403);
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&bytes).expect("denial"), DbResponse::Error { code, .. } if code == "token_read_only")
    );
    let scoped = harness
        .token_with_options(
            "https-scope",
            TokenMode::ReadOnly,
            3600,
            TokenChannels::Rest,
            Some(vec!["outside".into()]),
        )
        .await
        .token
        .to_string();
    let mut batch = Batch::new();
    batch.query("read", Query::from("items"));
    let body = rmp_serde::to_vec_named(&DbRequest::Execute {
        query_version: CURRENT,
        db: "default".into(),
        batch: batch.build(),
    })
    .expect("scoped body");
    let (head, bytes) = post(harness, true, &scoped, &body).await;
    assert_eq!(status(&head), 403);
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&bytes).expect("scope denial"), DbResponse::Error { code, .. } if code == "token_database_denied")
    );
    let rest_only = harness
        .token_with_channels("rest-only", TokenMode::ReadOnly, TokenChannels::Rest)
        .await;
    let ping = rmp_serde::to_vec_named(&DbRequest::Ping).expect("ping");
    let (head, bytes) = post(harness, true, &rest_only, &ping).await;
    assert_eq!(status(&head), 200);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&bytes).expect("REST ping"),
        DbResponse::Pong
    ));
    let mut ws_request = format!("wss://{}/shamir/v1/ws", harness.addr())
        .into_client_request()
        .expect("WS request");
    ws_request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {rest_only}")).expect("authorization"),
    );
    ws_request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let result = tokio::time::timeout(
        DEADLINE,
        tokio_tungstenite::client_async(ws_request, connect(harness, true).await),
    )
    .await
    .expect("WS channel check timeout");
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), 403)
        }
        _ => panic!("REST-only token upgraded"),
    }
    let ws_only = harness
        .token_with_channels("ws-only-rest", TokenMode::ReadOnly, TokenChannels::Ws)
        .await;
    let (mut ws_socket, _) = websocket(harness, true, &ws_only).await;
    tokio::time::timeout(DEADLINE, ws_socket.close(None))
        .await
        .expect("close WS-only timeout")
        .expect("close WS-only socket");
    let (head, bytes) = post(harness, true, &ws_only, &ping).await;
    assert_eq!(status(&head), 403);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&bytes).expect("WS-only REST denial"),
        DbResponse::Error { code, .. } if code == "token_channel_denied"
    ));
    let both = harness
        .token_with_channels("both", TokenMode::ReadOnly, TokenChannels::Both)
        .await;
    let (head, bytes) = post(harness, true, &both, &ping).await;
    assert_eq!(status(&head), 200);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&bytes).expect("both REST ping"),
        DbResponse::Pong
    ));
    let (mut socket, sid) = websocket(harness, true, &both).await;
    let request = RequestEnvelope::new(sid, Some(1), ping.clone())
        .to_msgpack()
        .expect("ping envelope");
    tokio::time::timeout(DEADLINE, socket.send(Message::Binary(request.into())))
        .await
        .expect("WSS ping send timeout")
        .expect("WSS ping send");
    let frame = tokio::time::timeout(DEADLINE, socket.next())
        .await
        .expect("WSS ping reply timeout")
        .expect("WSS ping reply")
        .expect("WSS ping frame");
    let Message::Binary(frame) = frame else {
        panic!("expected WSS ping response")
    };
    let response = ResponseEnvelope::from_msgpack(&frame).expect("WSS response envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&response.res).expect("WSS ping response"),
        DbResponse::Pong
    ));
    tokio::time::timeout(DEADLINE, socket.close(None))
        .await
        .expect("close both-channel timeout")
        .expect("close both-channel WS");
    let mut subscribe = Batch::new();
    subscribe.subscribe(
        "watch",
        shamir_query_builder::batch::Subscribe::table("items").build(),
    );
    let unsupported = [
        DbRequest::TxBegin {
            query_version: CURRENT,
            db: "default".into(),
            repo: "main".into(),
            isolation: None,
        },
        DbRequest::CreateCursor {
            query_version: CURRENT,
            db: "default".into(),
            query: Query::from("items").build(),
            page_size: 1,
        },
        DbRequest::Execute {
            query_version: CURRENT,
            db: "default".into(),
            batch: subscribe.build(),
        },
    ];
    for request in unsupported {
        let body = rmp_serde::to_vec_named(&request).expect("unsupported request");
        let (head, bytes) = post(harness, true, &both, &body).await;
        assert_eq!(status(&head), 400);
        assert!(matches!(
            rmp_serde::from_slice::<DbResponse>(&bytes).expect("not supported"),
            DbResponse::Error { code, .. } if code == "not_supported_over_rest"
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn https_channel_data_and_scope_matrix() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
        |_| {},
    )
    .await;
    https_rest_matrix(&harness).await;
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown()
    )
    .await
    .expect("shutdown timeout")
    .errors()
    .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_and_https_channel_missing_header_and_query_token_are_unauthorized() {
    for tls in [false, true] {
        let harness = Harness::launch_with(
            "127.0.0.1:0",
            shamir_server::config::HttpListenerSettings {
                tls: Some(tls),
                ..Default::default()
            },
        )
        .await;
        let token = harness.token("query-only", TokenMode::ReadOnly).await;
        let paths = if tls {
            vec![format!("/shamir/v1/ping?token={token}")]
        } else {
            vec![
                "/shamir/v1/ping".to_owned(),
                format!("/shamir/v1/ping?token={token}"),
            ]
        };
        for path in paths {
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let (head, body) = send_http(&harness, tls, &request).await;
            assert_eq!(status(&head), 401);
            assert!(head
                .to_ascii_lowercase()
                .contains("www-authenticate: bearer"));
            assert!(head.to_ascii_lowercase().contains("connection: close"));
            assert_eq!(body, b"\x81\xa5error\xacunauthorized");
        }
        let request = format!(
            "GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        );
        let (head, body) = send_http(&harness, tls, &request).await;
        assert_eq!(status(&head), 200);
        assert!(matches!(
            rmp_serde::from_slice::<DbResponse>(&body).expect("valid ping"),
            DbResponse::Pong
        ));
        assert!(tokio::time::timeout(
            std::time::Duration::from_secs(45),
            harness.handle.shutdown()
        )
        .await
        .expect("shutdown timeout")
        .errors()
        .is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_channel_duplicate_authorization_is_rejected() {
    let harness = Harness::launch().await;
    let token = harness.token("duplicate", TokenMode::ReadOnly).await;
    let request = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let (head, _) = send_http(&harness, false, &request).await;
    assert_eq!(status(&head), 401);
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown()
    )
    .await
    .expect("shutdown timeout")
    .errors()
    .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn https_channel_wrong_secret_and_unknown_id_are_unauthorized() {
    let harness = Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
    )
    .await;
    let token = harness.token("https-auth", TokenMode::ReadOnly).await;
    let mut wrong = token.as_bytes().to_vec();
    wrong[28] = if wrong[28] == b'A' { b'B' } else { b'A' };
    let wrong = String::from_utf8(wrong).expect("mutated token");
    for candidate in [
        &wrong,
        "shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        let request = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {candidate}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let (head, _) = support::send_http(&harness, true, &request).await;
        assert_eq!(status(&head), 401);
    }
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown()
    )
    .await
    .expect("shutdown timeout")
    .errors()
    .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_channel_header_flood_and_slow_upgrade_head_are_bounded() {
    let harness = Harness::launch().await;
    let token = harness.token("ws-head", TokenMode::ReadOnly).await;
    let flood = format!("GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nX-Flood: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n", "x".repeat(MAX_HEAD + 1024));
    let (head, _) = send_http(&harness, false, &flood).await;
    assert_eq!(status(&head), 431);
    let mut stream = connect(&harness, false).await;
    let partial = format!("GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: Upgrade\r\n");
    tokio::time::timeout(DEADLINE, stream.write_all(partial.as_bytes()))
        .await
        .expect("partial head timeout")
        .expect("partial head");
    let (head, _) = support::read_response(&mut stream).await;
    assert_eq!(status(&head), 408);
    let mut byte = [0; 1];
    assert!(tokio::time::timeout(DEADLINE, stream.read(&mut byte))
        .await
        .is_ok_and(|result| result.is_ok_and(|n| n == 0)));
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown()
    )
    .await
    .expect("shutdown timeout")
    .errors()
    .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wss_channel_header_flood_and_slow_upgrade_head_are_bounded() {
    let harness = Harness::launch_with(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
    )
    .await;
    let token = harness.token("wss-head", TokenMode::ReadOnly).await;
    let flood = format!("GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nX-Flood: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n", "x".repeat(MAX_HEAD + 1024));
    let (head, _) = send_http(&harness, true, &flood).await;
    assert_eq!(status(&head), 431);
    let mut stream = connect(&harness, true).await;
    let partial = format!("GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: Upgrade\r\n");
    tokio::time::timeout(DEADLINE, stream.write_all(partial.as_bytes()))
        .await
        .expect("partial head timeout")
        .expect("partial head");
    let (head, _) = support::read_response(&mut stream).await;
    assert_eq!(status(&head), 408);
    let mut byte = [0; 1];
    assert!(tokio::time::timeout(DEADLINE, stream.read(&mut byte))
        .await
        .is_ok_and(|result| result.is_ok_and(|n| n == 0)));
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown()
    )
    .await
    .expect("shutdown timeout")
    .errors()
    .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_channel_query_token_is_ignored() {
    for tls in [false, true] {
        let harness = if tls {
            Harness::launch_with(
                "127.0.0.1:0",
                shamir_server::config::HttpListenerSettings {
                    tls: Some(true),
                    ..Default::default()
                },
            )
            .await
        } else {
            Harness::launch().await
        };
        let token = harness.token("query-only", TokenMode::ReadOnly).await;
        let scheme = if tls { "wss" } else { "ws" };
        let mut request = format!("{scheme}://{}/shamir/v1/ws?token={token}", harness.addr())
            .into_client_request()
            .expect("query request");
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("shamir-v1"),
        );
        let result = tokio::time::timeout(
            DEADLINE,
            tokio_tungstenite::client_async(request, connect(&harness, tls).await),
        )
        .await
        .expect("handshake timeout");
        match result {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), 401)
            }
            _ => panic!("query token unexpectedly upgraded"),
        }
        assert!(tokio::time::timeout(
            std::time::Duration::from_secs(45),
            harness.handle.shutdown()
        )
        .await
        .expect("shutdown timeout")
        .errors()
        .is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wss_channel_data_scope_and_auth_matrix() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
        |_| {},
    )
    .await;
    let table_admin = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: zeroize::Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(DEADLINE),
        handshake_timeout: Some(DEADLINE),
        request_timeout: Some(DEADLINE),
    })
    .await
    .expect("admin TLS client");
    let mut table = Batch::new();
    table.create_table("items", ddl::create_table("items"));
    table_admin
        .execute("default", table.build())
        .await
        .expect("create WS table");
    drop(table_admin);
    ws_data(&harness, true).await;
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown()
    )
    .await
    .expect("shutdown timeout")
    .errors()
    .is_empty());
}
