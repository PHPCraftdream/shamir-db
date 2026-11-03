use super::super::http_rest::common::{grant_default_main_access, Harness, ADMIN_PASSWORD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use shamir_client::ConnectOptions;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::{PushEnvelope, PushKind};
use shamir_connect::server::token::{parse_token, TokenId, TokenMode};
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex, derive_session_hmac_key};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: Vec<u8>,
    expires_at_ns: u64,
}

async fn open_socket(
    addr: std::net::SocketAddr,
    token: &str,
) -> (WebSocketStream<TcpStream>, [u8; 32]) {
    let mut request = format!("ws://{addr}/shamir/v1/ws")
        .into_client_request()
        .expect("request URI");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let tcp = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    let (mut ws, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(request, tcp),
    )
    .await
    .expect("handshake timeout")
    .expect("handshake");
    assert_eq!(response.status().as_u16(), 101);
    let binding = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("binding timeout")
        .expect("binding frame")
        .expect("binding message");
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = binding else {
        panic!("expected binary binding frame");
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("decode binding");
    assert!(binding.ok && binding.expires_at_ns > 0);
    let sid = binding.sid.as_slice().try_into().expect("32-byte sid");
    (ws, sid)
}

fn token_id(token: &str) -> TokenId {
    parse_token(token.as_bytes())
        .expect("parse canonical token")
        .id
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

async fn send_request_collect_pushes(
    ws: &mut WebSocketStream<TcpStream>,
    sid: &[u8; 32],
    rid: u32,
    request: &DbRequest,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let body = rmp_serde::to_vec_named(request).expect("encode DB request");
    let envelope = RequestEnvelope::new(*sid, Some(rid), body)
        .to_msgpack()
        .expect("encode envelope");
    tokio::time::timeout(
        Duration::from_secs(15),
        ws.send(tokio_tungstenite::tungstenite::Message::Binary(
            envelope.into(),
        )),
    )
    .await
    .expect("request write timeout")
    .expect("send request");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut pushes = Vec::new();
    loop {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect("reply timeout")
            .expect("reply stream")
            .expect("reply frame");
        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
            panic!("expected binary reply, got {message:?}");
        };
        let fields: std::collections::BTreeMap<String, serde::de::IgnoredAny> =
            rmp_serde::from_slice(&bytes).expect("binary WebSocket map");
        if fields.contains_key("push") {
            assert_push(&bytes, "subscription push while waiting for reply");
            pushes.push(bytes.to_vec());
            continue;
        }
        let response = ResponseEnvelope::from_msgpack(&bytes).expect("response envelope");
        assert_eq!(
            response.request_id,
            Some(rid),
            "unexpected response request id"
        );
        return (bytes.to_vec(), pushes);
    }
}

async fn send_request(
    ws: &mut WebSocketStream<TcpStream>,
    sid: &[u8; 32],
    rid: u32,
    request: &DbRequest,
) -> Vec<u8> {
    let (response, pushes) = send_request_collect_pushes(ws, sid, rid, request).await;
    assert!(
        pushes.is_empty(),
        "unexpected subscription push during request"
    );
    response
}

fn assert_push(bytes: &[u8], context: &str) {
    let push: std::collections::BTreeMap<String, serde::de::IgnoredAny> =
        rmp_serde::from_slice(bytes).expect(context);
    assert!(
        push.contains_key("push") && push.contains_key("sub") && push.contains_key("seq"),
        "{context}"
    );
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushedEvent {
    table: String,
    op: String,
    value: Option<shamir_types::types::value::QueryValue>,
    #[serde(rename = "key")]
    _key: shamir_types::types::value::QueryValue,
    commit_version: u64,
}

async fn expect_ready_and_event(
    ws: &mut WebSocketStream<TcpStream>,
    mut saw_ready: bool,
    mut saw_event: bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !saw_ready || !saw_event {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect("subscription push timeout")
            .expect("subscription stream")
            .expect("subscription frame");
        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
            panic!("expected subscription push, got {message:?}");
        };
        observe_subscription_push(&bytes, &mut saw_ready, &mut saw_event);
    }
}

fn observe_subscription_push(bytes: &[u8], saw_ready: &mut bool, saw_event: &mut bool) {
    let push: PushEnvelope = rmp_serde::from_slice(bytes).expect("decode typed subscription push");
    match push.push {
        PushKind::Ready => *saw_ready = true,
        PushKind::Event => {
            assert!(push.sub > 0, "event identifies its subscription");
            let event_data = push.data.as_deref().expect("event payload");
            let event: PushedEvent =
                rmp_serde::from_slice(event_data).expect("decode event payload");
            assert_eq!(event.table, "items");
            assert_eq!(event.op, "put");
            if event
                .value
                .as_ref()
                .and_then(|value| value.get("n"))
                .and_then(shamir_types::types::value::QueryValue::as_i64)
                == Some(12345)
            {
                assert!(
                    event.commit_version > 0,
                    "live event carries the committed version"
                );
                *saw_event = true;
            }
        }
        other => panic!("unexpected subscription push: {other:?}"),
    }
}

async fn expect_close_without_binary(ws: &mut WebSocketStream<TcpStream>, expected: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect("close timeout")
            .expect("close stream")
            .expect("close frame");
        match message {
            tokio_tungstenite::tungstenite::Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), expected);
                return;
            }
            tokio_tungstenite::tungstenite::Message::Ping(_)
            | tokio_tungstenite::tungstenite::Message::Pong(_) => {}
            tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                assert_push(&bytes, "subscription push after revocation");
                panic!("subscription push arrived after revocation");
            }
            other => panic!("unexpected frame before revocation close: {other:?}"),
        }
    }
}

async fn expect_close(ws: &mut WebSocketStream<TcpStream>, expected: u16) {
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("close timeout")
        .expect("close message")
        .expect("close frame");
    let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = message else {
        panic!("expected websocket close frame");
    };
    assert_eq!(u16::from(frame.code), expected);
}

async fn native_admin(harness: &Harness) -> shamir_client::Client {
    shamir_client::Client::connect(ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        handshake_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("native admin session")
}

async fn prepare(harness: &Harness) {
    let admin = native_admin(harness).await;
    let mut batch = Batch::new();
    batch.create_table("items", ddl::create_table("items"));
    admin
        .execute("default", batch.build())
        .await
        .expect("create items table");
    grant_default_main_access(&admin).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_ws_revocation_expiry_group() {
    let harness = Harness::launch_with_native_seed().await;
    prepare(&harness).await;
    let mut seed = Batch::new();
    for n in 0..4 {
        seed.insert(
            format!("seed{n}"),
            shamir_query_builder::write::insert("items")
                .rows([shamir_query_builder::doc! { "n" => n }]),
        );
    }
    let seed_admin = native_admin(&harness).await;
    seed_admin
        .execute("default", seed.build())
        .await
        .expect("seed cursor rows before subscription");
    drop(seed_admin);
    let token = harness.token("ws-revoke", TokenMode::ReadWrite).await;
    let id = token_id(&token);
    let (mut ws, sid) = open_socket(harness.addr(), &token).await;
    assert_eq!(
        harness.handle.token_service.sessions().count_for_token(&id),
        1
    );
    let mut subscribe = Batch::new();
    subscribe.id("sub");
    subscribe.subscribe(
        "watch",
        shamir_query_builder::batch::Subscribe::table(shamir_query_types::TableRef::with_repo(
            "main", "items",
        ))
        .with_initial()
        .build(),
    );
    let subscribe_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: subscribe.build(),
    };
    let mut buffered_pushes = Vec::new();
    let (subscribed_bytes, mut pushes) =
        send_request_collect_pushes(&mut ws, &sid, 1, &subscribe_request).await;
    buffered_pushes.append(&mut pushes);
    let subscribed =
        ResponseEnvelope::from_msgpack(&subscribed_bytes).expect("subscription response");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&subscribed.res).expect("subscription result"),
        DbResponse::Batch { .. }
    ));
    let mut saw_ready = false;
    let mut saw_event = false;
    for bytes in buffered_pushes.drain(..) {
        observe_subscription_push(&bytes, &mut saw_ready, &mut saw_event);
    }
    let readiness_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !saw_ready {
        let message = tokio::time::timeout_at(readiness_deadline, ws.next())
            .await
            .expect("subscription ready timeout")
            .expect("subscription stream")
            .expect("subscription frame");
        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
            panic!("expected subscription push, got {message:?}");
        };
        observe_subscription_push(&bytes, &mut saw_ready, &mut saw_event);
    }
    let trigger_admin = native_admin(&harness).await;
    let mut trigger = Batch::new();
    trigger.insert(
        "trigger",
        shamir_query_builder::write::insert("items")
            .rows([shamir_query_builder::doc! { "n" => 12345 }]),
    );
    trigger_admin
        .execute("default", trigger.build())
        .await
        .expect("native admin trigger write");
    drop(trigger_admin);
    expect_ready_and_event(&mut ws, saw_ready, saw_event).await;
    let tx_begin = DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        repo: "main".into(),
        isolation: None,
    };
    let begun = ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 2, &tx_begin).await)
        .expect("tx begin envelope");
    let tx_handle =
        match rmp_serde::from_slice::<DbResponse>(&begun.res).expect("tx begin response") {
            DbResponse::TxOpened { tx_handle, .. } => tx_handle,
            other => panic!("expected opened transaction, got {other:?}"),
        };
    let mut staged = Batch::new();
    staged.insert(
        "staged",
        shamir_query_builder::write::insert("items")
            .rows([shamir_query_builder::doc! { "n" => 987654 }]),
    );
    let staged_write = DbRequest::TxExecute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        tx_handle,
        batch: staged.build(),
    };
    let staged_response =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 3, &staged_write).await)
            .expect("staged write envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&staged_response.res).expect("staged write"),
        DbResponse::TxBatch { .. }
    ));

    let cursor = DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        query: shamir_query_builder::Query::from("items").build(),
        page_size: 1,
    };
    let cursor_reply =
        ResponseEnvelope::from_msgpack(&send_request(&mut ws, &sid, 4, &cursor).await)
            .expect("cursor envelope");
    let cursor_id =
        match rmp_serde::from_slice::<DbResponse>(&cursor_reply.res).expect("cursor result") {
            DbResponse::CursorPage {
                cursor_id,
                has_more: true,
                ..
            } => cursor_id,
            other => panic!("expected cursor with remaining pages, got {other:?}"),
        };

    assert!(harness
        .handle
        .token_service
        .revoke(id, now_ns())
        .await
        .expect("revoke token"));
    expect_close_without_binary(&mut ws, 4011).await;
    assert_eq!(
        harness.handle.token_service.sessions().count_for_token(&id),
        0
    );
    drop(ws);

    let verify_token = harness.token("ws-verify", TokenMode::ReadWrite).await;
    let verify_id = token_id(&verify_token);
    let (mut verify_ws, verify_sid) = open_socket(harness.addr(), &verify_token).await;
    let mut visibility_query = Batch::new();
    visibility_query.query("all", shamir_query_builder::Query::from("items"));
    let visibility_request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: visibility_query.build(),
    };
    let visibility_reply = ResponseEnvelope::from_msgpack(
        &send_request(&mut verify_ws, &verify_sid, 6, &visibility_request).await,
    )
    .expect("native read envelope");
    let visible = match rmp_serde::from_slice::<DbResponse>(&visibility_reply.res)
        .expect("native read result")
    {
        DbResponse::Batch { response } => response,
        other => panic!("expected native read batch, got {other:?}"),
    };
    assert!(
        !visible
            .results
            .get("all")
            .expect("query result")
            .records
            .iter()
            .any(|record| record.get_value_i64("n") == Some(987654)),
        "revoked session's staged write must roll back"
    );
    let unknown_cursor = DbRequest::FetchNext {
        cursor_id,
        page_size: Some(1),
    };
    let cursor_error = ResponseEnvelope::from_msgpack(
        &send_request(&mut verify_ws, &verify_sid, 7, &unknown_cursor).await,
    )
    .expect("cursor error envelope");
    assert!(
        matches!(rmp_serde::from_slice::<DbResponse>(&cursor_error.res).expect("cursor error"),
        DbResponse::Error { ref code, .. } if code == "cursor_not_found")
    );
    let read = native_admin(&harness).await;
    let mut query = Batch::new();
    query.query("all", shamir_query_builder::Query::from("items"));
    let response = read
        .execute("default", query.build())
        .await
        .expect("read after revocation");
    assert!(
        !response
            .results
            .get("all")
            .expect("query result")
            .records
            .iter()
            .any(|record| record.get_value_i64("n") == Some(987654)),
        "revoked session's staged write must roll back"
    );
    drop(read);

    let expiring = harness
        .token_with_ttl("ws-expire", TokenMode::ReadWrite, 2)
        .await;
    let expiring_id = token_id(&expiring);
    let (mut expiring_ws, _) = open_socket(harness.addr(), &expiring).await;
    expect_close(&mut expiring_ws, 4012).await;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&expiring_id),
        0
    );
    drop(expiring_ws);

    let user_token = harness.token("ws-revoke-user", TokenMode::ReadWrite).await;
    let user_id = token_id(&user_token);
    let (mut user_ws, _) = open_socket(harness.addr(), &user_token).await;
    assert!(harness
        .handle
        .token_service
        .revoke(verify_id, now_ns())
        .await
        .expect("revoke verification token"));
    expect_close(&mut verify_ws, 4011).await;
    let actual_revocations = harness
        .handle
        .token_service
        .revoke_user(harness.owner, now_ns())
        .await
        .expect("revoke owner tokens");
    assert_eq!(actual_revocations, 1);
    expect_close(&mut user_ws, 4011).await;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&user_id),
        0
    );
    drop(user_ws);
    let admin = native_admin(&harness).await;
    let role_token = harness.token("ws-role-change", TokenMode::ReadWrite).await;
    let role_id = token_id(&role_token);
    let (mut role_ws, _) = open_socket(harness.addr(), &role_token).await;
    let owner_username = "admin";
    let session_key = derive_session_hmac_key(&admin.session_id());
    let unsigned_role = ddl::grant_role("e2e_revocation_role", owner_username)
        .hmac("placeholder")
        .build()
        .expect("build placeholder grant_role");
    let shamir_query_types::batch::BatchOp::GrantRole(mut unsigned_grant) = unsigned_role else {
        panic!("grant_role builder returned another operation");
    };
    unsigned_grant.hmac = None;
    let unsigned_role = shamir_query_types::batch::BatchOp::GrantRole(unsigned_grant);
    let role_tag = compute_tag_hex(
        &session_key,
        &canonical_batch_op("default", &unsigned_role).expect("canonical grant_role"),
    );
    let mut grant = Batch::new();
    grant.grant_role(
        "role_change",
        ddl::grant_role("e2e_revocation_role", owner_username)
            .hmac(role_tag)
            .build()
            .expect("build grant_role"),
    );
    admin
        .execute("default", grant.build())
        .await
        .expect("grant owner role");
    expect_close(&mut role_ws, 4011).await;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&role_id),
        0
    );
    drop(role_ws);
    drop(admin);
    drop(verify_ws);

    let report = tokio::time::timeout(Duration::from_secs(15), harness.handle.shutdown())
        .await
        .expect("shutdown timeout");
    assert!(report.errors().is_empty(), "{report:?}");
}
