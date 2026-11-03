use super::super::http_rest::common::Harness;
use super::common::{raw_response, upgrade};
use super::session_loop::{open_native, send_request};
use futures_util::{SinkExt, StreamExt};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::server::token::{parse_token, TokenId, TokenMode};
use shamir_query_builder::{batch::Batch, Query};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroizing;

#[derive(serde::Deserialize)]
struct ErrorWire {
    rid: u32,
    error: String,
}

fn token_id(token: &str) -> TokenId {
    parse_token(token.as_bytes()).expect("canonical token").id
}

fn now_ns() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos(),
    )
    .expect("timestamp fits")
}

async fn close_socket(ws: &mut WebSocketStream<TcpStream>) {
    tokio::time::timeout(Duration::from_secs(15), async {
        ws.send(Message::Close(None)).await.expect("send close");
        match ws.next().await {
            None => {}
            Some(Ok(Message::Close(_))) => {}
            Some(Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed))
            | Some(Err(tokio_tungstenite::tungstenite::Error::AlreadyClosed)) => {}
            other => panic!("unexpected close response: {other:?}"),
        }
    })
    .await
    .expect("close deadline");
}

async fn expect_close(ws: &mut WebSocketStream<TcpStream>, expected_code: u16) {
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("close deadline")
        .expect("close frame")
        .expect("websocket");
    let Message::Close(Some(frame)) = message else {
        panic!("expected close: {message:?}");
    };
    assert_eq!(u16::from(frame.code), expected_code);
}

async fn rejected(addr: std::net::SocketAddr, request: &str) -> (String, Vec<u8>) {
    let result = raw_response(addr, request).await;
    assert_eq!(
        result.0.split_whitespace().nth(1),
        Some("401"),
        "{}",
        result.0
    );
    result
}

async fn native_error_matrix(harness: &Harness, usable: &str) {
    let missing = upgrade("/shamir/v1/ws", "localhost", None, "");
    let duplicate = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {usable}\r\nAuthorization: Bearer {usable}\r\n"),
    );
    let malformed = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        "Authorization: Bearer malformed\r\n",
    );
    let unknown = "shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert!(parse_token(unknown.as_bytes()).is_ok());
    let unknown_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {unknown}\r\n"),
    );
    let mut wrong_secret = usable.as_bytes().to_vec();
    wrong_secret[28] = if wrong_secret[28] == b'A' { b'B' } else { b'A' };
    let wrong_secret = String::from_utf8(wrong_secret).expect("token remains UTF-8");
    assert!(parse_token(wrong_secret.as_bytes()).is_ok());
    let wrong_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {wrong_secret}\r\n"),
    );
    let revoked = harness
        .token("ws-revoked-matrix", TokenMode::ReadOnly)
        .await;
    let revoked_id = token_id(&revoked);
    assert!(harness
        .handle
        .token_service
        .revoke(revoked_id, now_ns())
        .await
        .expect("revoke"));
    let revoked_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {revoked}\r\n"),
    );
    let expired = harness
        .token_with_ttl("ws-expired-matrix", TokenMode::ReadOnly, 1)
        .await;
    let mut expired_socket = open_native(harness.addr(), &expired).await;
    expect_close(&mut expired_socket.0, 4012).await;
    let expired_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {expired}\r\n"),
    );
    let reference = rejected(harness.addr(), &missing).await;
    for (index, request) in [
        &duplicate,
        &malformed,
        &unknown_request,
        &wrong_request,
        &revoked_request,
        &expired_request,
    ]
    .into_iter()
    .enumerate()
    {
        let forwarded = request.replacen(
            "Host: localhost\r\n",
            &format!(
                "Host: localhost\r\nX-Forwarded-For: 10.{}.0.1\r\n",
                index + 1
            ),
            1,
        );
        let actual = rejected(harness.addr(), &forwarded).await;
        assert_eq!(actual.1, reference.1);
        assert_eq!(
            actual
                .0
                .lines()
                .filter(|line| !line.to_ascii_lowercase().starts_with("date:"))
                .collect::<Vec<_>>(),
            reference
                .0
                .lines()
                .filter(|line| !line.to_ascii_lowercase().starts_with("date:"))
                .collect::<Vec<_>>()
        );
    }
}

async fn caps_and_sid_isolation(harness: &Harness) {
    let token = harness.token("ws-cap", TokenMode::ReadOnly).await;
    let (mut first, _, _) = open_native(harness.addr(), &token).await;
    let mut second = open_native(harness.addr(), &token).await.0;
    let cap_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {token}\r\n"),
    );
    let (cap_headers, _) = raw_response(harness.addr(), &cap_request).await;
    assert_eq!(
        cap_headers.split_whitespace().nth(1),
        Some("503"),
        "{cap_headers}"
    );
    assert!(cap_headers.to_ascii_lowercase().contains("retry-after: 1"));
    let foreign = harness.token("ws-foreign", TokenMode::ReadOnly).await;
    let (mut foreign_socket, foreign_sid, _) = open_native(harness.addr(), &foreign).await;
    let owner_probe = harness.token("ws-owner-cap", TokenMode::ReadOnly).await;
    let foreign_cap_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {owner_probe}\r\n"),
    );
    let (foreign_cap_headers, _) = raw_response(harness.addr(), &foreign_cap_request).await;
    assert_eq!(
        foreign_cap_headers.split_whitespace().nth(1),
        Some("503"),
        "{foreign_cap_headers}"
    );
    assert!(foreign_cap_headers
        .to_ascii_lowercase()
        .contains("retry-after: 1"));
    let body = rmp_serde::to_vec_named(&RequestEnvelope::new(
        foreign_sid,
        Some(9),
        rmp_serde::to_vec_named(&DbRequest::Ping).unwrap(),
    ))
    .unwrap();
    first
        .send(Message::Binary(body.into()))
        .await
        .expect("send foreign sid");
    expect_close(&mut first, 4010).await;
    let zero_sid_body = RequestEnvelope::new(
        [0; 32],
        Some(10),
        rmp_serde::to_vec_named(&DbRequest::Ping).unwrap(),
    )
    .to_msgpack()
    .expect("encode zero sid request");
    second
        .send(Message::Binary(zero_sid_body.into()))
        .await
        .expect("send zero sid");
    expect_close(&mut second, 4010).await;
    close_socket(&mut foreign_socket).await;
}

async fn scram_login(harness: &Harness) -> shamir_client::Client {
    shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(super::super::http_rest::common::ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("SCRAM login despite token sessions")
}

async fn execute_rw_and_session_features(harness: &Harness) {
    let read_only = harness
        .token_with_options(
            "ws-ro-denials",
            TokenMode::ReadOnly,
            3600,
            shamir_connect::server::token::TokenChannels::Both,
            None,
        )
        .await;
    let read_only_token = read_only.token.to_string();
    let (mut read_only_socket, read_sid, _) = open_native(harness.addr(), &read_only_token).await;
    let write = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.insert(
                "i",
                shamir_query_builder::write::insert("items")
                    .rows([shamir_query_builder::doc! {"n" => -1}]),
            );
            batch.build()
        },
    };
    let denied = ResponseEnvelope::from_msgpack(
        &send_request(&mut read_only_socket, &read_sid, 68, &write).await,
    )
    .expect("write denial envelope");
    assert_eq!(denied.request_id, Some(68));
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&denied.res).expect("write denied"), DbResponse::Error { ref code, .. } if code == "token_read_only")
    );
    let tx_begin = DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        repo: "main".into(),
        isolation: None,
    };
    let begin = ResponseEnvelope::from_msgpack(
        &send_request(&mut read_only_socket, &read_sid, 69, &tx_begin).await,
    )
    .expect("read-only begin");
    let tx_handle = match rmp_serde::from_slice::<DbResponse>(&begin.res).expect("begin response") {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened: {other:?}"),
    };
    let commit_denied = DbRequest::TxCommit {
        query_version: CURRENT_QUERY_LANG_VERSION,
        visibility_wait: None,
        db: "default".into(),
        tx_handle,
    };
    let denied = ResponseEnvelope::from_msgpack(
        &send_request(&mut read_only_socket, &read_sid, 69, &commit_denied).await,
    )
    .expect("read-only commit envelope");
    assert_eq!(denied.request_id, Some(69));
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&denied.res).expect("commit denied"), DbResponse::Error { ref code, .. } if code == "token_read_only")
    );
    let mut scope_batch = Batch::new();
    scope_batch.insert(
        "i",
        shamir_query_builder::write::insert("items").rows([shamir_query_builder::doc! {"n" => -2}]),
    );
    let tx_execute = DbRequest::TxExecute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        tx_handle,
        batch: scope_batch.build(),
    };
    let denied = ResponseEnvelope::from_msgpack(
        &send_request(&mut read_only_socket, &read_sid, 70, &tx_execute).await,
    )
    .expect("tx write envelope");
    assert_eq!(denied.request_id, Some(70));
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&denied.res).expect("tx write denied"), DbResponse::Error { ref code, .. } if code == "token_read_only")
    );
    let _ = send_request(
        &mut read_only_socket,
        &read_sid,
        71,
        &DbRequest::TxRollback {
            db: "default".into(),
            tx_handle,
        },
    )
    .await;
    let owner = scram_login(harness).await;
    assert!(owner.ping().await.is_ok());
    let (mut spare, _, _) = open_native(harness.addr(), &read_only_token).await;
    let foreign_owner_request = RequestEnvelope::new(
        owner.session_id(),
        Some(81),
        rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping"),
    )
    .to_msgpack()
    .expect("encode foreign owner sid");
    tokio::time::timeout(
        Duration::from_secs(15),
        spare.send(Message::Binary(foreign_owner_request.into())),
    )
    .await
    .expect("write deadline")
    .expect("send foreign owner sid");
    expect_close(&mut spare, 4010).await;
    assert!(owner.ping().await.is_ok());
    let readonly_ping = ResponseEnvelope::from_msgpack(
        &send_request(&mut read_only_socket, &read_sid, 82, &DbRequest::Ping).await,
    )
    .expect("read-only ping envelope");
    assert_eq!(readonly_ping.request_id, Some(82));
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&readonly_ping.res).expect("read-only ping response"),
        DbResponse::Pong
    ));
    drop(owner);
    close_socket(&mut read_only_socket).await;

    let created = harness
        .token_with_options(
            "ws-rw",
            TokenMode::ReadWrite,
            3600,
            shamir_connect::server::token::TokenChannels::Both,
            None,
        )
        .await;
    let (mut ws, sid, _) = open_native(harness.addr(), created.token.as_str()).await;
    let mut insert = Batch::new();
    insert.insert(
        "i",
        shamir_query_builder::write::insert("items").rows([shamir_query_builder::doc! {"n" => 88}]),
    );
    let execute = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: insert.build(),
    };
    let reply = ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 70, &execute).await)
        .expect("Execute envelope");
    assert_eq!(reply.request_id, Some(70));
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&reply.res).expect("Execute response"),
        DbResponse::Batch { .. }
    ));
    let begin_request = DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        repo: "main".into(),
        isolation: None,
    };
    let begin =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 71, &begin_request).await)
            .expect("begin envelope");
    let tx_handle = match rmp_serde::from_slice::<DbResponse>(&begin.res).expect("begin response") {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened: {other:?}"),
    };
    let mut tx_batch = Batch::new();
    tx_batch.insert(
        "i",
        shamir_query_builder::write::insert("items").rows([shamir_query_builder::doc! {"n" => 89}]),
    );
    let tx_execute = DbRequest::TxExecute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        tx_handle,
        batch: tx_batch.build(),
    };
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(
            &ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 72, &tx_execute).await)
                .expect("tx execute envelope")
                .res
        )
        .expect("tx result"),
        DbResponse::TxBatch { .. }
    ));
    let commit = DbRequest::TxCommit {
        query_version: CURRENT_QUERY_LANG_VERSION,
        visibility_wait: None,
        db: "default".into(),
        tx_handle,
    };
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(
            &ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 73, &commit).await)
                .expect("commit envelope")
                .res
        )
        .expect("commit result"),
        DbResponse::TxCommitted { .. }
    ));

    let create_cursor = DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        query: Query::from("items").build(),
        page_size: 1,
    };
    let cursor_reply =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 74, &create_cursor).await)
            .expect("cursor envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&cursor_reply.res).expect("cursor page"),
        DbResponse::CursorPage { has_more: true, .. }
    ));
    let cursor_id =
        match rmp_serde::from_slice::<DbResponse>(&cursor_reply.res).expect("cursor page") {
            DbResponse::CursorPage { cursor_id, .. } => cursor_id,
            other => panic!("expected cursor page: {other:?}"),
        };
    let fetch = DbRequest::FetchNext {
        cursor_id,
        page_size: Some(1),
    };
    let fetch_reply =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 75, &fetch).await)
            .expect("fetch envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&fetch_reply.res).expect("fetch page"),
        DbResponse::CursorPage { has_more: true, .. }
    ));
    let cancel_cursor = DbRequest::CancelCursor { cursor_id };
    let cancel_reply =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 76, &cancel_cursor).await)
            .expect("cancel envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&cancel_reply.res).expect("cursor closed"),
        DbResponse::CursorClosed { .. }
    ));
    let admin_req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.create_db(
                "create_db",
                shamir_query_builder::ddl::create_db("forbidden"),
            );
            batch.build()
        },
    };
    let admin_reply =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 79, &admin_req).await)
            .expect("admin envelope");
    assert_eq!(admin_reply.request_id, Some(79));
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&admin_reply.res).expect("admin error"), DbResponse::Error { ref code, .. } if code == "token_request_denied")
    );

    let token_with_db_scope = harness
        .token_with_options(
            "ws-database",
            TokenMode::ReadOnly,
            3600,
            shamir_connect::server::token::TokenChannels::Both,
            Some(vec!["other".into()]),
        )
        .await;
    let (mut database_ws, database_sid, _) =
        open_native(harness.addr(), token_with_db_scope.token.as_str()).await;
    let database_req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: Batch::new().build(),
    };
    let database_reply = ResponseEnvelope::from_msgpack(
        &send_request(&mut database_ws, &database_sid, 80, &database_req).await,
    )
    .expect("database restricted envelope");
    assert_eq!(database_reply.request_id, Some(80));
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&database_reply.res).expect("database denied"), DbResponse::Error { ref code, .. } if code == "token_database_denied")
    );
    close_socket(&mut database_ws).await;
    let read_req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.query("items", Query::from("items"));
            batch.build()
        },
    };
    let read_reply =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 78, &read_req).await)
            .expect("read envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&read_reply.res).expect("read response"),
        DbResponse::Batch { .. }
    ));
    close_socket(&mut ws).await;
}

async fn frame_close_cases(harness: &Harness) {
    let token = harness
        .token("ws-frame-close-cases", TokenMode::ReadOnly)
        .await;
    let (mut text_socket, _, _) = open_native(harness.addr(), &token).await;
    tokio::time::timeout(
        Duration::from_secs(15),
        text_socket.send(Message::Text("not binary".into())),
    )
    .await
    .expect("write deadline")
    .expect("send text frame");
    expect_close(&mut text_socket, 1003).await;
    drop(text_socket);

    let (mut oversized_socket, _, _) = open_native(harness.addr(), &token).await;
    let mut header = vec![0x82, 0xff];
    header.extend_from_slice(&(16_u64 * 1024 * 1024 + 1).to_be_bytes());
    header.extend_from_slice(&[1, 2, 3, 4]);
    tokio::time::timeout(
        Duration::from_secs(15),
        oversized_socket.get_mut().write_all(&header),
    )
    .await
    .expect("write deadline")
    .expect("send oversized frame header");
    expect_close(&mut oversized_socket, 1009).await;
    drop(oversized_socket);

    let (mut oversized_payload_socket, _, _) = open_native(harness.addr(), &token).await;
    let payload = vec![0; 16 * 1024 * 1024 + 1];
    let send_result = tokio::time::timeout(
        Duration::from_secs(15),
        oversized_payload_socket.send(Message::Binary(payload.into())),
    )
    .await;
    assert!(send_result.is_ok(), "binary payload send deadline");
    let _send_result = send_result.expect("checked send deadline");
    expect_close(&mut oversized_payload_socket, 1009).await;
    drop(oversized_payload_socket);

    let (mut malformed_socket, _, _) = open_native(harness.addr(), &token).await;
    tokio::time::timeout(
        Duration::from_secs(15),
        malformed_socket.send(Message::Binary(vec![0xc1].into())),
    )
    .await
    .expect("write deadline")
    .expect("send malformed outer envelope");
    let Message::Binary(reply) =
        tokio::time::timeout(Duration::from_secs(15), malformed_socket.next())
            .await
            .expect("invalid-envelope reply deadline")
            .expect("invalid-envelope reply frame")
            .expect("invalid-envelope websocket")
    else {
        panic!("expected invalid-envelope binary reply");
    };
    let reply: std::collections::BTreeMap<String, String> =
        rmp_serde::from_slice(&reply).expect("invalid-envelope reply map");
    assert_eq!(reply.len(), 1);
    assert_eq!(
        reply.get("error").map(String::as_str),
        Some("invalid_envelope")
    );
    close_socket(&mut malformed_socket).await;

    let (mut invalid_request_socket, sid, _) = open_native(harness.addr(), &token).await;
    let invalid_request = RequestEnvelope::new(sid, Some(91), vec![0xc1])
        .to_msgpack()
        .expect("encode invalid request envelope");
    tokio::time::timeout(
        Duration::from_secs(15),
        invalid_request_socket.send(Message::Binary(invalid_request.into())),
    )
    .await
    .expect("write deadline")
    .expect("send invalid request");
    let Message::Binary(reply_bytes) =
        tokio::time::timeout(Duration::from_secs(15), invalid_request_socket.next())
            .await
            .expect("invalid-request reply deadline")
            .expect("invalid-request reply frame")
            .expect("invalid-request websocket")
    else {
        panic!("expected invalid-request binary reply");
    };
    let reply: std::collections::BTreeMap<String, serde::de::IgnoredAny> =
        rmp_serde::from_slice(&reply_bytes).expect("invalid-request reply map");
    assert_eq!(
        reply.keys().map(String::as_str).collect::<Vec<_>>(),
        ["error", "rid"]
    );
    let reply: ErrorWire =
        rmp_serde::from_slice(&reply_bytes).expect("invalid-request reply fields");
    assert_eq!(reply.rid, 91);
    assert_eq!(reply.error, "invalid_request");
    close_socket(&mut invalid_request_socket).await;
}

pub(super) async fn exercise_native_group(harness: &Harness) {
    let token = harness.token("ws-native", TokenMode::ReadOnly).await;
    let valid = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {token}\r\n"),
    );
    let (headers, _) = raw_response(harness.addr(), &valid).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("101"), "{headers}");
    let missing_protocol = valid.replace("Sec-WebSocket-Protocol: shamir-v1\r\n", "");
    let wrong_protocol = valid.replace(
        "Sec-WebSocket-Protocol: shamir-v1\r\n",
        "Sec-WebSocket-Protocol: wrong\r\n",
    );
    let bad_key = valid.replace(
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Key: bad\r\n",
    );
    for request in [&missing_protocol, &wrong_protocol, &bad_key] {
        let (headers, _) = raw_response(harness.addr(), request).await;
        assert_eq!(headers.split_whitespace().nth(1), Some("400"), "{headers}");
    }
    let rest_only = harness
        .token_with_channels(
            "ws-rest-only",
            TokenMode::ReadOnly,
            shamir_connect::server::token::TokenChannels::Rest,
        )
        .await;
    let rest_only_request = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        &format!("Authorization: Bearer {rest_only}\r\n"),
    );
    let (headers, body) = raw_response(harness.addr(), &rest_only_request).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("403"), "{headers}");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("REST-only denial body"),
        DbResponse::Error { ref code, .. } if code == "token_channel_denied"
    ));
    native_error_matrix(harness, &token).await;
    let case_token = harness.token("ws-case", TokenMode::ReadOnly).await;
    let invalid_origin = upgrade(
        "/shamir/v1/ws",
        "localhost",
        Some("https://forbidden.example"),
        &format!("Authorization: Bearer {case_token}\r\n"),
    );
    assert_eq!(
        raw_response(harness.addr(), &invalid_origin)
            .await
            .0
            .split_whitespace()
            .nth(1),
        Some("403")
    );
    let wrong_method = valid.replacen("GET", "POST", 1);
    let (headers, _) = raw_response(harness.addr(), &wrong_method).await;
    assert_eq!(headers.split_whitespace().nth(1), Some("405"));
    assert!(headers.to_ascii_lowercase().contains("allow: get"));
    caps_and_sid_isolation(harness).await;
    execute_rw_and_session_features(harness).await;
    frame_close_cases(harness).await;
    limiter_last(harness).await;
}

async fn limiter_last(harness: &Harness) {
    let valid = harness
        .token("ws-valid-after-limit", TokenMode::ReadOnly)
        .await;
    let bad = upgrade(
        "/shamir/v1/ws",
        "localhost",
        None,
        "Authorization: Bearer malformed\r\n",
    );
    let mut throttled = false;
    for _ in 0..32 {
        let (headers, _) = raw_response(harness.addr(), &bad).await;
        if headers.split_whitespace().nth(1) == Some("429") {
            assert!(headers.to_ascii_lowercase().contains("retry-after:"));
            throttled = true;
            break;
        }
    }
    assert!(
        throttled,
        "invalid credentials eventually trigger source limiter"
    );
    let (mut socket, _, _) = open_native(harness.addr(), &valid).await;
    close_socket(&mut socket).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_native_group() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(false),
            trusted_proxies: vec!["127.0.0.1/32".parse().expect("proxy CIDR")],
            ..Default::default()
        },
        |config| {
            config.tokens.max_sockets_per_token = 2;
            config.tokens.max_sockets_per_owner = 3;
        },
    )
    .await;
    super::session_loop::exercise_native_session(&harness).await;
    exercise_native_group(&harness).await;
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}
