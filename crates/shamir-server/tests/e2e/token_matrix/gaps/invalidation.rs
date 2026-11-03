use crate::http_rest::common::{Harness, ADMIN_PASSWORD};
use crate::token_matrix::support::{self, RawScram, DEADLINE};
use futures_util::StreamExt;
use shamir_client::{Client, ConnectOptions};
use shamir_connect::client::changepw::{build_request, try_build_challenge_request};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::token::{parse_token, TokenId};
use shamir_query_builder::{batch::Batch, ddl, token_admin};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex, derive_session_hmac_key};
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use shamir_query_types::wire::{ChangePasswordSecrets, DbRequest, DbResponse};
use std::time::Duration;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};
use zeroize::Zeroizing;

fn token_id(token: &str) -> TokenId {
    parse_token(token.as_bytes()).expect("canonical token").id
}

async fn stop(harness: Harness) {
    let report = tokio::time::timeout(Duration::from_secs(45), harness.handle.shutdown())
        .await
        .expect("server shutdown timed out");
    assert!(
        report.errors().is_empty(),
        "shutdown errors: {:?}",
        report.errors()
    );
}

async fn open_ws(
    harness: &Harness,
    tls: bool,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<support::Stream> {
    let scheme = if tls { "wss" } else { "ws" };
    let mut request = format!("{scheme}://{}/shamir/v1/ws", harness.addr())
        .into_client_request()
        .expect("WebSocket request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let (mut ws, response) = tokio::time::timeout(
        DEADLINE,
        tokio_tungstenite::client_async(request, support::connect(harness, tls).await),
    )
    .await
    .expect("WebSocket upgrade timed out")
    .expect("WebSocket upgrade");
    assert_eq!(response.status().as_u16(), 101);
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Binding {
        ok: bool,
        #[serde(with = "serde_bytes")]
        sid: Vec<u8>,
        expires_at_ns: u64,
    }
    let binding = tokio::time::timeout(DEADLINE, ws.next())
        .await
        .expect("binding timed out")
        .expect("binding stream")
        .expect("binding frame");
    let Message::Binary(bytes) = binding else {
        panic!("expected binary binding");
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("decode binding");
    assert!(binding.ok && binding.expires_at_ns > 0 && binding.sid.len() == 32);
    ws
}

async fn change_owner_role(admin: &Client) {
    let unsigned = ddl::grant_role("matrix_wss_role", "admin")
        .hmac("placeholder")
        .build()
        .expect("build placeholder grant_role");
    let BatchOp::GrantRole(mut grant) = unsigned else {
        panic!("expected GrantRole");
    };
    grant.hmac = None;
    let unsigned = BatchOp::GrantRole(grant);
    let key = derive_session_hmac_key(&admin.session_id());
    let tag = compute_tag_hex(
        &key,
        &canonical_batch_op("default", &unsigned).expect("canonical role op"),
    );
    let mut batch = Batch::new();
    batch.grant_role(
        "role_change",
        ddl::grant_role("matrix_wss_role", "admin")
            .hmac(tag)
            .build()
            .expect("signed grant_role"),
    );
    admin
        .execute("default", batch.build())
        .await
        .expect("grant owner role");
}

async fn wait_for_close_4011(
    ws: &mut tokio_tungstenite::WebSocketStream<support::Stream>,
    context: &str,
) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect(context);
        match message {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), 4011);
                return;
            }
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
            Some(other) => panic!("unexpected WebSocket frame before close: {other:?}"),
            None => panic!("WebSocket ended without close frame"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_role_change_over_wss_closes_token_session() {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(true),
            ..Default::default()
        },
        |_| {},
    )
    .await;
    let admin = Client::connect(ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(DEADLINE),
        handshake_timeout: Some(DEADLINE),
        request_timeout: Some(DEADLINE),
    })
    .await
    .expect("SCRAM admin session");
    let created = admin
        .create_api_token(
            token_admin::create_api_token("owner-wss-role")
                .ttl_secs(3600)
                .channels(ApiTokenChannels::Ws),
        )
        .await
        .expect("create owner token");
    let id = token_id(created.token.reveal());
    let mut ws = open_ws(&harness, true, created.token.reveal()).await;
    assert_eq!(
        harness.handle.token_service.sessions().count_for_token(&id),
        1
    );
    change_owner_role(&admin).await;
    wait_for_close_4011(&mut ws, "WSS close timed out").await;
    assert_eq!(
        harness.handle.token_service.sessions().count_for_token(&id),
        0
    );
    drop(ws);
    drop(admin);
    stop(harness).await;
}

async fn ping_status(harness: &Harness, tls: bool, token: &str) -> u16 {
    let head = format!(
        "GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let (response, _) = tokio::time::timeout(DEADLINE, support::send_http(harness, tls, &head))
        .await
        .expect("REST request timed out");
    support::status(&response)
}

async fn password_change_invalidation(tls: bool) {
    let harness = Harness::launch_with_native_seed_hook(
        "127.0.0.1:0",
        shamir_server::config::HttpListenerSettings {
            tls: Some(tls),
            ..Default::default()
        },
        |_| {},
    )
    .await;
    let admin = Client::connect(ConnectOptions {
        addr: harness.native_addr(),
        server_name: "localhost".into(),
        username: "admin".into(),
        password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(DEADLINE),
        handshake_timeout: Some(DEADLINE),
        request_timeout: Some(DEADLINE),
    })
    .await
    .expect("SCRAM admin session");
    admin
        .create_scram_user(
            "password-owner",
            Zeroizing::new("old-password".into()),
            vec![],
        )
        .await
        .expect("create ordinary owner");
    drop(admin);

    let (rest_token, ws_token) = {
        let owner = Client::connect(ConnectOptions {
            addr: harness.native_addr(),
            server_name: "localhost".into(),
            username: "password-owner".into(),
            password: Zeroizing::new(b"old-password".to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(DEADLINE),
            handshake_timeout: Some(DEADLINE),
            request_timeout: Some(DEADLINE),
        })
        .await
        .expect("ordinary SCRAM session");
        let rest = owner
            .create_api_token(
                token_admin::create_api_token("password-change-rest")
                    .ttl_secs(3600)
                    .mode(ApiTokenMode::ReadOnly)
                    .channels(ApiTokenChannels::Rest),
            )
            .await
            .expect("create REST token");
        let ws = owner
            .create_api_token(
                token_admin::create_api_token("password-change-ws")
                    .ttl_secs(3600)
                    .mode(ApiTokenMode::ReadOnly)
                    .channels(ApiTokenChannels::Ws),
            )
            .await
            .expect("create WS token");
        (rest.token.reveal().to_owned(), ws.token.reveal().to_owned())
    };
    let ws_id = token_id(&ws_token);
    let mut ws = open_ws(&harness, tls, &ws_token).await;
    assert_eq!(ping_status(&harness, tls, &rest_token).await, 200);

    let mut owner = RawScram::connect(&harness, "password-owner", b"old-password").await;
    let client_nonce = try_build_challenge_request().expect("change-password nonce");
    let raw = owner
        .request_raw(&DbRequest::ChangePasswordChallenge {
            client_nonce_cp: client_nonce.to_vec(),
        })
        .await;
    let challenge: DbResponse = rmp_serde::from_slice(&raw).expect("decode password challenge");
    let DbResponse::ChangePasswordChallenge {
        server_nonce_cp,
        salt,
        kdf_memory_kb,
        kdf_time,
        kdf_parallelism,
        kdf_argon2_version,
    } = challenge
    else {
        panic!("expected change-password challenge, got {challenge:?}");
    };
    let server_nonce: [u8; 32] = server_nonce_cp.try_into().expect("server nonce length");
    let salt: [u8; 16] = salt.try_into().expect("password salt length");
    let kdf = KdfParams {
        memory_kb: kdf_memory_kb,
        time: kdf_time,
        parallelism: kdf_parallelism,
        argon2_version: kdf_argon2_version,
    };
    let username = NormalizedUsername::from_raw("password-owner").expect("normalized username");
    let mut old_password = b"old-password".to_vec();
    let mut new_password = b"new-password-strong-long".to_vec();
    let changed = build_request(
        &username,
        &owner.session_id(),
        &client_nonce,
        &server_nonce,
        &salt,
        kdf,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        &owner.exporter(),
        &mut old_password,
        &mut new_password,
        kdf,
    )
    .expect("build SCRAM change-password proof");
    let raw = owner
        .request_raw(&DbRequest::ChangePasswordVerify(ChangePasswordSecrets {
            client_proof_old: changed.client_proof_old.to_vec(),
            new_salt: changed.new_salt.to_vec(),
            new_stored_key: changed.new_stored_key.to_vec(),
            new_server_key: changed.new_server_key.to_vec(),
        }))
        .await;
    let changed: DbResponse = rmp_serde::from_slice(&raw).expect("decode password-change result");
    assert!(matches!(changed, DbResponse::ChangePasswordOk));

    assert_eq!(ping_status(&harness, tls, &rest_token).await, 401);
    wait_for_close_4011(&mut ws, "password-change WebSocket close timed out").await;
    assert_eq!(
        harness
            .handle
            .token_service
            .sessions()
            .count_for_token(&ws_id),
        0
    );
    drop(ws);
    drop(owner);
    let new_owner = tokio::time::timeout(
        DEADLINE,
        Client::connect(ConnectOptions {
            addr: harness.native_addr(),
            server_name: "localhost".into(),
            username: "password-owner".into(),
            password: Zeroizing::new(b"new-password-strong-long".to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(DEADLINE),
            handshake_timeout: Some(DEADLINE),
            request_timeout: Some(DEADLINE),
        }),
    )
    .await
    .expect("new-password SCRAM login timed out")
    .expect("new-password SCRAM login");
    tokio::time::timeout(DEADLINE, new_owner.ping())
        .await
        .expect("new-password SCRAM ping timed out")
        .expect("new-password SCRAM ping");
    drop(new_owner);
    stop(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_user_password_change_revokes_http_ws_tokens() {
    password_change_invalidation(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_user_password_change_revokes_https_wss_tokens() {
    password_change_invalidation(true).await;
}
