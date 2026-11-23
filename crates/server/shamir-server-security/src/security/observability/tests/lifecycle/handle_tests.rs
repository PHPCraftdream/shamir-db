//! `ObservabilityHandle` lifecycle (item 9 / P2): `shutdown()` JOINS both of
//! the handle's tasks, and dropping a handle without `shutdown()` still
//! cancels the shared token — now through the private `shutdown_on_drop` field
//! instead of an `impl Drop for ObservabilityHandle` (which made the public
//! type non-move-partial).

use std::sync::Arc;
use std::time::Duration;

use shamir_db::{ShamirDb, SystemStoreConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::observability::{spawn_with_byte_budget, ObservabilityHandle, ObservabilityState};

/// Bounded await for a shutdown that has nothing left to drain. Real time, no
/// sleeps: the bound can only be hit by a task that fails to observe the
/// cancel, which is exactly what the assertion pins.
const SHUTDOWN_BOUND: Duration = Duration::from_secs(5);

/// A real observability server over a `TempDir`-backed `ShamirDb`, so the
/// poller has a database to sample and the listener has a live socket.
async fn spawn_on_tempdir() -> (TempDir, ObservabilityHandle) {
    let dir = TempDir::new().expect("tempdir");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(dir.path().join("system")))
        .await
        .expect("init db");
    // `install_recorder = false`: the Prometheus recorder is process-global and
    // another test in this binary may own it. The listener and the poller (over
    // `db`) are the real ones either way.
    let handle = spawn_with_byte_budget(
        "127.0.0.1:0".parse().expect("addr"),
        ObservabilityState::new(),
        false,
        None,
        None,
        Some(Arc::new(db)),
        false,
    )
    .await
    .expect("spawn observability");
    (dir, handle)
}

// Pins: `shutdown()` returns inside the bound after `cancel()` (both joins
// complete, the listener socket is really gone), and `Drop` without
// `shutdown()` cancels the token both tasks wait on.
//
// Mutants killed: "no cancel-on-drop" (drop the `CancelOnDrop` field or its
// `Drop`) — the second phase's token stays live; and "shutdown() does not wait
// for cancellation" (detach/early-return) — the listener socket survives the
// returned `shutdown()`. It does NOT pin the in-tick `select!` on its own: a
// tick body that merely races cancellation can only be caught deterministically
// by a `degraded_index_count` the test can park, which this fixture cannot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_joins_both_tasks_and_drop_cancels_the_token() {
    let (dir, handle) = spawn_on_tempdir().await;
    let bound_addr = handle.bound_addr;
    // The token is private; this module is a descendant of `observability`, so
    // it may read it — and reading it is what makes the cancel observable.
    let token = handle.shutdown_on_drop.0.clone();

    // Exercise the handler once through the live listener, then close the
    // connection, so the listener task is stopped with a request it has already
    // served (no parked handler, no sleep).
    let mut conn = TcpStream::connect(bound_addr).await.expect("connect");
    conn.write_all(b"GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .expect("write request");
    let mut response = Vec::new();
    tokio::time::timeout(SHUTDOWN_BOUND, conn.read_to_end(&mut response))
        .await
        .expect("the handler must answer and close")
        .expect("read response");
    assert!(
        String::from_utf8_lossy(&response).starts_with("HTTP/1.1 503"),
        "no recorder was installed for this handle, so /metrics must answer 503, got: {:?}",
        String::from_utf8_lossy(&response)
            .chars()
            .take(80)
            .collect::<String>()
    );
    drop(conn);

    handle.cancel();
    assert!(token.is_cancelled(), "cancel() must reach the shared token");
    tokio::time::timeout(SHUTDOWN_BOUND, handle.shutdown())
        .await
        .expect("shutdown() must join both tasks after cancel(), not detach them");
    assert!(
        TcpStream::connect(bound_addr).await.is_err(),
        "the listener socket must be gone once shutdown() returned (task joined)"
    );
    drop(dir);

    // Phase 2: Drop without `shutdown()`.
    let (_dir, handle) = spawn_on_tempdir().await;
    let token = handle.shutdown_on_drop.0.clone();
    assert!(
        !token.is_cancelled(),
        "a running handle must not be pre-cancelled"
    );
    drop(handle);
    assert!(
        token.is_cancelled(),
        "dropping the handle without shutdown() must cancel both tasks"
    );
}
