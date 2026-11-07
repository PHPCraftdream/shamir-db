use std::sync::atomic::{AtomicU8, Ordering};

use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerChallenge};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame};

use super::auth_ok::{verify_auth_ok, VerifiedAuthOk};
use super::wire::{WireAuthInit, WireAuthOk, WireChallenge, WireClientProof};
use crate::client::transport::ChannelBinding;
use crate::error::ClientError;

pub(in crate::client) const PHASE_SCRAM: u8 = 3;
pub(in crate::client) const PHASE_KDF: u8 = 4;

pub(crate) struct ScramRequest<'a> {
    pub(crate) username: NormalizedUsername,
    pub(crate) binding: ChannelBinding,
    pub(crate) trusted_pin: Option<[u8; 32]>,
    pub(crate) accept_new_host: bool,
    pub(crate) password: &'a [u8],
    pub(crate) phase: Option<&'a AtomicU8>,
}

pub(crate) struct ScramOutcome {
    pub(crate) verified: VerifiedAuthOk,
    pub(crate) server_query_version: u8,
    pub(crate) kdf_upgrade_required: bool,
}

fn mark_phase(phase: Option<&AtomicU8>, value: u8) {
    if let Some(phase) = phase {
        phase.store(value, Ordering::Relaxed);
    }
}

/// The single client-side SCRAM exchange shared by TCP+TLS and IPC.
pub(crate) async fn run_scram<R, W>(
    r: &mut R,
    w: &mut W,
    req: ScramRequest<'_>,
) -> Result<ScramOutcome, ClientError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    mark_phase(req.phase, PHASE_SCRAM);
    let mut hb = HandshakeBuilder::new(req.username, req.binding.transport, req.binding.mode)
        .tls_exporter(req.binding.exporter);
    hb = match req.trusted_pin {
        Some(pin) => hb.pinned_hash(pin),
        None => hb.accept_new_host(req.accept_new_host),
    };
    let mut hs = hb
        .build()
        .map_err(|e| ClientError::handshake_with(e.to_string(), e))?;

    // Step 1: auth_init
    let init = hs.auth_init();
    let init_wire = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(&mut *w, &rmp_serde::to_vec(&init_wire)?)
        .await
        .map_err(|e| ClientError::transport("send auth_init", e))?;

    // Step 2: challenge
    let ch_bytes = read_frame(&mut *r, MAX_PRE_AUTH_FRAME)
        .await
        .map_err(|e| ClientError::transport("read challenge", e))?;
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

    challenge
        .kdf_params
        .validate_client_limits()
        .map_err(|error| ClientError::handshake_with(error.to_string(), error))?;

    // Step 3: derive proof (Argon2id ~50ms-2s — spawn_blocking so
    // it doesn't stall the tokio worker thread).
    // Wrap the working copy in `Zeroizing` so it is wiped on EVERY exit
    // path of the blocking closure (including a `process_challenge`
    // error), not only the success path.
    let mut password_buf = Zeroizing::new(req.password.to_vec());
    let challenge_clone = challenge.clone();
    mark_phase(req.phase, PHASE_KDF);
    let (hs_ret, result) = tokio::task::spawn_blocking(move || {
        let res = hs.process_challenge(&challenge_clone, &mut password_buf);
        (hs, res)
    })
    .await
    .map_err(|e| ClientError::handshake_with("Argon2 spawn_blocking", e))?;
    hs = hs_ret;
    // KDF is done: the remaining waits (proof write, `auth_ok` read) are
    // SCRAM I/O again — a timeout here must not be labelled `kdf`.
    mark_phase(req.phase, PHASE_SCRAM);
    let (proof, derived, auth_message) =
        result.map_err(|e| ClientError::handshake_with(e.to_string(), e))?;

    // Step 4: send proof
    let proof_wire = WireClientProof {
        client_proof: proof.to_vec(),
    };
    write_frame(&mut *w, &rmp_serde::to_vec(&proof_wire)?)
        .await
        .map_err(|e| ClientError::transport("send proof", e))?;

    // Step 5: auth_ok
    let ok_bytes = read_frame(&mut *r, MAX_PRE_AUTH_FRAME)
        .await
        .map_err(|e| ClientError::transport("read auth_ok", e))?;
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes)?;
    let verified = verify_auth_ok(&mut hs, &ok_wire, &derived, &auth_message, req.trusted_pin)?;

    Ok(ScramOutcome {
        verified,
        server_query_version: ok_wire.server_query_version,
        kdf_upgrade_required: ok_wire.kdf_upgrade_required.unwrap_or(false),
    })
}
