//! Password login flow against a fake plaintext loopback server.

use std::time::Duration;

use shamir_connect::server::token::TokenId;
use zeroize::Zeroizing;

use super::login_fake::{
    BeginBody, Behavior, FakeLogin, FinishMode, RawReply, BEGIN_PATH, FINISH_PATH,
};
use crate::client::handshake::token::encode_begin;
use crate::client::transport::token::TokenEndpoint;
use crate::client::{login, LoginChannels, LoginMode, LoginOptions, LoginScope};
use crate::error::ClientError;

const PASSWORD: &str = "correct horse";
const HEADER_NAMES: [&str; 4] = ["accept", "content-length", "content-type", "host"];

fn options(fake: &FakeLogin) -> LoginOptions {
    LoginOptions::new(
        fake.endpoint(),
        "Alice",
        Zeroizing::new(PASSWORD.to_owned()),
    )
}

fn handshake_context(err: ClientError) -> String {
    match err {
        ClientError::Handshake { context, .. } => context,
        other => panic!("expected Handshake error, got {other:?}"),
    }
}

#[test]
fn login_future_is_send() {
    fn require_send<T: Send>(_: &T) {}
    let endpoint = TokenEndpoint::plain_loopback("127.0.0.1:9".parse().unwrap()).unwrap();
    require_send(&login(LoginOptions::new(
        endpoint,
        "alice",
        Zeroizing::new(String::new()),
    )));
}

fn begin_of(fake: &FakeLogin) -> BeginBody {
    let requests = fake.requests();
    rmp_serde::from_slice(&requests[0].body).unwrap()
}

#[tokio::test]
async fn honest_login_succeeds() {
    let fake = FakeLogin::start(Behavior::default()).await;
    let outcome = login(options(&fake)).await.unwrap();
    assert_eq!(outcome.token.id(), TokenId(outcome.id).to_b64());
    assert_eq!(outcome.id, [9u8; 16]);
    assert_eq!(outcome.owner, "alice");
    assert_eq!(outcome.scope, LoginScope::default());
    assert_eq!(outcome.created_at_ns, 1_000_000_000);
    assert_eq!(outcome.expires_at_ns, 2_000_000_000);

    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(fake.accepts(), 2);
    assert_eq!(requests[0].path, BEGIN_PATH);
    assert_eq!(requests[1].path, FINISH_PATH);
    for request in &requests {
        assert_eq!(request.header_names, HEADER_NAMES);
    }

    let begin = begin_of(&fake);
    assert_eq!(begin.user, "alice");
    assert_eq!(begin.binding_mode, 3);
    assert_eq!(begin.protocol_version, 1);
    assert_eq!(begin.client_nonce.len(), 32);
    assert!(begin.client_nonce.iter().any(|byte| *byte != 0));
    assert_eq!(begin.token.label, "http-login");
    assert_eq!(begin.token.mode, "read_only");
    assert_eq!(begin.token.channels, "rest");
    assert_eq!(begin.token.databases, None);
    let nonce: [u8; 32] = begin.client_nonce.as_slice().try_into().unwrap();
    let expected = encode_begin("alice", &nonce, &LoginScope::default()).unwrap();
    assert_eq!(requests[0].body, expected);
}

#[tokio::test]
async fn databases_are_sorted_and_deduped() {
    let fake = FakeLogin::start(Behavior::default()).await;
    let scope = LoginScope::default()
        .with_mode(LoginMode::ReadWrite)
        .with_channels(LoginChannels::Both)
        .with_databases(Some(vec![
            "zeta".to_owned(),
            "alpha".to_owned(),
            "alpha".to_owned(),
        ]));
    let outcome = login(options(&fake).with_scope(scope)).await.unwrap();
    let begin = begin_of(&fake);
    assert_eq!(
        begin.token.databases,
        Some(vec!["alpha".to_owned(), "zeta".to_owned()])
    );
    assert_eq!(begin.token.mode, "read_write");
    assert_eq!(begin.token.channels, "both");
    assert_eq!(
        outcome.scope.databases,
        Some(vec!["alpha".to_owned(), "zeta".to_owned()])
    );
    assert_eq!(outcome.scope.mode, LoginMode::ReadWrite);
    assert_eq!(outcome.scope.channels, LoginChannels::Both);
}

#[tokio::test]
async fn security_class_mismatch_is_rejected_before_finish() {
    let fake = FakeLogin::start(Behavior {
        security: 1,
        ..Behavior::default()
    })
    .await;
    let err = login(options(&fake)).await.unwrap_err();
    assert_eq!(handshake_context(err), "login: security class mismatch");
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn listener_pin_mismatch_is_rejected() {
    let fake = FakeLogin::start(Behavior::default()).await;
    let err = login(options(&fake).with_listener_id(Some([0x22; 16])))
        .await
        .unwrap_err();
    assert_eq!(handshake_context(err), "login: listener id mismatch");
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn listener_pin_match_is_accepted() {
    let fake = FakeLogin::start(Behavior::default()).await;
    login(options(&fake).with_listener_id(Some([0x11; 16])))
        .await
        .unwrap();
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn kdf_out_of_bounds_is_rejected_before_finish() {
    let fake = FakeLogin::start(Behavior {
        kdf: (1_024, 2, 1),
        ..Behavior::default()
    })
    .await;
    let err = login(options(&fake)).await.unwrap_err();
    assert!(handshake_context(err).starts_with("login: challenge rejected"));
    assert_eq!(fake.requests().len(), 1);
}

async fn finish_failure(finish: FinishMode) -> ClientError {
    let fake = FakeLogin::start(Behavior {
        finish,
        ..Behavior::default()
    })
    .await;
    let err = login(options(&fake)).await.unwrap_err();
    assert_eq!(fake.requests().len(), 2);
    err
}

#[tokio::test]
async fn bad_server_signature_is_rejected_and_redacted() {
    let err = finish_failure(FinishMode::BadSignature).await;
    let display = err.to_string();
    let debug = format!("{err:?}");
    assert!(!display.contains("shk1."));
    assert!(!debug.contains("shk1."));
    assert!(!display.contains(PASSWORD));
    assert!(!debug.contains(PASSWORD));
    assert_eq!(handshake_context(err), "login: server signature mismatch");
}

#[tokio::test]
async fn wrong_token_id_is_rejected() {
    let err = finish_failure(FinishMode::WrongTokenId).await;
    assert!(handshake_context(err).starts_with("login: token id mismatch (issued token "));
}

#[tokio::test]
async fn changed_scope_is_rejected() {
    let err = finish_failure(FinishMode::ScopeLabelChanged).await;
    assert!(handshake_context(err).starts_with("login: scope mismatch (issued token "));
}

#[tokio::test]
async fn invalid_expiry_is_rejected() {
    let err = finish_failure(FinishMode::BadExpiry).await;
    assert!(handshake_context(err).starts_with("login: invalid token expiry (issued token "));
}

#[tokio::test]
async fn finish_401_maps_to_http_error() {
    let reply = RawReply::error(401, &[], "authentication_failed", None);
    let err = finish_failure(FinishMode::Status(reply)).await;
    assert!(matches!(
        err,
        ClientError::Http {
            status: 401,
            code: Some(ref code),
            retry_after: None,
        } if code == "authentication_failed"
    ));
}

#[tokio::test]
async fn wrong_password_maps_to_http_401() {
    let fake = FakeLogin::start(Behavior {
        password: "another password",
        ..Behavior::default()
    })
    .await;
    let err = login(options(&fake)).await.unwrap_err();
    assert!(matches!(
        err,
        ClientError::Http {
            status: 401,
            code: Some(ref code),
            retry_after: None,
        } if code == "authentication_failed"
    ));
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn begin_429_carries_retry_after() {
    let reply = RawReply::error(429, &[("retry-after", "7")], "rate_limited", Some(7));
    let fake = FakeLogin::start(Behavior {
        begin_status: Some(reply),
        ..Behavior::default()
    })
    .await;
    let err = login(options(&fake)).await.unwrap_err();
    assert!(matches!(
        err,
        ClientError::Http {
            status: 429,
            code: Some(ref code),
            retry_after: Some(delay),
        } if code == "rate_limited" && delay == Duration::from_secs(7)
    ));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn debug_output_redacts_secrets() {
    let fake = FakeLogin::start(Behavior::default()).await;
    let opts = LoginOptions::new(
        fake.endpoint(),
        "Alice",
        Zeroizing::new("s3cret-pw".to_owned()),
    );
    let debug = format!("{opts:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("s3cret-pw"));

    let outcome = login(options(&fake)).await.unwrap();
    let debug = format!("{outcome:?}");
    assert!(!debug.contains(outcome.token.reveal()));
}

#[tokio::test]
async fn invalid_username_sends_nothing() {
    let fake = FakeLogin::start(Behavior::default()).await;
    let opts = LoginOptions::new(fake.endpoint(), "", Zeroizing::new(PASSWORD.to_owned()));
    let err = login(opts).await.unwrap_err();
    assert!(matches!(err, ClientError::InvalidUsername(_)));
    assert!(fake.requests().is_empty());
    assert_eq!(fake.accepts(), 0);
}
