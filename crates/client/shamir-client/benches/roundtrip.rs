//! Client envelope encode-path benchmarks for `shamir-client`.
//!
//! Run: `cargo bench -p shamir-client --bench roundtrip`
//!
//! Wire format (TRANSPORT_TCP §2): `[u32_be length][msgpack: length bytes]`.
//!
//! Compares the two client request-encode paths over a real `duplex` pipe
//! (the write must be actual I/O or the optimizer could drop it):
//!
//! - `client/envelope/preframed/<size>` — **new path**: serialize
//!   `RequestEnvelopeRef` (borrowed body) with
//!   `rmp_serde::encode::write_named` directly into a scratch `Vec<u8>`
//!   whose first 4 bytes are a placeholder, patch the BE length prefix in
//!   place, then `write_frame_prereserved` (no payload memcpy).
//! - `client/envelope/to_msgpack_baseline/<size>` — **old path**:
//!   `RequestEnvelopeRef::to_msgpack()` allocates its own buffer, then
//!   `write_frame` copies length + payload into the socket.
//!
//! Payload bodies are msgpack encodings of a public `DbRequest` variant
//! whose byte-array field makes the size directly controllable:
//! `DbRequest::ChangePasswordChallenge { client_nonce_cp }` with a nonce
//! of exactly `size` bytes.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`, `async`
//! feature): a fresh `duplex` pair is required every iteration (a shared
//! (a shared pipe would accumulate unread bytes / EOF state across iterations), so
//! every variant uses `bench_batched_async` — body build and scratch
//! allocation happen in the untimed `setup`, the encode + frame write is
//! the timed `routine`.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_connect::common::envelope::RequestEnvelopeRef;
use shamir_query_types::wire::DbRequest;
use shamir_transport_tcp::framing::{write_frame, write_frame_prereserved};
use tokio::io::duplex;

/// Dummy bearer session id (all zeros — length is what matters on the wire).
const SESSION_ID: [u8; 32] = [0u8; 32];

/// Sizes spanning the typical request-payload range: small point-reads up
/// to large batch inserts.
const SIZES: [usize; 3] = [64, 1024, 64 * 1024];

/// Build a `DbRequest` whose msgpack encoding is roughly `size` bytes:
/// `ChangePasswordChallenge` carries one byte-array field
/// (`client_nonce_cp`), so the encoded body tracks the nonce length.
fn make_body(size: usize) -> Vec<u8> {
    let req = DbRequest::ChangePasswordChallenge {
        client_nonce_cp: vec![0xaau8; size],
    };
    rmp_serde::to_vec_named(&req).expect("DbRequest encodes")
}

fn main() {
    let mut h = Harness::new("roundtrip", env!("CARGO_MANIFEST_DIR"));

    // --- client/envelope/preframed/<size> -----------------------------------
    // Prereserved path: the scratch buffer is allocated (untimed) in setup
    // with room for the 4-byte prefix; the timed routine clears + fills it,
    // patches the prefix, and writes it with no payload memcpy.
    for size in SIZES {
        let body = make_body(size);
        let id = format!("client/envelope/preframed/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let body = body.clone();
                async move { (body, Vec::<u8>::with_capacity(4 + size + 64)) }
            },
            move |(body, mut buf)| async move {
                let buf_cap = size + 1024;
                let (mut w, r) = duplex(buf_cap);
                buf.clear();
                buf.extend_from_slice(&[0; 4]); // placeholder length prefix
                rmp_serde::encode::write_named(
                    &mut buf,
                    &RequestEnvelopeRef {
                        session_id: &SESSION_ID,
                        request_id: Some(1),
                        req: &body,
                    },
                )
                .expect("envelope encodes into Vec");
                let inner = (buf.len() - 4) as u32;
                buf[..4].copy_from_slice(&inner.to_be_bytes());
                write_frame_prereserved(&mut w, &buf).await.unwrap();
                black_box(&buf);
                black_box(&r);
            },
        );
    }

    // --- client/envelope/to_msgpack_baseline/<size> -------------------------
    // OLD path baseline: `to_msgpack()` allocates its own output buffer,
    // then `write_frame` prepends the length prefix (an extra copy of the
    // whole payload into the writer's scratch). CI compares this against
    // the prereserved path above to quantify the improvement.
    for size in SIZES {
        let body = make_body(size);
        let id = format!("client/envelope/to_msgpack_baseline/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let body = body.clone();
                async move { body }
            },
            move |body| async move {
                let buf_cap = size + 1024;
                let (mut w, r) = duplex(buf_cap);
                let envelope = RequestEnvelopeRef {
                    session_id: &SESSION_ID,
                    request_id: Some(1),
                    req: &body,
                };
                let bytes = envelope.to_msgpack().expect("envelope encodes");
                write_frame(&mut w, &bytes).await.unwrap();
                black_box(bytes);
                black_box(&r);
            },
        );
    }

    h.run();
}
