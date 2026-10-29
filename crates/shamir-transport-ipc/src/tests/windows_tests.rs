use std::future::Future;
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;

use crate::validate_endpoint;
use crate::IpcListener;

/// Unique-enough per-test pipe name — Windows named pipes share one
/// process-wide namespace (`\\.\pipe\...`), so a fixed literal name would
/// collide across tests run in the same nextest process.
fn test_pipe_name(tag: &str) -> String {
    format!(
        r"\\.\pipe\shamir-transport-ipc-test-{tag}-{}",
        std::process::id()
    )
}

#[test]
fn rv_transport_ipc_bd4ebef7_windows_endpoint_rules_and_boundaries() {
    assert!(validate_endpoint(r"\\.\pipe\").is_err());
    assert!(validate_endpoint(r"\\.\pipe\parent\child").is_err());
    assert!(validate_endpoint("\\\\.\\pipe\\nested\\child").is_err());
    assert!(validate_endpoint(r"\\.\pipe\name").is_ok());
    assert!(validate_endpoint(r"\\.\PIPE\name").is_ok());
    assert!(validate_endpoint("\\\\server\\pipe\\x").is_err());
    assert!(validate_endpoint(r"\\?\pipe\x").is_err());
    assert!(validate_endpoint("//./pipe/x").is_err());
    assert!(validate_endpoint("shamir-db").is_err());
    assert!(validate_endpoint(&format!(r"\\.\pipe\{}", "x".repeat(247))).is_ok());
    assert!(validate_endpoint(&format!(r"\\.\pipe\{}", "x".repeat(248))).is_err());
    assert!(validate_endpoint("\\\\.\\pipe\\bad\0name").is_err());
}

#[tokio::test]
async fn rv_transport_ipc_bd4ebef7_windows_entrypoints_reject_invalid_addresses() {
    let connect_error = crate::connect("shamir-db")
        .await
        .expect_err("invalid endpoint");
    let bind_error = match IpcListener::bind("shamir-db").await {
        Ok(_) => panic!("invalid endpoint unexpectedly bound"),
        Err(error) => error,
    };
    let remote_error = crate::connect(r"\\server\pipe\x")
        .await
        .expect_err("invalid endpoint");
    for error in [connect_error, bind_error, remote_error] {
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    let name = test_pipe_name("8ed1b450");
    let mut listener = IpcListener::bind(&name).await.expect("first bind");
    let handle = listener.pipe_handle().expect("pending pipe handle");
    crate::tests::windows_dacl::check_handle_owner_dacl(handle as _)
        .expect("effective DACL is one owner SID allow ACE");
    let error = match IpcListener::bind(&name).await {
        Ok(_) => panic!("duplicate bind unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.to_string().contains(&name));
    assert!(error.to_string().contains("first named pipe instance"));
    let context = error.get_ref().expect("context");
    let original = std::error::Error::source(context)
        .and_then(|source| source.downcast_ref::<std::io::Error>())
        .expect("original OS error");
    assert_eq!(error.kind(), original.kind());
    assert!(original.raw_os_error().is_some());
    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept");
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.expect("read");
        byte
    });
    let mut client = crate::connect(&name).await.expect("owner may connect");
    client.write_all(&[0x5a]).await.expect("write");
    assert_eq!(server.await.expect("server"), [0x5a]);
}
#[tokio::test]
async fn accept_and_round_trip_bytes() {
    let name = test_pipe_name("roundtrip");
    let mut listener = IpcListener::bind(&name).await.expect("bind");
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept");
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("write");
    });

    let mut client = ClientOptions::new()
        .open(&client_name)
        .expect("open client");
    client.write_all(b"hello").await.expect("write");
    let mut buf = [0u8; 5];
    client.read_exact(&mut buf).await.expect("read");
    assert_eq!(&buf, b"world");

    server.await.expect("server task");
}

#[tokio::test]
async fn listener_serves_a_second_client_after_the_first_disconnects() {
    let name = test_pipe_name("second-client");
    let mut listener = IpcListener::bind(&name).await.expect("bind");
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let mut stream = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.expect("read");
            assert_eq!(&buf, b"ping");
        }
    });

    for _ in 0..2 {
        let mut client = ClientOptions::new()
            .open(&client_name)
            .expect("open client");
        client.write_all(b"ping").await.expect("write");
        drop(client);
    }

    server.await.expect("server task");
}

/// RV-TRANSPORT-IPC-EFF55124: aborting an in-flight `accept` (after it took
/// the pending instance but before the client connected) must leave the
/// listener usable — the next `accept` must still work instead of panicking
/// on a missing pending instance.
#[tokio::test]
async fn rv_transport_ipc_eff55124_aborted_accept_leaves_listener_usable() {
    let name = test_pipe_name("eff55124");
    let listener = IpcListener::bind(&name).await.expect("bind");
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();

    // Server task: poll the accept future exactly once (no client yet →
    // Pending), cancel it by dropping it, signal, then serve a real client
    // with a second accept.
    let server_handle = tokio::spawn(async move {
        let mut listener = listener;
        // Scope the non-Send `Context` so it is dropped before the later
        // `.await` (the spawned future must stay `Send`).
        {
            let mut accept_fut = Box::pin(listener.accept());
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            let poll_result = accept_fut.as_mut().poll(&mut cx);
            assert!(
                matches!(poll_result, Poll::Pending),
                "first accept with no client must be Pending, got {poll_result:?}"
            );
            drop(accept_fut); // the cancellation under test
        }
        ready_tx.send(()).expect("readiness channel still open");

        let mut stream = listener.accept().await.expect("second accept");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
    });

    // Open the client only AFTER the server polled once and cancelled —
    // deterministic, no race.
    ready_rx.await.expect("server signalled");
    let mut client = ClientOptions::new().open(&name).expect("open client");
    client.write_all(b"ping").await.expect("client write");

    server_handle.await.expect("server task");
}

/// RV-TRANSPORT-IPC-07D10AFD: driving `accept` through an injected
/// replacement-creation failure must return the error AND leave the
/// listener usable — the pending instance is restored, so the next
/// `accept` still serves a real client.
#[tokio::test]
async fn rv_transport_ipc_07d10afd_accept_error_keeps_listener_usable() {
    let name = test_pipe_name("07d10afd");
    let mut listener = IpcListener::bind(&name).await.expect("bind");

    let err = listener
        .accept_with(|_, _| Err(std::io::Error::other("injected create failure")))
        .await
        .expect_err("injected failure must propagate");
    assert_eq!(
        err.to_string(),
        "injected create failure",
        "the injected error must surface verbatim"
    );

    // The restored pending instance must still serve a client.
    let client_name = name.clone();
    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept after error");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
    });
    let mut client = ClientOptions::new()
        .open(&client_name)
        .expect("open client");
    client.write_all(b"ping").await.expect("client write");
    server.await.expect("server task");
}

/// RV-TRANSPORT-IPC-466D0307: after a first client consumed the sole
/// pending instance, a second `connect` must retry on ERROR_PIPE_BUSY and
/// succeed once the server's `accept` rotates in a fresh instance —
/// instead of surfacing the BUSY error.
#[tokio::test]
async fn rv_transport_ipc_466d0307_connect_retries_on_pipe_busy() {
    let name = test_pipe_name("466d0307");
    let mut listener = IpcListener::bind(&name).await.expect("bind");

    // Consume the sole pending instance; the server has not called
    // `accept` yet, so the pipe namespace is now empty. The client writes
    // so the server's first accept (against this already-connected
    // instance) completes deterministically.
    let mut first = ClientOptions::new().open(&name).expect("first client");
    first.write_all(b"ping").await.expect("first client write");

    // Precondition: with the sole instance consumed and no `accept` yet, a
    // raw (non-retrying) open MUST see ERROR_PIPE_BUSY — this is the window
    // the retry loop closes.
    let busy = ClientOptions::new()
        .open(&name)
        .expect_err("no pending instance: raw open must fail");
    assert_eq!(busy.raw_os_error(), Some(231), "expected ERROR_PIPE_BUSY");

    // Server: wait for the signal, then accept twice — the first accept
    // completes instantly against the already-connected first instance and
    // rotates in the fresh one the retrying client will land on.
    let (sig_tx, sig_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        sig_rx.await.expect("signal channel");
        let mut stream = listener.accept().await.expect("first accept");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("first read");
        let mut stream = listener.accept().await.expect("second accept");
        stream.write_all(b"pong").await.expect("second write");
    });

    let retrying = tokio::spawn(async move {
        let mut client = crate::connect(&name).await.expect("retrying connect");
        client.write_all(b"ping").await.expect("client write");
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.expect("client read");
        assert_eq!(&buf, b"pong");
    });

    // Single-threaded runtime: this yield runs the retrying client up to its
    // first BUSY retry (it yields inside `connect`) BEFORE the server may
    // rotate in a replacement, so the client provably hit BUSY first.
    tokio::task::yield_now().await;
    sig_tx.send(()).expect("receiver alive");

    server.await.expect("server task");
    retrying.await.expect("retrying client task");
}

/// RV-TRANSPORT-IPC-4E8BE4FD: lifecycle pin — while a listener is live, a
/// second `bind` on the same name must fail (`first_pipe_instance`), and
/// after the listener is dropped (all handles released) the same name must
/// be bindable again.
#[tokio::test]
async fn rv_transport_ipc_4e8be4fd_drop_releases_pipe_name_for_rebind() {
    let name = test_pipe_name("4e8be4fd");
    let listener = IpcListener::bind(&name).await.expect("first bind");

    assert!(
        IpcListener::bind(&name).await.is_err(),
        "second live bind must fail"
    );

    drop(listener);

    let mut rebound = IpcListener::bind(&name)
        .await
        .expect("rebind after drop must succeed");
    let client_name = name.clone();
    let server = tokio::spawn(async move {
        let mut stream = rebound.accept().await.expect("accept on rebound");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
    });
    let mut client = ClientOptions::new()
        .open(&client_name)
        .expect("open client on rebound");
    client.write_all(b"ping").await.expect("client write");
    server.await.expect("server task");
}
