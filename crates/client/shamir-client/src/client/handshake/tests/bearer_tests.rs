//! Token WebSocket binding frame decoding and `read_binding`.

use std::time::Duration;

use futures::SinkExt;
use serde::Serialize;
use tokio::io::{duplex, DuplexStream};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::client::handshake::token::{decode_binding, read_binding};
use crate::error::{ClientError, CloseCause};

#[derive(Serialize)]
struct Frame<'a> {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: &'a [u8],
    expires_at_ns: u64,
}

#[derive(Serialize)]
struct FrameExtra<'a> {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: &'a [u8],
    expires_at_ns: u64,
    extra: u8,
}

#[derive(Serialize)]
struct FrameMissing<'a> {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: &'a [u8],
}

#[derive(Serialize)]
struct FrameStrSid {
    ok: bool,
    sid: String,
    expires_at_ns: u64,
}

#[derive(Serialize)]
struct FrameArraySid {
    ok: bool,
    sid: Vec<u8>,
    expires_at_ns: u64,
}

const STEP: Duration = Duration::from_secs(5);
const SID: [u8; 32] = [7u8; 32];
const EXPIRY: u64 = 1_900_000_000_000_000_000;

fn frame(ok: bool, sid: &[u8], expires_at_ns: u64) -> Vec<u8> {
    rmp_serde::to_vec_named(&Frame {
        ok,
        sid,
        expires_at_ns,
    })
    .unwrap()
}

fn valid() -> Vec<u8> {
    frame(true, &SID, EXPIRY)
}

fn assert_invalid(buf: &[u8]) {
    match decode_binding(buf) {
        Err(ClientError::Protocol(msg)) => assert_eq!(msg, "invalid token binding frame"),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

#[test]
fn valid_frame_decodes() {
    let binding = decode_binding(&valid()).unwrap();
    assert_eq!(binding.sid, SID);
    assert_eq!(binding.expires_at_ns, EXPIRY);
}

#[test]
fn extra_key_is_rejected() {
    let buf = rmp_serde::to_vec_named(&FrameExtra {
        ok: true,
        sid: &SID,
        expires_at_ns: EXPIRY,
        extra: 1,
    })
    .unwrap();
    assert_invalid(&buf);
}

#[test]
fn missing_key_is_rejected() {
    let buf = rmp_serde::to_vec_named(&FrameMissing {
        ok: true,
        sid: &SID,
    })
    .unwrap();
    assert_invalid(&buf);
}

#[test]
fn ok_false_is_rejected() {
    assert_invalid(&frame(false, &SID, EXPIRY));
}

#[test]
fn short_and_long_sid_are_rejected() {
    assert_invalid(&frame(true, &[7u8; 31], EXPIRY));
    assert_invalid(&frame(true, &[7u8; 33], EXPIRY));
}

#[test]
fn all_zero_sid_is_rejected() {
    assert_invalid(&frame(true, &[0u8; 32], EXPIRY));
}

#[test]
fn zero_expiry_is_rejected() {
    assert_invalid(&frame(true, &SID, 0));
}

#[test]
fn trailing_byte_is_rejected() {
    let mut buf = valid();
    buf.push(0);
    assert_invalid(&buf);
}

#[test]
fn sid_as_str_is_rejected() {
    let buf = rmp_serde::to_vec_named(&FrameStrSid {
        ok: true,
        sid: "x".repeat(32),
        expires_at_ns: EXPIRY,
    })
    .unwrap();
    assert_invalid(&buf);
}

#[test]
fn sid_as_array_is_rejected() {
    let buf = rmp_serde::to_vec_named(&FrameArraySid {
        ok: true,
        sid: vec![7u8; 32],
        expires_at_ns: EXPIRY,
    })
    .unwrap();
    assert_invalid(&buf);
}

#[test]
fn array_instead_of_map_is_rejected() {
    let buf = rmp_serde::to_vec(&(true, serde_bytes::Bytes::new(&SID), EXPIRY)).unwrap();
    assert_invalid(&buf);
}

#[test]
fn empty_and_garbage_are_rejected() {
    assert_invalid(&[]);
    assert_invalid(&[0xc1, 0xff, 0x00]);
}

async fn pair() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (client_io, server_io) = duplex(64 * 1024);
    let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    (client, server)
}

#[tokio::test]
async fn read_binding_accepts_binary_frame() {
    let (mut client, mut server) = pair().await;
    server.send(Message::Binary(valid().into())).await.unwrap();
    let binding = tokio::time::timeout(STEP, read_binding(&mut client))
        .await
        .expect("binding in time")
        .unwrap();
    assert_eq!(binding.sid, SID);
    assert_eq!(binding.expires_at_ns, EXPIRY);
}

#[tokio::test]
async fn read_binding_skips_ping_before_frame() {
    let (mut client, mut server) = pair().await;
    server
        .send(Message::Ping(vec![1u8, 2].into()))
        .await
        .unwrap();
    server.send(Message::Binary(valid().into())).await.unwrap();
    let binding = tokio::time::timeout(STEP, read_binding(&mut client))
        .await
        .expect("binding in time")
        .unwrap();
    assert_eq!(binding.expires_at_ns, EXPIRY);
}

#[tokio::test]
async fn read_binding_maps_peer_close() {
    let (mut client, mut server) = pair().await;
    server
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(4011),
            reason: "token_revoked".into(),
        })))
        .await
        .unwrap();
    match tokio::time::timeout(STEP, read_binding(&mut client))
        .await
        .expect("binding in time")
    {
        Err(ClientError::ConnectionClosed(CloseCause::PeerClosed { code, .. })) => {
            assert_eq!(code, 4011);
        }
        other => panic!("expected PeerClosed, got {other:?}"),
    }
}

#[tokio::test]
async fn read_binding_rejects_text() {
    let (mut client, mut server) = pair().await;
    server.send(Message::Text("hello".into())).await.unwrap();
    match tokio::time::timeout(STEP, read_binding(&mut client))
        .await
        .expect("binding in time")
    {
        Err(ClientError::Protocol(_)) => {}
        other => panic!("expected Protocol error, got {other:?}"),
    }
}
