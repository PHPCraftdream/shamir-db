use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::super::http_rest::common::{assert_pong, ping_body, request, Harness};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use shamir_connect::server::token::{parse_token, TokenChannels, TokenMode};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{
    HeaderValue, AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL,
};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

const CAPTURE_LIMIT: usize = 8 * 1024 * 1024;

// This file is the sole initializer of the global test dispatch, so deactivate
// its capture after shutdown to avoid retaining later tests' trace output.
#[derive(Default)]
struct CaptureState {
    bytes: Mutex<Vec<u8>>,
    active: AtomicBool,
    overflow: AtomicBool,
}

#[derive(Clone)]
struct Capture(Arc<CaptureState>);

impl Default for Capture {
    fn default() -> Self {
        Self(Arc::new(CaptureState {
            active: AtomicBool::new(true),
            ..CaptureState::default()
        }))
    }
}

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !self.0.active.load(Ordering::Relaxed) {
            return Ok(bytes.len());
        }
        let mut captured = self.0.bytes.lock().expect("log capture lock");
        if !self.0.active.load(Ordering::Relaxed) {
            return Ok(bytes.len());
        }
        if bytes.len() > CAPTURE_LIMIT.saturating_sub(captured.len()) {
            self.0.overflow.store(true, Ordering::Relaxed);
            return Ok(bytes.len());
        }
        captured.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn subscriber(capture: Capture) -> tracing::Dispatch {
    tracing::Dispatch::new(
        tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(capture),
        ),
    )
}

fn text(capture: &Capture) -> String {
    String::from_utf8(capture.0.bytes.lock().expect("log capture lock").clone())
        .expect("captured logs are UTF-8")
}

fn assert_secret_absent(logs: &str, token: &str) {
    assert!(
        !logs.contains(token),
        "full bearer token leaked into tracing"
    );
    let parsed = parse_token(token.as_bytes()).expect("fixture token parses");
    let raw_secret = hex::encode(parsed.secret.as_bytes());
    let encoded_secret = token.rsplit('.').next().expect("secret segment");
    assert!(
        !logs.contains(encoded_secret),
        "encoded credential secret leaked"
    );
    assert!(
        !logs.contains(&raw_secret),
        "raw credential secret hex leaked"
    );
}

async fn native_ws(harness: &Harness, token: &str) {
    let uri = format!("ws://{}/shamir/v1/ws", harness.addr());
    let mut request = uri.into_client_request().expect("native WS request");
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {token}")).expect("bearer header");
    authorization.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, authorization);
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("shamir-v1"),
    );
    let connected = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("WS connection deadline");
    let (mut ws, response) = connected.expect("native WS upgrade");
    assert_eq!(response.status(), 101, "valid WS did not upgrade");
    let binding = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("WS binding deadline")
        .expect("WS stream closed before binding")
        .expect("WS binding frame");
    assert!(
        matches!(binding, tokio_tungstenite::tungstenite::Message::Binary(_)),
        "expected WS binding, got {binding:?}"
    );
    tokio::time::timeout(
        Duration::from_secs(15),
        ws.send(tokio_tungstenite::tungstenite::Message::Ping(
            Vec::new().into(),
        )),
    )
    .await
    .expect("WS ping send deadline")
    .expect("send WS ping");
    match tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("WS pong deadline")
        .expect("WS stream closed before pong")
        .expect("WS pong frame")
    {
        tokio_tungstenite::tungstenite::Message::Pong(_) => {}
        message => panic!("expected WS pong, got {message:?}"),
    }
    tokio::time::timeout(Duration::from_secs(15), ws.close(None))
        .await
        .expect("WS close deadline")
        .expect("WS close");
}

async fn failed_native_ws(harness: &Harness, token: &str) {
    let uri = format!("ws://{}/shamir/v1/ws", harness.addr());
    let mut request = uri.into_client_request().expect("failed WS request");
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {token}")).expect("invalid bearer header");
    authorization.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, authorization);
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("shamir-v1"),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("failed WS deadline");
    match result.expect_err("invalid native WS bearer unexpectedly upgraded") {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(
                response.status().as_u16(),
                401,
                "invalid WS bearer did not return 401"
            );
        }
        other => panic!("expected HTTP 401 for invalid WS bearer, got {other:?}"),
    }
}

#[tokio::test]
async fn token_listener_socket_traces_redact_rest_and_native_ws_credentials() {
    let capture = Capture::default();
    let dispatch = subscriber(capture.clone());
    tracing::dispatcher::set_global_default(dispatch).expect("install test tracing subscriber");
    let harness = Harness::launch_with_native_seed().await;
    let created = harness
        .token_with_options(
            "observability-redaction",
            TokenMode::ReadOnly,
            3600,
            TokenChannels::Both,
            None,
        )
        .await;
    let valid = created.token.to_string();
    let mut invalid = valid.as_bytes().to_vec();
    invalid[28] = if invalid[28] == b'A' { b'B' } else { b'A' };
    let invalid = String::from_utf8(invalid).expect("token ASCII");

    let (status, _, body) = request(harness.addr(), Some(&valid), &ping_body()).await;
    assert_pong(status, &body);
    let (status, _, _) = request(harness.addr(), Some(&invalid), &ping_body()).await;
    assert_eq!(status, 401);
    native_ws(&harness, &valid).await;
    failed_native_ws(&harness, &invalid).await;

    let parsed = parse_token(valid.as_bytes()).expect("valid token parses");
    let mac = harness
        .handle
        .token_service
        .mirror()
        .get(&parsed.id)
        .expect("issued token in mirror")
        .record()
        .mac;
    let mac = [
        hex::encode(mac),
        base64::engine::general_purpose::STANDARD.encode(mac),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac),
    ];

    let report = tokio::time::timeout(Duration::from_secs(45), harness.handle.shutdown())
        .await
        .expect("server shutdown deadline");
    assert!(
        report.errors().is_empty(),
        "server shutdown errors: {report:?}"
    );

    capture.0.active.store(false, Ordering::Relaxed);
    let logs = text(&capture);
    assert!(
        !capture.0.overflow.load(Ordering::Relaxed),
        "trace capture exceeded its 8 MiB limit"
    );
    assert!(
        logs.contains("rest request admitted"),
        "missing successful REST trace"
    );
    assert!(
        logs.contains("token authentication failed"),
        "missing fixed failure trace"
    );
    assert_secret_absent(&logs, &valid);
    assert_secret_absent(&logs, &invalid);
    for encoded_mac in mac {
        assert!(
            !logs.contains(&encoded_mac),
            "credential MAC leaked into tracing"
        );
    }
}
