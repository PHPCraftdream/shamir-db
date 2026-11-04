//! Password login over the HTTP listener, then use of the issued token.

use shamir_client::{
    login, ApiToken, Client, ClientError, LoginChannels, LoginMode, LoginOptions, LoginScope,
    TokenEndpoint, TokenHttpClient, TokenHttpOptions, TokenWsOptions,
};

use zeroize::Zeroizing;

use super::common::{
    boot_login, TokenFixture, ADMIN_PASSWORD_TEXT, LOGIN_LISTENER_ID, LOGIN_PASSWORD, LOGIN_USER,
};

async fn expect_login_error(options: LoginOptions) -> ClientError {
    match login(options).await {
        Err(err) => err,
        Ok(outcome) => panic!("login unexpectedly succeeded for owner {}", outcome.owner),
    }
}

async fn ping_rest(endpoint: TokenEndpoint, token: ApiToken) {
    let client = TokenHttpClient::connect(TokenHttpOptions::new(endpoint, token))
        .await
        .expect("token client connect");
    client.ping().await.expect("ping over REST");
    client.close().await;
}

fn assert_http_error(err: ClientError, expected_status: u16, expected_code: &str) {
    let ClientError::Http { status, code, .. } = err else {
        panic!("expected Http error, got {err:?}");
    };
    assert_eq!(status, expected_status);
    assert_eq!(code.as_deref(), Some(expected_code));
}

async fn finish(fixture: TokenFixture) {
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_over_tls_returns_token_usable_for_rest_and_ws() {
    let fixture = boot_login(false).await;

    let outcome = login(fixture.login_options_tls(LOGIN_USER, LOGIN_PASSWORD))
        .await
        .expect("login over TLS");
    let secret = outcome.token.reveal().to_string();
    let shown = format!("{outcome:?}");
    assert!(!shown.contains(&secret));
    let default_scope = LoginScope::default();
    assert_eq!(outcome.scope.label, default_scope.label);
    assert!(matches!(outcome.scope.mode, LoginMode::ReadOnly));
    assert!(matches!(outcome.scope.channels, LoginChannels::Rest));
    assert!(outcome.scope.databases.is_none());
    assert_eq!(outcome.owner, LOGIN_USER);
    assert!(outcome.expires_at_ns > outcome.created_at_ns);
    assert!(!outcome.token.id().is_empty());
    ping_rest(fixture.tls_endpoint(), outcome.token).await;

    let scope = LoginScope::default()
        .with_channels(LoginChannels::Both)
        .with_label("login-both");
    let options = fixture
        .login_options_tls(LOGIN_USER, LOGIN_PASSWORD)
        .with_scope(scope);
    let both = login(options).await.expect("login with both channels");
    assert!(matches!(both.scope.channels, LoginChannels::Both));
    assert_eq!(both.scope.label, "login-both");
    let both_secret = both.token.reveal().to_string();
    assert!(!format!("{both:?}").contains(&both_secret));
    let client = Client::connect_token_ws(TokenWsOptions::new(fixture.tls_endpoint(), both.token))
        .await
        .unwrap_or_else(|e| panic!("login token websocket connect failed: {e:?}"));
    client.ping().await.expect("ping over websocket");
    client.close().await;

    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_wrong_password_and_unknown_user_are_both_401() {
    let fixture = boot_login(false).await;

    let err = expect_login_error(fixture.login_options_tls(LOGIN_USER, "definitely-wrong")).await;
    assert!(!format!("{err} {err:?}").contains("definitely-wrong"));
    assert_http_error(err, 401, "authentication_failed");

    let err = expect_login_error(fixture.login_options_tls("no-such-user", LOGIN_PASSWORD)).await;
    assert_http_error(err, 401, "authentication_failed");

    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_read_write_refused_unless_server_allows() {
    let fixture = boot_login(false).await;
    let options = fixture
        .login_options_tls(LOGIN_USER, LOGIN_PASSWORD)
        .with_scope(LoginScope::default().with_mode(LoginMode::ReadWrite));
    let err = expect_login_error(options).await;
    assert_http_error(err, 403, "login_write_forbidden");
    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_read_write_allowed_when_flag_set() {
    let fixture = boot_login(true).await;
    let options = fixture
        .login_options_tls(LOGIN_USER, LOGIN_PASSWORD)
        .with_scope(LoginScope::default().with_mode(LoginMode::ReadWrite));
    let outcome = login(options).await.expect("read-write login");
    assert!(matches!(outcome.scope.mode, LoginMode::ReadWrite));
    ping_rest(fixture.tls_endpoint(), outcome.token).await;
    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_listener_id_pin_match_and_mismatch() {
    let fixture = boot_login(false).await;

    let options = fixture
        .login_options_tls(LOGIN_USER, LOGIN_PASSWORD)
        .with_listener_id(Some(LOGIN_LISTENER_ID));
    login(options).await.expect("matching listener pin");

    let options = fixture
        .login_options_tls(LOGIN_USER, LOGIN_PASSWORD)
        .with_listener_id(Some([0xAB; 16]));
    let err = expect_login_error(options).await;
    let ClientError::Handshake { context, .. } = err else {
        panic!("expected Handshake error, got {err:?}");
    };
    assert_eq!(context, "login: listener id mismatch");

    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_over_plain_loopback_works() {
    let fixture = boot_login(false).await;
    let outcome = login(fixture.login_options_plain(LOGIN_USER, LOGIN_PASSWORD))
        .await
        .expect("login over plain loopback");
    assert_eq!(outcome.owner, LOGIN_USER);
    ping_rest(fixture.plain_endpoint(), outcome.token).await;
    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_untrusted_tls_certificate_is_refused() {
    let fixture = boot_login(false).await;
    let options = LoginOptions::new(
        fixture.untrusted_endpoint(),
        LOGIN_USER,
        Zeroizing::new(LOGIN_PASSWORD.to_string()),
    );
    let err = expect_login_error(options).await;
    assert!(!format!("{err} {err:?}").contains(LOGIN_PASSWORD));
    assert!(
        matches!(err, ClientError::Tls { .. }),
        "expected a TLS refusal, got {err:?}"
    );
    finish(fixture).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_superuser_is_denied_after_proof() {
    let fixture = boot_login(false).await;
    let options = fixture.login_options_tls("admin", ADMIN_PASSWORD_TEXT);
    let err = expect_login_error(options).await;
    assert_http_error(err, 403, "login_account_denied");
    finish(fixture).await;
}
