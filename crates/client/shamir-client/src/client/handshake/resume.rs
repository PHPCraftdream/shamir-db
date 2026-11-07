use rand::RngCore;
use zeroize::Zeroizing;

use shamir_connect::common::crypto::{ed25519_verify_strict, sha256};
use shamir_connect::common::identity::resume_proof_message;
use shamir_connect::common::types::BindingMode;

use shamir_transport_tcp::framing::write_frame;

use super::wire::{WireResumeInit, WireResumeOk, WireResumeProofInit, WireResumeProofOk};
use crate::client::session::entry::resume_security::{
    read_resume_frame, serialize_resume, ProtectedResumeBuffer,
};
use crate::error::ClientError;

/// Result of the post-TLS resume wire exchange: everything
/// [`Client::assemble`](crate::client::Client::assemble) needs to finish a resumed session.
pub(crate) struct ResumedHandshake {
    pub(crate) session_id: [u8; 32],
    pub(crate) expires_at_ns: u64,
    pub(crate) resumption_ticket: Option<Zeroizing<Vec<u8>>>,
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
    ticket: ProtectedResumeBuffer,
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
        .map_err(|e| ClientError::transport("send resume_proof", e))?;
    let proof_bytes = read_resume_frame(r)
        .await
        .map_err(|e| ClientError::transport("read resume_proof_ok", e))?;
    let proof_ok: WireResumeProofOk = rmp_serde::from_slice(&proof_bytes)?;
    verify_resume_identity_proof(&proof_ok, &client_nonce, &tls_exporter, &pinned_hash)?;

    // ---- Send ResumeInit ----
    let init_wire = WireResumeInit {
        ticket: &ticket,
        client_nonce: client_nonce.to_vec(),
        binding_mode: BindingMode::TlsExporter as u8,
    };
    let init_bytes = serialize_resume(&init_wire, ticket.len().saturating_add(128))?;
    write_frame(w, &init_bytes)
        .await
        .map_err(|e| ClientError::transport("send resume_init", e))?;

    // ---- Read ResumeOk ----
    let ok_bytes = read_resume_frame(r)
        .await
        .map_err(|e| ClientError::transport("read resume_ok", e))?;
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
            Some(ok_wire.resumption_ticket.into_inner()),
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
        return Err(ClientError::handshake(
            "resume: server identity does not match pinned hash",
        ));
    }
    let msg = resume_proof_message(client_nonce, tls_exporter);
    if !ed25519_verify_strict(&server_pub_key, &msg, &signature) {
        return Err(ClientError::handshake(
            "resume: server identity proof invalid",
        ));
    }
    Ok(())
}

/// The resume exchange for the channel bindings WITHOUT a TLS exporter —
/// plain TCP and the browser WebSocket profile (binding modes 0x00 and
/// 0x02). The identity-proof preamble is not spoken on these profiles (the
/// server treats the preamble as optional; the local-IPC resume and the
/// TypeScript browser profile both go straight to `ResumeInit`): the ticket
/// IS the first frame, `binding_mode` carries the listener's mode, and the
/// exporter stays zeros on both sides. The server authenticates itself
/// out-of-band on these listeners: ordinary TLS certificate trust on the
/// browser profile, the loopback-only bind rule on plain TCP.
pub(crate) async fn resume_wire_exchange_unbound<R, W>(
    r: &mut R,
    w: &mut W,
    ticket: ProtectedResumeBuffer,
    binding_mode: BindingMode,
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

    // ---- Send ResumeInit (no proof preamble on this binding) ----
    let init_wire = WireResumeInit {
        ticket: &ticket,
        client_nonce: client_nonce.to_vec(),
        binding_mode: binding_mode as u8,
    };
    let init_bytes = serialize_resume(&init_wire, ticket.len().saturating_add(128))?;
    write_frame(w, &init_bytes)
        .await
        .map_err(|e| ClientError::transport("send resume_init", e))?;

    // ---- Read ResumeOk ----
    let ok_bytes = read_resume_frame(r)
        .await
        .map_err(|e| ClientError::transport("read resume_ok", e))?;
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
            Some(ok_wire.resumption_ticket.into_inner()),
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
