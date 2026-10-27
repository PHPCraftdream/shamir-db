//! Top-level connection entry-point, SCRAM-Argon2id handshake, and
//! session-resume fast-path.

use std::net::SocketAddr;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::common::wire::RotationInProgressWire;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::admission::{Admission, AdmitReject};
use shamir_connect::server::handshake::{
    AuthInitView, AuthOkView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
use shamir_connect::server::lockout::{
    subnet_of, username_hash, FailureOutcome, PairKey, BACKOFF_CAP_MS,
};
use shamir_connect::server::rate_limit::RateDecision;
use shamir_connect::server::session::{Session, SessionPermissions, MAX_SESSIONS_PER_USER};
use shamir_connect::server::user_record::UserRecord;

use shamir_query_types::wire::CURRENT_QUERY_LANG_VERSION;

use crate::framer::Framer;

use super::request_loop::request_loop;
use super::session::resume::{run_resume, run_resume_proof};
use super::wire;
use super::ConnectionContext;

pub(crate) fn checked_query_version(version: u32) -> Result<u8, std::num::TryFromIntError> {
    u8::try_from(version)
}

/// Helper for the auth_attempts_total counter — keeps the result label
/// values consistent across emit sites.
fn record_auth_attempt(result: &'static str) {
    metrics::counter!("auth_attempts_total", "result" => result).increment(1);
}

pub(super) fn audit_emit(
    ctx: &ConnectionContext,
    event: &str,
    user: &str,
    subnet: shamir_connect::server::lockout::Subnet,
    sid_prefix: Option<&[u8; 8]>,
    result: &str,
) {
    let now_ns = UnixNanos::now().as_u64();
    let prefix = sid_prefix.copied().unwrap_or([0u8; 8]);
    if let Err(error) = ctx.audit.append(
        event,
        match ctx.transport_kind {
            TransportKind::Tcp => "tcp",
            TransportKind::WebSocket => "ws",
            TransportKind::Unix => "unix",
        },
        user,
        format!("{:?}", subnet),
        prefix,
        result,
        Vec::new(),
        now_ns,
    ) {
        tracing::error!(%error, "audit append rejected event");
    }
}

/// Test-only clock seam (RV-SERVER-17329FC9): when non-zero, this value
/// replaces the real clock for the in-handshake timestamps, letting tests
/// advance time deterministically between authentication start and proof
/// arrival. Zero means "real clock"; the static does not exist outside
/// `cfg(test)`, so production behaviour is untouched.
#[cfg(test)]
pub(crate) static CLOCK_OVERRIDE_NS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The in-handshake clock: real time in production, the pinned
/// `CLOCK_OVERRIDE_NS` value in tests when one is set.
fn now_unix_nanos() -> u64 {
    #[cfg(test)]
    {
        let pinned = CLOCK_OVERRIDE_NS.load(Ordering::Relaxed);
        if pinned != 0 {
            return pinned;
        }
    }
    UnixNanos::now().as_u64()
}

/// Spec §8 `HANDSHAKE_TIMEOUT = max(15s, kdf_time × 5)` — the absolute
/// budget for the post-`auth_init` handshake (challenge → proof → auth
/// decision). `kdf_time` MUST be the EFFECTIVE challenge time — a known
/// user's stored KDF time, else the listener default
/// (`ServerHandshake::effective_kdf_params`).
pub(crate) fn handshake_budget(kdf_time: u32) -> Duration {
    Duration::from_secs(15).max(Duration::from_secs(u64::from(kdf_time) * 5))
}

/// Best-effort terminal close with a FIXED grace: a backpressured
/// transport (e.g. a WS close handshake that never completes) must not
/// retain the connection task — and the admission guards that live until
/// it returns — past this bound (RV-TRANSPORT-TCP-B5244FA2). The result
/// is intentionally ignored: shutdown is advisory.
async fn bounded_shutdown<F: Framer>(framer: &mut F) {
    const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, framer.shutdown()).await;
}

pub(super) fn prefix_8(sid: &[u8; 32]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&sid[..8]);
    out
}

#[derive(Debug)]
pub(super) enum HandshakeError {
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
    /// Session admission rejected by the per-user cap (spec §7.4; internal
    /// only — the wire sees a generic failure). NOT a wrong-password event:
    /// no lockout failure registration, no lockout reset.
    Admission,
    /// Client requested a handshake-protocol version this server does not
    /// implement. Fast-rejected before any Argon2id work.
    UnsupportedVersion,
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
    // RV-SERVER-17329FC9: `mut` — re-captured after the awaited proof read
    // (see the `ProofOutcome::Accepted` arm) so the bootstrap-TTL gate below
    // is evaluated at proof time, not at authentication-start time.
    let mut now_ns = now_unix_nanos();
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

    // RV-TRANSPORT-TCP-B5244FA2: the WHOLE post-challenge handshake —
    // challenge write, proof read, verify, bootstrap gate/rotation,
    // auth_ok write — runs under ONE spec §8 budget derived from the
    // EFFECTIVE challenge KDF (a known user's STORED kdf time, else the
    // listener default), so the deadline always covers the proof window
    // the client was actually told to run. The pre-challenge steps above
    // are synchronous (no await) and the first-frame read is bounded by
    // `auth_init_timeout`, so this wrap bounds the entire unauthenticated
    // lifetime. On elapse the inner future is dropped (releasing the
    // framer borrow) and the caller's error path closes the connection.
    let budget = handshake_budget(hs.effective_kdf_params().time);
    tokio::time::timeout(budget, async {
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
    // RV-TRANSPORT-TCP-B5244FA2: this proof read — after the challenge was
    // sent — had NO deadline of its own before the fix; it is bounded by
    // the WHOLE-handshake budget wrap around this function tail (a single
    // spec §8 deadline covering challenge → proof → decision, so a stalled
    // proof can never outlive it). Terminal shutdown is bounded separately
    // by `bounded_shutdown` in `handle_connection`.
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
        ProofOutcome::Accepted(ok) => {
            // RV-SERVER-17329FC9: the proof read above awaited an untrusted,
            // potentially unbounded client delay (Argon2id alone is ~2s).
            // Re-capture the clock HERE so the bootstrap-token TTL decision
            // (and everything downstream: lockout registration, session
            // grant, ticket issue) is judged at proof time, not at the
            // stale authentication-start timestamp.
            now_ns = now_unix_nanos();
            *ok
        }
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

    // F-25 (#818, P0) + F-38 (#846, P0): fail-closed bootstrap TTL check.
    // The rotation block below is best-effort/non-fatal by design (F-3) so
    // an outstanding bootstrap token whose TTL has already expired must be
    // rejected HERE, before any session is granted -- otherwise a login
    // against an expired (but not-yet-rotated) bootstrap credential would
    // still succeed if the rotation attempt also fails (e.g. the same
    // storage outage that left the boot-time TTL sweep's own rotation
    // incomplete).
    //
    // F-38: this read is now fail-CLOSED on a metadata read/decode error.
    // The three granular `bootstrap_*` getters each independently call
    // `read_blob(KEY_BOOTSTRAP)` and swallow any error via `.ok()` (failing
    // OPEN), so the old 3-getter `if` could silently skip this gate when
    // the underlying blob read transiently errored -- granting a session
    // for what may be an EXPIRED, readable-under-normal-conditions bootstrap
    // credential purely because the read happened to fail at that instant.
    // `read_bootstrap_token_state` reads the row ONCE and propagates the
    // error; we treat BOTH the "outstanding-and-expired-for-this-user" `Ok`
    // arm AND any `Err` arm as a reject, using the SAME client-facing
    // failure shape as a wrong-proof rejection (`HandshakeError::BadProof`,
    // same `register_failure`/`audit_emit` pattern as the
    // `ProofOutcome::Rejected` arm above) so this is indistinguishable from
    // a bad password to an outside observer -- no oracle telling an
    // attacker "the password was right, only a storage error happened."
    //
    // The two reject arms are byte-for-byte identical in client-observable
    // shape: ONE `read_blob`, ONE `register_failure`, ONE `audit_emit`, ONE
    // `HandshakeError::BadProof { backoff_ms }` return. The distinct
    // `audit_emit` reason string and `warn!` log for the metadata-error arm
    // are operator-side only (HMAC audit chain + server log) and are never
    // sent to the client. Must run BEFORE `reset_on_success` -- a login
    // that's about to be rejected must not reset the pair's lockout counter.
    match ctx.meta.read_bootstrap_token_state(now_ns) {
        Ok(state)
            if state.active
                && state.username.as_deref() == Some(username.as_str())
                && state.expired =>
        {
            let backoff_ms = match ctx.lockout.register_failure(pair, now_ns) {
                FailureOutcome::Backoff { delay_ms } => delay_ms,
                FailureOutcome::LockedOut => BACKOFF_CAP_MS,
            };
            tracing::info!(user_hash = %hex::encode(uhash), "auth_failed: bootstrap token expired");
            audit_emit(
                ctx,
                "auth_failed",
                username.as_str(),
                subnet,
                None,
                "bootstrap_token_expired",
            );
            return Err(HandshakeError::BadProof { backoff_ms });
        }
        Ok(_) => { /* not an outstanding-expired-bootstrap-for-this-user case — proceed */ }
        Err(e) => {
            // F-38 FAIL-CLOSED: a metadata read error after a valid
            // password proof must reject the login, never grant it. Use the
            // SAME client-facing failure shape as the expired-token arm
            // above (identical `register_failure` → backoff mapping +
            // `HandshakeError::BadProof`) so the two rejects are
            // indistinguishable to an outside observer. The password proof
            // already verified, so the only acceptable client signal is
            // "auth failed" -- never "auth failed because of storage".
            tracing::warn!(
                ?e,
                user_hash = %hex::encode(uhash),
                "auth_failed: bootstrap metadata read error -- failing closed"
            );
            let backoff_ms = match ctx.lockout.register_failure(pair, now_ns) {
                FailureOutcome::Backoff { delay_ms } => delay_ms,
                FailureOutcome::LockedOut => BACKOFF_CAP_MS,
            };
            audit_emit(
                ctx,
                "auth_failed",
                username.as_str(),
                subnet,
                None,
                "bootstrap_meta_read_error",
            );
            return Err(HandshakeError::BadProof { backoff_ms });
        }
    }

    // 8. Directory lookup, then build + admit the session under the
    // per-user cap (spec §7.4); success-only effects and auth_ok follow.
    // Task #557: ONE `state_by_user_id` lookup returns the authoritative
    // `superuser` flag plus the roles list; the roles value has exactly ONE
    // consumer — the freshly-built session (moved).
    let user_id = match ctx.user_dir.user_id(username.as_str()) {
        Some(id) => id,
        None => return Err(HandshakeError::UnknownUser),
    };
    let user_state = match ctx.user_dir.state_by_user_id(&user_id) {
        Some(s) => s,
        None => return Err(HandshakeError::UnknownUser),
    };
    // Admission timestamp sampled ONCE here (NOT the pre-challenge clock
    // from step 2) and passed to both `Session::new` and `admit`, so the
    // all-active/idle classification uses the true login instant.
    let admit_ns = UnixNanos::now().as_u64();
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
        admit_ns,
    );
    let evicted =
        match ctx
            .session_store
            .admit(auth_ok.session_id, session, admit_ns, MAX_SESSIONS_PER_USER)
        {
            Admission::Admitted { evicted, .. } => evicted,
            Admission::Rejected(reason) => {
                // Spec §7.4: every existing session active within 60 s (or sid
                // conflict) => reject. Internal audit reason only; no
                // `auth_success`, no ticket. NOT a wrong-password event: no
                // `register_failure` and no lockout reset.
                let reason_str = match reason {
                    AdmitReject::AllActive => "max_sessions",
                    AdmitReject::SidConflict => "sid_conflict",
                };
                tracing::info!(user_hash = %hex::encode(uhash), "auth_failed: admission rejected");
                audit_emit(
                    ctx,
                    "auth_failed",
                    username.as_str(),
                    subnet,
                    Some(&prefix_8(&auth_ok.session_id)),
                    reason_str,
                );
                return Err(HandshakeError::Admission);
            }
        };

    // Success-only effects start HERE — behind admission — so a rejected
    // login neither resets the lockout counter nor consumes bootstrap
    // credentials.

    // Reset lockout on success per spec §5.2.5 NORMATIVE.
    ctx.lockout.reset_on_success(pair);

    // RI-9 / CR-A6: consume the bootstrap token on the first successful
    // login for the username it was issued to, AND rotate the account's
    // SCRAM credential to a fresh random value nobody knows — otherwise
    // the token keeps working as the account's password forever (the bug
    // CR-A6 fixes; see `bootstrap.rs::rotate_bootstrap_credential_to_random`
    // for the residual-race note). Best-effort and non-fatal throughout —
    // a failure here must NEVER abort an otherwise-successful login; the
    // boot-time TTL sweep (`server_launcher/launch.rs`) is the backstop for
    // anything missed here.
    //
    // F-3: ordering is rotate-FIRST → consume-on-success → delete-file-last.
    // If the rotate fails the token metadata row is LEFT active so the next
    // login attempt (or the next boot's TTL sweep) gets another chance —
    // this makes the whole operation naturally retryable. Mirrors the
    // sweep's ordering in `server_launcher/launch.rs`. Same `kdf` this login just
    // verified against, same `now_ns` already computed for the lockout
    // pre-check.
    if ctx.meta.bootstrap_token_active()
        && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())
    {
        match crate::bootstrap::rotate_bootstrap_credential_to_random(
            &ctx.user_dir,
            username.as_str(),
            kdf,
            now_ns,
        )
        .await
        {
            Ok(()) => {
                // Read the token file path BEFORE `consume_bootstrap_token()`
                // clears the row — the same "read username/path before
                // consuming" constraint the original code respected.
                let token_path = ctx.meta.bootstrap_token_path();
                if let Err(e) = ctx.meta.consume_bootstrap_token() {
                    tracing::warn!(?e, "bootstrap: failed to consume token record on login");
                }
                if let Some(path) = token_path {
                    if let Err(e) = std::fs::remove_file(&path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            tracing::warn!(
                                ?path,
                                ?e,
                                "bootstrap: failed to delete token file on login"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "bootstrap: failed to rotate SCRAM credential on login \
                     -- token left active for retry"
                );
            }
        }
    }

    // Eviction audits from the capped admission (idle LRU, spec §7.4).
    for victim_sid in &evicted {
        audit_emit(
            ctx,
            "session_evicted",
            username.as_str(),
            subnet,
            Some(&prefix_8(victim_sid)),
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
            ctx.resume_config.ticket_key(),
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
        server_query_version: checked_query_version(CURRENT_QUERY_LANG_VERSION)
            .map_err(|_| HandshakeError::Io)?,
        rotation_in_progress: auth_ok
            .rotation_in_progress
            .as_ref()
            .map(RotationInProgressWire::from),
        kdf_upgrade_required: auth_ok.kdf_upgrade_required,
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
    })
    .await
    .unwrap_or_else(|_elapsed| {
        tracing::info!(
            budget_ms = budget.as_millis() as u64,
            "handshake budget exceeded (slow-loris post-challenge)",
        );
        Err(HandshakeError::Io)
    })
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
            bounded_shutdown(&mut framer).await;
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
            bounded_shutdown(&mut framer).await;
            return;
        }
        Err(_elapsed) => {
            tracing::info!(
                timeout_ms = ctx.auth_init_timeout.as_millis() as u64,
                ?subnet,
                "first frame timeout (slow-loris)",
            );
            bounded_shutdown(&mut framer).await;
            return;
        }
    }

    // RV-CLIENT-DE75ADDD: optional resume identity-proof preamble. A client
    // MAY open with a 1-field `resume_proof` frame to authenticate the
    // server's pinned Ed25519 identity BEFORE disclosing the bearer ticket;
    // after answering, the NEXT frame is dispatched exactly like a first
    // frame. Any other frame shape (arity-decoded) skips this branch.
    if let Ok(proof_init) = rmp_serde::from_slice::<wire::ResumeProofInit>(&frame_buf) {
        if proof_init.client_nonce.len() != 32
            || run_resume_proof(&ctx, &proof_init, &mut framer, &mut write_scratch, exporter)
                .await
                .is_err()
        {
            bounded_shutdown(&mut framer).await;
            return;
        }
        let read_fut = framer.read_frame_into(MAX_PRE_AUTH_FRAME, &mut frame_buf);
        match tokio::time::timeout(ctx.auth_init_timeout, read_fut).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::debug!(?e, "post-proof first frame read failed");
                bounded_shutdown(&mut framer).await;
                return;
            }
            Err(_elapsed) => {
                tracing::info!(?subnet, "post-proof first frame timeout (slow-loris)",);
                bounded_shutdown(&mut framer).await;
                return;
            }
        }
    }

    // Determine which auth path to take.
    //
    // A `ResumeInit` has a non-empty `ticket` field. Try to decode it first
    // (cheap msgpack sniff); on any failure fall through to the SCRAM path.
    // Unknown-frame shape → shutdown without leaking timing.
    let is_resume = matches!(rmp_serde::from_slice::<wire::ResumeInit>(&frame_buf), Ok(r) if !r.ticket.is_empty());

    let session_id = if is_resume {
        // Resume fast-path — success is deliberately UNPADDED (the ticket is
        // opaque ciphertext; timing reveals nothing about user existence).
        // A REJECTED resume, though, is externally indistinguishable from a
        // failed login, so the whole resume attempt is wrapped in a
        // LatencyPadGuard and the error path is padded to the constant-time
        // floor (backoff 0 — NOT a wrong-password event, no
        // register_failure) BEFORE shutdown (spec §8.5).
        let pad_guard = LatencyPadGuard::start();
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
                // No pad on success — the deliberate resume fast-path.
                record_auth_attempt("success");
                sid
            }
            Err(e) => {
                let label = if matches!(e, HandshakeError::Admission) {
                    "admission"
                } else {
                    "bad_proof"
                };
                record_auth_attempt(label);
                let pad = pad_guard.finish_with_target(target_constant_time_ms());
                if pad > Duration::ZERO {
                    tokio::time::sleep(pad).await;
                }
                bounded_shutdown(&mut framer).await;
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
                    HandshakeError::Admission => "admission",
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
                // RV-TRANSPORT-TCP-B5244FA2: bounded so a backpressured
                // transport cannot retain the connection task (and the
                // admission guards that live until it returns) after the
                // handshake is already over.
                bounded_shutdown(&mut framer).await;
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
