//! Per-connection orchestration — TLS accept → optional WS upgrade →
//! pre-Argon2id binding-mode check → rate-limit → SCRAM handshake under
//! Argon2 semaphore + latency padding → lockout register/reset →
//! session insert with per-user cap → request loop with
//! `dispatch_request_view` → 5s grace + audit emit on terminal events.
//!
//! This module wires every security primitive defined elsewhere in
//! `shamir-connect` (lockout, rate_limit, argon2_semaphore,
//! latency, audit_chain, ServerHandshake, dispatch_request_view).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelopeView};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::dispatch::{dispatch_request_view, DispatchOutcome, RequestHandler};
use shamir_connect::server::handshake::{
    AuthInitView, AuthOkView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
use shamir_connect::server::lockout::{
    subnet_of, username_hash, FailureOutcome, LockoutStore, PairKey, BACKOFF_CAP_MS,
};
use shamir_connect::server::rate_limit::{RateDecision, RateLimiter};
use shamir_connect::server::resume::{
    process_resume, ConsumedCounterStore, ResumeConfig, ResumeRequest,
};
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::{
    Session, SessionPermissions, SessionStore, MAX_SESSIONS_PER_USER,
};
use shamir_connect::server::user_record::UserRecord;
use shamir_connect::Error as ConnectError;

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

use crate::framer::{FrameReader, FrameWriter, Framer};
use crate::user_directory::RedbUserDirectory;
use crate::version::check_handshake_proto;

use super::in_flight_guard::InFlightGuard;
use super::user_state_lookup::RedbUserStateLookup;

/// Helper for the auth_attempts_total counter — keeps the result label
/// values consistent across emit sites.
fn record_auth_attempt(result: &'static str) {
    metrics::counter!("auth_attempts_total", "result" => result).increment(1);
}

/// Wire view of `auth_init`, `challenge`, `client_proof`, `auth_ok` —
/// these match the shapes used by the transport-tcp e2e test, kept
/// transport-binding-local.
mod wire {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    pub struct AuthInit {
        pub user: String,
        #[serde(with = "serde_bytes")]
        pub client_nonce: Vec<u8>,
        pub binding_mode: u8,
        pub version: u8,
    }

    #[derive(Serialize, Deserialize)]
    pub struct Challenge {
        #[serde(with = "serde_bytes")]
        pub salt: Vec<u8>,
        pub memory_kb: u32,
        pub time: u32,
        pub parallelism: u32,
        pub argon2_version: u8,
        #[serde(with = "serde_bytes")]
        pub server_nonce: Vec<u8>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct ClientProof {
        #[serde(with = "serde_bytes")]
        pub client_proof: Vec<u8>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct AuthOk {
        #[serde(with = "serde_bytes")]
        pub server_signature: Vec<u8>,
        #[serde(with = "serde_bytes")]
        pub server_pub_key: Vec<u8>,
        #[serde(with = "serde_bytes")]
        pub identity_sig: Vec<u8>,
        #[serde(with = "serde_bytes")]
        pub session_id: Vec<u8>,
        pub expires_at_ns: u64,
        /// Optional resumption ticket — when present, the client may
        /// reconnect later (within the TTL) without re-running Argon2id.
        /// Wire-encoded form per spec §5.4 / SESSION_RESUMPTION.
        #[serde(default, skip_serializing_if = "Vec::is_empty", with = "serde_bytes")]
        pub resumption_ticket: Vec<u8>,
        /// Absolute (unix nanos) expiry of the ticket above. `0` when no
        /// ticket was issued.
        #[serde(default, skip_serializing_if = "is_zero_u64")]
        pub resumption_expires_at_ns: u64,
    }

    /// Helper for `#[serde(skip_serializing_if = ...)]` on the optional
    /// `resumption_expires_at_ns` field.
    pub(super) fn is_zero_u64(v: &u64) -> bool {
        *v == 0
    }

    /// Client → server first frame when attempting a session resume.
    /// Carries the opaque ticket from the previous `auth_ok` plus a fresh
    /// client nonce.
    #[derive(Serialize, Deserialize)]
    pub struct ResumeInit {
        #[serde(with = "serde_bytes")]
        pub ticket: Vec<u8>,
        #[serde(with = "serde_bytes")]
        pub client_nonce: Vec<u8>,
        pub binding_mode: u8,
    }

    /// Server → client response for a successful resume.
    /// A subset of `AuthOk` — the client already has the server's Ed25519
    /// pub-key and signature from the original SCRAM handshake.
    #[derive(Serialize, Deserialize)]
    pub struct ResumeOkWire {
        #[serde(with = "serde_bytes")]
        pub session_id: Vec<u8>,
        pub expires_at_ns: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty", with = "serde_bytes")]
        pub resumption_ticket: Vec<u8>,
        #[serde(default, skip_serializing_if = "is_zero_u64")]
        pub resumption_expires_at_ns: u64,
    }
}

/// Live shared state passed into [`handle_connection`].
pub struct ConnectionContext {
    pub identity: Arc<ServerIdentityState>,
    /// Mirror of the identity keypair (constructed from same seed as
    /// `identity` at boot). `verify_proof` requires `&Ed25519Keypair`.
    identity_keypair_inner: shamir_connect::common::crypto::Ed25519Keypair,
    pub secrets: Arc<ServerSecrets>,
    pub kdf_defaults: KdfParams,
    pub session_store: Arc<SessionStore>,
    pub user_dir: Arc<RedbUserDirectory>,
    pub lockout: Arc<dyn LockoutStore>,
    pub rate_limit: Arc<dyn RateLimiter>,
    pub argon2_sem: Arc<Argon2Semaphore>,
    pub audit: Arc<AuditChainWriter>,
    pub resume_config: Arc<ResumeConfig>,
    pub consumed_counters: Arc<dyn ConsumedCounterStore>,
    pub handler: Arc<dyn RequestHandler>,
    /// Listener-pinned `binding_mode` (0x00 / 0x01 / 0x02) — pre-Argon2id
    /// policy check rejects mismatched client claims.
    pub binding_mode: BindingMode,
    /// Listener-pinned transport kind — encoded into auth_message.
    pub transport_kind: TransportKind,
    /// Listener-pinned KDF override, if any (browser endpoints lower the
    /// floor per `docs/roadmap/BROWSER_WASM_PLAN.md`).
    pub kdf_override: Option<KdfParams>,
    /// Maximum wall-clock time to wait for the client's `auth_init` after
    /// the TLS handshake completes. Defends against slow-loris attacks —
    /// a TLS-accepted client that never sends a frame holds a per-connection
    /// task + buffers indefinitely otherwise. Real clients send `auth_init`
    /// within ~50 ms; the default of 5 s is comfortably above network jitter.
    pub auth_init_timeout: Duration,
    /// Maximum number of requests in-flight concurrently on a single
    /// connection. Controls the per-connection semaphore + writer-channel
    /// capacity in [`request_loop`]. `1` gives lock-step semantics;
    /// default is [`shamir_tunables::instance_defaults::CONN_MAX_IN_FLIGHT`].
    pub max_in_flight: usize,
}

impl ConnectionContext {
    /// Borrow the current Ed25519 keypair for `verify_proof`. Wrapped here
    /// so the call site can stay short and the shamir-connect API stays
    /// keypair-based.
    fn identity_keypair_for_verify(&self) -> &shamir_connect::common::crypto::Ed25519Keypair {
        // ServerIdentityState exposes sign_with_current but not the
        // raw keypair. For this binding we shadow-copy the keypair into
        // ConnectionContext during boot (main.rs constructs both from
        // the same seed). See the field below.
        &self.identity_keypair_inner
    }

    /// Build a ConnectionContext from its fields plus an explicit keypair
    /// reference. The keypair MUST share its seed with `identity` — the
    /// boot path enforces this.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<ServerIdentityState>,
        identity_keypair: shamir_connect::common::crypto::Ed25519Keypair,
        secrets: Arc<ServerSecrets>,
        kdf_defaults: KdfParams,
        session_store: Arc<SessionStore>,
        user_dir: Arc<RedbUserDirectory>,
        lockout: Arc<dyn LockoutStore>,
        rate_limit: Arc<dyn RateLimiter>,
        argon2_sem: Arc<Argon2Semaphore>,
        audit: Arc<AuditChainWriter>,
        resume_config: Arc<ResumeConfig>,
        consumed_counters: Arc<dyn ConsumedCounterStore>,
        handler: Arc<dyn RequestHandler>,
        binding_mode: BindingMode,
        transport_kind: TransportKind,
        kdf_override: Option<KdfParams>,
        auth_init_timeout: Duration,
        max_in_flight: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            identity,
            identity_keypair_inner: identity_keypair,
            secrets,
            kdf_defaults,
            session_store,
            user_dir,
            lockout,
            rate_limit,
            argon2_sem,
            audit,
            resume_config,
            consumed_counters,
            handler,
            binding_mode,
            transport_kind,
            kdf_override,
            auth_init_timeout,
            max_in_flight,
        })
    }
}

/// Top-level entry — drive a single accepted connection through the
/// entire SCRAM handshake + post-handshake request loop.
///
/// Generic over [`Framer`] so the same code path serves both TCP+TLS
/// (`TcpFramer`) and WebSocket (`WsFramer`). Caller is responsible for:
/// - Performing the TLS handshake (and WS upgrade for WS listeners).
/// - Extracting the TLS exporter BEFORE constructing the framer (the
///   raw TLS stream is moved into the framer at construction time).
/// - Choosing the right exporter (`[0u8; 32]` for `binding_mode = 0x00`
///   or `0x02`; real exporter for `0x01`).
/// - Wiring the listener-pinned `binding_mode` into [`ConnectionContext`].
///
/// `peer_addr` is used for subnet derivation (rate-limit / lockout keys).
pub async fn handle_connection<F>(
    ctx: Arc<ConnectionContext>,
    peer_addr: SocketAddr,
    mut framer: F,
    exporter: [u8; 32],
) where
    F: Framer,
{
    let subnet = subnet_of(peer_addr.ip());

    // Pre-handshake: rate-limit per-subnet.
    let now_ns = UnixNanos::now().as_u64();
    match ctx.rate_limit.check(subnet, now_ns) {
        RateDecision::Allowed => {}
        RateDecision::RateLimited { retry_after_secs } => {
            tracing::info!(
                ip_subnet = ?subnet,
                retry_after_secs,
                "rate_limited at accept",
            );
            audit_emit(
                &ctx,
                "rate_limited",
                "<unknown>",
                subnet,
                None,
                "rate_limited",
            );
            record_auth_attempt("rate_limited");
            // Best-effort drop — peer hasn't sent anything yet.
            framer.shutdown().await;
            return;
        }
    }

    // Per-connection scratch buffers (Optim #1 / Optim #7 zero-alloc loop).
    let mut frame_buf: Vec<u8> =
        Vec::with_capacity(shamir_tunables::instance_defaults::IO_FRAME_BUFFER_CAP);
    let mut write_scratch: Vec<u8> =
        Vec::with_capacity(shamir_tunables::instance_defaults::IO_FRAME_BUFFER_CAP);

    // Read the first frame up-front (bounded by auth_init_timeout and the
    // pre-auth 4 KiB ceiling). This frame is then dispatched to either the
    // resume fast-path or the full SCRAM handshake depending on its shape.
    //
    // HIGH-1: MAX_PRE_AUTH_FRAME (4 KiB) — same reasoning as inside
    // run_handshake. An unauthenticated peer must never make the server
    // allocate the post-auth 16 MiB ceiling.
    let read_fut = framer.read_frame_into(MAX_PRE_AUTH_FRAME, &mut frame_buf);
    match tokio::time::timeout(ctx.auth_init_timeout, read_fut).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::debug!(?e, "first frame read failed");
            framer.shutdown().await;
            return;
        }
        Err(_elapsed) => {
            tracing::info!(
                timeout_ms = ctx.auth_init_timeout.as_millis() as u64,
                ?subnet,
                "first frame timeout (slow-loris)",
            );
            framer.shutdown().await;
            return;
        }
    }

    // Determine which auth path to take.
    //
    // A `ResumeInit` has a non-empty `ticket` field. Try to decode it first
    // (cheap msgpack sniff); on any failure fall through to the SCRAM path.
    // Unknown-frame shape → shutdown without leaking timing.
    let is_resume = matches!(rmp_serde::from_slice::<wire::ResumeInit>(&frame_buf), Ok(r) if !r.ticket.is_empty());

    let session_id = if is_resume {
        // Resume fast-path — no latency padding (the ticket is opaque
        // ciphertext; timing reveals nothing about user existence).
        match run_resume(
            &ctx,
            &frame_buf,
            &mut framer,
            &mut write_scratch,
            subnet,
            exporter,
        )
        .await
        {
            Ok(sid) => {
                record_auth_attempt("success");
                sid
            }
            Err(_) => {
                record_auth_attempt("bad_proof");
                framer.shutdown().await;
                return;
            }
        }
    } else {
        // Full SCRAM path — latency padding covers the entire auth flow so
        // negative paths can't be timed (spec §8.5).
        let pad_guard = LatencyPadGuard::start();

        let result = run_handshake(
            &ctx,
            &mut framer,
            &frame_buf,
            &mut write_scratch,
            subnet,
            exporter,
        )
        .await;

        match result {
            Ok(sid) => {
                // Pad on success path too — both paths must be wall-clock equivalent.
                let pad = pad_guard.finish_with_target(target_constant_time_ms());
                if pad > Duration::ZERO {
                    tokio::time::sleep(pad).await;
                }
                record_auth_attempt("success");
                sid
            }
            Err(e) => {
                // Bucket the failure into a coarse counter label. Detailed
                // categorisation (which user / which subnet) lives in the
                // audit log; the counter is for at-a-glance dashboards.
                let label = match &e {
                    HandshakeError::LockedOut => "locked_out",
                    HandshakeError::BadProof { .. } => "bad_proof",
                    HandshakeError::UnknownUser => "unknown_user",
                    HandshakeError::UnsupportedVersion => "unsupported_version",
                    HandshakeError::Policy => "policy",
                    HandshakeError::Io | HandshakeError::Decode => "io_or_decode",
                    HandshakeError::Storage(detail) => {
                        // Storage failure during role lookup — log the cause
                        // (operator-facing only; client sees a generic auth fail).
                        tracing::error!(error = %detail, "handshake storage failure");
                        "storage"
                    }
                };
                record_auth_attempt(label);
                // NEW-2: on a bad `client_proof`, widen the latency pad to the
                // per-pair exponential backoff (spec §5.2.5). The constant-time
                // floor (spec §8.5) remains the lower bound, so the
                // timing-oracle defence is preserved AND the escalating penalty
                // applies: the negative-path response is held for
                // `max(constant_time_floor, backoff_ms)`. All other failure
                // variants carry no backoff → pad with the floor only.
                let backoff_ms = match &e {
                    HandshakeError::BadProof { backoff_ms } => *backoff_ms,
                    _ => 0,
                };
                let target_ms = target_constant_time_ms().max(backoff_ms);
                let pad = pad_guard.finish_with_target(target_ms);
                if pad > Duration::ZERO {
                    tokio::time::sleep(pad).await;
                }
                framer.shutdown().await;
                return;
            }
        }
    };

    // Split the framer into directional halves — the duplex request loop
    // (M1) drives reading and writing from independent tasks.
    let (reader, writer) = framer.split();

    request_loop(ctx, reader, writer, session_id).await;

    // The writer task handles its own shutdown() call before exiting;
    // nothing left to do on this path.
}

#[derive(Debug)]
enum HandshakeError {
    Io,
    Decode,
    Policy,
    /// Client sent an invalid `client_proof`. Carries the per-pair
    /// exponential backoff (spec §5.2.5 NORMATIVE) returned by
    /// [`LockoutStore::register_failure`] so the negative-path latency pad
    /// can be widened to `max(constant_time_floor, backoff_ms)` — i.e. the
    /// Nth failure from a `(subnet, username_hash)` pair is delayed
    /// `100ms × 2^N` (capped 30s) before its response is released (NEW-2).
    BadProof {
        /// Backoff delay in milliseconds dictated by the failure count for
        /// this pair. `0` only if the store reports no backoff.
        backoff_ms: u64,
    },
    UnknownUser,
    LockedOut,
    /// Client requested a handshake-protocol version this server does not
    /// implement. Fast-rejected before any Argon2id work.
    UnsupportedVersion,
    /// Transient storage failure during role lookup (§C2: must not silently
    /// downgrade to empty roles).
    Storage(String),
}

/// Session-resume fast-path (SESSION_RESUMPTION §5).
///
/// Called when the first frame decodes as a [`wire::ResumeInit`] with a
/// non-empty `ticket`. Calls [`process_resume`] from `shamir-connect`,
/// inserts the new session, and sends a [`wire::ResumeOkWire`] response.
///
/// No latency padding is applied — the ticket is opaque AES-256-GCM
/// ciphertext, so timing the response cannot reveal user existence.
async fn run_resume<F: Framer>(
    ctx: &ConnectionContext,
    first_frame: &[u8],
    framer: &mut F,
    write_scratch: &mut Vec<u8>,
    _subnet: shamir_connect::server::lockout::Subnet,
    exporter: [u8; 32],
) -> Result<[u8; 32], HandshakeError> {
    let init: wire::ResumeInit =
        rmp_serde::from_slice(first_frame).map_err(|_| HandshakeError::Decode)?;

    if init.client_nonce.len() != 32 {
        return Err(HandshakeError::Decode);
    }
    let mut client_nonce = [0u8; 32];
    client_nonce.copy_from_slice(&init.client_nonce);

    let binding_mode =
        BindingMode::from_u8(init.binding_mode).map_err(|_| HandshakeError::Policy)?;

    const RESUMPTION_TICKET_TTL_NS: u64 = 24 * shamir_connect::common::time::ns::HOUR;

    let now_ns = UnixNanos::now().as_u64();
    let user_lookup = RedbUserStateLookup(&ctx.user_dir);

    let ok = process_resume(
        &ResumeRequest {
            ticket_wire_bytes: &init.ticket,
            client_nonce,
            binding_mode_now: binding_mode,
            channel_binding_now: exporter,
        },
        &ctx.resume_config,
        ctx.consumed_counters.as_ref(),
        &user_lookup,
        &ctx.session_store,
        &ctx.identity,
        SESSION_MAX_AGE_NS,
        RESUMPTION_TICKET_TTL_NS,
        now_ns,
    )
    .map_err(|_| HandshakeError::BadProof { backoff_ms: 0 })?;

    // Build and send ResumeOkWire.
    let wire_ok = wire::ResumeOkWire {
        session_id: ok.session_id.to_vec(),
        expires_at_ns: ok.expires_at_ns,
        resumption_ticket: ok.resumption_ticket.unwrap_or_default(),
        resumption_expires_at_ns: ok.resumption_expires_at_ns.unwrap_or(0),
    };
    let bytes = rmp_serde::to_vec(&wire_ok).map_err(|_| HandshakeError::Io)?;
    framer
        .write_frame_into(&bytes, write_scratch)
        .await
        .map_err(|_| HandshakeError::Io)?;

    Ok(ok.session_id)
}

/// Run the full SCRAM-Argon2id handshake.
///
/// `first_frame` is the already-read and buffered first frame (auth_init
/// bytes). The frame was read by `handle_connection` before bifurcating
/// between the resume and full-auth paths; passing it here avoids a
/// second read.
async fn run_handshake<F: Framer>(
    ctx: &ConnectionContext,
    framer: &mut F,
    first_frame: &[u8],
    write_scratch: &mut Vec<u8>,
    subnet: shamir_connect::server::lockout::Subnet,
    exporter: [u8; 32],
) -> Result<[u8; 32], HandshakeError> {
    // 1. Decode auth_init from the already-read first frame.
    //    HIGH-1: the frame was already bounded to MAX_PRE_AUTH_FRAME (4 KiB)
    //    by the caller — no second size check needed here.
    let init: wire::AuthInit = match rmp_serde::from_slice(first_frame) {
        Ok(v) => v,
        Err(_) => return Err(HandshakeError::Decode),
    };
    // Version dispatch — fast reject on unsupported versions BEFORE any
    // Argon2id work or username lookup. Hardcoded list lives in
    // `crate::version::SUPPORTED_HANDSHAKE_PROTO_VERSIONS`.
    if let Err(e) = check_handshake_proto(init.version) {
        tracing::info!(
            requested = init.version,
            err = %e,
            "handshake rejected: unsupported protocol version",
        );
        return Err(HandshakeError::UnsupportedVersion);
    }
    if init.client_nonce.len() != 32 {
        return Err(HandshakeError::Decode);
    }
    let mut client_nonce = [0u8; 32];
    client_nonce.copy_from_slice(&init.client_nonce);
    let username = match NormalizedUsername::from_raw(&init.user) {
        Ok(u) => u,
        Err(_) => return Err(HandshakeError::Decode),
    };
    let binding_mode = match BindingMode::from_u8(init.binding_mode) {
        Ok(b) => b,
        Err(_) => return Err(HandshakeError::Policy),
    };

    // 2. Lockout pre-check (silent reject — same wire as auth_failed).
    let now_ns = UnixNanos::now().as_u64();
    let uhash = username_hash(&ctx.secrets.lockout_secret, username.as_bytes());
    let pair: PairKey = (subnet, uhash);
    if ctx.lockout.is_locked_out(pair, now_ns) {
        tracing::info!(user = %username.as_str(), "locked_out at auth_init");
        audit_emit(
            ctx,
            "auth_failed",
            username.as_str(),
            subnet,
            None,
            "locked_out",
        );
        return Err(HandshakeError::LockedOut);
    }

    // 3. Construct ServerHandshake — runs pre-Argon2id binding_mode check.
    let kdf = ctx.kdf_override.unwrap_or(ctx.kdf_defaults);
    let user_dir = ctx.user_dir.clone();
    let username_for_lookup = username.clone();
    let lookup_user = move |u: &NormalizedUsername| -> Option<UserRecord> {
        // Always-equal-cost lookup: real-or-fake. The shamir-connect
        // `verify_proof` will internally derive a FakeBlob if we return
        // None.
        let _ = &username_for_lookup; // borrow used for clone above
        user_dir.lookup_by_name(u.as_str())
    };
    let auth_init_view = AuthInitView {
        user: username.clone(),
        client_nonce,
        binding_mode,
        version: init.version,
    };
    let listener_policy = shamir_connect::server::config::ListenerPolicy::new(ctx.binding_mode);
    let hs = match ServerHandshake::new(
        listener_policy,
        ctx.transport_kind,
        &ctx.secrets,
        auth_init_view,
        exporter,
        kdf,
        lookup_user,
    ) {
        Ok(h) => h,
        Err(_) => return Err(HandshakeError::Policy),
    };

    // 4. Send challenge.
    let ch = hs.challenge();
    let bytes = match rmp_serde::to_vec(&wire::Challenge {
        salt: ch.salt.to_vec(),
        memory_kb: ch.kdf_params.memory_kb,
        time: ch.kdf_params.time,
        parallelism: ch.kdf_params.parallelism,
        argon2_version: ch.kdf_params.argon2_version,
        server_nonce: ch.server_nonce.to_vec(),
    }) {
        Ok(b) => b,
        Err(_) => return Err(HandshakeError::Decode),
    };
    if framer
        .write_frame_into(&bytes, write_scratch)
        .await
        .is_err()
    {
        return Err(HandshakeError::Io);
    }

    // 5. Read client_proof (Argon2id ran on the client side, ~2s).
    //    Still pre-auth → `MAX_PRE_AUTH_FRAME` ceiling (HIGH-1). Real
    //    proof bytes are ~40; the 16 MiB post-auth ceiling MUST NOT be
    //    reachable before the proof is verified.
    let mut proof_frame_buf = Vec::new();
    if framer
        .read_frame_into(MAX_PRE_AUTH_FRAME, &mut proof_frame_buf)
        .await
        .is_err()
    {
        return Err(HandshakeError::Io);
    }
    let proof_msg: wire::ClientProof = match rmp_serde::from_slice(&proof_frame_buf) {
        Ok(v) => v,
        Err(_) => return Err(HandshakeError::Decode),
    };
    if proof_msg.client_proof.len() != 32 {
        return Err(HandshakeError::Decode);
    }
    let mut proof = [0u8; 32];
    proof.copy_from_slice(&proof_msg.client_proof);

    // 6. Verify the proof. NB: the proof-verify path runs NO Argon2id —
    // only FakeBlob HKDF + HMAC + Ed25519 sign (all µs-cheap). It is
    // therefore NOT gated by the Argon2 KDF semaphore. The previous code
    // took a non-blocking `try_acquire` here and REJECTED on contention,
    // which dropped legitimate concurrent logins the moment more than
    // `argon2_concurrent_max` clients reached verify at once — an
    // availability bug surfacing as `authentication_failed`/early-eof, not
    // real DoS protection (the verify does no expensive work). Burst is
    // already bounded by the global connection cap (`conn_limiter`) and the
    // per-subnet `auth_init` rate limit. `ctx.argon2_sem` stays reserved
    // for gating genuine server-side Argon2id (e.g. user-creation key
    // derivation), which this path does not perform. Identity keypair lives behind ServerIdentityState.
    // We need a concrete Ed25519Keypair to pass; ServerIdentityState owns
    // the keypair internally. For this v1 we use sign_with_current via
    // build_identity_input duplicate path — but verify_proof needs a
    // keypair ref. We work around via ServerIdentityState::sign_with_current
    // by NOT calling verify_proof directly.
    //
    // Cleaner: shamir-connect's `verify_proof` accepts &Ed25519Keypair.
    // ServerIdentityState exposes `sign_with_current` but not the keypair
    // directly. To bridge, we need a way; for v1 we use the existing API
    // by holding the keypair in ConnectionContext directly. To avoid
    // refactoring shamir-connect, the cleanest path is to keep an
    // Arc<Ed25519Keypair> alongside the ServerIdentityState, OR to do
    // the verify ourselves. For now: hold keypair in ctx as an extra
    // field. (See ConnectionContext doc.)
    //
    // For this implementation we'll keep things simple: after-handshake
    // identity_sig is generated INSIDE verify_proof from the keypair
    // we pass. But we don't have direct keypair access from
    // ServerIdentityState. So we bypass by using a workaround: extract
    // current pub key and sign manually using sign_with_current.
    //
    // Cleanest minimal fix: add an extra field to ConnectionContext —
    // `pub identity_keypair: Arc<Ed25519Keypair>` set up at boot
    // alongside ServerIdentityState. This duplicates the keypair
    // reference but is simplest.
    let outcome = match hs.verify_proof(
        &proof,
        ctx.identity_keypair_for_verify(),
        SESSION_MAX_AGE_NS,
    ) {
        Ok(o) => o,
        // Internal verify error (crypto failure, not an attacker's wrong
        // guess) — surface as a generic auth failure with no per-pair
        // backoff (we did not `register_failure`), so the floor pad applies.
        Err(_) => return Err(HandshakeError::BadProof { backoff_ms: 0 }),
    };

    let auth_ok: AuthOkView = match outcome {
        ProofOutcome::Accepted(ok) => *ok,
        ProofOutcome::Rejected => {
            // Register the failure and APPLY the resulting per-pair
            // exponential backoff (spec §5.2.5 NORMATIVE, NEW-2). The
            // returned delay is plumbed out via `HandshakeError::BadProof`
            // and folded into the negative-path latency pad as
            // `max(constant_time_floor, backoff_ms)` by `handle_connection`,
            // so the response to THIS failed attempt is held long enough to
            // pace the attacker (100ms × 2^N, capped 30s).
            //
            // Safety (no user-existence oracle): `register_failure` is keyed
            // by `(subnet, username_hash)` and `ProofOutcome::Rejected` is
            // reached identically for a real user with a wrong password and
            // for a non-existent user (shamir-connect verifies against an
            // internally-derived FakeBlob). The backoff depends only on the
            // failure count for the pair, never on whether the user exists —
            // so widening the pad to the backoff leaks nothing the flat
            // constant-time pad didn't already cover.
            let backoff_ms = match ctx.lockout.register_failure(pair, now_ns) {
                FailureOutcome::Backoff { delay_ms } => delay_ms,
                // Threshold crossed → pair is now locked out. Treat this
                // final response as the maximum backoff (the escalation
                // endpoint; `FailureState::backoff_ms` saturates here too).
                FailureOutcome::LockedOut => BACKOFF_CAP_MS,
            };
            tracing::info!(user = %username.as_str(), "auth_failed: bad proof");
            audit_emit(
                ctx,
                "auth_failed",
                username.as_str(),
                subnet,
                None,
                "bad_proof",
            );
            return Err(HandshakeError::BadProof { backoff_ms });
        }
    };

    // Reset lockout on success per spec §5.2.5 NORMATIVE.
    ctx.lockout.reset_on_success(pair);

    // 8. Build session, insert with per-user cap, send auth_ok.
    let user_id = match ctx.user_dir.user_id(username.as_str()) {
        Some(id) => id,
        None => return Err(HandshakeError::UnknownUser),
    };
    let roles = match ctx.user_dir.lookup_roles(username.as_str()) {
        Ok(Some(r)) => r,
        Ok(None) => Vec::new(),
        Err(e) => return Err(HandshakeError::Storage(format!("lookup_roles: {e}"))),
    };
    let session = Session::new(
        user_id,
        username.as_str().to_string(),
        SessionPermissions::from_roles(roles),
        ctx.transport_kind,
        binding_mode,
        exporter,
        now_ns,
    );
    let (_arc, evicted) = ctx.session_store.insert_with_per_user_cap(
        auth_ok.session_id,
        session,
        MAX_SESSIONS_PER_USER,
    );
    if let Some(victim_sid) = evicted {
        audit_emit(
            ctx,
            "session_evicted",
            username.as_str(),
            subnet,
            Some(&prefix_8(&victim_sid)),
            "max_sessions_lru",
        );
    }
    audit_emit(
        ctx,
        "auth_success",
        username.as_str(),
        subnet,
        Some(&prefix_8(&auth_ok.session_id)),
        "ok",
    );

    // Spec §5.4 / SESSION_RESUMPTION: issue a fresh resumption ticket so
    // the client can reconnect without re-running Argon2id. TTL = 24h
    // matches the session max-age default in `SchedulerInputs::session_max_age_ns`.
    const RESUMPTION_TICKET_TTL_NS: u64 = 24 * shamir_connect::common::time::ns::HOUR;
    let roles_for_ticket = match ctx.user_dir.lookup_roles(username.as_str()) {
        Ok(Some(r)) => r,
        Ok(None) => Vec::new(),
        Err(e) => {
            return Err(HandshakeError::Storage(format!(
                "lookup_roles (ticket): {e}"
            )))
        }
    };
    let (ticket_bytes, ticket_expires_at_ns) =
        match shamir_connect::server::resume::issue_initial_ticket(
            &ctx.resume_config.ticket_key,
            user_id,
            username.as_str().to_string(),
            ctx.transport_kind.as_u8(),
            binding_mode.as_u8(),
            exporter,
            roles_for_ticket,
            ctx.identity.current_version(),
            now_ns,
            RESUMPTION_TICKET_TTL_NS,
        ) {
            Ok(t) => t,
            Err(e) => {
                // Issuing the ticket is best-effort — a failure must not
                // tank a successful auth. Log and continue with no ticket.
                tracing::warn!(
                    ?e,
                    "resumption ticket issuance failed; auth_ok will carry no ticket"
                );
                (Vec::new(), 0u64)
            }
        };

    let bytes = match rmp_serde::to_vec(&wire::AuthOk {
        server_signature: auth_ok.server_signature.to_vec(),
        server_pub_key: auth_ok.server_pub_key.to_vec(),
        identity_sig: auth_ok.identity_sig.to_vec(),
        session_id: auth_ok.session_id.to_vec(),
        expires_at_ns: auth_ok.expires_at_ns,
        resumption_ticket: ticket_bytes,
        resumption_expires_at_ns: ticket_expires_at_ns,
    }) {
        Ok(b) => b,
        Err(_) => return Err(HandshakeError::Io),
    };
    if framer
        .write_frame_into(&bytes, write_scratch)
        .await
        .is_err()
    {
        return Err(HandshakeError::Io);
    }

    Ok(auth_ok.session_id)
}

// ---------------------------------------------------------------------------
// Duplex request loop — M1
//
// Architecture overview:
//
//   ┌──────────────┐       mpsc (cap=max_in_flight)       ┌─────────────┐
//   │  Reader task │──── WriterMsg::{Reply,AndClose} ────►│ Writer task │
//   │  (this task) │                                       │  (spawned)  │
//   └──────────────┘                                       └─────────────┘
//         │                                                       │
//         │  Semaphore (max_in_flight permits)                    │
//         │  JoinSet<()>  (one entry per in-flight request)       │
//         └────────────────────────────────────────────────────────┘
//
// Back-pressure chain:
//   1. Semaphore exhausted → reader blocks on `acquire_owned()` → no new
//      frames read.
//   2. Writer channel full → dispatch tasks block on `tx.send()` → permits
//      held → semaphore exhausted → reader stalls.
//
// Reply ordering:
//   Replies arrive in dispatch-completion order (not wire order). Clients
//   must correlate by `request_id` (rid). `max_in_flight = 1` gives
//   lock-step ordering identical to the old sequential loop.
//
// Teardown on any exit path:
//   - `join_set.abort_all()` cancels in-flight dispatch tasks.
//   - `tx` (Sender) is dropped, closing the channel.
//   - Writer task sees channel closed → calls `writer.shutdown().await` and
//     exits. `writer_handle.await` waits for that to complete.
// ---------------------------------------------------------------------------

/// Message sent from dispatch tasks to the writer task.
enum WriterMsg {
    /// Write these bytes and keep running.
    Reply(Vec<u8>),
    /// Write these bytes, then shut down the connection.
    ReplyAndClose(Vec<u8>),
}

/// Duplex post-handshake request loop for a single connection.
///
/// # Duplex model
///
/// After a successful SCRAM handshake the connection enters this loop which
/// drives reading and writing from two independent tasks:
///
/// * **Reader loop** (this task): reads frames from the client, acquires a
///   semaphore permit, and spawns a per-request dispatch task into a
///   `JoinSet`. Back-pressure: when `max_in_flight` permits are exhausted
///   the reader blocks and no new frames are accepted.
///
/// * **Writer task** (spawned): owns the write half of the framer. Receives
///   `WriterMsg::{Reply, ReplyAndClose}` over a bounded `mpsc` channel
///   (capacity = `max_in_flight`). Writes frames in receipt order; on
///   `ReplyAndClose` writes the final frame and shuts down.
///
/// * **Dispatch tasks** (one per request, in `JoinSet`): each owns the
///   frame bytes (fresh `Vec` — no per-connection buffer reuse on this
///   path), an `Arc<ConnectionContext>`, an `OwnedSemaphorePermit` (released
///   on task completion), and a clone of the `mpsc::Sender`. Each task
///   deserialises the envelope, runs `dispatch_request_view`, serialises
///   the response, and sends it to the writer.
///
/// # Reply ordering
///
/// Replies are written in dispatch-completion order, which is *not*
/// necessarily wire-arrival order. Clients must match responses to requests
/// by `request_id` (rid). Setting `max_in_flight = 1` reproduces the old
/// lock-step behaviour with strict ordering.
///
/// # Teardown
///
/// Any exit cause (client EOF, writer death, panic in dispatch) triggers:
/// 1. `join_set.abort_all()` — cancel in-flight tasks.
/// 2. Drop `tx` — signal writer task that no more messages are coming.
/// 3. `writer_handle.await` — wait for the writer to flush and shut down.
pub async fn request_loop<R, W>(
    ctx: Arc<ConnectionContext>,
    mut reader: R,
    writer: W,
    sid: [u8; 32],
) where
    R: FrameReader + 'static,
    W: FrameWriter + 'static,
{
    let cap = ctx.max_in_flight.max(1);
    let semaphore = Arc::new(Semaphore::new(cap));
    let (tx, mut rx) = mpsc::channel::<WriterMsg>(cap);

    // Shared flag: set to true by a dispatch task that sends ReplyAndClose.
    // The reader loop checks this flag and stops accepting new frames.
    let close_requested = Arc::new(AtomicBool::new(false));

    // --- Writer task ---------------------------------------------------------
    // Owns the write half; receives replies over mpsc; shuts down on
    // channel-close or ReplyAndClose. §B21: JoinHandle is always awaited.
    let mut writer_handle = tokio::spawn(async move {
        let mut writer = writer;
        let mut scratch =
            Vec::with_capacity(shamir_tunables::instance_defaults::IO_FRAME_BUFFER_CAP);
        loop {
            match rx.recv().await {
                None => {
                    // Channel closed (all senders dropped) — clean exit.
                    break;
                }
                Some(WriterMsg::Reply(bytes)) => {
                    // DEFECT C fix: on write error (broken pipe / dead client)
                    // break immediately so the JoinHandle resolves and the
                    // reader's select! branch wakes up (Defect B fix).
                    if writer.write_frame_into(&bytes, &mut scratch).await.is_err() {
                        break;
                    }
                }
                Some(WriterMsg::ReplyAndClose(bytes)) => {
                    // Write error here is ignored — we're closing anyway.
                    let _ = writer.write_frame_into(&bytes, &mut scratch).await;
                    break;
                }
            }
        }
        writer.shutdown().await;
    });

    let mut join_set: JoinSet<()> = JoinSet::new();

    // Tracks whether the writer task has already been consumed by the select!
    // branch below so teardown does not double-await it.
    let mut writer_done = false;

    // --- Reader loop ---------------------------------------------------------
    // Acquire permit → read frame → spawn dispatch.
    //
    // `read_frame_into` is placed inside `tokio::select!` with a branch that
    // watches the writer task handle. Cancel-safety of the read branch is
    // intentionally NOT required here: when the writer branch fires we are
    // tearing down the connection entirely, so any partially-read frame is
    // discarded along with everything else. We never resume the read after
    // the writer exits.
    'conn: loop {
        // Non-blocking drain of completed dispatch tasks: releases permits
        // and surfaces panics before we block on the next acquire.
        while let Some(result) = join_set.try_join_next() {
            if let Err(e) = result {
                if e.is_panic() {
                    tracing::error!("dispatch task panicked: {:?}", e);
                    // DEFECT A fix: a dispatch panic is fatal for this
                    // connection. Use the labeled break to exit the outer
                    // 'conn loop, not just this inner while.
                    break 'conn;
                }
            }
        }

        // Check the ReplyAndClose flag set by a dispatch task.
        if close_requested.load(Ordering::Relaxed) {
            break;
        }

        // Acquire a semaphore permit (back-pressure gate).
        // When all max_in_flight slots are taken this awaits the release of
        // an existing permit by a completing dispatch task.
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break, // semaphore closed — should never happen
        };

        // Double-check the close flag: a ReplyAndClose dispatch task may have
        // completed while we were waiting for the permit.
        if close_requested.load(Ordering::Relaxed) {
            drop(permit);
            break;
        }

        // Read the next frame. DEFECT B fix: run inside select! so that if
        // the writer task exits (e.g. after ReplyAndClose or a write error)
        // the reader does not block forever waiting for data from a client
        // that is intentionally holding the TCP connection open.
        // Cancel-safety of the read branch is not required — when the writer
        // branch fires we discard the partial read and tear down immediately.
        let mut frame_buf = Vec::new();
        tokio::select! {
            read_res = reader.read_frame_into(MAX_FRAME_SIZE_DEFAULT, &mut frame_buf) => {
                match read_res {
                    Ok(()) => {}
                    Err(_) => {
                        // Client closed or transport error.
                        drop(permit);
                        break;
                    }
                }
            }
            _ = &mut writer_handle => {
                // Writer task exited (ReplyAndClose sent, or write error).
                // Tear down immediately; do not block on a lingering client.
                drop(permit);
                writer_done = true;
                break;
            }
        }

        // Spawn a per-request dispatch task. Each task owns:
        //   - `frame_buf`: raw msgpack bytes (fresh Vec — no reuse on
        //     concurrent path)
        //   - `ctx_clone`: Arc — cheap clone, shared read-only state
        //   - `permit`: OwnedSemaphorePermit — released when task ends
        //   - `tx_clone`: mpsc Sender — pushes reply to writer task
        //   - `close_flag`: signals ReplyAndClose to the reader loop
        let ctx_clone = Arc::clone(&ctx);
        let tx_clone = tx.clone();
        let close_flag = Arc::clone(&close_requested);
        let sid_copy = sid;

        join_set.spawn(async move {
            let _guard = InFlightGuard::new();
            let _permit = permit;

            let msg = match RequestEnvelopeView::from_msgpack(&frame_buf) {
                Ok(v) => {
                    let lookup_tib = |uid: &[u8; 16]| -> u64 {
                        ctx_clone.user_dir.tickets_invalid_before_ns_by_user_id(uid)
                    };
                    match dispatch_request_view(
                        &v,
                        &ctx_clone.session_store,
                        lookup_tib,
                        ctx_clone.handler.as_ref(),
                    )
                    .await
                    {
                        Ok(DispatchOutcome::Response(resp)) => {
                            let rid = v.request_id;
                            match resp.to_msgpack() {
                                Ok(b) => Some(WriterMsg::Reply(b)),
                                Err(_) => {
                                    // Serialisation failure — best-effort error.
                                    let err = ErrorEnvelope::new(rid, "internal_error");
                                    err.to_msgpack().ok().map(WriterMsg::Reply)
                                }
                            }
                        }
                        Ok(DispatchOutcome::Error(err)) => {
                            let close = err.error == "session_invalidated"
                                || err.error == "session_expired";
                            match err.to_msgpack() {
                                Ok(b) => {
                                    if close {
                                        // Signal the reader loop to stop.
                                        close_flag.store(true, Ordering::Relaxed);
                                        Some(WriterMsg::ReplyAndClose(b))
                                    } else {
                                        Some(WriterMsg::Reply(b))
                                    }
                                }
                                Err(_) => None,
                            }
                        }
                        Err(_) => {
                            // Internal dispatch error.
                            let err = ErrorEnvelope::new(v.request_id, "internal_error");
                            err.to_msgpack().ok().map(WriterMsg::Reply)
                        }
                    }
                }
                Err(_) => {
                    // Malformed envelope.
                    let err = ErrorEnvelope::new(None, "invalid_envelope");
                    err.to_msgpack().ok().map(WriterMsg::Reply)
                }
            };

            if let Some(msg) = msg {
                // `send` provides back-pressure: blocks when the channel
                // is at capacity. A slow writer stalls dispatch tasks →
                // permits held → semaphore exhausted → reader stalls. §B14.
                let _ = tx_clone.send(msg).await;
            }

            let _ = sid_copy;
        });
    }

    // --- Teardown ------------------------------------------------------------
    // Cancel all in-flight dispatch tasks (they hold permits and tx_clones).
    join_set.abort_all();
    // Dropping tx closes the mpsc channel; the writer task sees None on its
    // next recv() and exits gracefully after flushing what it has.
    drop(tx);
    // Wait for the writer task to finish. §B21: no detached tasks.
    // If writer_done is true the select! branch already consumed the handle.
    if !writer_done {
        let _ = writer_handle.await;
    }

    let _ = sid;
    let _ = ConnectError::AuthFailed;
}

fn prefix_8(sid: &[u8; 32]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&sid[..8]);
    out
}

fn audit_emit(
    ctx: &ConnectionContext,
    event: &str,
    user: &str,
    subnet: shamir_connect::server::lockout::Subnet,
    sid_prefix: Option<&[u8; 8]>,
    result: &str,
) {
    let now_ns = UnixNanos::now().as_u64();
    let prefix = sid_prefix.copied().unwrap_or([0u8; 8]);
    ctx.audit.append(
        event,
        match ctx.transport_kind {
            TransportKind::Tcp => "tcp",
            TransportKind::WebSocket => "ws",
        },
        user,
        format!("{:?}", subnet),
        prefix,
        result,
        Vec::new(),
        now_ns,
    );
}
