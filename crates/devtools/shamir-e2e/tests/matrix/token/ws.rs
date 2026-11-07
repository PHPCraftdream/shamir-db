//! Token-authenticated native WebSocket client against a live HTTP listener.

use std::time::{Duration, Instant};

use shamir_connect::server::token::TokenMode;
use zeroize::Zeroizing;

use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};

use shamir_client::{
    ApiToken, AuthKind, Client, ClientError, CloseCause, TokenEndpoint, TokenWsOptions,
};

use super::common::{boot, read_batch, seed_batch, write_batch, TokenFixture};

const DEADLINE: Duration = Duration::from_secs(15);

async fn connect_ws(endpoint: TokenEndpoint, token: &str) -> Result<Client, ClientError> {
    let token = ApiToken::new(token).expect("well-formed token");
    Client::connect_token_ws(TokenWsOptions::new(endpoint, token)).await
}

async fn open(fixture: &TokenFixture, token: &str) -> Client {
    connect_ws(fixture.tls_endpoint(), token)
        .await
        .unwrap_or_else(|e| panic!("token websocket connect failed: {e:?}"))
}

async fn wait_peer_closed(client: &Client, expected: u16) {
    wait_peer_closed_with_marks(client, expected, |_| {}).await;
}

/// `wait_peer_closed` with failure-only diagnostics: `on_event` receives the
/// decisive observations (each NEW error seen while waiting, the observed
/// peer close code, or the deadline hit with the last error) so the guard's
/// report names the code or error received. Assertion, deadline and sleep
/// behaviour are identical to `wait_peer_closed`.
async fn wait_peer_closed_with_marks(
    client: &Client,
    expected: u16,
    mut on_event: impl FnMut(&str),
) {
    let started = Instant::now();
    let mut last = String::from("no failure observed");
    while started.elapsed() < DEADLINE {
        match client.ping().await {
            Ok(_) => {}
            Err(ClientError::ConnectionClosed(CloseCause::PeerClosed { code, .. })) => {
                on_event(&format!(
                    "peer closed with code {code} after {:?}",
                    started.elapsed()
                ));
                assert_eq!(code, expected);
                return;
            }
            Err(other) => {
                let rendered = format!("{other:?}");
                if rendered != last {
                    on_event(&format!("error observed: {rendered}"));
                    last = rendered;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    on_event(&format!(
        "no close with {expected} within deadline; last: {last}"
    ));
    panic!("session was not closed with {expected} in time, last: {last}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_ping_over_tls() {
    let fixture = boot().await;
    let token = fixture
        .issue_ws("ws-ping", TokenMode::ReadWrite, 3600)
        .await;
    let client = open(&fixture, &token).await;
    assert_eq!(client.auth_kind(), AuthKind::Token);
    client.ping().await.expect("ping");
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_execute_writes_and_reads() {
    let fixture = boot().await;
    let token = fixture.issue_ws("ws-rw", TokenMode::ReadWrite, 3600).await;
    let client = open(&fixture, &token).await;
    assert_eq!(client.auth_kind(), AuthKind::Token);
    client
        .execute("default", seed_batch())
        .await
        .expect("insert row");
    let resp = client
        .execute("default", read_batch())
        .await
        .expect("read items");
    let rd = resp.results.get("rd").expect("rd alias");
    assert_eq!(rd.records.len(), 1);
    assert_eq!(rd.records[0].get_value_str("marker"), Some("token-rest"));
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_read_only_write_is_denied() {
    let fixture = boot().await;
    let token = fixture.issue_ws("ws-ro", TokenMode::ReadOnly, 3600).await;
    let client = open(&fixture, &token).await;
    let err = client
        .execute("default", write_batch())
        .await
        .expect_err("read-only write must fail");
    let ClientError::Db { code, .. } = err else {
        panic!("expected Db error, got {err:?}");
    };
    assert_eq!(code, "token_read_only");
    client.execute("default", read_batch()).await.expect("read");
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_rest_only_token_is_channel_denied() {
    let fixture = boot().await;
    let Err(err) = connect_ws(fixture.tls_endpoint(), &fixture.read_write).await else {
        panic!("rest-only token must not open a websocket");
    };
    let ClientError::Db { code, .. } = err else {
        panic!("expected Db error, got {err:?}");
    };
    assert_eq!(code, "token_channel_denied");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_unknown_secret_is_unauthorized() {
    let fixture = boot().await;
    let Err(err) = connect_ws(fixture.tls_endpoint(), &fixture.unknown_secret_token()).await else {
        panic!("unknown token must not open a websocket");
    };
    let ClientError::Http { status, .. } = err else {
        panic!("expected Http error, got {err:?}");
    };
    assert_eq!(status, 401);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_revoked_token_closes_with_4011() {
    let fixture = boot().await;
    let token = fixture
        .issue_ws("ws-revoke", TokenMode::ReadWrite, 3600)
        .await;
    let client = open(&fixture, &token).await;
    client.ping().await.expect("ping before revoke");
    fixture.revoke_token(&token).await;
    wait_peer_closed(&client, 4011).await;
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_expired_token_closes_with_4012() {
    let guard = FlakeGuard::new("token_ws_expired_token_closes_with_4012");
    // Flake #490: the client saw a RST instead of the 4012 close code. The
    // client and the in-process server both emit `tracing` — capture both
    // into the report ring so the close-handshake decisions are in the
    // failure output. Graceful no-op if a dispatch is already installed.
    try_install_global_tracing_ring();
    let fixture = boot().await;
    guard.mark("server fixture booted");
    let token = fixture.issue_ws("ws-expire", TokenMode::ReadWrite, 4).await;
    guard.mark("ws token issued (ttl 4s); expiry window open");
    let client = open(&fixture, &token).await;
    guard.mark("ws connected");
    wait_peer_closed_with_marks(&client, 4012, |event| guard.mark(event)).await;
    guard.mark("peer close observed with code 4012");
    client.close().await;
    guard.mark("client closed");
    fixture.shutdown().await;
    guard.mark("fixture shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_refuses_admin_requests_locally() {
    let fixture = boot().await;
    let token = fixture
        .issue_ws("ws-admin", TokenMode::ReadWrite, 3600)
        .await;
    let client = open(&fixture, &token).await;

    let err = client
        .create_scram_user("mallory", Zeroizing::new("pw".to_string()), vec![])
        .await
        .expect_err("create_scram_user must be refused");
    let ClientError::Db { code, .. } = err else {
        panic!("expected Db error, got {err:?}");
    };
    assert_eq!(code, "token_request_denied");

    let err = client
        .list_api_tokens(None)
        .await
        .expect_err("list_api_tokens must be refused");
    let ClientError::Db { code, .. } = err else {
        panic!("expected Db error, got {err:?}");
    };
    assert_eq!(code, "token_request_denied");

    client.ping().await.expect("session still usable");
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_ws_plain_loopback_serves_ping() {
    let fixture = boot().await;
    let token = fixture
        .issue_ws("ws-plain", TokenMode::ReadWrite, 3600)
        .await;
    let Ok(client) = connect_ws(fixture.plain_endpoint(), &token).await else {
        panic!("plain loopback token websocket connect failed");
    };
    client.ping().await.expect("ping");
    client.close().await;
    fixture.shutdown().await;
}
