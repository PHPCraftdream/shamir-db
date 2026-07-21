//! Top-level connection entry-point, SCRAM-Argon2id handshake, and
//! session-resume fast-path.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::handshake::{
    AuthInitView, AuthOkView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
use shamir_connect::server::lockout::{
    subnet_of, username_hash, FailureOutcome, PairKey, BACKOFF_CAP_MS,
};
use shamir_connect::server::rate_limit::RateDecision;
use shamir_connect::server::resume::{process_resume, ResumeRequest};
use shamir_connect::server::session::{Session, SessionPermissions, MAX_SESSIONS_PER_USER};
use shamir_connect::server::user_record::UserRecord;

use shamir_query_types::wire::CURRENT_QUERY_LANG_VERSION;

use crate::framer::Framer;

use super::connection_context::ConnectionContext;
use super::request_loop::request_loop;
use super::user_state_lookup::RedbUserStateLookup;
use super::wire;

/// Helper for the auth_attempts_total counter — keeps the result label
/// values consistent across emit sites.
fn record_auth_attempt(result: &'static str) {
    metrics::counter!("auth_attempts_total", "result" => result).increment(1);
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

fn prefix_8(sid: &[u8; 32]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&sid[..8]);
    out
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
        server_query_version: CURRENT_QUERY_LANG_VERSION as u8,
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
    if let Err(e) = crate::version::check_handshake_proto(init.version) {
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
        // Audit C3: log only the HMAC-derived `username_hash` (already in
        // scope as the lockout key) on the general `info` channel — the full
        // plaintext identity goes ONLY to the protected HMAC audit chain via
        // the `audit_emit` call below. This avoids both PII exposure in
        // general-purpose log aggregation and a user-enumeration /
        // credential-stuffing signal (an attacker aggregating `auth_failed`
        // events by username).
        tracing::info!(user_hash = %hex::encode(uhash), "locked_out at auth_init");
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
            // Audit C3: see the comment on the `locked_out` arm above — log
            // only `user_hash` on the general `info` channel; the full
            // plaintext identity goes to the protected audit chain via the
            // `audit_emit` call below.
            tracing::info!(user_hash = %hex::encode(uhash), "auth_failed: bad proof");
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

    // RI-9: consume the bootstrap token on the first successful login for
    // the username it was issued to. Best-effort and non-fatal — a failure
    // here must NEVER abort an otherwise-successful login; the boot-time
    // TTL sweep (`server_launcher.rs`) is the backstop for anything missed
    // here.
    if ctx.meta.bootstrap_token_active()
        && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())
    {
        if let Some(path) = ctx.meta.bootstrap_token_path() {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(?path, ?e, "bootstrap: failed to delete token file on login");
                }
            }
        }
        if let Err(e) = ctx.meta.consume_bootstrap_token() {
            tracing::warn!(?e, "bootstrap: failed to consume token record on login");
        }
    }

    // 8. Build session, insert with per-user cap, send auth_ok.
    let user_id = match ctx.user_dir.user_id(username.as_str()) {
        Some(id) => id,
        None => return Err(HandshakeError::UnknownUser),
    };
    // Task #557: ONE directory lookup (`state_by_user_id`) replaces the two
    // prior `lookup_roles` calls — it returns the authoritative `superuser`
    // flag (read directly off the persisted blob) plus the post-migration
    // roles list in a single fjall get + msgpack decode. The session's
    // `is_superuser` is now driven by the flag, not by scanning the roles
    // list for the literal `"superuser"` string (which is reserved at the
    // directory write boundary as of #557, so the persisted list never
    // contains it anyway).
    //
    // The roles value is no longer threaded into the resumption ticket
    // (task #558 dropped `roles` from `TicketPlain`; resume re-verifies
    // against the directory on every reconnect), so `user_state.roles` has
    // exactly ONE consumer here — the freshly-built session. It is MOVED
    // rather than cloned.
    let user_state = match ctx.user_dir.state_by_user_id(&user_id) {
        Some(s) => s,
        None => return Err(HandshakeError::UnknownUser),
    };
    let session = Session::new(
        user_id,
        username.as_str().to_string(),
        SessionPermissions::new(
            user_state.superuser,
            user_state.replicator,
            user_state.roles,
        ),
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
        server_query_version: CURRENT_QUERY_LANG_VERSION as u8,
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
