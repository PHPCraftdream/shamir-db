use std::time::Duration;

use bytes::Bytes;
use shamir_query_types::wire::DbResponse;

use crate::client::transport::token::status::parse_retry_after;
use crate::client::transport::token::{map_ping_reply, map_reply, TokenReply};
use crate::error::ClientError;

fn reply(status: u16, body: Vec<u8>) -> TokenReply {
    TokenReply {
        status,
        content_type_msgpack: true,
        has_content_encoding: false,
        retry_after: None,
        body: Bytes::from(body),
    }
}

fn encode(response: &DbResponse) -> Vec<u8> {
    rmp_serde::encode::to_vec_named(response).unwrap()
}

fn error_body() -> Vec<u8> {
    encode(&DbResponse::Error {
        code: "token_read_only".into(),
        message: "token is read-only".into(),
    })
}

fn assert_http(err: ClientError, status: u16, code: Option<&str>, retry: Option<Duration>) {
    match err {
        ClientError::Http {
            status: got_status,
            code: got_code,
            retry_after,
        } => {
            assert_eq!(got_status, status);
            assert_eq!(got_code.as_deref(), code);
            assert_eq!(retry_after, retry);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn ok_200_decodes_pong() {
    let mapped = map_reply(reply(200, encode(&DbResponse::Pong))).unwrap();
    assert!(matches!(mapped, DbResponse::Pong));
}

#[test]
fn ok_200_with_error_body_stays_unmapped() {
    let mapped = map_reply(reply(200, error_body())).unwrap();
    assert!(matches!(mapped, DbResponse::Error { .. }));
}

#[test]
fn ok_200_with_wrong_content_type_is_protocol_error() {
    let mut r = reply(200, encode(&DbResponse::Pong));
    r.content_type_msgpack = false;
    match map_reply(r).unwrap_err() {
        ClientError::Protocol(message) => assert_eq!(message, "unexpected response content type"),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn ok_200_with_content_encoding_is_protocol_error() {
    let mut r = reply(200, encode(&DbResponse::Pong));
    r.has_content_encoding = true;
    assert!(matches!(
        map_reply(r).unwrap_err(),
        ClientError::Protocol(_)
    ));
}

#[test]
fn ok_200_with_garbage_body_is_decode_error() {
    let err = map_reply(reply(200, vec![0xc1, 0xff, 0x00])).unwrap_err();
    assert!(matches!(err, ClientError::Decode(_)));
}

#[test]
fn bad_request_and_forbidden_with_error_body_map_to_db() {
    for status in [400, 403] {
        match map_reply(reply(status, error_body())).unwrap_err() {
            ClientError::Db { code, message } => {
                assert_eq!(code, "token_read_only");
                assert_eq!(message, "token is read-only");
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}

#[test]
fn bad_request_with_garbage_body_is_plain_http() {
    let err = map_reply(reply(400, b"<html>secret detail</html>".to_vec())).unwrap_err();
    assert_http(err, 400, None, None);
}

#[test]
fn forbidden_with_non_error_response_is_plain_http() {
    let err = map_reply(reply(403, encode(&DbResponse::Pong))).unwrap_err();
    assert_http(err, 403, None, None);
}

#[test]
fn unauthorized_carries_fixed_code() {
    let err = map_reply(reply(401, error_body())).unwrap_err();
    assert_http(err, 401, Some("unauthorized"), None);
}

#[test]
fn other_statuses_are_plain_http() {
    for status in [404, 405, 408, 413, 415, 431, 307, 301, 500, 502] {
        let err = map_reply(reply(status, error_body())).unwrap_err();
        assert_http(err, status, None, None);
    }
}

#[test]
fn rate_limit_and_unavailable_keep_retry_after() {
    for status in [429, 503] {
        let mut r = reply(status, Vec::new());
        r.retry_after = Some(Duration::from_secs(7));
        let err = map_reply(r).unwrap_err();
        assert_http(err, status, None, Some(Duration::from_secs(7)));
    }
}

#[test]
fn retry_after_is_dropped_for_other_statuses() {
    let mut r = reply(404, Vec::new());
    r.retry_after = Some(Duration::from_secs(7));
    assert_http(map_reply(r).unwrap_err(), 404, None, None);
}

#[test]
fn http_error_display_never_contains_body() {
    let err = map_reply(reply(400, b"top secret body".to_vec())).unwrap_err();
    assert_eq!(err.to_string(), "http: status 400");
    let err = map_reply(reply(401, b"top secret body".to_vec())).unwrap_err();
    assert_eq!(err.to_string(), "http: status 401 (code unauthorized)");
}

#[test]
fn ping_200_is_ok_regardless_of_body() {
    assert!(map_ping_reply(reply(200, Vec::new())).is_ok());
    assert!(map_ping_reply(reply(200, encode(&DbResponse::Pong))).is_ok());
}

#[test]
fn ping_failures_follow_the_table() {
    assert_http(
        map_ping_reply(reply(401, Vec::new())).unwrap_err(),
        401,
        Some("unauthorized"),
        None,
    );
    assert_http(
        map_ping_reply(reply(404, Vec::new())).unwrap_err(),
        404,
        None,
        None,
    );
    let mut limited = reply(429, Vec::new());
    limited.retry_after = Some(Duration::from_secs(3));
    assert_http(
        map_ping_reply(limited).unwrap_err(),
        429,
        None,
        Some(Duration::from_secs(3)),
    );
    match map_ping_reply(reply(403, error_body())).unwrap_err() {
        ClientError::Db { code, .. } => assert_eq!(code, "token_read_only"),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn retry_after_parsing() {
    assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
    assert_eq!(parse_retry_after("0"), Some(Duration::from_secs(1)));
    assert_eq!(parse_retry_after("99999"), Some(Duration::from_secs(3600)));
    assert_eq!(
        parse_retry_after("99999999999999999999999"),
        Some(Duration::from_secs(3600))
    );
    assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    assert_eq!(parse_retry_after(""), None);
    assert_eq!(parse_retry_after("-5"), None);
    assert_eq!(parse_retry_after("1.5"), None);
    assert_eq!(parse_retry_after("+5"), None);
}
