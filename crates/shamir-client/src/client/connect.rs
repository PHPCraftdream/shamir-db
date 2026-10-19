//! Handshake entry points: [`Client::connect`] (TCP+TLS+SCRAM),
//! [`Client::connect_local`] (IPC+SCRAM) and [`Client::resume`]
//! (ticket-based resumption).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8};
use std::sync::{Arc, Mutex as StdMutex};

use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::ServerName;
use shamir_collections::TFxMap;
use tokio::io::split;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use crate::client::options::{ConnectLocalOptions, ConnectOptions, ResumeOptions};
use crate::client::subscription::{EarlyBuffer, SubscriptionMap};
use crate::client::transport::{connect_ipc, connect_tcp, reader_task, PendingMap, WriteSink};
use crate::client::transport::{
    WireAuthInit, WireAuthOk, WireChallenge, WireClientProof, WireResumeInit, WireResumeOk,
};
use crate::error::ClientError;
use crate::interner_cache::InternerCacheRegistry;

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
    pub(super) write: tokio::sync::Mutex<WriteSink>,
    pub(super) session_id: [u8; 32],
    pub(super) pinned_hash: [u8; 32],
    pub(super) expires_at_ns: u64,
    /// Bearer credential — held in `Zeroizing` so it is wiped when the
    /// client drops rather than lingering in freed heap.
    pub(super) resumption_ticket: Option<Zeroizing<Vec<u8>>>,
    pub(super) resumption_expires_at_ns: Option<u64>,
    pub(super) next_request_id: AtomicU32,
    /// Outstanding oneshot senders keyed by request_id.
    pub(super) pending: PendingMap,
    /// Active subscription channels keyed by sub_id.
    pub(super) subscriptions: SubscriptionMap,
    /// Early-buffered pushes for subs not yet registered via `subscribe_push`.
    pub(super) early_buffer: EarlyBuffer,
    /// True once the reader task has encountered EOF or I/O error.
    pub(super) closed: Arc<AtomicBool>,
    /// §B21: JoinHandle is stored so we can abort on close/drop.
    pub(super) reader_handle: Option<JoinHandle<()>>,
    /// Per-`(db, repo)` interner field-map cache (Stage 5 minimal). Lock-free
    /// registry; populated lazily by `dump_repo`/`touch_fields`. Ids come ONLY
    /// from server `interner_dump`/`interner_touch` responses.
    pub(crate) interner_cache: Arc<InternerCacheRegistry>,
    /// Max query-language version advertised by the server in `auth_ok` /
    /// `resume_ok`. `0` when the server did not send the field (pre-v2 server).
    /// Set once on connect/resume; never mutated afterward.
    pub(super) server_query_version: AtomicU8,
    /// Per-request response deadline carried over from
    /// [`ConnectOptions::request_timeout`]. `None` = unbounded wait
    /// (prior behaviour). Applied in [`Self::roundtrip`].
    pub(super) request_timeout: Option<std::time::Duration>,
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
        let tcp = connect_tcp(opts.addr, opts.connect_timeout).await?;
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
        let pending: PendingMap = Arc::new(StdMutex::new(TFxMap::default()));
        let subscriptions: SubscriptionMap = Arc::new(StdMutex::new(TFxMap::default()));
        let early_buffer: EarlyBuffer = Arc::new(StdMutex::new(TFxMap::default()));
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
            write: tokio::sync::Mutex::new(WriteSink::Tls(w)),
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
            server_query_version: AtomicU8::new(ok_wire.server_query_version),
            request_timeout: opts.request_timeout,
        })
    }

    /// Run the full IPC→SCRAM handshake (no TLS) over a local Unix domain
    /// socket / Windows Named Pipe and return a ready client. Mirrors
    /// [`Client::connect`] step for step — same SCRAM flow, same
    /// `auth_ok`/resumption-ticket/TOFU-pin handling — only the transport
    /// setup and the `transport_kind`/`binding_mode` fed to
    /// `HandshakeBuilder` differ (spec TRANSPORT_UNIX.md: `binding_mode =
    /// 0x00`, zeroed channel binding — no `.tls_exporter(...)` call needed,
    /// the builder's default is already all-zeros).
    pub async fn connect_local(opts: ConnectLocalOptions) -> Result<Self, ClientError> {
        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;

        // ---- IPC connect (no TLS) ----
        let stream = connect_ipc(&opts.addr, opts.connect_timeout).await?;

        // ---- SCRAM ----
        let mut hb = HandshakeBuilder::new(username, TransportKind::Unix, BindingMode::None);
        hb = match opts.trusted_pin {
            Some(pin) => hb.pinned_hash(pin),
            None => hb.accept_new_host(opts.accept_new_host),
        };
        let mut hs = hb
            .build()
            .map_err(|e| ClientError::Handshake(e.to_string()))?;

        let (mut r, mut w) = split(stream);

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

        // Step 3: derive proof (Argon2id — spawn_blocking, see `connect`).
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

        let pin_capture: Arc<StdMutex<Option<[u8; 32]>>> =
            Arc::new(StdMutex::new(opts.trusted_pin));
        let pin_for_cb = pin_capture.clone();

        let success = hs
            .process_auth_ok(&auth_ok, &derived, &auth_message, |pin| {
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
        let pending: PendingMap = Arc::new(StdMutex::new(TFxMap::default()));
        let subscriptions: SubscriptionMap = Arc::new(StdMutex::new(TFxMap::default()));
        let early_buffer: EarlyBuffer = Arc::new(StdMutex::new(TFxMap::default()));
        let closed = Arc::new(AtomicBool::new(false));

        let reader_handle = tokio::spawn(reader_task(
            r,
            pending.clone(),
            closed.clone(),
            subscriptions.clone(),
            early_buffer.clone(),
        ));

        Ok(Self {
            write: tokio::sync::Mutex::new(WriteSink::Ipc(w)),
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
            server_query_version: AtomicU8::new(ok_wire.server_query_version),
            request_timeout: opts.request_timeout,
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
        // ResumeOptions carries no timeout knobs — resumption preserves the
        // prior unbounded-wait connect behaviour (None).
        let tcp = connect_tcp(opts.addr, None).await?;
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
            rand::rng().fill_bytes(&mut client_nonce);
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
        let pending: PendingMap = Arc::new(StdMutex::new(TFxMap::default()));
        let subscriptions: SubscriptionMap = Arc::new(StdMutex::new(TFxMap::default()));
        let early_buffer: EarlyBuffer = Arc::new(StdMutex::new(TFxMap::default()));
        let closed = Arc::new(AtomicBool::new(false));
        let reader_handle = tokio::spawn(reader_task(
            r,
            pending.clone(),
            closed.clone(),
            subscriptions.clone(),
            early_buffer.clone(),
        ));

        Ok(Self {
            write: tokio::sync::Mutex::new(WriteSink::Tls(w)),
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
            server_query_version: AtomicU8::new(ok_wire.server_query_version),
            request_timeout: None,
        })
    }
}
