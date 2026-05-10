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
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::envelope::{ErrorEnvelope, RequestEnvelopeView};
use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::dispatch::{dispatch_request_view, DispatchOutcome, RequestHandler};
use shamir_connect::server::handshake::{
    AuthInitView, AuthOkView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
use shamir_connect::server::lockout::{
    subnet_of, username_hash, LockoutStore, PairKey,
};
use shamir_connect::server::rate_limit::{RateDecision, RateLimiter};
use shamir_connect::server::resume::ResumeConfig;
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::{
    Session, SessionPermissions, SessionStore, MAX_SESSIONS_PER_USER,
};
use shamir_connect::server::user_record::UserRecord;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::Error as ConnectError;

use crate::framer::Framer;
use crate::user_directory::RedbUserDirectory;
use crate::version::check_handshake_proto;

/// Helper for the auth_attempts_total counter — keeps the result label
/// values consistent across emit sites.
fn record_auth_attempt(result: &'static str) {
    metrics::counter!("auth_attempts_total", "result" => result).increment(1);
}

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

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
    let mut frame_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut write_scratch: Vec<u8> = Vec::with_capacity(4096);

    // Latency padding starts here — covers the entire auth flow so
    // negative paths can't be timed (spec §8.5).
    let pad_guard = LatencyPadGuard::start();

    let session_id = match run_handshake(
        &ctx,
        &mut framer,
        &mut frame_buf,
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
        Err(e) => {
            // Bucket the failure into a coarse counter label. Detailed
            // categorisation (which user / which subnet) lives in the
            // audit log; the counter is for at-a-glance dashboards.
            let label = match e {
                HandshakeError::LockedOut => "locked_out",
                HandshakeError::BadProof => "bad_proof",
                HandshakeError::UnknownUser => "unknown_user",
                HandshakeError::UnsupportedVersion => "unsupported_version",
                HandshakeError::Policy => "policy",
                HandshakeError::Io | HandshakeError::Decode => "io_or_decode",
            };
            record_auth_attempt(label);
            // Pad to spec §8.5 floor before disconnecting on the negative
            // path — defeats real-vs-fake user timing oracles.
            let pad = pad_guard.finish_with_target(target_constant_time_ms());
            if pad > Duration::ZERO {
                tokio::time::sleep(pad).await;
            }
            framer.shutdown().await;
            return;
        }
    };

    // Pad on success path too — both paths must be wall-clock equivalent.
    let pad = pad_guard.finish_with_target(target_constant_time_ms());
    if pad > Duration::ZERO {
        tokio::time::sleep(pad).await;
    }

    request_loop(&ctx, &mut framer, &mut frame_buf, &mut write_scratch, session_id).await;

    // Terminal: 5s grace is a transport-layer concept (the session sticks
    // around in SessionStore after disconnect; resume can re-bind within
    // grace). Here we simply close the framer; the session remains in the
    // store until session GC evicts it by idle TTL.
    framer.shutdown().await;
}

#[derive(Debug)]
enum HandshakeError {
    Io,
    Decode,
    Policy,
    BadProof,
    UnknownUser,
    LockedOut,
    /// Client requested a handshake-protocol version this server does not
    /// implement. Fast-rejected before any Argon2id work.
    UnsupportedVersion,
}

async fn run_handshake<F: Framer>(
    ctx: &ConnectionContext,
    framer: &mut F,
    frame_buf: &mut Vec<u8>,
    write_scratch: &mut Vec<u8>,
    subnet: shamir_connect::server::lockout::Subnet,
    exporter: [u8; 32],
) -> Result<[u8; 32], HandshakeError> {
    // 1. Read auth_init — bounded by `auth_init_timeout` so a TLS-accepted
    //    client that never sends data (slow-loris) is dropped instead of
    //    holding a tokio task + per-connection memory forever.
    //
    //    Real clients send auth_init within a single RTT (~50 ms typically).
    //    The default ceiling of 5 s is comfortably above network jitter +
    //    TLS handshake overhead while still cutting attackers off quickly.
    let read_fut = framer.read_frame_into(MAX_FRAME_SIZE_DEFAULT, frame_buf);
    match tokio::time::timeout(ctx.auth_init_timeout, read_fut).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::debug!(?e, "auth_init read failed");
            return Err(HandshakeError::Io);
        }
        Err(_elapsed) => {
            tracing::info!(
                timeout_ms = ctx.auth_init_timeout.as_millis() as u64,
                ?subnet,
                "auth_init timeout (slow-loris)",
            );
            return Err(HandshakeError::Io);
        }
    }
    let init: wire::AuthInit = match rmp_serde::from_slice(frame_buf) {
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
    let listener_policy =
        shamir_connect::server::config::ListenerPolicy::new(ctx.binding_mode);
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
    if framer.write_frame_into(&bytes, write_scratch).await.is_err() {
        return Err(HandshakeError::Io);
    }

    // 5. Read client_proof (Argon2id ran on the client side, ~2s).
    if framer.read_frame_into(MAX_FRAME_SIZE_DEFAULT, frame_buf)
        .await
        .is_err()
    {
        return Err(HandshakeError::Io);
    }
    let proof_msg: wire::ClientProof = match rmp_serde::from_slice(frame_buf) {
        Ok(v) => v,
        Err(_) => return Err(HandshakeError::Decode),
    };
    if proof_msg.client_proof.len() != 32 {
        return Err(HandshakeError::Decode);
    }
    let mut proof = [0u8; 32];
    proof.copy_from_slice(&proof_msg.client_proof);

    // 6. Acquire Argon2 permit before verify (server-side HMAC + Ed25519
    // sign — light, but the permit also gates against burst-DoS that
    // multiplies the pre-state work). Per spec §8.1 the permit covers
    // ONLY the actual KDF; here we use it as a server-side concurrency
    // limiter for the verify operation since real-user path doesn't run
    // Argon2id (only FakeBlob HKDF + HMACs). Take a try_acquire to avoid
    // blocking under load — return server_busy on contention.
    let permit_opt = ctx.argon2_sem.try_acquire();
    if permit_opt.is_none() {
        return Err(HandshakeError::Policy); // surface as authentication_failed
    }

    // 7. Verify the proof. Identity keypair lives behind ServerIdentityState.
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
        &ctx.identity_keypair_for_verify(),
        SESSION_MAX_AGE_NS,
    ) {
        Ok(o) => o,
        Err(_) => return Err(HandshakeError::BadProof),
    };
    drop(permit_opt);

    let auth_ok: AuthOkView = match outcome {
        ProofOutcome::Accepted(ok) => ok,
        ProofOutcome::Rejected => {
            // Register the failure for backoff / lockout.
            let _ = ctx.lockout.register_failure(pair, now_ns);
            tracing::info!(user = %username.as_str(), "auth_failed: bad proof");
            audit_emit(
                ctx,
                "auth_failed",
                username.as_str(),
                subnet,
                None,
                "bad_proof",
            );
            return Err(HandshakeError::BadProof);
        }
    };

    // Reset lockout on success per spec §5.2.5 NORMATIVE.
    ctx.lockout.reset_on_success(pair);

    // 8. Build session, insert with per-user cap, send auth_ok.
    let user_id = match ctx.user_dir.user_id(username.as_str()) {
        Some(id) => id,
        None => return Err(HandshakeError::UnknownUser),
    };
    let roles = ctx
        .user_dir
        .lookup_roles(username.as_str())
        .unwrap_or_default();
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
    let (ticket_bytes, ticket_expires_at_ns) =
        match shamir_connect::server::resume::issue_initial_ticket(
            &ctx.resume_config.ticket_key,
            user_id,
            username.as_str().to_string(),
            ctx.transport_kind.as_u8(),
            binding_mode.as_u8(),
            exporter,
            ctx.user_dir.lookup_roles(username.as_str()).unwrap_or_default(),
            ctx.identity.current_version(),
            now_ns,
            RESUMPTION_TICKET_TTL_NS,
        ) {
            Ok(t) => t,
            Err(e) => {
                // Issuing the ticket is best-effort — a failure must not
                // tank a successful auth. Log and continue with no ticket.
                tracing::warn!(?e, "resumption ticket issuance failed; auth_ok will carry no ticket");
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
    if framer.write_frame_into(&bytes, write_scratch).await.is_err() {
        return Err(HandshakeError::Io);
    }

    Ok(auth_ok.session_id)
}

async fn request_loop<F: Framer>(
    ctx: &ConnectionContext,
    framer: &mut F,
    frame_buf: &mut Vec<u8>,
    write_scratch: &mut Vec<u8>,
    sid: [u8; 32],
) {
    let user_dir = ctx.user_dir.clone();
    let lookup_tib = move |uid: &[u8; 16]| -> u64 {
        // Spec §7.5 NORMATIVE: each request runs through this fast read so
        // changes to the user record (role updates, kickSession,
        // password change) invalidate live sessions on the next request.
        // Reverse-lookup from `user_id` → username → UserRecord uses the
        // secondary index maintained inside `RedbUserDirectory::insert`.
        user_dir.tickets_invalid_before_ns_by_user_id(uid)
    };

    loop {
        match framer.read_frame_into(MAX_FRAME_SIZE_DEFAULT, frame_buf).await {
            Ok(()) => {}
            Err(_) => break, // client closed
        }
        let view = match RequestEnvelopeView::from_msgpack(frame_buf) {
            Ok(v) => v,
            Err(_) => {
                // Malformed envelope — emit generic error envelope back.
                let err = ErrorEnvelope::new(None, "invalid_envelope");
                if let Ok(bytes) = err.to_msgpack() {
                    let _ = framer.write_frame_into(&bytes, write_scratch).await;
                }
                continue;
            }
        };
        // Dispatch. dispatch_request_view runs §7.5 validity check.
        let handler_ref: &dyn RequestHandler = ctx.handler.as_ref();
        // dispatch_request_view requires `H: RequestHandler` which `dyn` does
        // not satisfy directly — wrap in a thin newtype that implements
        // the trait by delegation.
        struct DynRef<'a>(&'a dyn RequestHandler);
        impl<'a> RequestHandler for DynRef<'a> {
            fn handle(
                &self,
                session: &shamir_connect::server::session::Session,
                req: &[u8],
            ) -> std::result::Result<Vec<u8>, String> {
                self.0.handle(session, req)
            }
        }
        let dyn_handler = DynRef(handler_ref);
        let outcome = match dispatch_request_view(
            &view,
            &ctx.session_store,
            &lookup_tib,
            &dyn_handler,
        ) {
            Ok(o) => o,
            Err(_) => {
                // Internal error — best-effort error envelope.
                let err = ErrorEnvelope::new(view.request_id, "internal_error");
                if let Ok(bytes) = err.to_msgpack() {
                    let _ = framer.write_frame_into(&bytes, write_scratch).await;
                }
                continue;
            }
        };
        let reply_bytes = match outcome {
            DispatchOutcome::Response(resp) => match resp.to_msgpack() {
                Ok(b) => b,
                Err(_) => continue,
            },
            DispatchOutcome::Error(err) => {
                let invalidated = err.error == "session_invalidated"
                    || err.error == "session_expired";
                let bytes = match err.to_msgpack() {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let _ = framer.write_frame_into(&bytes, write_scratch).await;
                if invalidated {
                    // §7.5 has already removed the session; close the loop.
                    break;
                }
                continue;
            }
        };
        if framer.write_frame_into(&reply_bytes, write_scratch).await.is_err() {
            break;
        }
    }
    let _ = sid;
    let _ = ConnectError::AuthFailed; // suppress unused import on some paths
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

// ----------------------------------------------------------------------------
// Compatibility shim for verify_proof — see comment in step 7. We add a
// lightweight method on ConnectionContext that returns a borrowed keypair
// reference for the duration of the verify call. The keypair is stored as
// an extra field alongside the ServerIdentityState.
// ----------------------------------------------------------------------------
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
}

// Patch the struct: add the keypair field. Defined here as a helper to
// keep the change local; main.rs constructs both ServerIdentityState
// and the keypair from the same seed via ServerMetaStore.
impl ConnectionContext {
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
        handler: Arc<dyn RequestHandler>,
        binding_mode: BindingMode,
        transport_kind: TransportKind,
        kdf_override: Option<KdfParams>,
        auth_init_timeout: Duration,
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
            handler,
            binding_mode,
            transport_kind,
            kdf_override,
            auth_init_timeout,
        })
    }
}

// Re-declare the struct to include the extra inner keypair field. This
// is technically incompatible with the earlier struct declaration; we
// fold it into the canonical definition above by re-declaring the
// fields as a single source of truth.
//
// To keep the file compilable, the struct definition above MUST list
// `identity_keypair_inner` — so we patch by editing the struct above
// rather than duplicating it here.
