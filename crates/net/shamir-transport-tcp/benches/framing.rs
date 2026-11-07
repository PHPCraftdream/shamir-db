//! Per-frame cost benchmarks for `shamir-transport-tcp::framing`.
//!
//! Run: `cargo bench -p shamir-transport-tcp --bench framing`
//!
//! Wire format (TRANSPORT_TCP §2): `[u32_be length][msgpack: length bytes]`.
//!
//! Measures the read+write round-trip via `tokio::io::duplex` for several
//! frame sizes spanning the typical request-payload range up to the
//! 16 MB ceiling (`MAX_FRAME_SIZE_DATA`). We expect this to be I/O-bound at
//! large sizes (memcpy dominates) and length-encoding-bound at small sizes
//! (the 4-byte prefix plus tokio scheduler overhead).
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`, `async`
//! feature): a fresh `duplex` pair is required every iteration (a shared pipe
//! would accumulate unread bytes / EOF state across iterations), so every
//! variant uses `bench_batched_async` — payload setup (a plain `Vec<u8>`
//! clone) happens in the untimed `setup`, and the round trip is the timed
//! `routine`.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_transport_tcp::framing::{
    read_frame, read_frame_into, write_frame, write_frame_into, write_frame_prereserved,
    MAX_FRAME_SIZE_DEFAULT,
};
use tokio::io::duplex;

fn main() {
    let mut h = Harness::new("framing", env!("CARGO_MANIFEST_DIR"));

    // --- framing/round_trip/write_then_read/<size> --------------------------
    for size in [64usize, 1024, 16 * 1024, 256 * 1024, 1024 * 1024] {
        let payload = vec![0xabu8; size];
        let id = format!("framing/round_trip/write_then_read/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let payload = payload.clone();
                async move { payload }
            },
            move |p| async move {
                let buf_cap = size + 1024;
                let (mut a, mut b) = duplex(buf_cap);
                write_frame(&mut a, &p).await.unwrap();
                let got = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
                black_box(got);
            },
        );
    }

    // --- framing/round_trip_pooled/write_then_read/<size> -------------------
    // Buffer lives across iterations — simulates a per-connection scratch
    // buffer in a real request loop, so only the payload + duplex pair are
    // fresh per iteration; the scratch Vec is shared setup (plan 1 capture).
    for size in [64usize, 1024, 16 * 1024, 256 * 1024, 1024 * 1024] {
        let payload = vec![0xabu8; size];
        let id = format!("framing/round_trip_pooled/write_then_read/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let payload = payload.clone();
                async move { (payload, Vec::<u8>::with_capacity(size)) }
            },
            move |(p, mut scratch)| async move {
                let buf_cap = size + 1024;
                let (mut a, mut bb) = duplex(buf_cap);
                write_frame(&mut a, &p).await.unwrap();
                read_frame_into(&mut bb, MAX_FRAME_SIZE_DEFAULT, &mut scratch)
                    .await
                    .unwrap();
                black_box(&scratch);
            },
        );
    }

    // --- framing/write_only/write_frame/<size> -------------------------------
    for size in [64usize, 1024, 16 * 1024] {
        let payload = vec![0xcdu8; size];
        let id = format!("framing/write_only/write_frame/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let payload = payload.clone();
                // Sink wide enough that write_all never blocks — measures the
                // encode + flush overhead in isolation.
                async move { (payload, duplex(size + 1024)) }
            },
            move |(p, (mut w, r))| async move {
                // Keep the read half ALIVE for the whole timed routine —
                // `tokio::io::duplex`'s pipe closes as soon as EITHER half is
                // dropped, and `write_frame` writes 4-byte length + payload
                // via `write_all`, which returns `BrokenPipe` the moment the
                // reader is gone. Binding the receiver to a live local
                // (rather than `_r`, which is still dropped by the
                // destructuring pattern in the same expression) keeps the
                // pipe open until the routine returns.
                write_frame(&mut w, &p).await.unwrap();
                black_box(&r);
            },
        );
    }

    // --- framing/write_only/write_frame_into/<size> -------------------------
    // §3.4: write_frame_into copies the payload into a scratch buffer to
    // prepend the 4-byte length prefix. Compare against write_frame_prereserved
    // which writes an already-length-prefixed buffer directly (no memcpy).
    for size in [1024usize, 16 * 1024, 256 * 1024, 1024 * 1024] {
        let payload = vec![0xcdu8; size];
        let id = format!("framing/write_only/write_frame_into/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let payload = payload.clone();
                async move {
                    (
                        payload,
                        Vec::<u8>::with_capacity(size + 16),
                        duplex(size + 1024),
                    )
                }
            },
            move |(p, mut scratch, (mut w, r))| async move {
                write_frame_into(&mut w, &p, &mut scratch).await.unwrap();
                black_box(&r);
            },
        );
    }

    // --- framing/write_only/write_frame_prereserved/<size> ------------------
    // §3.4: the prereserved path skips the memcpy — the caller serializes
    // directly into a length-prefixed buffer, and the writer just does
    // write_all. The setup builds the prereserved buffer (untimed), the
    // routine does only the write.
    for size in [1024usize, 16 * 1024, 256 * 1024, 1024 * 1024] {
        let payload = vec![0xcdu8; size];
        let id = format!("framing/write_only/write_frame_prereserved/{size}");
        h.bench_batched_async(
            &id,
            move || {
                let payload = payload.clone();
                async move {
                    // Build the prereserved buffer (untimed setup).
                    let len = payload.len() as u32;
                    let mut buf = Vec::with_capacity(4 + payload.len());
                    buf.extend_from_slice(&len.to_be_bytes());
                    buf.extend_from_slice(&payload);
                    (buf, duplex(size + 1024))
                }
            },
            move |(buf, (mut w, r))| async move {
                write_frame_prereserved(&mut w, &buf).await.unwrap();
                black_box(&r);
            },
        );
    }

    h.run();
}
