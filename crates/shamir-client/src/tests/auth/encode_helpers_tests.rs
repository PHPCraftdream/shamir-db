//! Card RV-CLIENT-35A189A9 regression tests for the typed encode helper and
//! a wire-identity check for the preframed scratch layout (card
//! RV-TRANSPORT-TCP-79910BA5).

use shamir_connect::common::envelope::{RequestEnvelopeRef, ResponseEnvelope};
use shamir_transport_tcp::framing::write_frame;
use tokio::io::{duplex, AsyncReadExt, AsyncWrite};

use crate::client::encode_named_into;
use crate::error::ClientError;
use crate::{Client, DbRequest, DbResponse};

/// `Serialize` impl whose serializer call always fails with a custom error —
/// the input for the typed-source regression test.
struct FailingSerialize;

impl serde::Serialize for FailingSerialize {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("boom"))
    }
}

/// Card RV-CLIENT-35A189A9: an envelope-encoding failure must surface as the
/// source-bearing `ClientError::Encode` (wrapping the typed
/// `rmp_serde::encode::Error`), NOT as the stringly
/// `ClientError::Protocol(format!("envelope encode: {e}"))` the old
/// `RequestEnvelopeRef::to_msgpack` path produced — which erased the source
/// chain entirely (`source()` was `None`).
#[test]
fn rv_client_35a189a9_envelope_encode_error_keeps_typed_source() {
    let mut buf = Vec::new();
    let err = encode_named_into(&mut buf, &FailingSerialize).expect_err("serialize impl fails");
    assert!(matches!(err, ClientError::Encode(_)));
    assert!(
        std::error::Error::source(&err).is_some(),
        "Encode must carry the typed rmp_serde source"
    );
}

/// Wire-format compatibility: encoding `RequestEnvelopeRef` into the
/// preframed scratch (`4 placeholder bytes` → `encode_named_into` → patch
/// the BE `u32` prefix) must produce byte-identical output to the old path —
/// `to_msgpack()` (which is `to_vec_named` of the same struct) manually
/// prefixed with the same length header.
#[test]
fn rv_client_79910ba5_preframed_scratch_bytes_match_to_msgpack_layout() {
    let session_id = [0x42u8; 32];
    let body = b"hello-preframed-frame".to_vec();
    let envelope = RequestEnvelopeRef {
        session_id: &session_id,
        request_id: Some(7),
        req: &body,
    };

    let mut scratch = vec![0u8; 4];
    encode_named_into(&mut scratch, &envelope).expect("scratch encode");
    let len = ((scratch.len() - 4) as u32).to_be_bytes();
    scratch[0..4].copy_from_slice(&len);

    let payload = envelope.to_msgpack().expect("reference to_msgpack encode");
    let mut expected = (payload.len() as u32).to_be_bytes().to_vec();
    expected.extend_from_slice(&payload);

    assert_eq!(scratch, expected);
}

/// A valid framed Pong answer so the production roundtrip completes.
async fn answer_pong(writer: &mut (impl AsyncWrite + Unpin), rid: u32) {
    let payload = rmp_serde::to_vec_named(&DbResponse::Pong).expect("encode pong");
    let env = ResponseEnvelope::ok(Some(rid), payload);
    let bytes = env.to_msgpack().expect("encode response envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

/// Wire-format identity exercised through PRODUCTION (card
/// RV-TRANSPORT-TCP-79910BA5): a real `Client::for_test` duplex roundtrip of
/// a `Ping`; the peer captures the RAW request frame — the 4-byte BE length
/// prefix `read_exact`-ed into a stack buffer, then the body — and
/// reassembles `prefix ++ body` (framing::read_frame strips the prefix, so it
/// must NOT be used here). It then answers with a valid framed Pong so the
/// roundtrip completes. The raw bytes must equal, byte for byte, the
/// independently computed `RequestEnvelopeRef { session_id, request_id, req:
/// rmp to_vec_named of DbRequest::Ping }` → `to_msgpack()` prefixed with the
/// same BE `u32`. This FAILS on any wire-format regression of the
/// preframed-scratch production path (the previous test only reconstructed
/// the encoding, never exercising production).
#[tokio::test(flavor = "current_thread")]
async fn rv_client_79910ba5_production_roundtrip_bytes_match_envelope_encoding() {
    let (client_write, mut server_read) = duplex(64 * 1024);
    let (client_read, mut server_write) = duplex(64 * 1024);
    let client = Client::for_test(
        client_read,
        client_write,
        crate::client::LifecycleHooks::default(),
    );
    client.set_next_request_id_for_test(7);

    let fut = client.ping();
    let peer = async {
        // RAW frame capture: the length prefix goes into a STACK buffer and
        // the payload into a Vec; reassemble prefix ++ body afterwards.
        let mut prefix = [0u8; 4];
        server_read
            .read_exact(&mut prefix)
            .await
            .expect("frame prefix");
        let len = u32::from_be_bytes(prefix) as usize;
        let mut body = vec![0u8; len];
        server_read.read_exact(&mut body).await.expect("frame body");
        let mut raw = prefix.to_vec();
        raw.extend_from_slice(&body);
        answer_pong(&mut server_write, 7).await;
        raw
    };
    let (res, raw) = tokio::join!(fut, peer);
    res.expect("ping roundtrip ok");

    let req = rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode DbRequest::Ping");
    let envelope = RequestEnvelopeRef {
        session_id: &client.session_id(),
        request_id: Some(7),
        req: &req,
    };
    let payload = envelope.to_msgpack().expect("reference to_msgpack encode");
    let mut expected = (payload.len() as u32).to_be_bytes().to_vec();
    expected.extend_from_slice(&payload);

    assert_eq!(raw, expected, "wire bytes must match the envelope encoding");
}
