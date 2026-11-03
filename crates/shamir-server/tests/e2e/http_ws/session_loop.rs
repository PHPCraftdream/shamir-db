use super::super::http_rest::common::{grant_default_main_access, Harness, ADMIN_PASSWORD};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::pem::PemObject;
use serde::{Deserialize, Serialize};
use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::extract_tls_exporter;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_rustls::TlsConnector;
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

pub(super) async fn open_native(
    addr: std::net::SocketAddr,
    token: &str,
) -> (WebSocketStream<TcpStream>, [u8; 32], u64) {
    let url = format!("ws://{addr}/shamir/v1/ws");
    let mut request = url.into_client_request().expect("build WS request");
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("shamir-v1"),
    );
    let stream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect");
    let (mut ws, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(request, stream),
    )
    .await
    .expect("WS handshake timeout")
    .expect("WS handshake");
    assert_eq!(response.status().as_u16(), 101);
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some("shamir-v1")
    );
    assert!(!response.headers().contains_key("sec-websocket-extensions"));
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("binding timeout")
        .expect("binding stream")
        .expect("binding frame");
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("binding is not binary");
    };
    let binding: Binding = rmp_serde::from_slice(&bytes).expect("decode binding");
    assert!(binding.ok && binding.expires_at_ns > 0);
    assert_eq!(binding.sid.len(), 32);
    assert!(binding.sid.iter().any(|byte| *byte != 0));
    let sid: [u8; 32] = binding.sid.as_slice().try_into().expect("32-byte sid");
    (ws, sid, binding.expires_at_ns)
}

pub(super) async fn send_request(
    ws: &mut WebSocketStream<TcpStream>,
    sid: &[u8; 32],
    rid: u32,
    request: &DbRequest,
) -> Vec<u8> {
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
    let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("reply timeout")
        .expect("reply stream")
        .expect("reply frame");
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("reply is not binary");
    };
    let response = ResponseEnvelope::from_msgpack(&bytes).expect("response envelope");
    assert_eq!(
        response.request_id,
        Some(rid),
        "unexpected response request id"
    );
    let raw_map: std::collections::BTreeMap<String, serde::de::IgnoredAny> =
        rmp_serde::from_slice(&bytes).expect("raw response map");
    assert_eq!(
        raw_map.keys().map(String::as_str).collect::<Vec<_>>(),
        ["res", "rid"]
    );
    bytes.to_vec()
}

pub(super) async fn assert_token_sid_rejected_over_scram(harness: &Harness, token_sid: [u8; 32]) {
    #[derive(Serialize)]
    struct AuthInit {
        user: String,
        #[serde(with = "serde_bytes")]
        client_nonce: Vec<u8>,
        binding_mode: u8,
        version: u8,
    }
    #[derive(Deserialize)]
    struct Challenge {
        #[serde(with = "serde_bytes")]
        salt: Vec<u8>,
        memory_kb: u32,
        time: u32,
        parallelism: u32,
        argon2_version: u8,
        #[serde(with = "serde_bytes")]
        server_nonce: Vec<u8>,
    }
    #[derive(Serialize)]
    struct ClientProof {
        #[serde(with = "serde_bytes")]
        client_proof: Vec<u8>,
    }
    #[derive(Deserialize)]
    struct AuthOk {
        #[serde(with = "serde_bytes")]
        server_signature: Vec<u8>,
        #[serde(with = "serde_bytes")]
        server_pub_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        identity_sig: Vec<u8>,
        #[serde(with = "serde_bytes")]
        session_id: Vec<u8>,
        expires_at_ns: u64,
        #[serde(default, with = "serde_bytes")]
        resumption_ticket: Vec<u8>,
        #[serde(default)]
        resumption_expires_at_ns: u64,
        #[serde(default)]
        _server_query_version: u8,
        #[serde(default)]
        _rotation_in_progress: Option<shamir_connect::common::wire::RotationInProgressWire>,
        #[serde(default)]
        _kdf_upgrade_required: Option<bool>,
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let pem = std::fs::read(harness.temp.path().join("cert.pem")).expect("read generated cert");
    let cert = rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .next()
        .expect("certificate PEM entry")
        .expect("parse certificate");
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("trust generated certificate");
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 supported")
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(std::sync::Arc::new(config));
    let tcp = tokio::time::timeout_at(deadline, TcpStream::connect(harness.native_addr()))
        .await
        .expect("connect timeout")
        .expect("connect");
    let tls = tokio::time::timeout_at(
        deadline,
        connector.connect(
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
            tcp,
        ),
    )
    .await
    .expect("TLS timeout")
    .expect("TLS handshake");
    let exporter = extract_tls_exporter(&tls).expect("client exporter");
    let username = NormalizedUsername::from_raw("admin").expect("username");
    let hs = HandshakeBuilder::new(username, TransportKind::Tcp, BindingMode::TlsExporter)
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .expect("handshake builder");
    let (mut r, mut w) = tokio::io::split(tls);
    let init = hs.auth_init();
    let init = AuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    let bytes = rmp_serde::to_vec(&init).expect("encode auth_init");
    tokio::time::timeout_at(deadline, write_frame(&mut w, &bytes))
        .await
        .expect("auth_init write timeout")
        .expect("send auth_init");
    let bytes = tokio::time::timeout_at(deadline, read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT))
        .await
        .expect("challenge timeout")
        .expect("read challenge");
    let challenge: Challenge = rmp_serde::from_slice(&bytes).expect("decode challenge");
    let mut salt = [0; 16];
    salt.copy_from_slice(&challenge.salt);
    let mut server_nonce = [0; 32];
    server_nonce.copy_from_slice(&challenge.server_nonce);
    let challenge = ServerChallenge {
        salt,
        kdf_params: KdfParams {
            memory_kb: challenge.memory_kb,
            time: challenge.time,
            parallelism: challenge.parallelism,
            argon2_version: challenge.argon2_version,
        },
        server_nonce,
    };
    let mut password = ADMIN_PASSWORD.to_vec();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password)
        .expect("process challenge");
    let bytes = rmp_serde::to_vec(&ClientProof {
        client_proof: proof.to_vec(),
    })
    .expect("encode proof");
    tokio::time::timeout_at(deadline, write_frame(&mut w, &bytes))
        .await
        .expect("proof write timeout")
        .expect("send proof");
    let bytes = tokio::time::timeout_at(deadline, read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT))
        .await
        .expect("auth_ok timeout")
        .expect("read auth_ok");
    let ok: AuthOk = rmp_serde::from_slice(&bytes).expect("decode auth_ok");
    let mut signature = [0; 32];
    signature.copy_from_slice(&ok.server_signature);
    let mut public_key = [0; 32];
    public_key.copy_from_slice(&ok.server_pub_key);
    let mut identity_sig = [0; 64];
    identity_sig.copy_from_slice(&ok.identity_sig);
    let mut session_id = [0; 32];
    session_id.copy_from_slice(&ok.session_id);
    let auth_ok = ServerAuthOk {
        server_signature: signature,
        server_pub_key: public_key,
        identity_sig,
        session_id,
        expires_at_ns: ok.expires_at_ns,
        resumption_ticket: Some(ok.resumption_ticket),
        resumption_expires_at_ns: Some(ok.resumption_expires_at_ns),
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    hs.process_auth_ok(&auth_ok, &derived, &am, |_pin| {})
        .expect("process auth_ok");
    let request = RequestEnvelope::new(
        token_sid,
        Some(83),
        rmp_serde::to_vec(&DbRequest::Ping).expect("encode ping"),
    );
    let bytes = request.to_msgpack().expect("encode request envelope");
    tokio::time::timeout_at(deadline, write_frame(&mut w, &bytes))
        .await
        .expect("request write timeout")
        .expect("send ping");
    let bytes = tokio::time::timeout_at(deadline, read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT))
        .await
        .expect("error response timeout")
        .expect("read error response");
    let error = ErrorEnvelope::from_msgpack(&bytes).expect("decode error envelope");
    assert_eq!(error.request_id, Some(83));
    assert_eq!(error.error, "session_expired");
    let _ = tokio::time::timeout_at(deadline, w.shutdown()).await;
}

pub(super) async fn exercise_native_session(harness: &Harness) {
    let admin = shamir_client::Client::connect(shamir_client::ConnectOptions {
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
    .expect("native SCRAM session");
    let mut create = Batch::new();
    create.create_table("items", ddl::create_table("items"));
    admin
        .execute("default", create.build())
        .await
        .expect("create table");
    for value in 0..3 {
        let mut seed = Batch::new();
        seed.insert(
            &format!("seed{value}"),
            shamir_query_builder::write::insert("items")
                .rows([shamir_query_builder::doc! { "n" => value }]),
        );
        admin
            .execute("default", seed.build())
            .await
            .expect("seed row");
    }
    grant_default_main_access(&admin).await;
    drop(admin);
    let created = harness
        .token_with_options(
            "ws-readonly",
            shamir_connect::server::token::TokenMode::ReadOnly,
            3600,
            shamir_connect::server::token::TokenChannels::Both,
            None,
        )
        .await;
    let (mut ws, sid, expires_at_ns) = open_native(harness.addr(), created.token.as_str()).await;
    assert_eq!(expires_at_ns, created.record_view.expires_at_ns);
    let ping = send_request(&mut ws, &sid, 51, &DbRequest::Ping).await;
    let pong = ResponseEnvelope::from_msgpack(&ping).expect("response envelope");
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&pong.res).expect("pong"),
        DbResponse::Pong
    ));
    let write = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: {
            let mut batch = Batch::new();
            batch.insert(
                "i",
                shamir_query_builder::write::insert("items")
                    .rows([shamir_query_builder::doc! { "n" => 1 }]),
            );
            batch.build()
        },
    };
    let denial = send_request(&mut ws, &sid, 52, &write).await;
    let response = ResponseEnvelope::from_msgpack(&denial).expect("response envelope");
    assert_eq!(response.request_id, Some(52));
    let error = rmp_serde::from_slice::<DbResponse>(&response.res).expect("scope response");
    assert!(
        matches!(error, DbResponse::Error { ref code, ref message } if code == "token_read_only" && message == "token_read_only")
    );
    assert_token_sid_rejected_over_scram(harness, sid).await;
    let report_close = ws.close(None).await;
    assert!(report_close.is_ok());
    let close_frame = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("close response timeout");
    match close_frame {
        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {}
        Some(Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed))
        | Some(Err(tokio_tungstenite::tungstenite::Error::AlreadyClosed)) => {}
        other => panic!("unexpected frame after close: {other:?}"),
    }
}
