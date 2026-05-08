//! Server-side `changePassword` flow (spec §12.5).
//!
//! Two endpoints (logically — wire format is up to the transport binding):
//! - `start_change_password_challenge(session)` → fresh `server_nonce_cp` +
//!   stores [`PendingChangePwChallenge`] on the session.
//! - `verify_and_apply_change_password(session, request, ...)` → SCRAM verify
//!   of the old password, atomic update of user record, kill all sessions of
//!   user, set `tickets_invalid_before_ns = now_ns`.

use crate::common::changepw::{
    build_auth_message_cp, ChangePwAuthMessageInputs, CHANGEPW_CHALLENGE_TTL_NS,
};
use crate::common::crypto::{constant_time_eq, hmac_sha256, random_array, sha256, StoredKey};
use crate::common::error::{Error, Result};
use crate::common::kdf_params::KdfParams;
use crate::common::types::limits;
use crate::server::session::{PendingChangePwChallenge, Session, SessionStore};
use zeroize::Zeroizing;

/// Server-issued challenge view (`challenge_cp`).
#[derive(Debug, Clone)]
pub struct ChangePwChallengeView {
    /// Fresh CSPRNG nonce.
    pub server_nonce_cp: [u8; 32],
    /// User's current salt (echoed for client convenience — same value
    /// already lives in the user record).
    pub salt: [u8; limits::SALT_BYTES],
    /// User's current KDF parameters (proof_old uses these).
    pub kdf_params: KdfParams,
}

/// Client → server `changePassword` body.
#[derive(Debug, Clone)]
pub struct ChangePwRequest {
    /// SCRAM proof recovered from the OLD password.
    pub client_proof_old: [u8; 32],
    /// New per-user salt (CSPRNG).
    pub new_salt: [u8; limits::SALT_BYTES],
    /// New stored_key = SHA256(HMAC(new_salted_pw, "Client Key")).
    pub new_stored_key: [u8; 32],
    /// New server_key = HMAC(new_salted_pw, "Server Key").
    pub new_server_key: [u8; 32],
}

/// New material to persist after a successful change.
pub struct ChangePwApply {
    /// New salt.
    pub salt: [u8; limits::SALT_BYTES],
    /// New stored_key.
    pub stored_key: StoredKey,
    /// New server_key (zeroized on drop).
    pub server_key: Zeroizing<[u8; 32]>,
    /// `kdf_params` to persist — server defaults (client's value ignored).
    pub kdf_params: KdfParams,
}

/// Step 1: server records pending challenge state on the session and emits
/// the `challenge_cp` view for the wire.
///
/// Multi-tab semantics: a second `changePasswordChallenge` overwrites the
/// previous pending state (single-in-flight per session, spec §12.5).
pub fn start_change_password_challenge(
    session: &Session,
    user_salt: [u8; limits::SALT_BYTES],
    user_kdf_params: KdfParams,
    client_nonce_cp: [u8; 32],
    now_ns: u64,
) -> ChangePwChallengeView {
    let server_nonce_cp = random_array::<32>();
    let pending = PendingChangePwChallenge {
        server_nonce_cp,
        client_nonce_cp,
        issued_at_ns: now_ns,
    };
    *session.pending_changepw_challenge.lock() = Some(pending);

    ChangePwChallengeView {
        server_nonce_cp,
        salt: user_salt,
        kdf_params: user_kdf_params,
    }
}

/// Step 2: server verifies the request and (on success) returns the new
/// material to persist. The pending challenge is cleared regardless of
/// outcome (single-use), and on success ALL sessions of the user must be
/// killed by the caller (`session_store.snapshot_by_user(user_id)` then
/// remove + bump `tickets_invalid_before_ns = now_ns`).
///
/// Helper: kill all sessions for a user after a successful changePassword.
///
/// Per spec §12.5.3: "Все сессии юзера убиваются (включая текущую) И
/// tickets_invalid_before_ns = now_ns".
///
/// Caller is responsible for persisting the new `tickets_invalid_before_ns`
/// to the user record (we just return it for atomicity reasons).
pub fn finalize_change_password(
    store: &SessionStore,
    user_id: &[u8; 16],
    now_ns: u64,
) -> u64 {
    let victim_sids = store.snapshot_by_user(user_id);
    for sid in victim_sids {
        store.remove(&sid);
    }
    now_ns
}

/// Verify a `changePassword` request: bind via `auth_message_cp`, run SCRAM
/// proof check on `proof_old`, and return the new (salt, stored_key,
/// server_key, kdf_params) the caller should persist.
///
/// `session_id` is passed explicitly because [`Session`] does not carry its
/// own id (the caller looking up the session in [`SessionStore`] already has
/// it).
///
/// The pending challenge is popped atomically (single-use) regardless of
/// outcome.
#[allow(clippy::too_many_arguments)]
pub fn verify_change_password_request_with_sid(
    session: &Session,
    session_id: &[u8; limits::SESSION_ID_BYTES],
    user_salt: [u8; limits::SALT_BYTES],
    user_stored_key: &StoredKey,
    user_kdf_params: KdfParams,
    request: &ChangePwRequest,
    current_kdf_params: KdfParams,
    now_ns: u64,
) -> Result<ChangePwApply> {
    let pending = session.pending_changepw_challenge.lock().take();
    let pending = pending.ok_or(Error::AuthFailed)?;
    if now_ns - pending.issued_at_ns > CHANGEPW_CHALLENGE_TTL_NS {
        return Err(Error::AuthFailed);
    }

    let am_cp = build_auth_message_cp(ChangePwAuthMessageInputs {
        username: &crate::common::username::NormalizedUsername::from_normalized_unchecked(
            session.username.clone(),
        ),
        session_id,
        client_nonce_cp: &pending.client_nonce_cp,
        server_nonce_cp: &pending.server_nonce_cp,
        salt: &user_salt,
        kdf_params: user_kdf_params,
        transport_kind: session.transport_kind,
        binding_mode: session.binding_mode,
        channel_binding_at_auth: &session.channel_binding_at_auth,
    })?;

    let signature = hmac_sha256(&user_stored_key.0, &am_cp);
    let mut recovered = [0u8; 32];
    for i in 0..32 {
        recovered[i] = request.client_proof_old[i] ^ signature[i];
    }
    let recomputed = sha256(&recovered);
    if !constant_time_eq(&recomputed, &user_stored_key.0) {
        return Err(Error::AuthFailed);
    }

    let mut sk = Zeroizing::new([0u8; 32]);
    sk.copy_from_slice(&request.new_server_key);
    Ok(ChangePwApply {
        salt: request.new_salt,
        stored_key: StoredKey(request.new_stored_key),
        server_key: sk,
        kdf_params: current_kdf_params,
    })
}
