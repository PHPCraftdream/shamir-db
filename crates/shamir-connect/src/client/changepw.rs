//! Client-side `changePassword` flow (spec §12.5).

use crate::common::changepw::{build_auth_message_cp, ChangePwAuthMessageInputs};
use crate::common::crypto::{hmac_sha256, random_array};
use crate::common::error::Result;
use crate::common::kdf_params::KdfParams;
use crate::common::scram::DerivedKeys;
use crate::common::types::{limits, BindingMode, TransportKind};
use crate::common::username::NormalizedUsername;
use crate::server::changepw::ChangePwRequest;
use zeroize::Zeroize;

/// Step 1: client emits `changePasswordChallenge { client_nonce_cp }`.
pub fn build_challenge_request() -> [u8; 32] {
    random_array::<32>()
}

/// Step 3-4: client computes proof_old + new material and builds the
/// `changePassword` body.
///
/// `old_password` and `new_password` are zeroized after use.
#[allow(clippy::too_many_arguments)]
pub fn build_request(
    username: &NormalizedUsername,
    session_id: &[u8; limits::SESSION_ID_BYTES],
    client_nonce_cp: &[u8; 32],
    server_nonce_cp: &[u8; 32],
    salt: &[u8; limits::SALT_BYTES],
    kdf_params: KdfParams,
    transport_kind: TransportKind,
    binding_mode: BindingMode,
    channel_binding_at_auth: &[u8; 32],
    old_password: &mut [u8],
    new_password: &mut [u8],
    new_kdf_params_recommendation: KdfParams,
) -> Result<ChangePwRequest> {
    new_kdf_params_recommendation.validate_client_limits()?;

    // Build canonical auth_message_cp.
    let am_cp = build_auth_message_cp(ChangePwAuthMessageInputs {
        username,
        session_id,
        client_nonce_cp,
        server_nonce_cp,
        salt,
        kdf_params,
        transport_kind,
        binding_mode,
        channel_binding_at_auth,
    })?;

    // Derive material from OLD password to build proof_old.
    let derived_old = DerivedKeys::derive(old_password, salt, &kdf_params)?;
    old_password.zeroize();

    let signature = hmac_sha256(&derived_old.stored_key.0, &am_cp);
    let mut proof = [0u8; 32];
    for i in 0..32 {
        proof[i] = derived_old.client_key[i] ^ signature[i];
    }

    // Derive material from NEW password (with recommended params).
    let new_salt = random_array::<{ limits::SALT_BYTES }>();
    let derived_new = DerivedKeys::derive(new_password, &new_salt, &new_kdf_params_recommendation)?;
    new_password.zeroize();

    let mut new_server_key = [0u8; 32];
    new_server_key.copy_from_slice(&derived_new.server_key[..]);

    Ok(ChangePwRequest {
        client_proof_old: proof,
        new_salt,
        new_stored_key: derived_new.stored_key.0,
        new_server_key,
    })
}
