//! Handshake entry points: [`Client::connect`] (TCP+TLS+SCRAM),
//! [`Client::connect_local`] (IPC+SCRAM) and [`Client::resume`]
//! (ticket-based resumption).

use std::sync::atomic::{AtomicU32, AtomicU8};
// Ordering is only referenced from the `#[cfg(test)]` test-accessor impl below.
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};

use rand::RngCore;
use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::ServerName;
use tokio::io::split;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::crypto::{ed25519_verify_strict, sha256};
use shamir_connect::common::identity::resume_proof_message;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use crate::client::options::{ConnectLocalOptions, ConnectOptions, ResumeOptions};
use crate::client::transport::{
    connect_ipc, connect_tcp, reader_task, Lifecycle, LifecycleHooks, WriteSink,
};
use crate::client::transport::{
    WireAuthInit, WireAuthOk, WireChallenge, WireClientProof, WireResumeInit, WireResumeOk,
    WireResumeProofInit, WireResumeProofOk,
};
use crate::error::ClientError;
use crate::interner_cache::InternerCacheRegistry;

/// Session metadata shared by every client-construction path; consumed by
/// [`Client::assemble`].
pub(crate) struct SessionMeta {
    pub(crate) session_id: [u8; 32],
    pub(crate) pinned_hash: [u8; 32],
    pub(crate) expires_at_ns: u64,
    pub(crate) resumption_ticket: Option<Zeroizing<Vec<u8>>>,
    pub(crate) resumption_expires_at_ns: Option<u64>,
    pub(crate) server_query_version: u8,
    pub(crate) request_timeout: Option<std::time::Duration>,
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
    pub(super) write: tokio::sync::Mutex<WriteSink>,
    pub(super) session_id: [u8; 32],
    pub(super) pinned_hash: [u8; 32],
    pub(super) expires_at_ns: u64,
    /// Bearer credential — held in `Zeroizing` so it is wiped when the
    /// client drops rather than lingering in freed heap.
    pub(super) resumption_ticket: Option<Zeroizing<Vec<u8>>>,
    pub(super) resumption_expires_at_ns: Option<u64>,
    pub(super) next_request_id: AtomicU32,
    /// The ONE closeable lifecycle state: pending-request registry,
    /// subscription registry (+ early buffer) and the `closed_hint` fast
    /// path. See `transport::Lifecycle`.
    pub(super) state: Lifecycle,
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
    /// Test-only lifecycle hooks (P0 C1). The type is zero-sized in
    /// production; the fields (and this field's readers) are test-only.
    #[cfg(test)]
    pub(super) hooks: LifecycleHooks,
}

impl Client {
    /// Shared finalisation for every construction path (TCP, IPC, resume,
    /// tests): builds the lifecycle state, spawns the reader task with its
    /// shutdown guard, and assembles the [`Client`].
    pub(crate) fn assemble<R>(
        reader: R,
        write_sink: WriteSink,
        meta: SessionMeta,
        hooks: LifecycleHooks,
    ) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let state = Lifecycle::new();
        // §B21: store JoinHandle — never drop silently. The shutdown guard
        // is constructed BEFORE the task can be spawned (or dropped
        // unpolled): every exit path of `reader_task` — including
        // cancellation before the first poll — closes the lifecycle.
        let guard = crate::client::transport::ShutdownGuard::new(state.clone());
        let reader_handle = tokio::spawn(reader_task(reader, state.clone(), guard, hooks.clone()));

        Self {
            write: tokio::sync::Mutex::new(write_sink),
            session_id: meta.session_id,
            pinned_hash: meta.pinned_hash,
            expires_at_ns: meta.expires_at_ns,
            resumption_ticket: meta.resumption_ticket,
            resumption_expires_at_ns: meta.resumption_expires_at_ns,
            next_request_id: AtomicU32::new(1),
            state,
            reader_handle: Some(reader_handle),
            interner_cache: Arc::new(InternerCacheRegistry::new()),
            server_query_version: AtomicU8::new(meta.server_query_version),
            request_timeout: meta.request_timeout,
            #[cfg(test)]
            hooks,
        }
    }

    /// Run the full TCP→TLS→SCRAM handshake and return a ready client.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ClientError> {
        // Install rustls crypto provider once. `install_default` is a
        // no-op on second call.
        let _ = default_provider().install_default();

        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;

        // ---- TLS ----
        let client_cfg = make_client_config_no_ca().map_err(|e| ClientError::Tls(e.to_string()))?;
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

        Ok(Self::assemble(
            r,
            WriteSink::Tls(w),
            SessionMeta {
                session_id: success.session_id,
                pinned_hash,
                expires_at_ns: success.expires_at_ns,
                resumption_ticket: resumption_ticket.map(Zeroizing::new),
                resumption_expires_at_ns,
                server_query_version: ok_wire.server_query_version,
                request_timeout: opts.request_timeout,
            },
            LifecycleHooks::default(),
        ))
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

        Ok(Self::assemble(
            r,
            WriteSink::Ipc(w),
            SessionMeta {
                session_id: success.session_id,
                pinned_hash,
                expires_at_ns: success.expires_at_ns,
                resumption_ticket: resumption_ticket.map(Zeroizing::new),
                resumption_expires_at_ns,
                server_query_version: ok_wire.server_query_version,
                request_timeout: opts.request_timeout,
            },
            LifecycleHooks::default(),
        ))
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
        let client_cfg = make_client_config_no_ca().map_err(|e| ClientError::Tls(e.to_string()))?;
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
        let tls_exporter = extract_tls_exporter(&tls)
            .ok_or_else(|| ClientError::Handshake("TLS exporter unavailable".into()))?;

        let (mut r, mut w) = split(tls);

        let resumed =
            resume_wire_exchange(&mut r, &mut w, opts.ticket, opts.pinned_hash, tls_exporter)
                .await?;

        Ok(Self::assemble(
            r,
            WriteSink::Tls(w),
            SessionMeta {
                session_id: resumed.session_id,
                pinned_hash: opts.pinned_hash,
                expires_at_ns: resumed.expires_at_ns,
                resumption_ticket: resumed.resumption_ticket.map(Zeroizing::new),
                resumption_expires_at_ns: resumed.resumption_expires_at_ns,
                server_query_version: resumed.server_query_version,
                request_timeout: None,
            },
            LifecycleHooks::default(),
        ))
    }
}

/// Result of the post-TLS resume wire exchange: everything
/// [`Client::assemble`] needs to finish a resumed session.
pub(crate) struct ResumedHandshake {
    pub(crate) session_id: [u8; 32],
    pub(crate) expires_at_ns: u64,
    pub(crate) resumption_ticket: Option<Vec<u8>>,
    pub(crate) resumption_expires_at_ns: Option<u64>,
    pub(crate) server_query_version: u8,
}

/// The post-TLS resume wire exchange (transport-agnostic, so tests can run it
/// over duplex pipes with an emulated server): nonce generation, the identity
/// proof preamble send/recv, [`verify_resume_identity_proof`], the
/// [`WireResumeInit`] send and the [`WireResumeOk`] read.
///
/// ORDERING IS LOAD-BEARING (RV-CLIENT-DE75ADDD): the bearer `ticket` is
/// written ONLY after the peer has proven possession of the pinned Ed25519
/// identity over this connection's TLS exporter. A peer without the proof key
/// gets nothing — the caller's write half is left untouched.
pub(crate) async fn resume_wire_exchange<R, W>(
    r: &mut R,
    w: &mut W,
    ticket: Vec<u8>,
    pinned_hash: [u8; 32],
    tls_exporter: [u8; 32],
) -> Result<ResumedHandshake, ClientError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // ---- Generate 32-byte client nonce ----
    let mut client_nonce = [0u8; 32];
    {
        rand::rng().fill_bytes(&mut client_nonce);
    }

    // ---- Identity proof BEFORE ticket disclosure (RV-CLIENT-DE75ADDD) ----
    // The ticket is a bearer credential: it must never be written until
    // the peer has proven possession of the pinned Ed25519 identity over
    // this connection's TLS exporter. A peer without the proof key gets
    // nothing.
    let proof_init = WireResumeProofInit {
        client_nonce: client_nonce.to_vec(),
    };
    write_frame(w, &rmp_serde::to_vec(&proof_init)?)
        .await
        .map_err(|e| ClientError::Transport(format!("send resume_proof: {e}")))?;
    let proof_bytes = read_frame(r, MAX_FRAME_SIZE_DEFAULT)
        .await
        .map_err(|e| ClientError::Transport(format!("read resume_proof_ok: {e}")))?;
    let proof_ok: WireResumeProofOk = rmp_serde::from_slice(&proof_bytes)?;
    verify_resume_identity_proof(&proof_ok, &client_nonce, &tls_exporter, &pinned_hash)?;

    // ---- Send ResumeInit ----
    let init_wire = WireResumeInit {
        ticket,
        client_nonce: client_nonce.to_vec(),
        binding_mode: BindingMode::TlsExporter as u8,
    };
    write_frame(w, &rmp_serde::to_vec(&init_wire)?)
        .await
        .map_err(|e| ClientError::Transport(format!("send resume_init: {e}")))?;

    // ---- Read ResumeOk ----
    let ok_bytes = read_frame(r, MAX_FRAME_SIZE_DEFAULT)
        .await
        .map_err(|e| ClientError::Transport(format!("read resume_ok: {e}")))?;
    let ok_wire: WireResumeOk = rmp_serde::from_slice(&ok_bytes)?;

    let session_id: [u8; 32] = ok_wire
        .session_id
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("resume: session_id size".into()))?;

    let (resumption_ticket, resumption_expires_at_ns) = if ok_wire.resumption_ticket.is_empty() {
        (None, None)
    } else {
        (
            Some(ok_wire.resumption_ticket),
            Some(ok_wire.resumption_expires_at_ns),
        )
    };

    Ok(ResumedHandshake {
        session_id,
        expires_at_ns: ok_wire.expires_at_ns,
        resumption_ticket,
        resumption_expires_at_ns,
        server_query_version: ok_wire.server_query_version,
    })
}

/// Verify the server's resume identity proof (RV-CLIENT-DE75ADDD) — called
/// BEFORE the bearer ticket is written. The proof must come from the pinned
/// key (SHA256(pub) == `pinned_hash`) and carry a strict Ed25519 signature
/// over `RESUME_PROOF_V1 || client_nonce || tls_exporter` with that key, so
/// neither a substitute endpoint nor a replay of a previous proof passes.
pub(crate) fn verify_resume_identity_proof(
    ok: &WireResumeProofOk,
    client_nonce: &[u8; 32],
    tls_exporter: &[u8; 32],
    pinned_hash: &[u8; 32],
) -> Result<(), ClientError> {
    let server_pub_key: [u8; 32] = ok
        .server_pub_key
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("resume proof: server_pub_key size".into()))?;
    let signature: [u8; 64] = ok
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("resume proof: signature size".into()))?;
    if &sha256(&server_pub_key) != pinned_hash {
        return Err(ClientError::Handshake(
            "resume: server identity does not match pinned hash".into(),
        ));
    }
    let msg = resume_proof_message(client_nonce, tls_exporter);
    if !ed25519_verify_strict(&server_pub_key, &msg, &signature) {
        return Err(ClientError::Handshake(
            "resume: server identity proof invalid".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
impl Client {
    /// Test-only constructor (P0 C1): build a client over injected
    /// simplex/duplex pipe halves, going through the SAME
    /// [`Self::assemble`] path (real lifecycle state + reader task).
    /// `reader` is the server→client half; `writer` the client→server half.
    pub(crate) fn for_test<R, W>(reader: R, writer: W, hooks: LifecycleHooks) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        Self::assemble(
            reader,
            WriteSink::Test(Box::new(writer)),
            SessionMeta {
                session_id: [0u8; 32],
                pinned_hash: [0u8; 32],
                expires_at_ns: 0,
                resumption_ticket: None,
                resumption_expires_at_ns: None,
                server_query_version: 0,
                request_timeout: None,
            },
            hooks,
        )
    }

    /// Test accessor: take the reader task's join handle so tests can await
    /// the reader's completion deterministically (no sleeps).
    pub(crate) fn take_reader_handle(&mut self) -> Option<JoinHandle<()>> {
        self.reader_handle.take()
    }

    /// Test accessor: the client's lifecycle state (`pub(super)` fields are
    /// not visible from the crate-root tests module).
    pub(crate) fn lifecycle_for_test(&self) -> &Lifecycle {
        &self.state
    }

    /// Test-only: force the negotiated server query version (e.g. to run the
    /// v2 id-keyed write/read path without a real handshake).
    pub(crate) fn set_server_query_version_for_test(&self, v: u8) {
        self.server_query_version.store(v, Ordering::SeqCst);
    }
}
