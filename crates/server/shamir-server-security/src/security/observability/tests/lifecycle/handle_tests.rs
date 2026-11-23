//! `ObservabilityHandle` lifecycle (item 9 / P2): `shutdown()` JOINS both of
//! the handle's tasks and REPORTS which of them actually joined, and dropping a
//! handle without `shutdown()` still cancels the shared token — now through the
//! private `shutdown_on_drop` field instead of an `impl Drop for
//! ObservabilityHandle` (which made the public type non-move-partial).

use std::sync::Arc;
use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::State;
use axum::http::StatusCode;
use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::{ShamirDb, SystemStoreConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::observability::{
    metrics_handler, spawn_with_byte_budget, AppState, CancelOnDrop, ObservabilityHandle,
    ObservabilityShutdownOutcome, ObservabilityState, OBS_TASK_JOIN_TIMEOUT,
};

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
// complete, the listener socket is really gone), `Drop` without
// `shutdown()` cancels the token both tasks wait on, and the returned outcome
// names each side instead of collapsing them into "the listener or the poller".
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
    let outcome = tokio::time::timeout(SHUTDOWN_BOUND, handle.shutdown())
        .await
        .expect("shutdown() must join both tasks after cancel(), not detach them");
    assert_eq!(
        outcome,
        ObservabilityShutdownOutcome {
            listener_joined: true,
            poller_joined: true,
        },
        "a fully stopped server must not report a side as still draining"
    );
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

// Pins the parked-handler case from the review: a client that sends a
// `/metrics` request and never reads the response leaves a live TCP connection
// in the listener's drain set. `shutdown()` must still complete inside the
// bound, report BOTH sides as joined, and leave the socket closed — the
// listener drains/aborts its connection rather than holding the database past
// the caller's deadline.
//
// Rendezvous is `connect` + `write_all` only (no sleep, no read before the
// shutdown): `shutdown()`'s own `OBS_TASK_JOIN_TIMEOUT` per side is the
// deadline under test.
//
// Mutants killed: "shutdown() detaches the listener join" (the socket stays
// open and the read below times out); "shutdown() returns () and collapses
// both sides" (no outcome to assert at all); "listener_joined hard-coded true"
// is NOT killed here — nothing parks the drain past the bound — that case is
// covered by `shutdown_names_a_wedged_listener_without_hiding_the_poller`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_scrape_connection_does_not_outlive_shutdown() {
    let (dir, handle) = spawn_on_tempdir().await;
    let bound_addr = handle.bound_addr;

    let mut parked = TcpStream::connect(bound_addr).await.expect("connect");
    // No `Connection: close`: the connection is meant to be parked mid-drain,
    // not politely finished by the client.
    parked
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .expect("write request");
    // Deliberately do NOT read the response.

    handle.cancel();
    let outcome = tokio::time::timeout(SHUTDOWN_BOUND, handle.shutdown())
        .await
        .expect("a parked, unread scrape must not keep shutdown() past its bound");
    assert!(
        outcome.listener_joined,
        "the listener must drain/abort the parked connection inside its own \
         {OBS_TASK_JOIN_TIMEOUT:?} bound, got {outcome:?}"
    );
    assert!(
        outcome.poller_joined,
        "the poller was idle and cancelled, so it must not be reported as stuck, \
         got {outcome:?}"
    );

    // Closed afterwards: either FIN (EOF) or a reset if the server closed with
    // the request bytes still unread — both mean the socket is gone, while a
    // still-served connection blocks here until the timeout.
    let mut drained = Vec::new();
    let read_result = tokio::time::timeout(SHUTDOWN_BOUND, parked.read_to_end(&mut drained))
        .await
        .expect("the parked socket must be closed by the time shutdown() returned");
    match read_result {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ) => {}
        Err(error) => panic!("unexpected error reading the parked socket after shutdown: {error}"),
    }
    assert!(
        TcpStream::connect(bound_addr).await.is_err(),
        "the listening socket itself must be closed after shutdown()"
    );
    drop(dir);
}

// Pins the per-side report over the real fixture: on a NORMAL shutdown the
// poller (which holds its own `Arc<ShamirDb>`) really did join, and `cancel()`
// reaches the token the poller selects on before `shutdown()` is awaited.
//
// Mutants killed: "poller_joined hard-coded false" / "the outcome swaps the two
// sides" (caught together with the wedged-listener test, which has one `true`
// and one `false`); "shutdown() returns ()" (nothing to assert).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reports_the_poller_joined_over_a_real_database() {
    let (_dir, handle) = spawn_on_tempdir().await;
    let bound_addr = handle.bound_addr;
    let token = handle.shutdown_on_drop.0.clone();

    handle.cancel();
    assert!(
        token.is_cancelled(),
        "cancel() must reach the token the poller selects on, immediately"
    );
    let outcome = tokio::time::timeout(SHUTDOWN_BOUND, handle.shutdown())
        .await
        .expect("shutdown() must join both tasks after cancel(), not detach them");
    assert!(
        outcome.poller_joined,
        "the poller was cancelled before its first tick, so it must join, got {outcome:?}"
    );
    assert!(
        outcome.listener_joined,
        "the listener saw no request, so it must join too, got {outcome:?}"
    );
    assert!(TcpStream::connect(bound_addr).await.is_err());
}

// Pins that ONE wedged side cannot hide the other: a handle whose listener task
// ignores cancellation (it never awaits the token) must report
// `listener_joined == false`, still report the healthy poller as joined, and
// still return inside the caller's bound. The wedged task is `pending()`, not a
// sleep — it deterministically never finishes, so the per-side bound is the
// only thing that can end the wait.
//
// Mutants killed: "one stuck side hides the other" (the old sequential
// unbounded awaits, or one shared budget that consumed the poller's share);
// "the outcome swaps the two sides" (`listener_joined == false` +
// `poller_joined == true` cannot be produced by a swap); "an expired side is
// reported as joined" (`!outcome.listener_joined`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_names_a_wedged_listener_without_hiding_the_poller() {
    let started = std::time::Instant::now();
    let handle = ObservabilityHandle {
        bound_addr: "127.0.0.1:0".parse().expect("addr"),
        state: ObservabilityState::new(),
        shutdown_on_drop: CancelOnDrop(CancellationToken::new()),
        // Never observes the token, never returns.
        listener_task: Some(tokio::spawn(std::future::pending::<()>())),
        poller_task: Some(tokio::spawn(async {})),
    };

    let outcome = tokio::time::timeout(SHUTDOWN_BOUND, handle.shutdown())
        .await
        .expect("one wedged side must not stall the whole shutdown() call");
    assert_eq!(
        outcome,
        ObservabilityShutdownOutcome {
            listener_joined: false,
            poller_joined: true,
        },
        "the wedged listener must be reported as still draining and the healthy \
         poller as joined"
    );
    assert!(
        started.elapsed() >= OBS_TASK_JOIN_TIMEOUT,
        "the wedged side must actually be awaited up to its own bound \
         ({OBS_TASK_JOIN_TIMEOUT:?}) before it is reported, elapsed {:?}",
        started.elapsed()
    );
}

// Pins the handler half of item 9: a `/metrics` scrape in flight when the
// server is cancelled answers `503 not ready` promptly instead of sampling the
// database under the caller's `Arc<ShamirDb>`.
//
// Mutants killed: "no cancellation in the handler" — without the `select!` the
// same `AppState` (recorder handle present, database present) renders and
// answers `200 OK`, so the status assertion fails.
#[tokio::test]
async fn cancelled_metrics_handler_answers_503_without_sampling_the_database() {
    let dir = TempDir::new().expect("tempdir");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(dir.path().join("system")))
        .await
        .expect("init db");
    // A recorder handle that is NOT installed globally: `render()` works on it,
    // so the handler would answer `200` if it did not observe the token.
    let recorder = PrometheusBuilder::new().build_recorder();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let state = AppState {
        state: ObservabilityState::new(),
        prom: Some(recorder.handle()),
        db: Some(Arc::new(db)),
        byte_budget: None,
        shutdown,
    };

    let response = tokio::time::timeout(SHUTDOWN_BOUND, metrics_handler(State(state)))
        .await
        .expect("a cancelled handler must answer at once, not sample the database");
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a cancelled server must answer 503, not render metrics off the database"
    );
    let body = to_bytes(response.into_body(), 64)
        .await
        .expect("metrics body");
    assert_eq!(&body[..], b"not ready\n");
}
