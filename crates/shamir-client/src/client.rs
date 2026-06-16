//! Async TLS+SCRAM client with rid-demux multiplexer.
//!
//! After the handshake a background reader task owns the `ReadHalf` and
//! routes every incoming frame to the caller that is waiting for its
//! `request_id` via a `oneshot` channel stored in a pending map.
//!
//! Concurrent callers can issue multiple requests in flight simultaneously:
//! each `execute`/`ping` call registers its oneshot **before** writing to the
//! socket, sends, then awaits the oneshot independently. Responses arrive in
//! completion order (not send order); the reader task matches each by `rid`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::ServerName;
use shamir_collections::THasher;
use tokio::io::{split, AsyncWriteExt, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use zeroize::{Zeroize, Zeroizing};

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::push_envelope::PushEnvelope;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{
    read_frame, read_frame_into, write_frame, FrameError, MAX_FRAME_SIZE_DEFAULT,
};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use crate::error::ClientError;
use crate::interner_cache::InternerCacheRegistry;
use crate::subscription::{
    EarlyBuffer, SubscriptionHandle, SubscriptionMap, CLIENT_SUB_CHANNEL_CAP, EARLY_BUFFER_CAP,
};
use crate::wire_frames::{
    WireAuthInit, WireAuthOk, WireChallenge, WireClientProof, WireResumeInit, WireResumeOk,
};

/// Connection parameters.
pub struct ConnectOptions {
    /// Server address (host:port).
    pub addr: SocketAddr,
    /// SNI hostname for TLS — usually the same hostname the server
    /// generated its self-signed certificate for (`localhost` in tests).
    pub server_name: String,
    /// Username (raw — will be NFC + UsernameCaseMapped normalised).
    pub username: String,
    /// Plaintext password. Zeroized after handshake completes.
    pub password: Zeroizing<Vec<u8>>,
    /// Trust-on-first-use: if `trusted_pin` is `None`, accept whatever
    /// Ed25519 public key the server presents on this first connection.
    /// Set to `false` once the pin is known and persisted.
    pub accept_new_host: bool,
    /// Pre-pinned `SHA256(server_ed25519_pub_key)`. When `Some`, the
    /// SDK validates against this hash and refuses on mismatch (spec
    /// `ServerIdentityChanged`). When `None`, requires
    /// `accept_new_host = true` and captures the pin during handshake;
    /// retrieve it via [`Client::server_pub_key_pin`] for persistence.
    pub trusted_pin: Option<[u8; 32]>,
}

/// Options for fast session resumption via a previously-issued ticket.
///
/// Obtain the ticket and `pinned_hash` from a prior [`Client::connect`] via
/// [`Client::resumption_ticket`] and [`Client::server_pub_key_pin`].
pub struct ResumeOptions {
    /// Server address (host:port).
    pub addr: SocketAddr,
    /// SNI hostname for TLS.
    pub server_name: String,
    /// Resumption ticket bytes obtained from a prior session.
    pub ticket: Vec<u8>,
    /// `SHA256(server_ed25519_pub_key)` pinned during the initial connection.
    /// The resumed session will carry the same pin.
    pub pinned_hash: [u8; 32],
}

/// Result of a demux decode: either a response payload or a transport-level
/// error string, both tagged with the correlation id.
enum DemuxResult {
    /// Success envelope: `(rid, payload_bytes)`.
    Response { rid: Option<u32>, payload: Vec<u8> },
    /// Error envelope: `(rid, error_string)`.
    Error { rid: Option<u32>, error: String },
}

/// Decode one raw frame into a [`DemuxResult`].
///
/// Strategy: try `ResponseEnvelope` first (contains `res` bytes field);
/// on failure try `ErrorEnvelope` (contains `error` string field).
/// If both fail, return `None` so the reader task can log-and-drop.
fn decode_frame(buf: &[u8]) -> Option<DemuxResult> {
    if let Ok(env) = ResponseEnvelope::from_msgpack(buf) {
        return Some(DemuxResult::Response {
            rid: env.request_id,
            payload: env.res,
        });
    }
    if let Ok(env) = ErrorEnvelope::from_msgpack(buf) {
        return Some(DemuxResult::Error {
            rid: env.request_id,
            error: env.error,
        });
    }
    None
}

/// Per-pending-request slot: either resolved with response bytes or with a
/// transport-level error.
pub(crate) type PendingSender = oneshot::Sender<Result<Vec<u8>, ClientError>>;
pub(crate) type PendingMap = Arc<StdMutex<HashMap<u32, PendingSender, THasher>>>;

/// Background reader loop.  Owns `ReadHalf`; demuxes frames to pending waiters.
///
/// On EOF or I/O error: marks `closed`, drains `pending` (sends
/// `ConnectionClosed` to every waiter), then exits.
pub(crate) async fn reader_task<R>(
    mut reader: R,
    pending: PendingMap,
    closed: Arc<AtomicBool>,
    subscriptions: SubscriptionMap,
    early_buffer: EarlyBuffer,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    // T-tcp-1: reuse a single buffer across frames — avoids per-frame heap
    // allocation. Capacity grows to the high-water mark and is never freed
    // until the task exits. Borrow is fully released before every `.await`
    // below that is not `read_frame_into` itself, so no lifetime crosses an
    // unrelated await point.
    let mut frame_buf: Vec<u8> = Vec::with_capacity(4096);
    loop {
        match read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut frame_buf).await {
            Ok(()) => {}
            Err(FrameError::PeerClose) => {
                // Graceful close from server.
                break;
            }
            Err(e) => {
                tracing::debug!("reader_task: read_frame error: {e}");
                break;
            }
        }
        let result = match decode_frame(&frame_buf) {
            Some(r) => r,
            None => {
                // Not a response/error envelope — try push frame.
                if let Ok(envelope) = rmp_serde::from_slice::<PushEnvelope>(&frame_buf) {
                    let sender = {
                        let map = subscriptions.lock().unwrap_or_else(|p| p.into_inner());
                        map.get(&envelope.sub).cloned()
                    };
                    if let Some(tx) = sender {
                        match tx.try_send(envelope) {
                            Ok(()) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Full(env)) => {
                                tracing::warn!(
                                    "client mpsc full for sub={}; dropping push",
                                    env.sub
                                );
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                // Consumer dropped — SubscriptionHandle::Drop has or
                                // will clean up the registry entry.
                            }
                        }
                    } else {
                        let mut buf = early_buffer.lock().unwrap_or_else(|p| p.into_inner());
                        let entry = buf.entry(envelope.sub).or_default();
                        if entry.len() < EARLY_BUFFER_CAP {
                            entry.push(envelope);
                        } else {
                            tracing::debug!(
                                "reader_task: early buffer full for sub={}, dropping",
                                envelope.sub
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        "reader_task: could not decode frame ({} bytes), dropping",
                        frame_buf.len()
                    );
                }
                continue;
            }
        };

        let (rid, outcome) = match result {
            DemuxResult::Response { rid, payload } => (rid, Ok(payload)),
            DemuxResult::Error { rid, error } => (
                rid,
                Err(ClientError::Protocol(format!(
                    "server error envelope: {error}"
                ))),
            ),
        };

        let rid = match rid {
            Some(r) => r,
            None => {
                // Response/error envelope without rid — unusual but harmless.
                tracing::debug!("reader_task: frame without rid, dropping");
                continue;
            }
        };

        let sender = {
            // SAFETY of lock: std::sync::Mutex, no .await while held.
            let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
            map.remove(&rid)
        };

        match sender {
            Some(tx) => {
                // Ignore the error: the waiter may have been cancelled.
                let _ = tx.send(outcome);
            }
            None => {
                // Late response for a cancelled/timed-out request.
                tracing::debug!("reader_task: no waiter for rid={rid}, dropping");
            }
        }
    }

    // Connection is dead — mark closed and drain pending map.
    closed.store(true, Ordering::Release);
    let waiters: Vec<PendingSender> = {
        let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
        map.drain().map(|(_, tx)| tx).collect()
    };
    for tx in waiters {
        let _ = tx.send(Err(ClientError::ConnectionClosed));
    }
}

/// Connected, authenticated client.
///
/// Concurrent calls to `execute`/`ping`/`create_scram_user` are fully
/// supported: multiple requests can be in flight simultaneously on the same
/// connection.  The server may send responses in any order (completion order,
/// not send order); a background reader task correlates each response to its
/// caller via the `rid` (request-id) field of the wire envelopes.
///
/// Each public method registers a `oneshot` receiver in the pending map
/// **before** writing to the socket (avoiding a race with a very fast server),
/// then awaits its own channel independently.
pub struct Client {
    write: tokio::sync::Mutex<WriteHalf<TlsStream<TcpStream>>>,
    session_id: [u8; 32],
    pinned_hash: [u8; 32],
    expires_at_ns: u64,
    /// Bearer credential — held in `Zeroizing` so it is wiped when the
    /// client drops rather than lingering in freed heap.
    resumption_ticket: Option<Zeroizing<Vec<u8>>>,
    resumption_expires_at_ns: Option<u64>,
    next_request_id: AtomicU32,
    /// Outstanding oneshot senders keyed by request_id.
    pending: PendingMap,
    /// Active subscription channels keyed by sub_id.
    subscriptions: SubscriptionMap,
    /// Early-buffered pushes for subs not yet registered via `subscribe_push`.
    early_buffer: EarlyBuffer,
    /// True once the reader task has encountered EOF or I/O error.
    closed: Arc<AtomicBool>,
    /// §B21: JoinHandle is stored so we can abort on close/drop.
    reader_handle: Option<JoinHandle<()>>,
    /// Per-`(db, repo)` interner field-map cache (Stage 5 minimal). Lock-free
    /// registry; populated lazily by `dump_repo`/`touch_fields`. Ids come ONLY
    /// from server `interner_dump`/`interner_touch` responses.
    pub(crate) interner_cache: Arc<InternerCacheRegistry>,
}

impl Client {
    /// Run the full TCP→TLS→SCRAM handshake and return a ready client.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ClientError> {
        // Install rustls crypto provider once. `install_default` is a
        // no-op on second call.
        let _ = default_provider().install_default();

        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;

        // ---- TLS ----
        let client_cfg = make_client_config_no_ca();
        let connector = TlsConnector::from(client_cfg);
        let server_name = ServerName::try_from(opts.server_name.clone())
            .map_err(|e| ClientError::Tls(e.to_string()))?;
        let tcp = TcpStream::connect(opts.addr).await?;
        let tls = connector.connect(server_name, tcp).await?;
        let exporter = extract_tls_exporter(&tls)
            .ok_or_else(|| ClientError::Handshake("TLS exporter unavailable".into()))?;

        // ---- SCRAM ----
        let mut hb = HandshakeBuilder::new(username, TransportKind::Tcp, BindingMode::TlsExporter)
            .tls_exporter(exporter);
        hb = match opts.trusted_pin {
            Some(pin) => hb.pinned_hash(pin),
            None => hb.accept_new_host(opts.accept_new_host),
        };
        let mut hs = hb
            .build()
            .map_err(|e| ClientError::Handshake(e.to_string()))?;

        let (mut r, mut w) = split(tls);

        // Step 1: auth_init
        let init = hs.auth_init();
        let init_wire = WireAuthInit {
            user: init.user,
            client_nonce: init.client_nonce.to_vec(),
            binding_mode: init.binding_mode,
            version: init.version,
        };
        write_frame(&mut w, &rmp_serde::to_vec(&init_wire)?)
            .await
            .map_err(|e| ClientError::Transport(format!("send auth_init: {e}")))?;

        // Step 2: challenge
        let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
            .await
            .map_err(|e| ClientError::Transport(format!("read challenge: {e}")))?;
        let ch_wire: WireChallenge = rmp_serde::from_slice(&ch_bytes)?;
        let salt: [u8; 16] = ch_wire
            .salt
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol(format!("salt size {}", ch_wire.salt.len())))?;
        let server_nonce: [u8; 32] = ch_wire.server_nonce.as_slice().try_into().map_err(|_| {
            ClientError::Protocol(format!("server_nonce size {}", ch_wire.server_nonce.len()))
        })?;
        let challenge = ServerChallenge {
            salt,
            kdf_params: KdfParams {
                memory_kb: ch_wire.memory_kb,
                time: ch_wire.time,
                parallelism: ch_wire.parallelism,
                argon2_version: ch_wire.argon2_version,
            },
            server_nonce,
        };

        // Step 3: derive proof (Argon2id ~50ms-2s — spawn_blocking so
        // it doesn't stall the tokio worker thread).
        // Wrap the working copy in `Zeroizing` so it is wiped on EVERY exit
        // path of the blocking closure (including a `process_challenge`
        // error), not only the success path.
        let mut password_buf = Zeroizing::new(opts.password.to_vec());
        let challenge_clone = challenge.clone();
        let (hs_ret, result) = tokio::task::spawn_blocking(move || {
            let res = hs.process_challenge(&challenge_clone, &mut password_buf);
            (hs, res)
        })
        .await
        .map_err(|e| ClientError::Handshake(format!("Argon2 spawn_blocking: {e}")))?;
        hs = hs_ret;
        let (proof, derived, auth_message) =
            result.map_err(|e| ClientError::Handshake(e.to_string()))?;

        // Step 4: send proof
        let proof_wire = WireClientProof {
            client_proof: proof.to_vec(),
        };
        write_frame(&mut w, &rmp_serde::to_vec(&proof_wire)?)
            .await
            .map_err(|e| ClientError::Transport(format!("send proof: {e}")))?;

        // Step 5: auth_ok
        let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
            .await
            .map_err(|e| ClientError::Transport(format!("read auth_ok: {e}")))?;
        let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes)?;

        let server_signature: [u8; 32] = ok_wire
            .server_signature
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("server_signature size".into()))?;
        let server_pub_key: [u8; 32] = ok_wire
            .server_pub_key
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("server_pub_key size".into()))?;
        let identity_sig: [u8; 64] = ok_wire
            .identity_sig
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("identity_sig size".into()))?;
        let session_id: [u8; 32] = ok_wire
            .session_id
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("session_id size".into()))?;

        let resumption_ticket = if ok_wire.resumption_ticket.is_empty() {
            None
        } else {
            Some(ok_wire.resumption_ticket.clone())
        };
        let resumption_expires_at_ns = if ok_wire.resumption_expires_at_ns > 0 {
            Some(ok_wire.resumption_expires_at_ns)
        } else {
            None
        };

        let auth_ok = ServerAuthOk {
            server_signature,
            server_pub_key,
            identity_sig,
            session_id,
            expires_at_ns: ok_wire.expires_at_ns,
            resumption_ticket: resumption_ticket.clone(),
            resumption_expires_at_ns,
            rotation_in_progress: None,
            kdf_upgrade_required: None,
        };

        // TOFU pin capture: if user supplied trusted_pin we already
        // pre-loaded it; otherwise the callback fires once with the
        // discovered pin.
        let pin_capture: Arc<StdMutex<Option<[u8; 32]>>> =
            Arc::new(StdMutex::new(opts.trusted_pin));
        let pin_for_cb = pin_capture.clone();

        let success = hs
            .process_auth_ok(&auth_ok, &derived, &auth_message, |pin| {
                // §B2 audit: poison-tolerant — the mutex is local to
                // this stack frame, so any poison can only originate
                // from a panic in this scope; recovering the inner
                // value is sound and lets the handshake complete its
                // error path cleanly rather than double-panicking.
                let mut guard = pin_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *guard = Some(*pin);
            })
            .map_err(|e| ClientError::Handshake(e.to_string()))?;

        let pinned_hash = pin_capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .expect("either trusted_pin pre-set or TOFU callback fired");

        // ---- Spawn background reader ----
        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::<_, _, THasher>::default()));
        let subscriptions: SubscriptionMap =
            Arc::new(StdMutex::new(HashMap::<_, _, THasher>::default()));
        let early_buffer: EarlyBuffer =
            Arc::new(StdMutex::new(HashMap::<_, _, THasher>::default()));
        let closed = Arc::new(AtomicBool::new(false));

        // §B21: store JoinHandle — never drop silently.
        let reader_handle = tokio::spawn(reader_task(
            r,
            pending.clone(),
            closed.clone(),
            subscriptions.clone(),
            early_buffer.clone(),
        ));

        Ok(Self {
            write: tokio::sync::Mutex::new(w),
            session_id: success.session_id,
            pinned_hash,
            expires_at_ns: success.expires_at_ns,
            resumption_ticket: resumption_ticket.map(Zeroizing::new),
            resumption_expires_at_ns,
            next_request_id: AtomicU32::new(1),
            pending,
            subscriptions,
            early_buffer,
            closed,
            reader_handle: Some(reader_handle),
            interner_cache: Arc::new(InternerCacheRegistry::new()),
        })
    }

    /// Resume a session using a previously-issued resumption ticket, bypassing
    /// the full Argon2id SCRAM handshake.
    ///
    /// # Flow
    /// 1. Open a new TLS connection (same as `connect`).
    /// 2. Send [`WireResumeInit`] with the ticket, a fresh 32-byte nonce, and
    ///    the TLS-exporter channel-binding mode.
    /// 3. Read [`WireResumeOk`] — the server validates the ticket and responds
    ///    with a new session id and optionally a rotated ticket.
    /// 4. Spawn background reader task; return ready `Client`.
    pub async fn resume(opts: ResumeOptions) -> Result<Self, ClientError> {
        let _ = default_provider().install_default();

        // ---- TLS ----
        let client_cfg = make_client_config_no_ca();
        let connector = TlsConnector::from(client_cfg);
        let server_name = ServerName::try_from(opts.server_name.clone())
            .map_err(|e| ClientError::Tls(e.to_string()))?;
        let tcp = TcpStream::connect(opts.addr).await?;
        let tls = connector.connect(server_name, tcp).await?;
        // Verify TLS exporter is available (required for channel-binding). The
        // exporter value will be used by the server to verify binding; the
        // client sends it implicitly via binding_mode in WireResumeInit.
        let _exporter = extract_tls_exporter(&tls)
            .ok_or_else(|| ClientError::Handshake("TLS exporter unavailable".into()))?;

        // ---- Generate 32-byte client nonce ----
        let mut client_nonce = [0u8; 32];
        {
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut client_nonce);
        }

        let (mut r, mut w) = split(tls);

        // ---- Send ResumeInit ----
        let init_wire = WireResumeInit {
            ticket: opts.ticket,
            client_nonce: client_nonce.to_vec(),
            binding_mode: BindingMode::TlsExporter as u8,
        };
        write_frame(&mut w, &rmp_serde::to_vec(&init_wire)?)
            .await
            .map_err(|e| ClientError::Transport(format!("send resume_init: {e}")))?;

        // ---- Read ResumeOk ----
        let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
            .await
            .map_err(|e| ClientError::Transport(format!("read resume_ok: {e}")))?;
        let ok_wire: WireResumeOk = rmp_serde::from_slice(&ok_bytes)?;

        let session_id: [u8; 32] = ok_wire
            .session_id
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::Protocol("resume: session_id size".into()))?;

        let (resumption_ticket, resumption_expires_at_ns) = if ok_wire.resumption_ticket.is_empty()
        {
            (None, None)
        } else {
            (
                Some(ok_wire.resumption_ticket),
                Some(ok_wire.resumption_expires_at_ns),
            )
        };

        // ---- Spawn background reader ----
        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::<_, _, THasher>::default()));
        let subscriptions: SubscriptionMap =
            Arc::new(StdMutex::new(HashMap::<_, _, THasher>::default()));
        let early_buffer: EarlyBuffer =
            Arc::new(StdMutex::new(HashMap::<_, _, THasher>::default()));
        let closed = Arc::new(AtomicBool::new(false));
        let reader_handle = tokio::spawn(reader_task(
            r,
            pending.clone(),
            closed.clone(),
            subscriptions.clone(),
            early_buffer.clone(),
        ));

        Ok(Self {
            write: tokio::sync::Mutex::new(w),
            session_id,
            pinned_hash: opts.pinned_hash,
            expires_at_ns: ok_wire.expires_at_ns,
            resumption_ticket: resumption_ticket.map(Zeroizing::new),
            resumption_expires_at_ns,
            next_request_id: AtomicU32::new(1),
            pending,
            subscriptions,
            early_buffer,
            closed,
            reader_handle: Some(reader_handle),
            interner_cache: Arc::new(InternerCacheRegistry::new()),
        })
    }

    /// 32-byte session id assigned by the server.
    pub fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    /// `SHA256(server_ed25519_pub_key)` — persist this for subsequent
    /// connections via `ConnectOptions.trusted_pin`.
    pub fn server_pub_key_pin(&self) -> [u8; 32] {
        self.pinned_hash
    }

    /// Absolute session expiry (unix nanoseconds).
    pub fn expires_at_ns(&self) -> u64 {
        self.expires_at_ns
    }

    /// Resumption ticket bytes (if the server issued one). Persist
    /// per-server to skip Argon2id on the next connection.
    pub fn resumption_ticket(&self) -> Option<&[u8]> {
        self.resumption_ticket.as_deref().map(Vec::as_slice)
    }

    /// Resumption expiry (paired with [`Self::resumption_ticket`]).
    pub fn resumption_expires_at_ns(&self) -> Option<u64> {
        self.resumption_expires_at_ns
    }

    /// Register a subscription and get a handle to receive push frames.
    ///
    /// The caller must have already sent a subscribe request to the server
    /// and obtained the `sub_id`. Push frames arriving for this `sub_id`
    /// will be routed to the returned handle.
    pub fn subscribe_push(&self, sub_id: u64) -> SubscriptionHandle {
        let (tx, rx) = tokio::sync::mpsc::channel(CLIENT_SUB_CHANNEL_CAP);
        {
            let mut map = self.subscriptions.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(sub_id, tx.clone());
        }
        // Flush any early-buffered pushes that arrived before registration.
        {
            let mut buf = self.early_buffer.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(buffered) = buf.remove(&sub_id) {
                for envelope in buffered {
                    match tx.try_send(envelope) {
                        Ok(()) => {}
                        Err(tokio::sync::mpsc::error::TrySendError::Full(env)) => {
                            tracing::warn!(
                                "client mpsc full for sub={} while flushing early buffer; dropping push",
                                env.sub
                            );
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            // Receiver gone before we finished flushing — bail.
                            break;
                        }
                    }
                }
            }
        }
        SubscriptionHandle::new(sub_id, rx, self.subscriptions.clone())
    }

    /// Health check.
    pub async fn ping(&self) -> Result<(), ClientError> {
        match self.roundtrip(&DbRequest::Ping).await? {
            DbResponse::Pong => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected Pong, got {other:?}"
            ))),
        }
    }

    /// Execute a [`BatchRequest`] against the named database.
    ///
    /// Ambient interner epoch-delta sync (Stage 5-wire Part A): before sending,
    /// the client advertises its per-repo interner epoch for every distinct
    /// repo the batch targets (so the server can attach a delta); after the
    /// response, any `interner_delta` entries are merged into the cache
    /// (`insert_entry` + `set_epoch` CAS-max). This is transparent — a
    /// backward-compatible server leaves `interner_delta` empty → no-op.
    pub async fn execute(
        &self,
        db: &str,
        mut batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        // BEFORE send: populate interner_epochs from the per-(db,repo) cache.
        // `distinct_repos` walks the batch's data ops' table-refs; admin ops
        // (no table_ref) are skipped. For each repo with a FieldMap, advertise
        // its current epoch so the server can attach a delta.
        if batch.interner_epochs.is_empty() {
            for repo in shamir_query_types::batch::distinct_repos(&batch.queries) {
                let epoch = self.interner_cache().get_or_create(db, &repo).epoch();
                batch.interner_epochs.insert(repo, epoch);
            }
        }

        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.to_string(),
            batch,
        };
        let response = match self.roundtrip(&req).await? {
            DbResponse::Batch { response } => response,
            other => {
                return Err(ClientError::Protocol(format!(
                    "expected Batch, got {other:?}"
                )))
            }
        };

        // AFTER receive: merge interner_delta into the cache. Ids come ONLY
        // from the server (§9.4). The merge mirrors dump/refresh's apply:
        // insert_entry (idempotent) + set_epoch (CAS-max).
        if !response.interner_delta.is_empty() {
            self.merge_interner_delta(db, &response);
        }

        Ok(response)
    }

    /// Create a SCRAM-authenticatable user. Requires the current
    /// session to be a superuser (server enforces). Returns the
    /// stable 16-byte `user_id`.
    pub async fn create_scram_user(
        &self,
        name: &str,
        password: Zeroizing<String>,
        roles: Vec<String>,
    ) -> Result<Vec<u8>, ClientError> {
        let mut req = DbRequest::CreateScramUser {
            name: name.to_string(),
            password: password.as_str().to_owned(),
            roles,
        };
        let result = self.roundtrip(&req).await;
        // Wipe the cleartext password copy placed into the request before it
        // drops. (The caller's `password` is `Zeroizing` and wipes on its own
        // drop; the transient msgpack frame built inside `roundtrip` shares
        // every request's lifecycle and is not separately wiped.)
        if let DbRequest::CreateScramUser { password, .. } = &mut req {
            password.zeroize();
        }
        match result? {
            DbResponse::UserCreated { user_id, .. } => Ok(user_id),
            other => Err(ClientError::Protocol(format!(
                "expected UserCreated, got {other:?}"
            ))),
        }
    }

    /// Send a request and route the response via the rid-demux pending map.
    ///
    /// 1. Allocate rid and register the oneshot **before** writing (no race
    ///    with a fast server response).
    /// 2. Take the write mutex only for the duration of `write_frame`.
    /// 3. Await the oneshot; the reader task delivers the result.
    async fn roundtrip(&self, req: &DbRequest) -> Result<DbResponse, ClientError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ClientError::ConnectionClosed);
        }

        // T-cl-1: thread-local scratch buffer for request serialisation.
        // The buffer's capacity grows to the high-water mark and is reused
        // across calls, avoiding repeated grow-from-0 allocations.
        // The borrow is released before any `.await` — only a sized copy of
        // the bytes is passed forward, keeping the thread_local exclusive.
        thread_local! {
            static REQ_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1024));
        }
        let req_bytes: Vec<u8> = REQ_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            rmp_serde::encode::write_named(&mut *buf, req)?;
            Ok::<Vec<u8>, rmp_serde::encode::Error>(buf.clone())
        })?;

        let rid = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let envelope = RequestEnvelope::new(self.session_id, Some(rid), req_bytes);
        let envelope_bytes = envelope
            .to_msgpack()
            .map_err(|e| ClientError::Protocol(format!("envelope encode: {e}")))?;

        // Register BEFORE writing — avoid a race where the server sends the
        // response before we reach the oneshot registration.
        let (tx, rx) = oneshot::channel();
        {
            // §B2 audit: std::sync::Mutex, no .await while held — fine.
            let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(rid, tx);
        }

        // Send the request.  If write fails, remove the pending entry and
        // propagate the error so the caller is not left waiting on a dead rx.
        {
            let mut write = self.write.lock().await;
            if let Err(e) = write_frame(&mut *write, &envelope_bytes).await {
                // Clean up pending entry.
                let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
                map.remove(&rid);
                return Err(ClientError::Transport(format!("send: {e}")));
            }
        }

        // Await our oneshot.  The reader task delivers Ok(payload) or Err.
        match rx.await {
            Ok(Ok(payload)) => {
                let response: DbResponse = rmp_serde::from_slice(&payload)?;
                if let DbResponse::Error { code, message } = &response {
                    return Err(ClientError::Db {
                        code: code.clone(),
                        message: message.clone(),
                    });
                }
                Ok(response)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // oneshot sender was dropped — reader task died.
                Err(ClientError::ConnectionClosed)
            }
        }
    }

    /// Close the TLS write half cleanly and abort the reader task.
    ///
    /// After this call the session is dead on the server side too
    /// (TCP close → session evicted).
    pub async fn close(mut self) {
        // Abort reader task first — then shut down write half.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        let mut w = self.write.lock().await;
        let _ = AsyncWriteExt::shutdown(&mut *w).await;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // §B21: abort the reader task when the Client drops without an
        // explicit `close()` call so the task does not outlive the Client.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
    }
}
