//! Session-resume fast-path and the resume identity-proof preamble
//! (RV-CLIENT-DE75ADDD), split out of `handshake.rs`.

use shamir_connect::common::identity::resume_proof_message;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::BindingMode;
use shamir_connect::server::admission::AdmitReject;
use shamir_connect::server::resume::{process_resume, ResumeRequest};
use shamir_query_types::wire::CURRENT_QUERY_LANG_VERSION;

use crate::framer::Framer;

use super::super::handshake::{audit_emit, prefix_8, HandshakeError};
use super::super::user_state_lookup::RedbUserStateLookup;
use super::wire;
use super::ConnectionContext;

/// Session-resume fast-path (SESSION_RESUMPTION §5).
///
/// Called when the first frame decodes as a [`wire::ResumeInit`] with a
/// non-empty `ticket`. Calls [`process_resume`] from `shamir-connect`,
/// inserts the new session, and sends a [`wire::ResumeOkWire`] response.
///
/// Success is unpadded — the ticket is opaque AES-256-GCM ciphertext, so
/// timing the response cannot reveal user existence. A rejection returns
/// `HandshakeError::Admission` (after emitting the same `auth_failed` audit
/// event as login); the OUTER handler pads that path to the constant-time
/// floor before closing.
pub(crate) async fn run_resume<F: Framer>(
    ctx: &ConnectionContext,
    first_frame: &[u8],
    framer: &mut F,
    write_scratch: &mut Vec<u8>,
    subnet: shamir_connect::server::lockout::Subnet,
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

    let ok = match process_resume(
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
        shamir_connect::server::handshake::SESSION_MAX_AGE_NS,
        RESUMPTION_TICKET_TTL_NS,
        now_ns,
    ) {
        Ok(ok) => ok,
        // Admission rejection is a distinct internal outcome — the SAME
        // `auth_failed` audit event as login (spec §7.4, RV-CONNECT-
        // 8AD8B1C8), then the outer handler closes without a success frame;
        // no backoff registration. There is no proposed sid to log on
        // rejection (the new session id is only assigned on success), so the
        // sid prefix is `None` — the same convention as the other resume
        // failure audit sites.
        Err(shamir_connect::common::error::Error::AdmissionRejected { reason, username }) => {
            let reason_str = match reason {
                AdmitReject::AllActive => "max_sessions",
                AdmitReject::SidConflict => "sid_conflict",
            };
            audit_emit(ctx, "auth_failed", &username, subnet, None, reason_str);
            return Err(HandshakeError::Admission);
        }
        Err(_) => return Err(HandshakeError::BadProof { backoff_ms: 0 }),
    };

    // Audit the idle sessions the capped admission evicted (same reason
    // string as login — RV-CONNECT-8AD8B1C8).
    for victim_sid in &ok.evicted {
        audit_emit(
            ctx,
            "session_evicted",
            &ok.username,
            subnet,
            Some(&prefix_8(victim_sid)),
            "max_sessions_lru",
        );
    }

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

/// Answer a resume identity-proof preamble (RV-CLIENT-DE75ADDD): sign
/// `RESUME_PROOF_V1 || client_nonce || tls_exporter` with the CURRENT
/// Ed25519 identity key so a resuming client can verify the pinned identity
/// before disclosing the bearer ticket. Signing is µs-cheap and the exchange
/// is pre-auth, so no Argon2-semaphore gating applies — the caller's
/// `auth_init_timeout` frame reads bound the round-trip.
pub(crate) async fn run_resume_proof<F: Framer>(
    ctx: &ConnectionContext,
    init: &wire::ResumeProofInit,
    framer: &mut F,
    write_scratch: &mut Vec<u8>,
    exporter: [u8; 32],
) -> Result<(), HandshakeError> {
    let mut client_nonce = [0u8; 32];
    client_nonce.copy_from_slice(&init.client_nonce);
    let msg = resume_proof_message(&client_nonce, &exporter);
    let ok = wire::ResumeProofOkWire {
        server_pub_key: ctx.identity.current_pub().to_vec(),
        signature: ctx.identity.sign_with_current(&msg).to_vec(),
    };
    let bytes = rmp_serde::to_vec(&ok).map_err(|_| HandshakeError::Io)?;
    framer
        .write_frame_into(&bytes, write_scratch)
        .await
        .map_err(|_| HandshakeError::Io)
}
