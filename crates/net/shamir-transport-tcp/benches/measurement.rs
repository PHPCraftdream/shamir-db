//! Loopback plain-TCP framing measurement bench for `shamir-transport-tcp`.
//!
//! Run: `cargo bench -p shamir-transport-tcp --bench measurement`
//!
//! CI-only recipe: `tests/fixtures/measurement.md`. CI collects the
//! deterministic per-size result lines this bench prints to stdout as the
//! measurement artifact; percentile computation (p50/p95/p99, if wanted)
//! happens on that recorded output, NOT inside the bench — p50/p95/p99 are
//! not computable inside `bench_batched_async`. The bench itself does not
//! gate on latency.
//!
//! Replaces the former `tests/measurement.rs` ignored integration test,
//! which deadlocked (its echo task awaited `accept()` before any client
//! connected) and echoed the bare payload without the 4-byte frame prefix
//! while the client used `read_frame`. Here the echo peer is CORRECTLY
//! FRAMED: it loops `read_frame` and answers with
//! `write_frame_with_cap(.., MAX_FRAME_SIZE_DEFAULT)` (prefix + payload).
//!
//! Connection policy: a fresh `TcpStream` connect per timed iteration.
//! Connection setup is part of what the recipe measures (per-frame
//! round-trip over loopback plain TCP), and the harness's fixed-iteration
//! model has no per-variant persistent-state channel anyway — a shared
//! connection created in untimed setup could not be handed to the closure
//! and reused, and reusing one socket would entangle iterations through
//! its shared kernel buffers.
//!
//! Wire format (TRANSPORT_TCP §2): `[u32_be length][msgpack: length bytes]`.

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_transport_tcp::framing::{read_frame, write_frame_with_cap, MAX_FRAME_SIZE_DEFAULT};
use tokio::net::{TcpListener, TcpStream};

/// Recipe payload sizes (tests/fixtures/measurement.md).
const SIZES: [usize; 5] = [1, 64, 1024, 65536, 1048576];

/// Echo server: bound listener + correctly framed echo task. Spawned ONCE
/// on the harness's shared runtime (guarded by `SERVER_SPAWNED`, because
/// `bench_batched_async` gives no public way to pre-spawn on that runtime).
/// The task lives for the whole bench run; it answers every framed request
/// with a correctly framed echo (4-byte prefix + payload).
static SERVER_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Bind the loopback listener and spawn the framed echo task. Returns the
/// listener address for clients to connect to.
async fn spawn_echo_server() -> std::io::Result<std::net::SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let _ = sock.set_nodelay(true);
            tokio::spawn(async move {
                // Echo the payload WITH the frame prefix: read_frame returns
                // the bare payload, write_frame_with_cap re-prefixes it.
                loop {
                    let Ok(payload) = read_frame(&mut sock, MAX_FRAME_SIZE_DEFAULT).await else {
                        break;
                    };
                    if write_frame_with_cap(&mut sock, &payload, MAX_FRAME_SIZE_DEFAULT)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    Ok(addr)
}

/// Untimed setup per variant: bind + spawn the echo server on first use,
/// then return (addr, payload) so the timed routine does only
/// connect / write / read / verify.
async fn setup(
    addr_cell: Arc<tokio::sync::OnceCell<std::net::SocketAddr>>,
    size: usize,
) -> (std::net::SocketAddr, Vec<u8>) {
    if !SERVER_SPAWNED.swap(true, Ordering::SeqCst) {
        let addr = spawn_echo_server().await.expect("bind loopback listener");
        addr_cell.set(addr).expect("server addr set once");
    }
    let addr = *addr_cell.get().expect("echo server addr");
    (addr, vec![0xabu8; size])
}

fn main() {
    let mut h = Harness::new("measurement", env!("CARGO_MANIFEST_DIR"));

    // Shared cell the first setup stores the echo server's address into;
    // later setups reuse it (the server task lives for the whole run).
    let addr_cell = Arc::new(tokio::sync::OnceCell::new());

    for size in SIZES {
        let id = format!("plain_tcp/framing_round_trip/{size}");
        let addr_cell = Arc::clone(&addr_cell);
        h.bench_batched_async(
            &id,
            move || {
                let addr_cell = Arc::clone(&addr_cell);
                async move { setup(addr_cell, size).await }
            },
            move |(addr, payload)| async move {
                let mut sock = TcpStream::connect(addr).await.expect("connect echo server");
                sock.set_nodelay(true).expect("set nodelay");
                write_frame_with_cap(&mut sock, &payload, MAX_FRAME_SIZE_DEFAULT)
                    .await
                    .expect("write framed payload");
                let echoed = read_frame(&mut sock, MAX_FRAME_SIZE_DEFAULT)
                    .await
                    .expect("read framed echo");
                // Byte-for-byte verification, not black_box-only: a framing
                // or prefix bug must panic loudly, not silently skew timing.
                assert_eq!(
                    echoed, payload,
                    "echoed frame payload mismatch for size {size}"
                );
                black_box(&echoed);
            },
        );
    }

    h.run();
}
