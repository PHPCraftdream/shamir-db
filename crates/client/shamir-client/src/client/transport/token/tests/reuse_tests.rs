use std::time::Duration;

use shamir_query_types::wire::DbResponse;
use tokio::net::TcpListener;

use super::fake_server::{auth_value, FakeServer, Scripted};
use crate::client::transport::token::{map_reply, TokenEndpoint, TokenTransport};
use crate::error::ClientError;

const MSGPACK: (&str, &str) = ("content-type", "application/msgpack");

#[tokio::test]
async fn keep_alive_reuses_one_connection() {
    let server = FakeServer::start(vec![Scripted::pong(), Scripted::pong()]).await;
    let transport = server.transport(None, None).await;

    assert_eq!(transport.post_request(vec![1]).await.unwrap().status, 200);
    assert_eq!(transport.post_request(vec![2]).await.unwrap().status, 200);

    assert_eq!(server.accepts(), 1);
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn connection_close_forces_a_fresh_connection() {
    let server = FakeServer::start(vec![Scripted::pong_closing(), Scripted::pong()]).await;
    let transport = server.transport(None, None).await;

    assert_eq!(transport.post_request(vec![1]).await.unwrap().status, 200);
    assert_eq!(transport.post_request(vec![2]).await.unwrap().status, 200);

    assert_eq!(server.accepts(), 2);
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn chunked_response_is_assembled() {
    let full = rmp_serde::encode::to_vec_named(&DbResponse::Pong).unwrap();
    let (first, second) = full.split_at(full.len() / 2);
    let server =
        FakeServer::start(vec![Scripted::chunked(200, &[MSGPACK], &[first, second])]).await;
    let transport = server.transport(None, None).await;

    let reply = transport.post_request(vec![1]).await.unwrap();

    assert_eq!(&reply.body[..], full.as_slice());
    assert!(matches!(map_reply(reply).unwrap(), DbResponse::Pong));
}

#[tokio::test]
async fn redirect_is_not_followed() {
    let redirect = Scripted::fixed(307, &[("location", "http://127.0.0.1:1/elsewhere")], b"");
    let server = FakeServer::start(vec![redirect, Scripted::pong()]).await;
    let transport = server.transport(None, None).await;

    let reply = transport.post_request(vec![1]).await.unwrap();

    assert_eq!(reply.status, 307);
    let err = map_reply(reply).unwrap_err();
    assert!(matches!(
        err,
        ClientError::Http {
            status: 307,
            code: None,
            retry_after: None
        }
    ));
    assert_eq!(server.heads().len(), 1);
    assert_eq!(server.accepts(), 1);
}

#[tokio::test]
async fn oversized_declared_body_is_rejected() {
    let oversized = Scripted::declared(200, &[MSGPACK], 17 * 1024 * 1024);
    let server = FakeServer::start(vec![oversized]).await;
    let transport = server.transport(None, None).await;

    let err = transport.post_request(vec![1]).await.unwrap_err();

    match err {
        ClientError::Protocol(message) => assert_eq!(message, "response body too large"),
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn silent_server_hits_request_timeout() {
    let server = FakeServer::start(vec![Scripted::hang()]).await;
    let limit = Duration::from_millis(150);
    let transport = server.transport(None, Some(limit)).await;

    let err = transport.post_request(vec![1]).await.unwrap_err();

    assert!(matches!(err, ClientError::RequestTimeout(d) if d == limit));
}

#[tokio::test]
async fn refused_connection_fails_to_connect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let endpoint = TokenEndpoint::plain_loopback(addr).unwrap();

    let result =
        TokenTransport::connect(&endpoint, auth_value(), Some(Duration::from_secs(5)), None).await;

    assert!(result.is_err());
}
