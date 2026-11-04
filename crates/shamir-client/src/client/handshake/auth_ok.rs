use shamir_connect::client::handshake::{ClientHandshake, HandshakeSuccess, ServerAuthOk};
use shamir_connect::common::auth_message::AuthMessage;
use shamir_connect::common::scram::DerivedKeys;

use super::wire::WireAuthOk;
use crate::error::ClientError;

pub(crate) struct VerifiedAuthOk {
    pub(crate) auth_ok: ServerAuthOk,
    pub(crate) resumption_ticket: Option<Vec<u8>>,
    pub(crate) resumption_expires_at_ns: Option<u64>,
    pub(crate) success: HandshakeSuccess,
    pub(crate) pinned_hash: [u8; 32],
}

pub(super) fn verify_auth_ok(
    hs: &mut ClientHandshake,
    wire: &WireAuthOk,
    derived: &DerivedKeys,
    auth_message: &AuthMessage,
    trusted_pin: Option<[u8; 32]>,
) -> Result<VerifiedAuthOk, ClientError> {
    let server_signature: [u8; 32] = wire
        .server_signature
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("server_signature size".into()))?;
    let server_pub_key: [u8; 32] = wire
        .server_pub_key
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("server_pub_key size".into()))?;
    let identity_sig: [u8; 64] = wire
        .identity_sig
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("identity_sig size".into()))?;
    let session_id: [u8; 32] = wire
        .session_id
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::Protocol("session_id size".into()))?;
    let resumption_ticket =
        (!wire.resumption_ticket.is_empty()).then(|| wire.resumption_ticket.clone());
    let resumption_expires_at_ns =
        (wire.resumption_expires_at_ns > 0).then_some(wire.resumption_expires_at_ns);
    let rotation_in_progress = wire
        .rotation_in_progress
        .clone()
        .map(|payload| {
            payload
                .into_payload()
                .ok_or_else(|| ClientError::Protocol("rotation_in_progress size".into()))
        })
        .transpose()?;
    let auth_ok = ServerAuthOk {
        server_signature,
        server_pub_key,
        identity_sig,
        session_id,
        expires_at_ns: wire.expires_at_ns,
        resumption_ticket: resumption_ticket.clone(),
        resumption_expires_at_ns,
        rotation_in_progress,
        kdf_upgrade_required: wire.kdf_upgrade_required,
    };
    let mut pin_capture = trusted_pin;
    let success = match hs.process_auth_ok(&auth_ok, derived, auth_message, |pin| {
        pin_capture = Some(*pin);
    }) {
        Ok(success) => success,
        Err(error)
            if matches!(
                &error,
                shamir_connect::common::error::Error::ServerIdentityChanged
            ) && trusted_pin.is_some()
                && auth_ok.rotation_in_progress.is_some() =>
        {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .min(u64::MAX as u128) as u64;
            let recovery = hs
                .verify_rotation_recovery(&auth_ok, derived, auth_message, now_ns)
                .map_err(|error| ClientError::handshake_with(error.to_string(), error))?;
            return Err(ClientError::IdentityRotationPending {
                previous_pin: recovery.previous_pin,
                new_pin: recovery.new_pin,
                transition_until_ns: recovery.transition_until_ns,
            });
        }
        Err(error) => return Err(ClientError::handshake_with(error.to_string(), error)),
    };
    let pinned_hash = pin_capture.expect("either trusted_pin pre-set or TOFU callback fired");
    Ok(VerifiedAuthOk {
        auth_ok,
        resumption_ticket,
        resumption_expires_at_ns,
        success,
        pinned_hash,
    })
}
