use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore};
use tokio_tungstenite::tungstenite::{protocol::Role, Message};
use tokio_tungstenite::WebSocketStream;

use crate::framer::{Framer, TcpFramer, WsFramer};
use crate::tests::engine::connection::handshake_e2e_tests::heartbeat_test_context;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{HandlerFuture, RequestHandler};
use shamir_connect::server::session::{Session, SessionPermissions};

/// Lets the spawned loop and its reader/writer tasks run to quiescence
/// without advancing the paused clock.
async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

fn wire_frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

async fn receive_ping(client: &mut WebSocketStream<tokio::io::DuplexStream>) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Ping(payload))) => break payload.to_vec(),
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("codec error before Ping: {error}"),
                None => panic!("stream ended before Ping"),
            }
        }
    })
    .await
    .expect("Ping deadline")
}

async fn receive_wire_close(io: &mut tokio::io::DuplexStream) -> (u16, String) {
    tokio::time::timeout(Duration::from_secs(1), async {
        // A heartbeat Ping due at the same instant may precede the Close.
        loop {
            let mut header = [0u8; 2];
            io.read_exact(&mut header).await.unwrap();
            assert_eq!(header[1] & 0x80, 0, "server frames must not be masked");
            let mut len = usize::from(header[1] & 0x7f);
            if len == 126 {
                let mut ext = [0u8; 2];
                io.read_exact(&mut ext).await.unwrap();
                len = usize::from(u16::from_be_bytes(ext));
            } else if len == 127 {
                let mut ext = [0u8; 8];
                io.read_exact(&mut ext).await.unwrap();
                len = usize::try_from(u64::from_be_bytes(ext)).unwrap();
            }
            let mut payload = vec![0; len];
            io.read_exact(&mut payload).await.unwrap();
            match header[0] & 0x0f {
                0x9 => continue,
                0x8 => {
                    break (
                        u16::from_be_bytes([payload[0], payload[1]]),
                        String::from_utf8(payload[2..].to_vec()).unwrap(),
                    )
                }
                opcode => panic!("expected WS Close frame, got opcode {opcode:#x}"),
            }
        }
    })
    .await
    .expect("wire Close deadline")
}

async fn receive_wire_ping(io: &mut tokio::io::DuplexStream) -> [u8; 8] {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut header = [0u8; 2];
        io.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0] & 0x0f, 0x9, "expected server Ping");
        assert_eq!(header[1] & 0x7f, 8);
        let mut ping = [0u8; 8];
        io.read_exact(&mut ping).await.unwrap();
        ping
    })
    .await
    .expect("server Ping deadline")
}

async fn write_client_pong(io: &mut tokio::io::DuplexStream, payload: &[u8; 8]) {
    const MASK: [u8; 4] = [0x31, 0x72, 0xa5, 0xc9];
    io.write_all(&[0x8a, 0x88]).await.unwrap();
    io.write_all(&MASK).await.unwrap();
    let masked = std::array::from_fn::<_, 8, _>(|i| payload[i] ^ MASK[i % 4]);
    io.write_all(&masked).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rv_transport_ws_a3b8161a_request_loop_ping_and_matched_pongs() {
    let (server_io, client_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (reader, writer) = WsFramer::new(server).split();
    let mut context = Arc::try_unwrap(heartbeat_test_context(Duration::from_secs(2)))
        .ok()
        .unwrap();
    context.idle_timeout = Duration::from_secs(600);
    let task = tokio::spawn(crate::connection::request_loop(
        Arc::new(context),
        reader,
        writer,
        [0; 32],
    ));
    settle().await;
    let mut now = 0;
    for ping_due in [30, 60, 90, 120, 150] {
        tokio::time::advance(Duration::from_secs(ping_due - now)).await;
        now = ping_due;
        let ping = receive_ping(&mut client).await;
        assert_eq!(ping.len(), 8);
        client.send(Message::Pong(ping.into())).await.unwrap();
        settle().await;
        assert!(
            !task.is_finished(),
            "matched Pong keeps request loop alive at {now}s"
        );
    }
    client.send(Message::Close(None)).await.unwrap();
    settle().await;
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rv_transport_ws_a3b8161a_unanswered_ping_closes_4003() {
    let (server_io, mut client_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let (reader, writer) = WsFramer::new(server).split();
    let mut context = Arc::try_unwrap(heartbeat_test_context(Duration::from_secs(2)))
        .ok()
        .unwrap();
    context.idle_timeout = Duration::from_secs(600);
    let task = tokio::spawn(crate::connection::request_loop(
        Arc::new(context),
        reader,
        writer,
        [0; 32],
    ));
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let _ping = receive_wire_ping(&mut client_io).await;
    settle().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert!(
        task.is_finished(),
        "request_loop returns at the Pong deadline"
    );
    let (code, reason) = receive_wire_close(&mut client_io).await;
    assert_eq!((code, reason.as_str()), (4003, "heartbeat_timeout"));
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rv_transport_ws_a3b8161a_mismatched_pong_times_out() {
    let (server_io, mut client_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let (reader, writer) = WsFramer::new(server).split();
    let task = tokio::spawn(crate::connection::request_loop(
        heartbeat_test_context(Duration::from_secs(2)),
        reader,
        writer,
        [0; 32],
    ));
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let ping = tokio::time::timeout(Duration::from_secs(1), async {
        let mut header = [0u8; 2];
        client_io.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0] & 0x0f, 0x9, "expected server Ping");
        assert_eq!(header[1] & 0x7f, 8);
        let mut ping = [0u8; 8];
        client_io.read_exact(&mut ping).await.unwrap();
        ping
    })
    .await
    .expect("server Ping deadline");
    let mut wrong = ping;
    wrong[0] ^= 1;
    write_client_pong(&mut client_io, &wrong).await;
    settle().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert_eq!(receive_wire_close(&mut client_io).await.0, 4003);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rv_transport_ws_a3b8161a_matched_pongs_do_not_reset_idle_timeout() {
    let (server_io, mut client_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let (reader, writer) = WsFramer::new(server).split();
    let mut context = Arc::try_unwrap(heartbeat_test_context(Duration::from_secs(2)))
        .ok()
        .unwrap();
    context.idle_timeout = Duration::from_secs(600);
    let task = tokio::spawn(crate::connection::request_loop(
        Arc::new(context),
        reader,
        writer,
        [0; 32],
    ));
    settle().await;
    let mut now = 0u64;
    for interval in 1..=19 {
        let ping_due = interval * 30;
        tokio::time::advance(Duration::from_secs(ping_due - now)).await;
        now = ping_due;
        let ping = tokio::time::timeout(Duration::from_secs(1), async {
            let mut header = [0u8; 2];
            client_io.read_exact(&mut header).await.unwrap();
            assert_eq!(header[0] & 0x0f, 0x9, "expected server Ping");
            assert_eq!(header[1] & 0x7f, 8);
            let mut ping = [0u8; 8];
            client_io.read_exact(&mut ping).await.unwrap();
            ping
        })
        .await
        .expect("server Ping deadline");
        write_client_pong(&mut client_io, &ping).await;
        settle().await;
        let pong_due = now + 1;
        tokio::time::advance(Duration::from_secs(pong_due - now)).await;
        now = pong_due;
        settle().await;
        assert!(!task.is_finished(), "idle at {}s", now);
    }
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert!(!task.is_finished(), "idle deadline still open at 581s");
    tokio::time::advance(Duration::from_secs(19)).await;
    settle().await;
    assert!(task.is_finished(), "idle timeout closes at 600s");
    assert_eq!(receive_wire_close(&mut client_io).await.0, 1000);
    task.await.unwrap();
}

struct HeldHandler {
    started: Arc<Notify>,
    release: Arc<Semaphore>,
}
impl RequestHandler for HeldHandler {
    fn handle<'a>(
        &'a self,
        _session: &'a Session,
        _req: &'a [u8],
        _conn: &'a ConnectionServices,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            self.started.notify_one();
            let permit = self
                .release
                .acquire()
                .await
                .map_err(|_| "test request held")?;
            permit.forget();
            Ok(Vec::new())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn rv_transport_ws_a3b8161a_parked_reader_not_heartbeat_closed() {
    let (server_io, client_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (reader, writer) = WsFramer::new(server).split();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let handler: Arc<dyn RequestHandler> = Arc::new(HeldHandler {
        started: started.clone(),
        release: release.clone(),
    });
    let mut context = Arc::try_unwrap(heartbeat_test_context(Duration::from_secs(2)))
        .ok()
        .unwrap();
    context.handler = handler;
    context.max_in_flight = 1;
    context.idle_timeout = Duration::from_secs(600);
    context.session_store.insert(
        [0x42; 32],
        Session::new(
            [0x11; 16],
            "heartbeat-test".into(),
            SessionPermissions::from_roles(vec!["read_write".into()]),
            TransportKind::WebSocket,
            BindingMode::TlsExporter,
            [0; 32],
            1,
        ),
    );
    let task = tokio::spawn(crate::connection::request_loop(
        Arc::new(context),
        reader,
        writer,
        [0; 32],
    ));
    let request =
        shamir_connect::common::envelope::RequestEnvelope::new([0x42; 32], Some(1), Vec::new());
    let payload = request.to_msgpack().unwrap();
    client
        .send(Message::Binary(wire_frame(&payload).into()))
        .await
        .unwrap();
    started.notified().await;
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert!(
        !task.is_finished(),
        "missed Pong deferred while reader parked"
    );
    tokio::time::advance(Duration::from_secs(10)).await;
    release.add_permits(1);
    settle().await;
    let response = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Binary(body))) => break body,
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("codec error while reading response: {error}"),
                None => panic!("closed before handler response"),
            }
        }
    })
    .await
    .expect("response deadline");
    assert!(!response.is_empty());
    client.send(Message::Close(None)).await.unwrap();
    settle().await;
    assert!(task.is_finished());
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rv_transport_ws_b8ed1366_matched_heartbeat_pong_exempt_during_request_loop() {
    let (server_io, client_io) = duplex(128 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (reader, writer) = WsFramer::new(server).split();
    let mut context = Arc::try_unwrap(heartbeat_test_context(Duration::from_secs(2)))
        .ok()
        .unwrap();
    context.idle_timeout = Duration::from_secs(600);
    context.session_store.insert(
        [0x42; 32],
        Session::new(
            [0x11; 16],
            "heartbeat-test".into(),
            SessionPermissions::from_roles(vec!["read_write".into()]),
            TransportKind::WebSocket,
            BindingMode::TlsExporter,
            [0; 32],
            1,
        ),
    );
    let task = tokio::spawn(crate::connection::request_loop(
        Arc::new(context),
        reader,
        writer,
        [0; 32],
    ));
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    // Reading the Ping queues tungstenite's matching auto-Pong; the first
    // peer Ping below flushes it, so no explicit Pong is sent.
    receive_ping(&mut client).await;
    for _ in 0..64 {
        client.send(Message::Ping(vec![0x5a].into())).await.unwrap();
    }
    settle().await;
    let request =
        shamir_connect::common::envelope::RequestEnvelope::new([0x42; 32], Some(7), Vec::new());
    client
        .send(Message::Binary(
            wire_frame(&request.to_msgpack().unwrap()).into(),
        ))
        .await
        .unwrap();
    settle().await;
    let response = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Binary(body))) => break body,
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("codec error with live heartbeat: {error}"),
                None => panic!("closed with live heartbeat"),
            }
        }
    })
    .await
    .expect("application response deadline");
    assert!(!response.is_empty());
    assert!(
        !task.is_finished(),
        "64 control Pings and matched Pong survive live heartbeat loop"
    );
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert!(!task.is_finished(), "matching Pong keeps heartbeat alive");
    client.send(Message::Close(None)).await.unwrap();
    settle().await;
    task.await.unwrap();
}
#[tokio::test]
async fn rv_transport_ws_b8ed1366_sixty_five_peer_pings_still_hit_control_budget() {
    let (server_io, client_io) = duplex(128 * 1024);
    let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let state = shamir_transport_ws::HeartbeatState::default();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let sender = tokio::spawn(async move {
        for _ in 0..65 {
            client.send(Message::Ping(vec![0x33].into())).await.unwrap();
        }
        client
            .send(Message::Binary(vec![0, 0, 0, 0].into()))
            .await
            .unwrap();
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                incoming = client.next() => match incoming {
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => panic!("peer WS drain failed: {error}"),
                    None => break,
                }
            }
        }
    });
    let result = shamir_transport_ws::framing::ws_recv_into_stream_observing(
        &mut server,
        1024,
        &mut Vec::new(),
        &state,
    )
    .await;
    assert!(matches!(
        result,
        Err(shamir_transport_ws::WsFrameError::ControlFlood { .. })
    ));
    let _ = stop_tx.send(());
    sender.await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn rv_transport_ws_a3b8161a_tcp_does_not_send_heartbeat() {
    let (stream, mut peer) = duplex(64);
    let (reader, writer) = TcpFramer::new(stream).split();
    let mut context = Arc::try_unwrap(heartbeat_test_context(Duration::from_secs(2)))
        .ok()
        .unwrap();
    context.idle_timeout = Duration::from_secs(30);
    let task = tokio::spawn(crate::connection::request_loop(
        Arc::new(context),
        reader,
        writer,
        [0; 32],
    ));
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    task.await.unwrap();

    let mut bytes = Vec::new();
    peer.read_to_end(&mut bytes).await.unwrap();
    assert!(
        bytes.is_empty(),
        "TCP request loop must not emit heartbeat bytes"
    );
}
