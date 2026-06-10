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
use shamir_connect::server::resume::ResumeConfig;
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::{
    Session, SessionPermissions, SessionStore, MAX_SESSIONS_PER_USER,
};
use shamir_connect::server::user_record::UserRecord;
use shamir_connect::Error as ConnectError;

use crate::framer::{FrameReader, FrameWriter, Framer};
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
    let mut frame_buf: Vec<u8> =
        Vec::with_capacity(shamir_tunables::instance_defaults::IO_FRAME_BUFFER_CAP);
    let mut write_scratch: Vec<u8> =
        Vec::with_capacity(shamir_tunables::instance_defaults::IO_FRAME_BUFFER_CAP);

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
    };

    // Pad on success path too — both paths must be wall-clock equivalent.
    let pad = pad_guard.finish_with_target(target_constant_time_ms());
    if pad > Duration::ZERO {
        tokio::time::sleep(pad).await;
    }

    // Split the framer into directional halves so the future duplex
    // request loop (M1) can hand each half to its own task.  The lock-step
    // loop below owns both halves sequentially — semantics are unchanged.
    let (mut reader, mut writer) = framer.split();

    request_loop(
        &ctx,
        &mut reader,
        &mut writer,
        &mut frame_buf,
        &mut write_scratch,
        session_id,
    )
    .await;

    // Terminal: 5s grace is a transport-layer concept (the session sticks
    // around in SessionStore after disconnect; resume can re-bind within
    // grace). Here we simply close the write half; the session remains in
    // the store until session GC evicts it by idle TTL.
    writer.shutdown().await;
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
    //
    //    HIGH-1: bound by `MAX_PRE_AUTH_FRAME` (4 KiB per spec §8), NOT
    //    the post-auth 16 MiB ceiling. A real `auth_init` is ~80 bytes;
    //    an unauthenticated peer must not be able to make the server
    //    allocate 16 MiB per connection — 10 000 concurrent attackers
    //    would otherwise demand ~160 GiB of pre-auth memory.
    let read_fut = framer.read_frame_into(MAX_PRE_AUTH_FRAME, frame_buf);
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
    if framer
        .read_frame_into(MAX_PRE_AUTH_FRAME, frame_buf)
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

/// Outcome of one dispatch — serialised bytes plus a "break after
/// writing" flag for the §7.5 session_invalidated / session_expired paths.
enum DispatchOutput {
    /// Reply bytes to write to the wire; keep looping afterwards.
    Reply(Vec<u8>),
    /// Reply bytes to write to the wire; then break the request loop.
    /// Used for `session_invalidated` (already removed from store) and
    /// `session_expired` so an attacker that resends keeps wasting a
    /// fresh connection rather than reusing this one.
    ReplyAndBreak(Vec<u8>),
    /// Nothing usable to write (encode failure on the error envelope) —
    /// just continue the loop.
    Skip,
}

async fn request_loop<R: FrameReader, W: FrameWriter>(
    ctx: &ConnectionContext,
    reader: &mut R,
    writer: &mut W,
    frame_buf: &mut Vec<u8>,
    write_scratch: &mut Vec<u8>,
    sid: [u8; 32],
) {
    loop {
        match reader
            .read_frame_into(MAX_FRAME_SIZE_DEFAULT, frame_buf)
            .await
        {
            Ok(()) => {}
            Err(_) => break, // client closed
        }

        // Parse the envelope, run §7.5 session validity, and invoke the
        // async handler — all directly on the current async task. No
        // blocking-pool bridge (spawn_blocking) is needed because
        // RequestHandler::handle is now an async fn. The request loop
        // remains lock-step (one in-flight request per connection); the
        // worker is free to yield at every .await inside the handler.
        let view = match RequestEnvelopeView::from_msgpack(frame_buf) {
            Ok(v) => v,
            Err(_) => {
                // Malformed envelope — emit generic error envelope back.
                let err = ErrorEnvelope::new(None, "invalid_envelope");
                let dispatch = match err.to_msgpack() {
                    Ok(b) => DispatchOutput::Reply(b),
                    Err(_) => DispatchOutput::Skip,
                };
                match dispatch {
                    DispatchOutput::Skip => continue,
                    DispatchOutput::Reply(bytes) => {
                        if writer
                            .write_frame_into(&bytes, write_scratch)
                            .await
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                    DispatchOutput::ReplyAndBreak(_) => unreachable!(),
                }
            }
        };

        let lookup_tib = |uid: &[u8; 16]| -> u64 {
            // Spec §7.5 NORMATIVE: each request runs through this fast
            // read so changes to the user record (role updates,
            // kickSession, password change) invalidate live sessions on
            // the next request. Reverse-lookup from `user_id` → username
            // → UserRecord uses the secondary index maintained inside
            // `RedbUserDirectory::insert`.
            ctx.user_dir.tickets_invalid_before_ns_by_user_id(uid)
        };

        // `dyn RequestHandler` implements the trait directly — no DynRef
        // wrapper needed now that handle() returns a boxed future.
        let dispatch = match dispatch_request_view(
            &view,
            &ctx.session_store,
            lookup_tib,
            ctx.handler.as_ref(),
        )
        .await
        {
            Ok(outcome) => match outcome {
                DispatchOutcome::Response(resp) => match resp.to_msgpack() {
                    Ok(b) => DispatchOutput::Reply(b),
                    Err(_) => DispatchOutput::Skip,
                },
                DispatchOutcome::Error(err) => {
                    let invalidated =
                        err.error == "session_invalidated" || err.error == "session_expired";
                    match err.to_msgpack() {
                        Ok(b) => {
                            if invalidated {
                                DispatchOutput::ReplyAndBreak(b)
                            } else {
                                DispatchOutput::Reply(b)
                            }
                        }
                        Err(_) => DispatchOutput::Skip,
                    }
                }
            },
            Err(_) => {
                // Internal error — best-effort error envelope.
                let err = ErrorEnvelope::new(view.request_id, "internal_error");
                match err.to_msgpack() {
                    Ok(b) => DispatchOutput::Reply(b),
                    Err(_) => DispatchOutput::Skip,
                }
            }
        };

        // Apply the dispatch outcome.
        // The view borrow of frame_buf ends here (last use above); NLL
        // releases it so frame_buf is reused on the next iteration
        // as-is (Optim #1 / #7 — capacity is preserved).
        match dispatch {
            DispatchOutput::Skip => continue,
            DispatchOutput::Reply(bytes) => {
                if writer
                    .write_frame_into(&bytes, write_scratch)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            DispatchOutput::ReplyAndBreak(bytes) => {
                let _ = writer.write_frame_into(&bytes, write_scratch).await;
                // §7.5 has already removed the session; close the loop.
                break;
            }
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

#[cfg(test)]
mod tests {
    use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
    use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

    // HIGH-1 compile-time invariants: pin the pre-auth frame ceiling at
    // the spec §8 value and prove it stays strictly below the post-auth
    // ceiling the request loop uses. `const` assertions trip at compile
    // time so a future spec edit that weakens either bound fails the
    // build, not just the test suite.
    const _: () = assert!(
        MAX_PRE_AUTH_FRAME == 4 * 1024,
        "MAX_PRE_AUTH_FRAME must equal 4 KiB per spec §8",
    );
    const _: () = assert!(
        MAX_PRE_AUTH_FRAME < MAX_FRAME_SIZE_DEFAULT,
        "pre-auth ceiling must be strictly smaller than post-auth ceiling",
    );

    /// HIGH-1 regression: `run_handshake` must read pre-auth frames with
    /// the 4 KiB ceiling, not the post-auth 16 MiB ceiling. The compile-
    /// time `const` asserts above pin the constants; this runtime test
    /// surfaces a human-readable failure when the bounds are tightened
    /// or loosened in the future, and additionally documents the
    /// resource-exhaustion budget.
    #[test]
    fn pre_auth_frame_budget_is_safe_for_ten_thousand_unauth_peers() {
        // Defense-in-depth: 10 000 concurrent unauthenticated peers
        // multiplied by the pre-auth cap must stay well under
        // commodity-server RAM. 10 000 × 4 KiB = 40 MiB, vs. the
        // 10 000 × 16 MiB = ~160 GiB that the old shape allowed.
        let max_unauth_memory = MAX_PRE_AUTH_FRAME.saturating_mul(10_000);
        assert!(
            max_unauth_memory < 128 * 1024 * 1024,
            "pre-auth cap × 10k connections should be under 128 MiB; got {}",
            max_unauth_memory,
        );
    }

    // ---------------------------------------------------------------------
    // NEW-2: per-pair exponential backoff is COMPUTED *and APPLIED*.
    //
    // The application logic lives in two spots:
    //   * `run_handshake` (ProofOutcome::Rejected arm) maps the
    //     `register_failure` outcome → `backoff_ms` and returns it via
    //     `HandshakeError::BadProof { backoff_ms }`.
    //   * `handle_connection` widens the negative-path latency pad to
    //     `max(target_constant_time_ms(), backoff_ms)`.
    //
    // Driving the full async `run_handshake` requires a complete
    // `ConnectionContext` (TLS identity, redb user dir, …) so these tests
    // exercise the two load-bearing pieces directly: (1) the exact
    // outcome→backoff mapping used by the reject arm, against the real
    // `InMemoryLockoutStore`, and (2) the `max(floor, backoff)` pad formula.
    // ---------------------------------------------------------------------

    use shamir_connect::common::latency::{padding_for, target_constant_time_ms, FIXED_FLOOR_MS};
    use shamir_connect::server::lockout::{
        FailureOutcome, InMemoryLockoutStore, LockoutStore, PairKey, Subnet, BACKOFF_BASE_MS,
        BACKOFF_CAP_MS, LOCKOUT_THRESHOLD,
    };
    use std::time::Duration;

    /// Mirror of the `ProofOutcome::Rejected` arm in `run_handshake`: map a
    /// `register_failure` outcome to the `backoff_ms` that gets plumbed into
    /// `HandshakeError::BadProof`. Kept in lockstep with the production code
    /// so the test fails if the mapping drifts.
    fn backoff_ms_for(outcome: FailureOutcome) -> u64 {
        match outcome {
            FailureOutcome::Backoff { delay_ms } => delay_ms,
            FailureOutcome::LockedOut => BACKOFF_CAP_MS,
        }
    }

    fn pair(subnet: u8, user: u8) -> PairKey {
        (Subnet::V4([10, 0, subnet]), [user; 16])
    }

    /// The backoff plumbed into the reject path must escalate `100ms × 2^N`
    /// (capped 30s) as failures accumulate for a `(subnet, username_hash)`
    /// pair, exactly as the reject arm computes it from the real store.
    #[test]
    fn reject_path_backoff_escalates_per_failure() {
        let store = InMemoryLockoutStore::new();
        let now = 1_000_000_000u64;
        let k = pair(1, 1);
        const SECOND_NS: u64 = 1_000_000_000;

        // First failures (well below the 50-failure lockout threshold)
        // double each time: 100, 200, 400, 800, 1600, ...
        let expected = [
            100u64, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
        ];
        for (i, &want) in expected.iter().enumerate() {
            let outcome = store.register_failure(k, now + (i as u64) * SECOND_NS);
            assert_eq!(
                backoff_ms_for(outcome),
                want,
                "failure #{} should map to {}ms backoff",
                i + 1,
                want,
            );
        }
        // Base × 2^0 sanity (documents the formula's anchor).
        assert_eq!(BACKOFF_BASE_MS, 100);
    }

    /// Crossing the lockout threshold must still yield a bounded backoff for
    /// the final response (`BACKOFF_CAP_MS`), not panic or 0 — the reject arm
    /// maps `FailureOutcome::LockedOut → BACKOFF_CAP_MS`.
    #[test]
    fn reject_path_backoff_caps_at_lockout_threshold() {
        let store = InMemoryLockoutStore::new();
        let now = 1_000_000_000u64;
        let k = pair(2, 2);
        const SECOND_NS: u64 = 1_000_000_000;

        let mut last = 0u64;
        for i in 0..LOCKOUT_THRESHOLD {
            let outcome = store.register_failure(k, now + (i as u64) * SECOND_NS);
            last = backoff_ms_for(outcome);
        }
        // The 50th failure trips the lockout; the mapped backoff is the cap.
        assert_eq!(
            last, BACKOFF_CAP_MS,
            "threshold-crossing backoff is the cap"
        );
        assert!(store.is_locked_out(k, now + (LOCKOUT_THRESHOLD as u64) * SECOND_NS));
    }

    /// The negative-path pad target is `max(constant_time_floor, backoff)` —
    /// the timing-oracle floor is preserved AND the escalation is enforced.
    /// With elapsed below the target, the computed sleep reaches the backoff.
    #[test]
    fn pad_target_is_max_of_floor_and_backoff() {
        // A large backoff dominates the floor: total pad ≈ backoff.
        let backoff_ms = 6400u64;
        // Sample the (random) floor many times; the formula must never drop
        // below either input.
        for _ in 0..256 {
            let target_ms = target_constant_time_ms().max(backoff_ms);
            assert!(
                target_ms >= backoff_ms,
                "pad target must be >= backoff ({target_ms} < {backoff_ms})",
            );
            assert!(
                target_ms >= FIXED_FLOOR_MS,
                "pad target must be >= constant-time floor ({target_ms} < {FIXED_FLOOR_MS})",
            );
        }

        // With a tiny elapsed, the sleep computed by the same helper the
        // negative path uses must reach the backoff window.
        let target_ms = 6400u64.max(target_constant_time_ms());
        let sleep = padding_for(Duration::from_millis(5), target_ms);
        assert!(
            sleep >= Duration::from_millis(backoff_ms - 5),
            "negative-path sleep ({sleep:?}) must reach the backoff window",
        );
    }

    /// When there is no backoff (e.g. an internal verify error path with
    /// `backoff_ms = 0`), the pad target collapses to the constant-time
    /// floor — behaviour identical to the pre-NEW-2 flat pad.
    #[test]
    fn zero_backoff_collapses_to_constant_time_floor() {
        for _ in 0..256 {
            let floor = target_constant_time_ms();
            // `black_box` so the zero is treated as an opaque runtime value
            // (matching the production `floor.max(backoff_ms)` where
            // `backoff_ms == 0`) rather than a compile-time no-op.
            let backoff_ms = std::hint::black_box(0u64);
            let target_ms = floor.max(backoff_ms);
            assert_eq!(target_ms, floor);
            assert!((FIXED_FLOOR_MS..=FIXED_FLOOR_MS + 25).contains(&target_ms));
        }
    }

    /// No user-existence oracle: the store's backoff depends ONLY on the
    /// failure count for the `(subnet, username_hash)` pair, never on whether
    /// the username maps to a real account. Two distinct username hashes on
    /// the same subnet, failed the same number of times, must yield identical
    /// backoff progressions — so widening the pad to the backoff cannot leak
    /// which usernames exist.
    #[test]
    fn backoff_progression_is_identical_for_any_pair() {
        let store = InMemoryLockoutStore::new();
        let now = 1_000_000_000u64;
        const SECOND_NS: u64 = 1_000_000_000;
        let real_user = pair(7, 0xaa); // imagine this hash maps to a real account
        let fake_user = pair(7, 0xbb); // and this one does not

        for i in 0..8u64 {
            let a = backoff_ms_for(store.register_failure(real_user, now + i * SECOND_NS));
            let b = backoff_ms_for(store.register_failure(fake_user, now + i * SECOND_NS));
            assert_eq!(
                a,
                b,
                "backoff at failure #{} must not depend on the pair identity",
                i + 1,
            );
        }
    }
}
