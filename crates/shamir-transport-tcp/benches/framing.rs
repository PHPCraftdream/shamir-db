//! Per-frame cost benchmarks for `shamir-transport-tcp::framing`.
//!
//! Run: `cargo bench -p shamir-transport-tcp --bench framing`
//!
//! Wire format (TRANSPORT_TCP §2): `[u32_be length][msgpack: length bytes]`.
//!
//! Measures the read+write round-trip via `tokio::io::duplex` for several
//! frame sizes spanning the typical request-payload range up to the
//! 16 MB ceiling (`MAX_FRAME_SIZE_DATA`). Throughput in bytes/sec is the
//! primary metric — we expect this to be I/O-bound at large sizes (memcpy
//! dominates) and length-encoding-bound at small sizes (the 4-byte prefix
//! plus tokio scheduler overhead).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_transport_tcp::framing::{
    read_frame, read_frame_into, write_frame, MAX_FRAME_SIZE_DEFAULT,
};
use tokio::io::duplex;
use tokio::runtime::Builder;

fn bench_round_trip(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let mut g = c.benchmark_group("framing/round_trip");
    for size in [64usize, 1024, 16 * 1024, 256 * 1024, 1024 * 1024]
        .iter()
        .copied()
    {
        let payload = vec![0xabu8; size];
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::new("write_then_read", size), &payload, |b, p| {
            b.to_async(&rt).iter(|| async {
                let buf_cap = size + 1024;
                let (mut a, mut b) = duplex(buf_cap);
                write_frame(&mut a, p).await.unwrap();
                let got = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
                black_box(got);
            });
        });
    }
    g.finish();
}

fn bench_write_only(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let mut g = c.benchmark_group("framing/write_only");
    for size in [64usize, 1024, 16 * 1024].iter().copied() {
        let payload = vec![0xcdu8; size];
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::new("write_frame", size), &payload, |b, p| {
            b.to_async(&rt).iter(|| async {
                // Use a sink wide enough that write_all never blocks — measures
                // the encode + flush overhead in isolation.
                let (mut w, mut _r) = duplex(size + 1024);
                write_frame(&mut w, p).await.unwrap();
            });
        });
    }
    g.finish();
}

/// Same as `bench_round_trip` but uses `read_frame_into` with a reused
/// buffer (Optim #1). Compare against `framing/round_trip` to see the
/// per-frame allocation cost saved.
fn bench_round_trip_pooled(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let mut g = c.benchmark_group("framing/round_trip_pooled");
    for size in [64usize, 1024, 16 * 1024, 256 * 1024, 1024 * 1024]
        .iter()
        .copied()
    {
        let payload = vec![0xabu8; size];
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::new("write_then_read", size), &payload, |b, p| {
            // Buffer lives across iterations — simulates a per-connection
            // scratch buffer in a real request loop.
            let mut scratch: Vec<u8> = Vec::with_capacity(size);
            b.iter(|| {
                rt.block_on(async {
                    let buf_cap = size + 1024;
                    let (mut a, mut bb) = duplex(buf_cap);
                    write_frame(&mut a, p).await.unwrap();
                    read_frame_into(&mut bb, MAX_FRAME_SIZE_DEFAULT, &mut scratch)
                        .await
                        .unwrap();
                    black_box(&scratch);
                });
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench_round_trip, bench_round_trip_pooled, bench_write_only);
criterion_main!(benches);
