//! Round-3 handshake-lifetime tests (RV-TRANSPORT-TCP-B5244FA2 +
//! RV-SERVER-17329FC9): paused-clock scripted-framer tests, one real-duplex
//! smoke test, and the real-SCRAM bootstrap-TTL e2e regression.

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use futures_util::{SinkExt, StreamExt};
use shamir_connect::client::handshake::{ClientHandshake, HandshakeBuilder, ServerChallenge};
use shamir_connect::common::crypto::Ed25519Keypair;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{HandlerFuture, RequestHandler};
use shamir_connect::server::lockout::InMemoryLockoutStore;
use shamir_connect::server::rate_limit::InMemoryRateLimiter;
use shamir_connect::server::resume::{InMemoryConsumedCounters, ResumeConfig};
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::{Session, SessionStore};
use shamir_connect::server::user_record::UserRecord;
use shamir_transport_tcp::framing::{read_frame_into, write_frame_into, MAX_FRAME_SIZE_DEFAULT};
use tokio::sync::{mpsc, oneshot};

use crate::connection::wire::{
    AuthInit as WireAuthInit, AuthOk as WireAuthOk, Challenge as WireChallenge,
    ClientProof as WireClientProof,
};
use crate::connection::CLOCK_OVERRIDE_NS;
use crate::connection::{request_loop, ConnTasks};
use crate::framer::{FrameReader, FrameWriter, Framer, FramerError, TcpFramer};
use crate::server::drain_connections;
use crate::server_meta::ServerMetaStore;

// ---------------------------------------------------------------------
// RV-TRANSPORT-TCP-B5244FA2 harness: paused-clock tests with a SCRIPTED
// framer (no real transport, no sleeps — tokio's paused clock advances the
// spec budget deterministically), plus one real-duplex smoke test.
// ---------------------------------------------------------------------

/// Minimal no-op request handler — the handshake tests never reach the
/// request loop.
struct NoopHandler;

impl RequestHandler for NoopHandler {
    fn handle<'a>(
        &'a self,
        _session: &'a Session,
        _req: &'a [u8],
        _conn: &'a ConnectionServices,
    ) -> HandlerFuture<'a> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

/// Default test KDF (listener time = 2 → spec §8 floor budget 15 s).
fn test_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Build a minimal `Arc<ConnectionContext>` with a SHORT `auth_init_timeout`
/// (bounds only the first-frame read) and real SCRAM users — each entry is
/// (`username`, password, stored kdf TIME override) so full proof-carrying
/// handshakes can succeed.
fn handshake_ctx_with_users(
    auth_init_timeout: Duration,
    users: &[(&str, &[u8], u32)],
    tasks: ConnTasks,
) -> Arc<crate::connection::ConnectionContext> {
    let kdf = test_kdf();
    let seed = [0x11u8; 32];
    let identity = ServerIdentityState::from_material(&seed, None, None, 1);
    let keypair = Ed25519Keypair::from_seed(&seed);
    let secrets = Arc::new(ServerSecrets {
        server_secret: [0x22u8; 32],
        lockout_secret: [0x33u8; 32],
    });

    let temp = TempDir::new().expect("tempdir");
    let user_dir = crate::user_directory::FjallUserDirectory::open(temp.path().join("u.redb"))
        .expect("open user dir for test");
    for (username, password, stored_time) in users {
        // Derive the SCRAM record the same way production user creation
        // does, so `verify_proof` accepts a proof from `process_challenge`.
        // `stored_time` overrides the stored kdf TIME: the challenge (and
        // therefore the spec budget) must follow THIS value, not the
        // listener default.
        let stored_kdf = KdfParams {
            time: *stored_time,
            ..kdf
        };
        let derived =
            DerivedKeys::derive(password, &[0x42u8; 16], &stored_kdf).expect("derive scram keys");
        user_dir
            .insert_with_scope(
                username.to_string(),
                UserRecord {
                    salt: [0x42u8; 16],
                    stored_key: derived.stored_key,
                    server_key: derived.server_key,
                    kdf_params: stored_kdf,
                    tickets_invalid_before_ns: 0,
                },
                None,
            )
            .expect("insert test user");
    }
    let meta = Arc::new(
        ServerMetaStore::open_or_init(temp.path().join("meta")).expect("open meta store for test"),
    );
    Box::leak(Box::new(temp));

    let chain = AuditChain::new([0u8; 32]);
    struct NoopAppender;
    impl AuditAppender for NoopAppender {
        fn append_entry(&self, _entry: &AuditEntry) -> Result<(), std::io::Error> {
            Ok(())
        }
        fn checkpoint(&self, _next_seq: u64, _prev_hmac: &[u8; 32]) -> Result<(), std::io::Error> {
            Ok(())
        }
    }
    let appender = Arc::new(NoopAppender) as Arc<dyn AuditAppender>;

    crate::connection::ConnectionContext::new(
        Arc::new(identity),
        keypair,
        secrets,
        kdf,
        Arc::new(SessionStore::new()),
        Arc::new(user_dir),
        Arc::new(InMemoryLockoutStore::new()),
        Arc::new(InMemoryRateLimiter::new(0)),
        Arc::new(Argon2Semaphore::with_capacity(4)),
        Arc::new(AuditChainWriter::new(chain, appender)),
        Arc::new(ResumeConfig::new([0x44u8; 32], None, false, false)),
        Arc::new(InMemoryConsumedCounters::new()),
        Arc::new(NoopHandler),
        BindingMode::TlsExporter,
        TransportKind::Tcp,
        None,
        auth_init_timeout,
        Duration::from_secs(60),
        1,
        meta,
        tasks,
    )
}

pub(super) fn heartbeat_test_context(
    auth_init_timeout: Duration,
) -> Arc<crate::connection::ConnectionContext> {
    handshake_ctx_with_users(
        auth_init_timeout,
        &[],
        ConnTasks::new(tokio_util::sync::CancellationToken::new()),
    )
}

fn handshake_ctx(auth_init_timeout: Duration) -> Arc<crate::connection::ConnectionContext> {
    heartbeat_test_context(auth_init_timeout)
}

#[tokio::test]
async fn rv_transport_ws_b0f1151c_preauth_close_codes() {
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::Role;
    use tokio_tungstenite::WebSocketStream;

    for (message, expected) in [
        (
            tokio_tungstenite::tungstenite::Message::Text("bad".into()),
            1003u16,
        ),
        (
            tokio_tungstenite::tungstenite::Message::Binary(vec![0, 0, 0, 3, 7].into()),
            1002,
        ),
        (
            tokio_tungstenite::tungstenite::Message::Binary({
                let mut bytes = 4097u32.to_be_bytes().to_vec();
                bytes.resize(4 + 4097, 0);
                bytes.into()
            }),
            1009,
        ),
    ] {
        let (server_io, client_io) = duplex(32 * 1024);
        let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let task = tokio::spawn(crate::connection::handle_connection(
            handshake_ctx(Duration::from_secs(2)),
            "127.0.0.1:1234".parse().unwrap(),
            crate::framer::WsFramer::new(server),
            [0; 32],
        ));
        let mut client = client;
        client.send(message).await.unwrap();
        let code = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(Some(frame)))) => {
                        break u16::from(frame.code)
                    }
                    Some(Err(error)) => panic!("client codec error before Close: {error}"),
                    None => panic!("client stream ended before Close"),
                    _ => continue,
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(code, expected);
        task.await.unwrap();
    }
}

#[tokio::test]
async fn rv_transport_ws_b0f1151c_codec_overflow_lifecycle_close_1009() {
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::WebSocketStream;

    let (server_io, client_io) = duplex(32 * 1024);
    let config = WebSocketConfig::default()
        .max_message_size(Some(32))
        .max_frame_size(Some(32));
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, Some(config)).await;
    let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let task = tokio::spawn(crate::connection::handle_connection(
        handshake_ctx(Duration::from_secs(2)),
        "127.0.0.1:1234".parse().unwrap(),
        crate::framer::WsFramer::new(server),
        [0; 32],
    ));
    let mut client = client;
    client
        .send(Message::Binary(vec![0x55; 64].into()))
        .await
        .unwrap();
    let code = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Close(Some(frame)))) => break u16::from(frame.code),
                Some(Err(error)) => panic!("client codec error before Close: {error}"),
                None => panic!("client stream ended before Close"),
                _ => continue,
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(code, 1009);
    task.await.unwrap();
}

struct DeadReader;
impl FrameReader for DeadReader {
    fn read_frame_into<'a>(
        &'a mut self,
        _max: usize,
        _buf: &'a mut Vec<u8>,
    ) -> impl Future<Output = Result<(), FramerError>> + Send + 'a {
        std::future::pending()
    }
}

#[tokio::test]
async fn rv_transport_ws_a3b8161a_tcp_does_not_send_heartbeat() {
    use tokio::io::duplex;

    let (tcp, peer) = duplex(64);
    let framer = TcpFramer::new(tcp);
    let (_, mut writer) = framer.split();
    assert!(writer.heartbeat_state().is_none());
    assert!(writer.send_heartbeat([0; 8]).await.is_err());
    writer.shutdown().await;
    let (reader, _writer) = tokio::io::split(peer);
    drop(reader);
}

struct DeadWriter;
impl FrameWriter for DeadWriter {
    fn write_frame_into<'a>(
        &'a mut self,
        _payload: &'a [u8],
        _scratch: &'a mut Vec<u8>,
    ) -> impl Future<Output = Result<(), FramerError>> + Send + 'a {
        std::future::pending()
    }
    fn shutdown<'a>(&'a mut self) -> impl Future<Output = ()> + Send + 'a {
        std::future::pending()
    }
}

// ---------------------------------------------------------------------------
// Scripted framer: deterministic reads/writes with optional stall points,
// for paused-clock tests (no real transport, no sleeps).
// ---------------------------------------------------------------------------

/// Serves queued frames as reads; the SECOND read (the proof) optionally
/// resolves only after a virtual delay, and shutdown optionally never
/// resolves. Flags record what the production path actually consumed.
struct ScriptedFramer {
    reads: VecDeque<Vec<u8>>,
    /// Virtual delay before the proof read resolves (None = it never does).
    proof_wait: Option<Duration>,
    /// When true, `shutdown` never resolves (backpressured transport).
    shutdown_stalls: bool,
    proof_delivered: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    read_index: usize,
}

impl ScriptedFramer {
    fn new(reads: VecDeque<Vec<u8>>, proof_wait: Option<Duration>, shutdown_stalls: bool) -> Self {
        Self {
            reads,
            proof_wait,
            shutdown_stalls,
            proof_delivered: Arc::new(AtomicBool::new(false)),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            read_index: 0,
        }
    }
}

impl Framer for ScriptedFramer {
    type Reader = DeadReader;
    type Writer = DeadWriter;

    async fn read_frame_into(&mut self, _max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        self.read_index += 1;
        if self.read_index == 2 {
            // The proof read — optionally delayed (virtual time).
            if let Some(wait) = self.proof_wait.take() {
                tokio::time::sleep(wait).await;
            }
        }
        match self.reads.pop_front() {
            Some(frame) => {
                *buf = frame;
                if self.read_index == 2 {
                    self.proof_delivered.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
            // The client never sends another frame AND never closes — the
            // pre-fix server would await this forever.
            None => std::future::pending().await,
        }
    }

    async fn write_frame_into(
        &mut self,
        _payload: &[u8],
        _scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        Ok(())
    }

    async fn shutdown(&mut self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        if self.shutdown_stalls {
            std::future::pending::<()>().await;
        }
    }

    fn split(self) -> (DeadReader, DeadWriter) {
        (DeadReader, DeadWriter)
    }
}

fn auth_init_frame() -> Vec<u8> {
    let init = WireAuthInit {
        user: "nobody".into(),
        client_nonce: vec![0x5A; 32],
        binding_mode: 0x01, // TlsExporter — matches the listener policy
        version: 1,
    };
    rmp_serde::to_vec(&init).expect("encode auth_init")
}

/// Shared client-side handshake state factory (real `process_challenge`
/// machinery — no proof mocking).
fn client_for(user: &str) -> ClientHandshake {
    HandshakeBuilder::new(
        NormalizedUsername::from_raw(user).expect("normalized username"),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .tls_exporter([0u8; 32])
    .accept_new_host(true)
    .build()
    .expect("client handshake")
}

async fn send_client_auth_init(reads_tx: &mpsc::Sender<Vec<u8>>, hs: &ClientHandshake) {
    let init = hs.auth_init();
    let wire_init = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    reads_tx
        .send(rmp_serde::to_vec(&wire_init).expect("encode auth_init"))
        .await
        .expect("server stopped reading before auth_init");
}

/// Receive the server's challenge from the framer's write channel and
/// build the client-side `ServerChallenge` from the WIRE kdf params (the
/// effective ones — what the client actually derives against).
async fn recv_challenge(
    writes_rx: &mut mpsc::Receiver<Vec<u8>>,
) -> (WireChallenge, ServerChallenge) {
    let bytes = writes_rx
        .recv()
        .await
        .expect("challenge frame never arrived");
    let wire: WireChallenge = rmp_serde::from_slice(&bytes).expect("decode challenge");
    let challenge = ServerChallenge {
        salt: wire.salt.clone().try_into().expect("salt is 16 bytes"),
        kdf_params: KdfParams {
            memory_kb: wire.memory_kb,
            time: wire.time,
            parallelism: wire.parallelism,
            argon2_version: wire.argon2_version,
        },
        server_nonce: wire
            .server_nonce
            .clone()
            .try_into()
            .expect("nonce is 32 bytes"),
    };
    (wire, challenge)
}

fn proof_frame(proof: [u8; 32]) -> Vec<u8> {
    rmp_serde::to_vec(&WireClientProof {
        client_proof: proof.to_vec(),
    })
    .expect("encode proof")
}

// ---------------------------------------------------------------------------
// Paused-clock tests over a channel framer: the real client-side proof
// machinery drives a real server handshake; virtual time advances the
// spec budget deterministically (no real sleeps).
// ---------------------------------------------------------------------------

#[test]
fn rv_transport_tcp_d1882068_resume_proof_is_exporter_bound() {
    let identity = Ed25519Keypair::generate();
    let nonce = [0x61; 32];
    let exporter_x = [0x71; 32];
    let exporter_y = [0x72; 32];
    let signed_x = shamir_connect::common::identity::resume_proof_message(&nonce, &exporter_x);
    let signature = identity.sign(&signed_x);
    let verified_x = shamir_connect::common::crypto::ed25519_verify_strict(
        &identity.public_bytes(),
        &signed_x,
        &signature,
    );
    assert!(verified_x);
    let signed_y = shamir_connect::common::identity::resume_proof_message(&nonce, &exporter_y);
    assert!(!shamir_connect::common::crypto::ed25519_verify_strict(
        &identity.public_bytes(),
        &signed_y,
        &signature
    ));
}

/// RV-TRANSPORT-TCP-B5244FA2 — a peer that completes `auth_init`, receives
/// the challenge, and then goes SILENT must be dropped once the spec §8
/// handshake budget elapses.
/// FAILS IF the whole-handshake `tokio::time::timeout(budget, …)` wrap in
/// `run_handshake` is reverted: that wrap is the ONLY timer covering the
/// proof read, so without it the read (and the whole connection) never
/// terminate and this test's outer guard fires.
#[tokio::test(start_paused = true)]
async fn rv_transport_tcp_b5244fa2_silent_peer_after_challenge_is_dropped() {
    let ctx = handshake_ctx(Duration::from_millis(500));

    let mut reads = VecDeque::new();
    reads.push_back(auth_init_frame());
    let framer = ScriptedFramer::new(reads, None, false);
    let peer_addr: std::net::SocketAddr = "127.0.0.1:45001".parse().expect("peer addr");
    let conn_task = tokio::spawn(crate::connection::handle_connection(
        ctx, peer_addr, framer, [0u8; 32],
    ));

    // Paused clock: the 15 s budget (kdf time = 2) elapses deterministically
    // the moment the proof read is the only pending work. The outer guard
    // only fires if NO timer ever bounds the handshake (fix reverted).
    tokio::time::timeout(Duration::from_secs(120), conn_task)
        .await
        .expect("connection task never finished — handshake budget not enforced")
        .expect("connection task panicked");
}

/// RV-TRANSPORT-TCP-B5244FA2 — after the handshake ends (any error path),
/// terminal `shutdown` must be bounded: a backpressured transport whose
/// close never completes must not retain the connection task (and the
/// admission guards that live until it returns).
/// FAILS IF `bounded_shutdown`'s 2 s grace is reverted to a bare
/// `framer.shutdown().await`: the stalled shutdown then never resolves and
/// this test's outer guard fires.
#[tokio::test(start_paused = true)]
async fn rv_transport_tcp_b5244fa2_stalled_shutdown_releases_connection_within_grace() {
    let ctx = handshake_ctx(Duration::from_millis(500));

    let mut reads = VecDeque::new();
    reads.push_back(auth_init_frame());
    // Proof read NEVER resolves + shutdown NEVER resolves.
    let framer = ScriptedFramer::new(reads, None, true);
    let shutdown_requested = Arc::clone(&framer.shutdown_requested);
    let peer_addr: std::net::SocketAddr = "127.0.0.1:45003".parse().expect("peer addr");
    let conn_task = tokio::spawn(crate::connection::handle_connection(
        ctx, peer_addr, framer, [0u8; 32],
    ));

    tokio::time::timeout(Duration::from_secs(120), conn_task)
        .await
        .expect("connection task retained past the shutdown grace (fix reverted)")
        .expect("connection task panicked");
    assert!(
        shutdown_requested.load(Ordering::SeqCst),
        "shutdown must have been attempted before the task finished"
    );
}

/// Bidirectional in-memory framer: reads come from an mpsc channel the
/// test feeds, writes go to an mpsc channel the test asserts on.
struct ChannelFramer {
    reads_rx: mpsc::Receiver<Vec<u8>>,
    writes_tx: mpsc::Sender<Vec<u8>>,
    shutdown_stalls: bool,
}

impl Framer for ChannelFramer {
    type Reader = DeadReader;
    type Writer = DeadWriter;

    async fn read_frame_into(&mut self, _max: usize, buf: &mut Vec<u8>) -> Result<(), FramerError> {
        match self.reads_rx.recv().await {
            Some(frame) => {
                *buf = frame;
                Ok(())
            }
            // All test senders dropped = client closed the connection.
            None => Err(FramerError::PeerClose),
        }
    }

    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        _scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        self.writes_tx
            .send(payload.to_vec())
            .await
            .map_err(|_| FramerError::Io("write side closed".into()))
    }

    async fn shutdown(&mut self) {
        if self.shutdown_stalls {
            std::future::pending::<()>().await;
        }
    }

    fn split(self) -> (DeadReader, DeadWriter) {
        (DeadReader, DeadWriter)
    }
}

struct StalledShutdownWriter {
    entered: Option<oneshot::Sender<()>>,
    _drop_flag: DropFlag,
}

impl FrameWriter for StalledShutdownWriter {
    async fn write_frame_into(
        &mut self,
        _payload: &[u8],
        _scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        Ok(())
    }

    async fn shutdown(&mut self) {
        if let Some(tx) = self.entered.take() {
            let _ = tx.send(());
        }
        std::future::pending::<()>().await;
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// RV-SERVER-FE6B4C98: the post-handshake writer cannot hold shutdown open.
#[tokio::test(start_paused = true)]
async fn rv_server_fe6b4c98_stalled_request_writer_is_aborted_before_shutdown_returns() {
    let drain_token = tokio_util::sync::CancellationToken::new();
    let tasks = ConnTasks::new(drain_token.clone());
    let ctx = handshake_ctx_with_users(Duration::from_secs(1), &[], tasks.clone());
    let (entered_tx, entered_rx) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let writer = StalledShutdownWriter {
        entered: Some(entered_tx),
        _drop_flag: DropFlag(Arc::clone(&dropped)),
    };
    let _conn = tasks.spawn(request_loop(ctx, DeadReader, writer, [7u8; 32]));

    drain_token.cancel();
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("writer shutdown was not reached")
        .expect("writer task dropped before notifying shutdown entry");

    let mut report = shamir_db::ShutdownReport::pending(Vec::new());
    let started = tokio::time::Instant::now();
    let joined = tokio::time::timeout(
        Duration::from_secs(5),
        drain_connections(
            &tasks,
            Duration::from_secs(1),
            Duration::from_secs(1),
            &mut report,
        ),
    )
    .await
    .expect("bounded connection drain timed out");
    let elapsed = started.elapsed();

    assert!(elapsed <= Duration::from_secs(2), "drain took {elapsed:?}");
    assert!(joined, "forced abort must join tracked connection task");
    assert!(
        !report.is_clean(),
        "drain timeout must report unclean shutdown"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "writer future was not dropped"
    );
    assert_eq!(tasks.live(), 0);
}

fn channel_conn(
    ctx: Arc<crate::connection::ConnectionContext>,
    shutdown_stalls: bool,
) -> (
    tokio::task::JoinHandle<()>,
    mpsc::Sender<Vec<u8>>,
    mpsc::Receiver<Vec<u8>>,
) {
    let (reads_tx, reads_rx) = mpsc::channel(16);
    let (writes_tx, writes_rx) = mpsc::channel(16);
    let framer = ChannelFramer {
        reads_rx,
        writes_tx,
        shutdown_stalls,
    };
    let peer_addr: std::net::SocketAddr = "127.0.0.1:45005".parse().expect("peer addr");
    let task = tokio::spawn(crate::connection::handle_connection(
        ctx, peer_addr, framer, [0u8; 32],
    ));
    (task, reads_tx, writes_rx)
}

/// RV-TRANSPORT-TCP-B5244FA2 — the budget follows the EFFECTIVE challenge
/// KDF: listener time = 2 but a STORED user with time = 8 → 40 s budget.
/// A real, CORRECT proof (client-side `process_challenge` against the real
/// challenge) arriving at virtual t = 30 s is ACCEPTED: the server answers
/// with an AuthOk carrying a session id.
/// FAILS IF the budget reverts to the listener-default KDF (time = 2 →
/// 15 s < 30 s): the wrap fires first, the connection drops, and no
/// AuthOk ever arrives — the receive below returns None.
#[tokio::test(start_paused = true)]
async fn rv_transport_tcp_b5244fa2_proof_at_30s_within_effective_40s_budget_is_accepted() {
    let ctx = handshake_ctx_with_users(
        Duration::from_millis(500),
        &[("alice", b"kdf-budget-password", 8)],
        ConnTasks::new(tokio_util::sync::CancellationToken::new()),
    );

    let (conn, reads_tx, mut writes_rx) = channel_conn(ctx, false);
    let hs = client_for("alice");
    send_client_auth_init(&reads_tx, &hs).await;

    // The challenge must advertise the STORED time = 8 (this is what makes
    // the effective budget 40 s, not the 15 s listener floor).
    let (wire, challenge) = recv_challenge(&mut writes_rx).await;
    assert_eq!(wire.time, 8, "challenge must carry the stored kdf time");

    // Virtual t = 30 s — past the 15 s floor, inside the 40 s budget.
    tokio::time::sleep(Duration::from_secs(30)).await;

    let mut pw = b"kdf-budget-password".to_vec();
    let (proof, _derived, _am) = hs
        .process_challenge(&challenge, &mut pw)
        .expect("client proof");
    reads_tx
        .send(proof_frame(proof))
        .await
        .expect("server gone");

    let frame = tokio::time::timeout(Duration::from_secs(120), writes_rx.recv())
        .await
        .expect("auth flow stalled")
        .expect("no AuthOk arrived — connection dropped before the proof was judged");
    let ok: WireAuthOk = rmp_serde::from_slice(&frame).expect("decode auth_ok");
    assert!(
        !ok.session_id.is_empty(),
        "proof at t=30 s must be ACCEPTED under the effective 40 s budget"
    );
    conn.abort();
}

/// RV-TRANSPORT-TCP-B5244FA2 — negative twin: the SAME setup with the
/// proof arriving at virtual t = 41 s, past the 40 s effective budget, is
/// rejected as a timeout (connection drops; no AuthOk is ever written).
/// FAILS IF the effective-KDF budget reverts to something LARGER; it stays
/// green under the listener-default revert (15 s also < 41 s) — the
/// POSITIVE twin above is the discriminating one for that revert.
#[tokio::test(start_paused = true)]
async fn rv_transport_tcp_b5244fa2_proof_at_41s_past_effective_40s_budget_is_rejected() {
    let ctx = handshake_ctx_with_users(
        Duration::from_millis(500),
        &[("alice", b"kdf-budget-password", 8)],
        ConnTasks::new(tokio_util::sync::CancellationToken::new()),
    );

    let (conn, reads_tx, mut writes_rx) = channel_conn(ctx, false);
    let hs = client_for("alice");
    send_client_auth_init(&reads_tx, &hs).await;
    let (_wire, challenge) = recv_challenge(&mut writes_rx).await;

    // Virtual t = 41 s — past the 40 s budget.
    tokio::time::sleep(Duration::from_secs(41)).await;

    let mut pw = b"kdf-budget-password".to_vec();
    let (proof, _derived, _am) = hs
        .process_challenge(&challenge, &mut pw)
        .expect("client proof");
    // The server may already be gone — sending is best-effort.
    let _ = reads_tx.send(proof_frame(proof)).await;

    // All write-senders dropped = the server ended WITHOUT answering.
    assert!(
        writes_rx.recv().await.is_none(),
        "no AuthOk may arrive for a proof past the effective budget"
    );
    conn.abort();
}

/// RV-SERVER-17329FC9 — DETERMINISTIC (injected clock, zero sleeps): the
/// bootstrap token is VALID when authentication starts (the gate's
/// authentication-start clock reads T0) and EXPIRED when the proof arrives
/// (the proof-time clock reads T0 + 6 s > T0 + 5 s TTL). The login must be
/// REJECTED.
///
/// Attribution is exact: the CONTROL scenario in the SAME directory logs
/// `alice_ok` in with the same clock (valid token) and the SAME proof
/// machinery — and is GRANTED a session. The only difference in the main
/// scenario is the clock advance past the TTL between auth start and
/// proof. (Each scenario uses its OWN account and token: a successful
/// bootstrap login ROTATES that account's SCRAM credential to an unknown
/// random password, so reusing an authenticated account would fail on a
/// wrong proof before ever reaching the TTL gate.)
///
/// FAILS IF ONLY THE TTL FIX IS REVERTED (the `Accepted` arm of
/// `run_handshake` stops re-capturing `now_unix_nanos()`): the gate is then
/// evaluated with the authentication-start clock (T0), sees a not-yet-
/// expired token, proceeds, and an AuthOk frame arrives — the EOF
/// expectation below fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_server_17329fc9_bootstrap_token_expiring_between_auth_start_and_proof_is_rejected() {
    let ctx = handshake_ctx_with_users(
        Duration::from_secs(5),
        &[
            ("alice_ok", b"ttl-control-password", 2),
            ("alice_ttl", b"ttl-scenario-password", 2),
        ],
        ConnTasks::new(tokio_util::sync::CancellationToken::new()),
    );
    let meta = Arc::clone(&ctx.meta);

    // Pin the in-handshake clock to a deterministic T0.
    let t0 = UnixNanos::now().as_u64();
    CLOCK_OVERRIDE_NS.store(t0, Ordering::SeqCst);

    let spawn_conn = |ctx: Arc<crate::connection::ConnectionContext>| {
        let (server_io, client) = tokio::io::duplex(64 * 1024);
        let framer = TcpFramer::new(server_io);
        let peer_addr: std::net::SocketAddr = "127.0.0.1:45002".parse().expect("peer addr");
        let task = tokio::spawn(crate::connection::handle_connection(
            ctx, peer_addr, framer, [0u8; 32],
        ));
        (task, client)
    };

    async fn duplex_send_auth_init(
        client: &mut (impl tokio::io::AsyncWrite + Unpin),
        hs: &ClientHandshake,
    ) {
        let init = hs.auth_init();
        let wire_init = WireAuthInit {
            user: init.user,
            client_nonce: init.client_nonce.to_vec(),
            binding_mode: init.binding_mode,
            version: init.version,
        };
        let mut scratch = Vec::new();
        write_frame_into(
            client,
            &rmp_serde::to_vec(&wire_init).expect("encode auth_init"),
            &mut scratch,
        )
        .await
        .expect("write auth_init");
    }

    async fn duplex_read_challenge(
        client: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> ServerChallenge {
        let mut challenge_buf = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            read_frame_into(client, MAX_FRAME_SIZE_DEFAULT, &mut challenge_buf),
        )
        .await
        .expect("challenge never arrived")
        .expect("challenge read failed");
        let wire: WireChallenge = rmp_serde::from_slice(&challenge_buf).expect("decode challenge");
        ServerChallenge {
            salt: wire.salt.try_into().expect("salt is 16 bytes"),
            kdf_params: KdfParams {
                memory_kb: wire.memory_kb,
                time: wire.time,
                parallelism: wire.parallelism,
                argon2_version: wire.argon2_version,
            },
            server_nonce: wire.server_nonce.try_into().expect("nonce is 32 bytes"),
        }
    }

    async fn duplex_send_proof(
        client: &mut (impl tokio::io::AsyncWrite + Unpin),
        hs: &ClientHandshake,
        challenge: &ServerChallenge,
        password: &[u8],
    ) {
        let mut pw = password.to_vec();
        let (proof, _derived, _am) = hs
            .process_challenge(challenge, &mut pw)
            .expect("client proof");
        let frame = rmp_serde::to_vec(&WireClientProof {
            client_proof: proof.to_vec(),
        })
        .expect("encode proof");
        let mut scratch = Vec::new();
        write_frame_into(client, &frame, &mut scratch)
            .await
            .expect("write proof");
    }

    // ----- CONTROL: token valid at BOTH clocks → the same machinery
    // GRANTS a session (this is what makes the main scenario's reject
    // attributable to the TTL advance alone).
    meta.set_bootstrap_token(
        "alice_ok",
        [0xAAu8; 32],
        t0 + 60 * 1_000_000_000,
        PathBuf::from("bootstrap_token_control.txt"),
    )
    .expect("set_bootstrap_token (control)");

    let (task, mut client) = spawn_conn(Arc::clone(&ctx));
    let hs_ok = client_for("alice_ok");
    duplex_send_auth_init(&mut client, &hs_ok).await;
    let challenge = duplex_read_challenge(&mut client).await;
    duplex_send_proof(&mut client, &hs_ok, &challenge, b"ttl-control-password").await;
    let mut ok_buf = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(10),
        read_frame_into(&mut client, MAX_FRAME_SIZE_DEFAULT, &mut ok_buf),
    )
    .await
    .expect("control login never answered");
    assert!(read.is_ok(), "control login was rejected — harness broken");
    let ok: WireAuthOk = rmp_serde::from_slice(&ok_buf).expect("decode auth_ok");
    assert!(
        !ok.session_id.is_empty(),
        "control login must yield a session id"
    );
    drop(client);
    let _ = task.await;

    // ----- MAIN SCENARIO: valid at authentication start (clock T0),
    // expired at proof arrival (clock T0 + 6 s > TTL T0 + 5 s).
    meta.set_bootstrap_token(
        "alice_ttl",
        [0xAAu8; 32],
        t0 + 5 * 1_000_000_000,
        PathBuf::from("bootstrap_token_ttl.txt"),
    )
    .expect("set_bootstrap_token (ttl)");

    let (task, mut client) = spawn_conn(ctx);
    let hs_ttl = client_for("alice_ttl");
    duplex_send_auth_init(&mut client, &hs_ttl).await;
    // Auth start: the gate's authentication-start clock reads T0 — the
    // token (T0 + 5 s) is still valid here.
    let challenge = duplex_read_challenge(&mut client).await;
    // The proof arrives after the clock advanced past the TTL.
    CLOCK_OVERRIDE_NS.store(t0 + 6 * 1_000_000_000, Ordering::SeqCst);
    duplex_send_proof(&mut client, &hs_ttl, &challenge, b"ttl-scenario-password").await;

    // The proof is CORRECT and the credential untouched — only the TTL gate
    // can reject. The next read must hit EOF (shutdown), never an auth_ok.
    let mut buf = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        read_frame_into(&mut client, MAX_FRAME_SIZE_DEFAULT, &mut buf),
    )
    .await
    .expect("server never answered the proof");
    assert!(
        result.is_err(),
        "token expired between auth start and proof must REJECT; a frame          (auth_ok) arrived — TTL fix reverted"
    );

    drop(client);
    let _ = task.await;
}
